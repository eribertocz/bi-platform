//! Bounded pool of query connections (M13 §13). Each connection is owned by a
//! dedicated thread; async callers wait on a channel, so DuckDB never runs on
//! a Tokio executor. Dropping the future of a running query interrupts it.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

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
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        // Closing the channel ends each worker after its current job.
        lock(&self.sender).take();
        for thread in lock(&self.threads).drain(..) {
            thread.join().ok();
        }
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

struct Worker {
    conn: Connection,
    root: Arc<Mutex<Connection>>,
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

    /// Never reuse a connection whose state is uncertain (M13 §13.3).
    fn replace_connection(&mut self) {
        match lock(&self.root).try_clone() {
            Ok(conn) => self.conn = conn,
            Err(error) => tracing::error!(%error, "cannot replace query connection"),
        }
    }
}

fn worker_loop(mut worker: Worker, jobs: &Mutex<Receiver<Job>>) {
    loop {
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
        let (sender, receiver) = sync_channel::<Job>(config.queue_capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let root = Arc::new(Mutex::new(root));
        let mut threads = Vec::new();
        for index in 0..config.workers.max(1) {
            let worker = Worker {
                conn: lock(&root).try_clone()?,
                root: Arc::clone(&root),
            };
            let receiver = Arc::clone(&receiver);
            threads.push(
                std::thread::Builder::new()
                    .name(format!("bi-duckdb-{index}"))
                    .spawn(move || worker_loop(worker, &receiver))?,
            );
        }
        Ok(Self {
            shared: Arc::new(Shared {
                sender: Mutex::new(Some(sender)),
                threads: Mutex::new(threads),
            }),
        })
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
        let boxed: Job = Box::new(move |worker: &mut Worker| {
            reply.send(worker.execute(&job_control, job)).ok();
        });
        {
            let sender = lock(&self.shared.sender);
            let sender = sender.as_ref().ok_or(RuntimeError::Unavailable)?;
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
