//! One governed execution of a semantic query (#137): the request bounded
//! by the execution profile, prepared once (#135), lowered onto the sources
//! that preparation captured, run on the analytical pool and assembled
//! (#146) only from the one stream this call starts and owns. No HTTP
//! route answers it yet (#138).
//!
//! Every phase shares one monotonic deadline, set once from admission.
//! Dropping the returned future, or passing the deadline, cancels the
//! queue, DuckDB and the Rust emulation, and discards every provisional
//! batch: only a complete, coherent execution becomes a [`SemanticRun`].

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};
use sef_adapter_duckdb::execution::{DuckDbPlan, ExecutionError, ExecutionLimits};
use sef_model_ir::time::TimestampSemantics;
use sef_model_ir::types::ValueType;
use sef_query_ir::query::SelectionItem;
use tracing::Instrument;

use super::Audit;
use super::context::{ContextKey, Scope};
use super::envelope::{self, RunError, SemanticRun};
use super::prepare::{self, Prepared, Refusal, Session, Stop};
use super::request::{ExplainRequest, SemanticOrder, SemanticQuery, SemanticSelection};
use super::result::CodecError;
use crate::AppState;
use crate::semantic_execution::{Event, Failure, SemanticExecutor, SemanticStream};

/// What one execution may use. Set by the service, never by a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budgets {
    /// The row limit of a query that sets none.
    pub default_limit: u64,
    pub max_limit: u64,
    pub max_offset: u64,
    /// Rows the executor may deliver.
    pub max_rows: usize,
    /// Arrow bytes the executor may deliver, all batches together.
    pub max_arrow_bytes: usize,
    /// Bytes of the Rust emulation's intermediate state.
    pub max_intermediate_bytes: usize,
    /// Batches in flight between the worker and this call.
    pub capacity: usize,
    /// From admission to the assembled answer, every phase included.
    pub timeout: Duration,
}

/// The initial profile (#137). The JSON body keeps its own 4 MiB and the
/// cell its 64 KiB (#136); DuckDB keeps the memory quota it was started
/// with, which bounds managed memory, not the process.
pub const INITIAL: Budgets = Budgets {
    default_limit: 1_000,
    max_limit: 10_000,
    max_offset: 10_000,
    max_rows: 10_000,
    max_arrow_bytes: 4 * 1024 * 1024,
    max_intermediate_bytes: 64 * 1024 * 1024,
    capacity: 2,
    timeout: crate::config::DEFAULT_QUERY_TIMEOUT,
};

/// A profile the service cannot run with, and which budget is wrong.
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidProfile(pub &'static str);

impl fmt::Display for InvalidProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid semantic execution budget: {}", self.0)
    }
}

impl std::error::Error for InvalidProfile {}

/// Finite, non-zero budgets, validated once when the service starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile(Budgets);

impl Profile {
    /// # Errors
    /// A zero budget, a default limit over the maximum, or a timeout no
    /// deadline can represent.
    pub fn new(budgets: Budgets) -> Result<Self, InvalidProfile> {
        let b = &budgets;
        if b.max_limit == 0 || b.default_limit == 0 || b.default_limit > b.max_limit {
            return Err(InvalidProfile("limit"));
        }
        if b.max_rows == 0 {
            return Err(InvalidProfile("rows"));
        }
        if b.max_arrow_bytes == 0 || b.max_intermediate_bytes == 0 {
            return Err(InvalidProfile("bytes"));
        }
        if b.capacity == 0 {
            return Err(InvalidProfile("capacity"));
        }
        // Both shares of the timeout, execution's and the audit's, are real.
        if (b.timeout / 4).is_zero() || Instant::now().checked_add(b.timeout).is_none() {
            return Err(InvalidProfile("timeout"));
        }
        Ok(Self(budgets))
    }

    /// [`INITIAL`], which [`Profile::new`] accepts.
    #[must_use]
    pub const fn initial() -> Self {
        Self(INITIAL)
    }

    #[must_use]
    pub const fn budgets(&self) -> &Budgets {
        &self.0
    }

    /// The part of [`Budgets::timeout`] execution may use: what is left
    /// stays for the terminal audit event, so the whole request, its
    /// confirmed audit included, ends within the timeout (#138).
    #[must_use]
    pub fn execution_timeout(&self) -> Duration {
        let timeout = self.0.timeout;
        timeout.saturating_sub((timeout / 4).min(AUDIT_RESERVE))
    }
}

/// At most this much of the timeout is kept for the terminal audit event.
pub const AUDIT_RESERVE: Duration = Duration::from_millis(500);

/// The execution service: its profile and its instruments.
#[derive(Clone)]
pub struct Runs {
    profile: Profile,
    metrics: Option<Metrics>,
}

#[derive(Clone)]
struct Metrics {
    runs: Counter<u64>,
    phases: Histogram<f64>,
    catalogs: Counter<u64>,
    catalog_phases: Histogram<f64>,
}

impl Default for Runs {
    fn default() -> Self {
        Self::new(Profile::initial())
    }
}

impl Runs {
    #[must_use]
    pub const fn new(profile: Profile) -> Self {
        Self {
            profile,
            metrics: None,
        }
    }

    /// Outcomes by stable code and phase durations, with closed labels:
    /// never an identity, a query, SQL or a value.
    #[must_use]
    pub fn with_meter(mut self, meter: &Meter) -> Self {
        self.metrics = Some(Metrics {
            runs: meter.u64_counter("bi.semantic.runs").build(),
            phases: meter
                .f64_histogram("bi.semantic.run.phase.duration")
                .with_unit("s")
                .build(),
            catalogs: meter.u64_counter("bi.semantic.catalog.requests").build(),
            catalog_phases: meter
                .f64_histogram("bi.semantic.catalog.phase.duration")
                .with_unit("s")
                .build(),
        });
        self
    }

    #[must_use]
    pub const fn profile(&self) -> &Profile {
        &self.profile
    }

    /// One answer of the session's catalog route (#140), which shares this
    /// profile's timeout: its stable code and how long its projection and
    /// its audit event took, with closed labels only.
    pub(super) fn catalog_answered(
        &self,
        code: &'static str,
        projection: Duration,
        audit: Duration,
    ) {
        if let Some(metrics) = &self.metrics {
            metrics.catalogs.add(1, &[KeyValue::new("code", code)]);
            for (phase, elapsed) in [("projection", projection), ("audit", audit)] {
                metrics
                    .catalog_phases
                    .record(elapsed.as_secs_f64(), &[KeyValue::new("phase", phase)]);
            }
        }
    }
}

/// Why no answer was produced. None of it carries rows or a receipt, SQL,
/// an engine message or a private fingerprint.
pub enum RunFailure {
    /// The preparation answered instead (#135): its outcome, as explain
    /// would give it.
    Refused(Refusal),
    /// `limit` or `offset` outside the profile.
    LimitInvalid,
    /// The decision masks values, which execution cannot apply yet.
    MaskedUnsupported,
    /// A bucket of a pure instant with no governed time semantics (#203).
    TemporalUnsupported,
    /// The engine's queue is full.
    Busy,
    Deadline,
    Cancelled,
    /// Rows, bytes, intermediates, a cell or the body over budget.
    Budget,
    /// The engine refused or failed the work, its sources included.
    Engine,
    /// The stream closed without its completion.
    Incomplete,
    /// The completion does not belong to the batches, or something
    /// followed it.
    Evidence,
    /// The rows do not fit their certified contract.
    Codec,
    /// No key, no worker or no canonical form.
    Unavailable,
}

impl RunFailure {
    /// The stable code: a closed vocabulary.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Refused(_) => "refused",
            Self::LimitInvalid => "limit_invalid",
            Self::MaskedUnsupported => "masked_unsupported",
            Self::TemporalUnsupported => "temporal_unsupported",
            Self::Busy => "busy",
            Self::Deadline => "deadline",
            Self::Cancelled => "cancelled",
            Self::Budget => "budget",
            Self::Engine => "engine",
            Self::Incomplete => "incomplete",
            Self::Evidence => "evidence",
            Self::Codec => "codec",
            Self::Unavailable => "unavailable",
        }
    }
}

impl fmt::Debug for RunFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// A complete answer, and what its audit event records: the decision and
/// the model it was taken on.
pub struct Answer {
    pub run: SemanticRun,
    pub(super) audit: Audit,
}

/// Why there is no answer, and what was decided before it failed: `None`
/// when the preparation never reached a decision, or when its refusal
/// carries its own.
pub struct Failed {
    pub failure: RunFailure,
    pub(super) audit: Option<Audit>,
    /// The redacted shape of the query as bounded, limit applied; `None`
    /// when it failed its bounds.
    pub(super) shape: Option<(u32, u32, u32, u32)>,
}

impl Failed {
    const fn before(failure: RunFailure) -> Self {
        Self {
            failure,
            audit: None,
            shape: None,
        }
    }
}

impl fmt::Debug for Failed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.failure.fmt(f)
    }
}

/// Executes `request` for `session`, admitted at `admitted`: the answer of
/// one complete execution, or why there is none. Execution ends within
/// [`Profile::execution_timeout`] of admission.
///
/// # Errors
/// A [`Failed`] for anything but a complete, coherent execution whose
/// answer fits its budgets before the deadline.
pub async fn execute(
    state: &AppState,
    session: Session,
    request: ExplainRequest,
    request_id: &str,
    admitted: Instant,
) -> Result<Answer, Failed> {
    let span = tracing::info_span!("semantic_run", request_id);
    async {
        let mut observed = Observed::new(state.semantic_runs.metrics.as_ref());
        let outcome = governed(state, session, request, request_id, admitted, &observed).await;
        observed.finish(
            outcome
                .as_ref()
                .map_or_else(|failed| failed.failure.code(), |_| "complete"),
        );
        outcome
    }
    .instrument(span)
    .await
}

async fn governed(
    state: &AppState,
    session: Session,
    mut request: ExplainRequest,
    request_id: &str,
    admitted: Instant,
    observed: &Observed<'_>,
) -> Result<Answer, Failed> {
    let profile = state.semantic_runs.profile;
    // Bounds first, then the limit into the query itself: SQIR, its
    // fingerprint and the receipt all carry the limit applied.
    bounded(&mut request.query, profile.budgets()).map_err(Failed::before)?;
    let shape = prepare::shape(&request);
    let shaped = |failure| Failed {
        shape: Some(shape),
        ..Failed::before(failure)
    };
    let deadline = admitted
        .checked_add(profile.execution_timeout())
        .ok_or_else(|| shaped(RunFailure::Deadline))?;
    if Instant::now() >= deadline {
        return Err(shaped(RunFailure::Deadline));
    }
    // Without the instance's key nothing can be bound, and nothing is read.
    if !state.context_key.is_available() {
        return Err(shaped(RunFailure::Unavailable));
    }
    let scope = Owned {
        project: state
            .models
            .directory
            .as_deref()
            .map(|directory| directory.to_string_lossy().into_owned())
            .unwrap_or_default(),
        principal: session.user.clone(),
    };
    let ready = observed
        .phase("prepare", prepared(state, session, &request, deadline))
        .await
        .map_err(|failed| Failed {
            shape: Some(shape),
            ..failed
        })?;
    let Ready { prepared, plan } = ready;
    // From here on every failure was decided on: it records the decision.
    let audit = prepared.audit().clone();
    let decided = |failure| Failed {
        audit: Some(audit.clone()),
        ..shaped(failure)
    };
    let events = observed
        .phase(
            "execute",
            executed(state, &prepared, plan, request_id, deadline),
        )
        .await
        .map_err(decided)?;
    let run = observed
        .phase(
            "assemble",
            assembled(prepared, events, state.context_key.clone(), scope, deadline),
        )
        .await
        .map_err(decided)?;
    Ok(Answer { run, audit })
}

/// The profile's limit and offset bounds. An omitted limit gets the
/// default, and a subset needs an explicit order: a query that states none
/// is ordered by its grain, every selected dimension in selection order,
/// ascending with nulls last, which is total once aggregated. An aggregate
/// with no grain is one row at most: without any pagination of its own,
/// there is no subset to choose and nothing is added.
fn bounded(query: &mut SemanticQuery, budgets: &Budgets) -> Result<(), RunFailure> {
    let grain: Vec<String> = query
        .selections
        .iter()
        .filter_map(|selection| match selection {
            SemanticSelection::Dimension { id, .. }
            | SemanticSelection::TimeDimension { id, .. } => Some(id.clone()),
            SemanticSelection::Metric { .. } => None,
        })
        .collect();
    if grain.is_empty()
        && query.limit.is_none()
        && query.offset.is_none()
        && query.order_by.is_empty()
    {
        return Ok(());
    }
    let limit = query.limit.unwrap_or(budgets.default_limit);
    if limit == 0
        || limit > budgets.max_limit
        || query
            .offset
            .is_some_and(|offset| offset > budgets.max_offset)
    {
        return Err(RunFailure::LimitInvalid);
    }
    // A limit the client wrote without an order stays the preparation's
    // refusal; only the default comes with its order.
    if query.limit.is_none() && query.order_by.is_empty() {
        query.order_by = grain
            .into_iter()
            .map(|selection| SemanticOrder {
                selection,
                descending: false,
                nulls_first: false,
            })
            .collect();
    }
    query.limit = Some(limit);
    Ok(())
}

/// A preparation and its plan, lowered from what the preparation captured.
/// Only [`prepared`] makes one, and one execution consumes it.
struct Ready {
    prepared: Prepared,
    plan: Arc<DuckDbPlan>,
}

/// Prepares and lowers `request` within `deadline`.
async fn prepared(
    state: &AppState,
    session: Session,
    request: &ExplainRequest,
    deadline: Instant,
) -> Result<Ready, Failed> {
    let deadline = tokio::time::Instant::from_std(deadline);
    let loaded = tokio::time::timeout_at(deadline, prepare::load_request(state, session, request))
        .await
        .map_err(|_| Failed::before(RunFailure::Deadline))?
        .map_err(|refusal| Failed::before(RunFailure::Refused(refusal)))?;
    // Lowered in the preparation's native work, on its single read.
    match prepare::prepare_until(state, loaded, Some(deadline), lowered).await {
        Ok(ready) => ready,
        Err(Stop::Refused(refusal)) => Err(Failed::before(RunFailure::Refused(refusal))),
        Err(Stop::Deadline) => Err(Failed::before(RunFailure::Deadline)),
        Err(Stop::Engine(bi_runtime::RuntimeError::Busy)) => Err(Failed::before(RunFailure::Busy)),
        Err(Stop::Engine(_)) => Err(Failed::before(RunFailure::Engine)),
    }
}

/// What execution can honour of `prepared`, lowered onto the sources it
/// captured; nothing of the data is read.
fn lowered(prepared: Prepared) -> Result<Ready, Failed> {
    let decided = |failure| Failed {
        audit: Some(prepared.audit().clone()),
        ..Failed::before(failure)
    };
    let plan = {
        // A masked use would return raw values: refused whole (#137).
        let execution = prepared
            .execution()
            .map_err(|prepare::Masked| decided(RunFailure::MaskedUnsupported))?;
        if ungoverned_bucket(&prepared) {
            return Err(decided(RunFailure::TemporalUnsupported));
        }
        sef_adapter_duckdb::execution::lower(
            execution.model(),
            execution.validated(),
            execution.plan(),
            execution.certificate(),
            &execution.catalog(),
        )
        .map_err(|_| decided(RunFailure::Engine))?
    };
    Ok(Ready {
        prepared,
        plan: Arc::new(plan),
    })
}

/// Whether a selection buckets a pure instant whose dimension governs no
/// time semantics: the adapter would ignore the requested zone and deliver
/// dates under an instant's type (#203).
fn ungoverned_bucket(prepared: &Prepared) -> bool {
    let dimensions = &prepared.model().definition().dimensions;
    prepared
        .validated()
        .query
        .blocks
        .iter()
        .flat_map(|block| &block.selections)
        .any(|selection| match &selection.item {
            SelectionItem::TimeDimension(time) => dimensions
                .iter()
                .find(|d| d.metadata.id() == &time.dimension.dimension)
                .is_some_and(|d| {
                    d.time_semantics.is_none()
                        && matches!(
                            d.value_type,
                            ValueType::Timestamp {
                                timezone_semantics: TimestampSemantics::Instant
                            }
                        )
                }),
            _ => false,
        })
}

/// Starts the one stream of this execution and gathers it.
async fn executed(
    state: &AppState,
    prepared: &Prepared,
    plan: Arc<DuckDbPlan>,
    request_id: &str,
    deadline: Instant,
) -> Result<Vec<Event>, RunFailure> {
    let runtime = state.analytics.runtime().ok_or(RunFailure::Unavailable)?;
    let budgets = state.semantic_runs.profile.budgets();
    let limits = ExecutionLimits {
        max_rows: budgets.max_rows,
        max_bytes: budgets.max_arrow_bytes,
        max_intermediate_bytes: budgets.max_intermediate_bytes,
        deadline: Some(deadline),
        cancelled: Arc::default(),
    };
    let mut stream = SemanticExecutor::new(runtime)
        .start(
            plan,
            prepared.context().clone(),
            request_id,
            limits,
            budgets.capacity,
        )
        .map_err(|failure| failed(&failure, deadline))?;
    gathered(&mut stream, deadline).await
}

/// Every event of `stream` in the order delivered, until it closes. The
/// first failure ends it; so does the deadline, cancelling the stream.
async fn gathered(
    stream: &mut SemanticStream,
    deadline: Instant,
) -> Result<Vec<Event>, RunFailure> {
    let until = tokio::time::Instant::from_std(deadline);
    let mut events = Vec::new();
    loop {
        match tokio::time::timeout_at(until, stream.next()).await {
            Err(_) => return Err(RunFailure::Deadline),
            Ok(None) => return Ok(events),
            Ok(Some(Event::Failed(failure))) => return Err(failed(&failure, deadline)),
            Ok(Some(event)) => events.push(event),
        }
    }
}

/// The scope of the receipt's protected fingerprints, owned to cross into
/// blocking work.
struct Owned {
    project: String,
    principal: String,
}

/// The answer, encoded off the executor and within the deadline.
async fn assembled(
    prepared: Prepared,
    events: Vec<Event>,
    key: ContextKey,
    scope: Owned,
    deadline: Instant,
) -> Result<SemanticRun, RunFailure> {
    let work = tokio::task::spawn_blocking(move || {
        let scope = Scope {
            project: &scope.project,
            principal: &scope.principal,
        };
        envelope::assemble(&prepared, events, &key, &scope)
    });
    match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), work).await {
        Err(_) => Err(RunFailure::Deadline),
        Ok(Err(_)) => Err(RunFailure::Unavailable),
        Ok(Ok(run)) => run.map_err(|error| refused_assembly(error, deadline)),
    }
}

fn failed(failure: &Failure, deadline: Instant) -> RunFailure {
    match failure {
        Failure::Busy => RunFailure::Busy,
        Failure::Deadline | Failure::Rejected(ExecutionError::Deadline) => RunFailure::Deadline,
        Failure::Cancelled | Failure::Rejected(ExecutionError::Cancelled) => {
            if Instant::now() >= deadline {
                RunFailure::Deadline
            } else {
                RunFailure::Cancelled
            }
        }
        Failure::Rejected(ExecutionError::Budget) => RunFailure::Budget,
        Failure::Rejected(_) | Failure::Worker | Failure::InvalidCapacity => RunFailure::Engine,
    }
}

fn refused_assembly(error: RunError, deadline: Instant) -> RunFailure {
    match error {
        RunError::Failed(failure) => failed(&failure, deadline),
        RunError::Incomplete => RunFailure::Incomplete,
        RunError::Evidence(_) => RunFailure::Evidence,
        RunError::Codec(CodecError::CellTooLarge | CodecError::ResultTooLarge) => {
            RunFailure::Budget
        }
        RunError::Codec(_) => RunFailure::Codec,
        RunError::Unavailable => RunFailure::Unavailable,
    }
}

/// The outcome and phases of one execution; dropped unfinished, it was
/// cancelled.
struct Observed<'a> {
    metrics: Option<&'a Metrics>,
    started: Instant,
    recorded: bool,
}

impl<'a> Observed<'a> {
    fn new(metrics: Option<&'a Metrics>) -> Self {
        Self {
            metrics,
            started: Instant::now(),
            recorded: false,
        }
    }

    async fn phase<T>(&self, phase: &'static str, work: impl Future<Output = T>) -> T {
        let started = Instant::now();
        let output = work.await;
        let elapsed = started.elapsed();
        if let Some(metrics) = self.metrics {
            metrics
                .phases
                .record(elapsed.as_secs_f64(), &[KeyValue::new("phase", phase)]);
        }
        tracing::debug!(
            phase,
            elapsed_ms = elapsed.as_millis(),
            "semantic run phase"
        );
        output
    }

    fn finish(&mut self, code: &'static str) {
        if let Some(metrics) = self.metrics {
            metrics.runs.add(1, &[KeyValue::new("code", code)]);
        }
        tracing::info!(
            code,
            elapsed_ms = self.started.elapsed().as_millis(),
            "semantic run finished"
        );
        self.recorded = true;
    }
}

impl Drop for Observed<'_> {
    fn drop(&mut self) {
        if !self.recorded {
            self.finish("cancelled");
        }
    }
}

#[cfg(test)]
mod tests;
