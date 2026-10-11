//! Bounded pool of query connections (M13 §13). Each connection is owned by a
//! dedicated thread; async callers wait on a channel, so DuckDB never runs on
//! a Tokio executor. Dropping the future of a running query interrupts it,
//! and dropping the pool never waits for native work.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use duckdb::{Connection, InterruptHandle};
use tokio::sync::oneshot;

use crate::{RuntimeConfig, RuntimeError};

type Job = Box<dyn FnOnce(&mut Worker) + Send>;

/// Cheap to clone; the pool stops when the last clone is dropped.
#[derive(Clone)]
pub struct Runtime {
    shared: Arc<Shared>,
}

struct Shared {
    sender: Mutex<Option<SyncSender<Job>>>,
    load: Arc<Mutex<PoolLoad>>,
}

/// Instantaneous pool occupancy, independent of any telemetry SDK.
/// Cancelled queued jobs occupy a slot until dequeued; running jobs keep
/// their slot through interruption and connection recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolLoad {
    pub queued: usize,
    pub running: usize,
    /// Live worker slots (decreases when a worker retires).
    pub workers: usize,
    pub queue_capacity: usize,
}

struct Occupancy {
    load: Arc<Mutex<PoolLoad>>,
    running: bool,
}

impl Occupancy {
    fn start(&mut self) {
        let mut load = lock(&self.load);
        load.queued -= 1;
        load.running += 1;
        self.running = true;
    }
}

impl Drop for Occupancy {
    fn drop(&mut self) {
        let mut load = lock(&self.load);
        if self.running {
            load.running -= 1;
        } else {
            load.queued -= 1;
        }
    }
}

struct WorkerSlot(Arc<Mutex<PoolLoad>>);

impl Drop for WorkerSlot {
    fn drop(&mut self) {
        lock(&self.0).workers -= 1;
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // Closing the channel ends each worker after its current job. The
        // workers are detached: waiting for them here would block the caller,
        // often a Tokio executor, on native work. The last worker to exit
        // releases the database on its own thread.
        lock(&self.sender).take();
    }
}

/// Lifecycle of one job. The interrupt is delivered while holding this lock
/// and only in `Running`, so it can never reach another job's query.
enum JobState {
    Queued,
    Running(Arc<InterruptHandle>),
    /// Cancelled while running: the connection state is uncertain.
    Interrupted,
    /// Cancelled before a worker picked it up.
    Skipped,
    Done,
}

struct Control {
    state: Mutex<JobState>,
}

impl Control {
    fn cancel(&self) {
        let mut state = lock(&self.state);
        match &*state {
            JobState::Queued => *state = JobState::Skipped,
            JobState::Running(interrupt) => {
                interrupt.interrupt();
                *state = JobState::Interrupted;
            }
            JobState::Interrupted | JobState::Skipped | JobState::Done => {}
        }
    }
}

/// Cancels the job if the caller stops waiting (timeout, client gone).
struct CancelOnDrop(Option<Arc<Control>>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(control) = self.0.take() {
            control.cancel();
        }
    }
}

type Replacement = Box<dyn FnMut() -> duckdb::Result<Connection> + Send>;

struct Worker {
    conn: Connection,
    /// Produces a fresh connection to the same database.
    replace: Replacement,
    /// Set when the connection is uncertain and could not be replaced.
    retired: bool,
}

impl Worker {
    fn execute<T>(
        &mut self,
        control: &Control,
        job: impl FnOnce(&Connection) -> duckdb::Result<T>,
    ) -> Result<T, RuntimeError> {
        {
            let mut state = lock(&control.state);
            if matches!(*state, JobState::Skipped) {
                return Err(RuntimeError::Cancelled);
            }
            *state = JobState::Running(self.conn.interrupt_handle());
        }
        let outcome = catch_unwind(AssertUnwindSafe(|| job(&self.conn)));
        let interrupted = {
            let mut state = lock(&control.state);
            let interrupted = matches!(*state, JobState::Interrupted);
            *state = JobState::Done;
            interrupted
        };
        if interrupted || outcome.is_err() {
            self.replace_connection();
        }
        match outcome {
            _ if interrupted => Err(RuntimeError::Cancelled),
            Ok(result) => result.map_err(RuntimeError::Engine),
            Err(_) => Err(RuntimeError::Panicked),
        }
    }

    /// Never reuse a connection whose state is uncertain (M13 §13.3): if no
    /// replacement can be made, the worker retires instead.
    fn replace_connection(&mut self) {
        match (self.replace)() {
            Ok(conn) => self.conn = conn,
            Err(error) => {
                tracing::error!(%error, "cannot replace an uncertain query connection; retiring worker");
                self.retired = true;
            }
        }
    }
}

fn worker_loop(mut worker: Worker, jobs: &Mutex<Receiver<Job>>) {
    while !worker.retired {
        let job = lock(jobs).recv();
        match job {
            Ok(job) => job(&mut worker),
            Err(_) => return,
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Runtime {
    /// Starts `config.workers` threads with connections cloned from `root`,
    /// which must already be locked down.
    pub(crate) fn start(root: Connection, config: &RuntimeConfig) -> Result<Self, RuntimeError> {
        let root = Arc::new(Mutex::new(root));
        let mut workers = Vec::new();
        for _ in 0..config.workers.max(1) {
            let conn = lock(&root).try_clone()?;
            let root = Arc::clone(&root);
            workers.push(Worker {
                conn,
                replace: Box::new(move || lock(&root).try_clone()),
                retired: false,
            });
        }
        Self::spawn(workers, config.queue_capacity)
    }

    fn spawn(workers: Vec<Worker>, queue_capacity: usize) -> Result<Self, RuntimeError> {
        let (sender, receiver) = sync_channel::<Job>(queue_capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let load = Arc::new(Mutex::new(PoolLoad {
            queued: 0,
            running: 0,
            workers: 0,
            queue_capacity,
        }));
        for (index, worker) in workers.into_iter().enumerate() {
            let receiver = Arc::clone(&receiver);
            lock(&load).workers += 1;
            let slot = WorkerSlot(Arc::clone(&load));
            // Detached on purpose; see `Shared::drop`.
            std::thread::Builder::new()
                .name(format!("bi-duckdb-{index}"))
                .spawn(move || {
                    let _slot = slot;
                    worker_loop(worker, &receiver);
                })?;
        }
        Ok(Self {
            shared: Arc::new(Shared {
                sender: Mutex::new(Some(sender)),
                load,
            }),
        })
    }

    /// Coherent snapshot of confirmed admissions and live worker capacity.
    #[must_use]
    pub fn load(&self) -> PoolLoad {
        // Admission holds this lock until a failed send has rolled back.
        let _sender = lock(&self.shared.sender);
        *lock(&self.shared.load)
    }

    /// Runs `job` on a pooled connection. Dropping the returned future before
    /// it completes interrupts the query.
    ///
    /// # Errors
    /// [`RuntimeError::Busy`] when the queue is full, [`RuntimeError::Engine`]
    /// for engine errors, [`RuntimeError::Cancelled`], [`RuntimeError::Panicked`]
    /// or [`RuntimeError::Unavailable`].
    pub async fn run<T, F>(&self, job: F) -> Result<T, RuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> duckdb::Result<T> + Send + 'static,
    {
        let control = Arc::new(Control {
            state: Mutex::new(JobState::Queued),
        });
        let (reply, result) = oneshot::channel();
        let job_control = Arc::clone(&control);
        {
            let sender = lock(&self.shared.sender);
            let sender = sender.as_ref().ok_or(RuntimeError::Unavailable)?;
            lock(&self.shared.load).queued += 1;
            let mut occupancy = Occupancy {
                load: Arc::clone(&self.shared.load),
                running: false,
            };
            let boxed: Job = Box::new(move |worker: &mut Worker| {
                occupancy.start();
                let outcome = worker.execute(&job_control, job);
                // Release before replying; completion implies no active work.
                drop(occupancy);
                reply.send(outcome).ok();
            });
            sender.try_send(boxed).map_err(|error| match error {
                TrySendError::Full(_) => RuntimeError::Busy,
                TrySendError::Disconnected(_) => RuntimeError::Unavailable,
            })?;
        }
        let mut guard = CancelOnDrop(Some(control));
        let outcome = result.await.map_err(|_| RuntimeError::Unavailable);
        guard.0 = None;
        outcome?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_that_cannot_replace_an_uncertain_connection_retires() {
        let worker = Worker {
            conn: Connection::open_in_memory().unwrap(),
            replace: Box::new(|| Err(duckdb::Error::QueryReturnedNoRows)),
            retired: false,
        };
        let runtime = Runtime::spawn(vec![worker], 4).unwrap();
        let executor = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        executor.block_on(async {
            let panicked = runtime
                .run(|_conn| -> duckdb::Result<()> { panic!("leaves the connection uncertain") })
                .await;
            assert!(matches!(panicked, Err(RuntimeError::Panicked)));
            // The only worker retired: nothing runs on its old connection.
            let next = runtime
                .run(|conn| conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0)))
                .await;
            assert!(matches!(next, Err(RuntimeError::Unavailable)), "{next:?}");
            assert_eq!(runtime.load().workers, 0);
            assert_eq!(runtime.load().queued, 0);
            assert_eq!(runtime.load().running, 0);
        });
    }
}
