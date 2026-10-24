//! Composes the semantic adapter with the G1 pool. This adds no HTTP route:
//! a governed caller supplies a certified plan and its security context.
use bi_runtime::{Runtime, RuntimeError};
use opentelemetry::metrics::{Counter, Histogram};
use sef_adapter_duckdb::execution::{BlockResult, DuckDbPlan, ExecutionError, ExecutionLimits};
use sef_adapter_duckdb::provenance::ResultProvenance;
use sef_correctness::SemanticContext;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;
use tracing::Instrument;

#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    Rejected(ExecutionError),
    Busy,
    Deadline,
    Cancelled,
    Worker,
    InvalidCapacity,
}

impl Failure {
    const fn code(&self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Deadline | Self::Rejected(ExecutionError::Deadline) => "deadline",
            Self::Cancelled | Self::Rejected(ExecutionError::Cancelled) => "cancelled",
            Self::Rejected(ExecutionError::Budget) => "budget",
            Self::Rejected(ExecutionError::ResultOutOfRange) => "out_of_range",
            Self::Rejected(_) => "rejected",
            Self::Worker => "worker",
            Self::InvalidCapacity => "invalid_capacity",
        }
    }
}

pub enum Event {
    /// Provisional; a caller may certify/store these only after Complete.
    Batch(BlockResult),
    Complete(Box<ResultProvenance>),
    Failed(Failure),
}

/// Bounded delivery and explicit completion. Drop cancels queued, native and
/// Rust emulation work and releases a worker waiting on backpressure.
pub struct SemanticStream {
    receiver: mpsc::Receiver<Event>,
    task: tokio::task::JoinHandle<()>,
    limits: ExecutionLimits,
}

impl SemanticStream {
    pub async fn next(&mut self) -> Option<Event> {
        self.receiver.recv().await
    }
}

impl Drop for SemanticStream {
    fn drop(&mut self) {
        self.limits.cancelled.store(true, Ordering::Release);
        self.receiver.close();
        self.task.abort();
    }
}

#[derive(Clone)]
pub struct SemanticExecutor {
    runtime: Runtime,
    outcomes: Counter<u64>,
    duration: Histogram<f64>,
}

struct Observation {
    executor: SemanticExecutor,
    started: std::time::Instant,
    recorded: bool,
}

impl Observation {
    fn record(&mut self, code: &'static str) {
        let attributes = [opentelemetry::KeyValue::new("code", code)];
        self.executor.outcomes.add(1, &attributes);
        self.executor
            .duration
            .record(self.started.elapsed().as_secs_f64(), &attributes);
        tracing::info!(
            code,
            elapsed_ms = self.started.elapsed().as_millis(),
            "semantic execution finished"
        );
        self.recorded = true;
    }
}

impl Drop for Observation {
    fn drop(&mut self) {
        if !self.recorded {
            self.record("cancelled");
        }
    }
}

impl SemanticExecutor {
    #[must_use]
    pub fn new(runtime: Runtime) -> Self {
        let meter = opentelemetry::global::meter("bi.semantic_execution");
        Self {
            runtime,
            outcomes: meter.u64_counter("bi.semantic.executions").build(),
            duration: meter
                .f64_histogram("bi.semantic.execution.duration")
                .with_unit("s")
                .build(),
        }
    }

    /// Runs all DuckDB/FFI and Rust emulation on one analytical worker.
    /// Capacity counts Arrow batches, while limits bound their total bytes.
    /// The G1 runtime enforces queue admission and interrupts native work.
    ///
    /// # Errors
    /// Zero capacity, already cancelled or an expired deadline.
    pub fn start(
        &self,
        plan: Arc<DuckDbPlan>,
        context: SemanticContext,
        request_id: &str,
        limits: ExecutionLimits,
        capacity: usize,
    ) -> Result<SemanticStream, Failure> {
        if capacity == 0 {
            return Err(Failure::InvalidCapacity);
        }
        limits.checkpoint().map_err(Failure::Rejected)?;
        let (sender, receiver) = mpsc::channel(capacity);
        let worker_sender = sender.clone();
        let worker_limits = limits.clone();
        let supervisor_limits = limits.clone();
        let executor = self.clone();
        let span = tracing::info_span!("semantic_execution", request_id);
        let task = tokio::spawn(
            async move {
                let mut observation = Observation {
                    executor: executor.clone(),
                    started: std::time::Instant::now(),
                    recorded: false,
                };
                let job = executor.runtime.run(move |connection| {
                    Ok(plan.stream(connection, &context, &worker_limits, |batch| {
                        let mut event = Event::Batch(batch);
                        loop {
                            worker_limits.checkpoint()?;
                            match worker_sender.try_send(event) {
                                Ok(()) => return Ok(()),
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    return Err(ExecutionError::Cancelled);
                                }
                                Err(mpsc::error::TrySendError::Full(value)) => event = value,
                            }
                            // This is a native pool thread, never a Tokio worker.
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                    }))
                });
                let result = if let Some(deadline) = supervisor_limits.deadline {
                    if let Ok(result) = tokio::time::timeout_at(deadline.into(), job).await {
                        result
                    } else {
                        supervisor_limits.cancelled.store(true, Ordering::Release);
                        Err(RuntimeError::Cancelled)
                    }
                } else {
                    job.await
                };
                let result = match result {
                    Ok(Ok(receipt)) => Ok(receipt),
                    Ok(Err(error)) => Err(Failure::Rejected(error)),
                    Err(RuntimeError::Busy) => Err(Failure::Busy),
                    Err(RuntimeError::Cancelled) => Err(
                        if supervisor_limits
                            .deadline
                            .is_some_and(|d| std::time::Instant::now() >= d)
                        {
                            Failure::Deadline
                        } else {
                            Failure::Cancelled
                        },
                    ),
                    Err(_) => Err(Failure::Worker),
                };
                let code = result.as_ref().map_or_else(Failure::code, |_| "complete");
                observation.record(code);
                let event = match result {
                    Ok(receipt) => Event::Complete(Box::new(receipt)),
                    Err(error) => Event::Failed(error),
                };
                let _ = sender.send(event).await;
            }
            .instrument(span),
        );
        Ok(SemanticStream {
            receiver,
            task,
            limits,
        })
    }
}
