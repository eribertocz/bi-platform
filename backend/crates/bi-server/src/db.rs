//! PostgreSQL metadata database: the connection pools, the administrative
//! migration step and the single admission state that readiness and every
//! `/api/v1` handler consult (#115).

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Meter};
use sqlx::migrate::{MigrateError, Migrator};
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgListener, PgPool, PgPoolOptions};
use sqlx::{ConnectOptions as _, Connection as _, Postgres, Transaction};
use tokio::sync::{Notify, watch};
use tracing::instrument::WithSubscriber as _;

use crate::metadata::{self, Assessment, MetadataMode};

pub static MIGRATOR: Migrator = sqlx::migrate!();

/// Connections that serve metadata work; each validates itself on opening.
const WORK_CONNECTIONS: u32 = 10;
/// The idle sentinel that notices a lost database, and the readiness ping.
const PROBE_CONNECTIONS: u32 = 2;
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);
/// Upper bound for a readiness probe, so a dead database never stalls `/health/ready`.
const PING_TIMEOUT: Duration = Duration::from_secs(1);
/// Upper bound for one validation, so a hung server leaves the state bounded.
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(10);
/// Development migrates inside a validation, so within part of its bound.
const DEVELOPMENT_MIGRATION_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound for the whole `bi-server migrate` command, lock waits included:
/// the default changelog-lock wait of Liquibase.
pub const MIGRATION_TIMEOUT: Duration = Duration::from_secs(300);
/// What the client allows beyond PostgreSQL's own timeouts before giving up.
const MIGRATION_GRACE: Duration = Duration::from_secs(5);
const RETRY_MIN: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(10);

/// Builds a pool that connects on first use, for the audit commands.
///
/// # Errors
/// Returns an error when `url` is not a valid PostgreSQL connection URL.
pub fn lazy_pool(url: &str) -> Result<PgPool, sqlx::Error> {
    let options: PgConnectOptions = url.parse()?;
    Ok(PgPoolOptions::new()
        .acquire_timeout(ACQUIRE_TIMEOUT)
        .connect_lazy_with(options))
}

/// Every metadata connection: `SQLx` logs no statement, ordinary or slow, so
/// no SQL text reaches the operator's logs.
fn quiet(options: &PgConnectOptions) -> PgConnectOptions {
    options.clone().disable_statement_logging()
}

/// Why [`migrate`] did not complete.
#[derive(Debug)]
pub enum MigrationError {
    /// The bound elapsed, waiting for a lock or running a statement.
    TimedOut,
    /// The database was unreachable or a migration failed.
    Failed(MigrateError),
}

impl fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimedOut => formatter.write_str("metadata migration timed out"),
            Self::Failed(_) => formatter.write_str("metadata migration failed"),
        }
    }
}

impl std::error::Error for MigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TimedOut => None,
            Self::Failed(error) => Some(error),
        }
    }
}

/// Applies the embedded migrations with the lock and checksums of `SQLx`:
/// the administrative step (`bi-server migrate`, or development start-up).
///
/// The whole operation ends within `limit`. PostgreSQL itself stops every
/// lock wait and statement at `limit`, so no wait outlives the command on
/// the server; the client gives up shortly after, should the server not
/// answer at all.
///
/// `SQLx` names its history table without a schema; this pins it to
/// `public._sqlx_migrations`, where the restricted server reads it, through
/// a dedicated single-connection pool closed afterwards, so no connection
/// that serves requests ever carries these settings.
///
/// # Errors
/// [`MigrationError::TimedOut`] when `limit` elapsed, otherwise the
/// database or migration error.
pub async fn migrate(options: &PgConnectOptions, limit: Duration) -> Result<(), MigrationError> {
    let milliseconds = limit.as_millis().to_string();
    let options = quiet(options).options([
        ("search_path", "public"),
        ("lock_timeout", milliseconds.as_str()),
        ("statement_timeout", milliseconds.as_str()),
    ]);
    let administrative = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(ACQUIRE_TIMEOUT.min(limit))
        .connect_lazy_with(options);
    let applied =
        tokio::time::timeout(limit + MIGRATION_GRACE, MIGRATOR.run(&administrative)).await;
    // Dropping the pool afterwards closes whatever this bound leaves open.
    tokio::time::timeout(MIGRATION_GRACE, administrative.close())
        .await
        .ok();
    match applied {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) if timed_out(&error) => Err(MigrationError::TimedOut),
        Ok(Err(error)) => Err(MigrationError::Failed(error)),
        Err(_) => Err(MigrationError::TimedOut),
    }
}

/// Whether PostgreSQL ended the migration for its `lock_timeout`
/// (`lock_not_available`) or `statement_timeout` (`query_canceled`).
fn timed_out(error: &MigrateError) -> bool {
    let (MigrateError::Execute(sqlx::Error::Database(error))
    | MigrateError::ExecuteMigration(sqlx::Error::Database(error), _)) = error
    else {
        return false;
    };
    matches!(error.code().as_deref(), Some("55P03" | "57014"))
}

/// The migrations this build expects, as `(version, checksum)`.
fn embedded() -> Vec<(i64, &'static [u8])> {
    MIGRATOR
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| (migration.version, migration.checksum.as_ref()))
        .collect()
}

/// The admission state. There is no interval of admission before the first
/// validation, and none after a lost connection until a new one validates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataStatus {
    /// Nothing has been validated yet.
    Validating,
    /// PostgreSQL could not be reached or inspected, or the pool is closed.
    Unavailable,
    /// The verdict of the last inspection, admitted or not.
    Assessed(Assessment),
}

impl MetadataStatus {
    #[must_use]
    pub fn admits(&self) -> bool {
        matches!(self, Self::Assessed(assessment) if assessment.admits())
    }
}

/// `bi.metadata.validation.attempts{result=ready|invalid|unavailable}`:
/// closed labels only, never a role, URL or database error.
#[derive(Clone, Default)]
pub struct MetadataMetrics {
    attempts: Option<Counter<u64>>,
}

impl MetadataMetrics {
    #[must_use]
    pub fn new(meter: &Meter) -> Self {
        Self {
            attempts: Some(
                meter
                    .u64_counter("bi.metadata.validation.attempts")
                    .with_description("Metadata role and schema validations by result")
                    .build(),
            ),
        }
    }

    fn record(&self, status: &MetadataStatus) {
        let result = match status {
            MetadataStatus::Assessed(assessment) if assessment.admits() => "ready",
            MetadataStatus::Assessed(_) => "invalid",
            MetadataStatus::Validating | MetadataStatus::Unavailable => "unavailable",
        };
        if let Some(attempts) = &self.attempts {
            attempts.add(1, &[KeyValue::new("result", result)]);
        }
    }
}

/// Why no metadata connection was handed out.
#[derive(Debug)]
pub enum AcquireError {
    /// Work is not admitted, or was withdrawn while the connection was
    /// prepared (for instance by that very connection failing validation).
    NotAdmitted,
    /// The pool could not provide a connection.
    Unavailable(sqlx::Error),
}

impl AcquireError {
    /// The problem code to answer with: `metadata_not_ready` when work is
    /// not admitted, otherwise the caller's own code for a store failure.
    #[must_use]
    pub fn problem_code(&self, unavailable: &'static str) -> &'static str {
        match self {
            Self::NotAdmitted => "metadata_not_ready",
            Self::Unavailable(_) => unavailable,
        }
    }
}

impl fmt::Display for AcquireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAdmitted => formatter.write_str("metadata work not admitted"),
            Self::Unavailable(_) => formatter.write_str("metadata database unavailable"),
        }
    }
}

impl std::error::Error for AcquireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotAdmitted => None,
            Self::Unavailable(error) => Some(error),
        }
    }
}

#[derive(Debug, Clone)]
struct Observed {
    /// Bumped by every invalidation, so a validation that started before it
    /// can never publish a stale admission afterwards.
    epoch: u64,
    status: MetadataStatus,
}

struct Shared {
    status: watch::Sender<Observed>,
    /// Wakes the supervisor when something outside it saw the database fail.
    disrupted: Notify,
}

impl Shared {
    fn epoch(&self) -> u64 {
        self.status.borrow().epoch
    }

    fn admits(&self) -> bool {
        self.status.borrow().status.admits()
    }

    /// Publishes a validation unless it was invalidated meanwhile; returns
    /// whether the published state admits work.
    fn publish(&self, epoch: u64, status: MetadataStatus) -> bool {
        let admits = status.admits();
        let mut current = false;
        self.status.send_if_modified(|observed| {
            if observed.epoch != epoch {
                return false;
            }
            current = true;
            if observed.status == status {
                return false;
            }
            log_transition(&status);
            observed.status = status;
            true
        });
        current && admits
    }

    /// Stops admitted work after an observer outside the supervisor saw the
    /// database fail, and wakes the supervisor to revalidate. Does nothing
    /// when work is not admitted, so frequent failing probes cannot keep
    /// discarding a validation that is already under way.
    fn invalidate(&self) {
        let demoted = self.status.send_if_modified(|observed| {
            if !observed.status.admits() {
                return false;
            }
            observed.epoch += 1;
            log_transition(&MetadataStatus::Unavailable);
            observed.status = MetadataStatus::Unavailable;
            true
        });
        if demoted {
            self.disrupted.notify_one();
        }
    }

    /// Stops admission until the next validation succeeds.
    fn demote(&self) {
        self.status.send_modify(|observed| {
            observed.epoch += 1;
            if observed.status != MetadataStatus::Unavailable {
                log_transition(&MetadataStatus::Unavailable);
                observed.status = MetadataStatus::Unavailable;
            }
        });
    }
}

fn log_transition(status: &MetadataStatus) {
    match status {
        MetadataStatus::Validating => {}
        MetadataStatus::Unavailable => tracing::warn!(
            event_code = "metadata_unavailable",
            "metadata database unavailable; admission stopped until revalidated"
        ),
        MetadataStatus::Assessed(assessment) if assessment.admits() && assessment.secure() => {
            tracing::info!(event_code = "metadata_ready", "metadata database ready");
        }
        MetadataStatus::Assessed(assessment) if assessment.admits() => {
            let forbidden: Vec<String> = assessment
                .forbidden
                .iter()
                .map(ToString::to_string)
                .collect();
            tracing::warn!(
                event_code = "metadata_privileged_development",
                forbidden = forbidden.join(","),
                "development mode is using a privileged metadata role; never deploy it"
            );
        }
        MetadataStatus::Assessed(assessment) => tracing::warn!(
            event_code = "metadata_not_ready",
            reason = assessment.reason().unwrap_or_default(),
            "metadata database not ready; application operations refused"
        ),
    }
}

/// Inspects the schema and the role as seen by `connection`.
async fn assess(connection: &mut PgConnection, mode: MetadataMode) -> MetadataStatus {
    match metadata::inspect::facts(connection).await {
        Ok(facts) => MetadataStatus::Assessed(Assessment::of(&facts, &embedded(), mode)),
        Err(_) => MetadataStatus::Unavailable,
    }
}

/// Validates every physical connection of the work pool as it opens, so a
/// connection the pool replaces (one lost backend, a restart, a failover)
/// never serves before its own schema and role check. One inspection per
/// opened connection; none per request.
#[derive(Clone)]
struct Gate {
    shared: Arc<Shared>,
    mode: MetadataMode,
    metrics: MetadataMetrics,
}

impl Gate {
    async fn admit(&self, connection: &mut PgConnection) -> Result<(), sqlx::Error> {
        // Nothing may use the work pool while admission is off.
        if !self.shared.admits() {
            return Err(refused());
        }
        let status = assess(connection, self.mode).await;
        self.metrics.record(&status);
        if status.admits() {
            return Ok(());
        }
        let reason = match &status {
            MetadataStatus::Assessed(assessment) => assessment.reason().unwrap_or_default(),
            MetadataStatus::Validating | MetadataStatus::Unavailable => "unavailable".to_owned(),
        };
        tracing::warn!(
            event_code = "metadata_connection_refused",
            reason,
            "a new metadata connection failed validation; admission stopped until revalidated"
        );
        self.shared.invalidate();
        Err(refused())
    }
}

/// What `SQLx` reports, and retries, for a connection the gate refused.
fn refused() -> sqlx::Error {
    sqlx::Error::Configuration("metadata_connection_refused".into())
}

#[derive(Clone)]
pub struct Database {
    /// Serves metadata work: handlers reach it through [`Self::acquire`]
    /// and [`Self::begin`].
    work: PgPool,
    /// The sentinel and the readiness ping; never serves metadata work.
    probe: PgPool,
    mode: MetadataMode,
    shared: Arc<Shared>,
}

impl Database {
    /// Connects lazily and validates in the background; see [`Self::start_observed`].
    #[must_use]
    pub fn start(options: &PgConnectOptions, mode: MetadataMode) -> Self {
        Self::start_observed(options, mode, MetadataMetrics::default())
    }

    /// Connects to `options` on first use and starts the supervisor that
    /// validates the schema and the role on its own connection, publishes
    /// the admission state, notices a lost database and revalidates before
    /// admitting again. Each connection of the work pool also validates
    /// itself when it opens and withdraws admission if it fails. Only
    /// [`MetadataMode::Development`] migrates. The supervisor ends when the
    /// pools close ([`Self::close`]).
    #[must_use]
    pub fn start_observed(
        options: &PgConnectOptions,
        mode: MetadataMode,
        metrics: MetadataMetrics,
    ) -> Self {
        let options = quiet(options);
        let (status, _) = watch::channel(Observed {
            epoch: 0,
            status: MetadataStatus::Validating,
        });
        let shared = Arc::new(Shared {
            status,
            disrupted: Notify::new(),
        });
        let gate = Gate {
            shared: shared.clone(),
            mode,
            metrics: metrics.clone(),
        };
        let work = PgPoolOptions::new()
            .max_connections(WORK_CONNECTIONS)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .after_connect(move |connection, _| {
                let gate = gate.clone();
                Box::pin(async move { gate.admit(connection).await })
            })
            .connect_lazy_with(options.clone());
        let probe = PgPoolOptions::new()
            .max_connections(PROBE_CONNECTIONS)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .connect_lazy_with(options.clone());
        let supervisor = Supervisor {
            options,
            work: work.clone(),
            probe: probe.clone(),
            mode,
            shared: shared.clone(),
            metrics,
        };
        // The supervisor logs through the subscriber of whoever started it.
        tokio::spawn(supervisor.run().with_current_subscriber());
        Self {
            work,
            probe,
            mode,
            shared,
        }
    }

    /// The work pool. Handlers use [`Self::acquire`] or [`Self::begin`],
    /// which also answer at once when admission is withdrawn.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.work
    }

    #[must_use]
    pub fn mode(&self) -> MetadataMode {
        self.mode
    }

    /// Closes both pools; the supervisor ends with them.
    pub async fn close(&self) {
        self.work.close().await;
        self.probe.close().await;
    }

    #[must_use]
    pub fn status(&self) -> MetadataStatus {
        if self.work.is_closed() {
            return MetadataStatus::Unavailable;
        }
        self.shared.status.borrow().status.clone()
    }

    /// Whether application work may run now. Reads the prepared state; never
    /// queries PostgreSQL.
    #[must_use]
    pub fn admits(&self) -> bool {
        !self.work.is_closed() && self.shared.admits()
    }

    /// A connection for metadata work, handed out only while work is admitted.
    ///
    /// # Errors
    /// [`AcquireError::NotAdmitted`] when work is not admitted or stops
    /// being admitted meanwhile, otherwise the pool error.
    pub async fn acquire(&self) -> Result<PoolConnection<Postgres>, AcquireError> {
        self.admitted(self.work.acquire()).await
    }

    /// A transaction for metadata work, under the rules of [`Self::acquire`].
    ///
    /// # Errors
    /// As [`Self::acquire`].
    pub async fn begin(&self) -> Result<Transaction<'static, Postgres>, AcquireError> {
        self.admitted(self.work.begin()).await
    }

    /// Races `preparing` against admission being withdrawn, which a new
    /// connection failing its own validation does, so a refused connection
    /// answers at once instead of after the acquire timeout. Reads the
    /// prepared state; never queries PostgreSQL.
    async fn admitted<T>(
        &self,
        preparing: impl Future<Output = Result<T, sqlx::Error>>,
    ) -> Result<T, AcquireError> {
        let mut status = self.shared.status.subscribe();
        let prepared = tokio::select! {
            biased;
            _ = status.wait_for(|observed| !observed.status.admits()) => {
                return Err(AcquireError::NotAdmitted);
            }
            prepared = preparing => prepared.map_err(AcquireError::Unavailable)?,
        };
        if self.admits() {
            Ok(prepared)
        } else {
            Err(AcquireError::NotAdmitted)
        }
    }

    /// Resolves once work is admitted; `false` if the pool closes first.
    pub async fn wait_until_ready(&self) -> bool {
        let mut status = self.shared.status.subscribe();
        tokio::select! {
            admitted = status.wait_for(|observed| observed.status.admits()) => admitted.is_ok(),
            () = self.work.close_event() => false,
        }
    }

    /// Whether PostgreSQL answers a trivial query within [`PING_TIMEOUT`]. A
    /// failure stops admitted work and asks the supervisor to revalidate.
    pub async fn ping(&self) -> bool {
        let query = sqlx::query("SELECT 1").execute(&self.probe);
        let answered = matches!(tokio::time::timeout(PING_TIMEOUT, query).await, Ok(Ok(_)));
        if !answered {
            self.shared.invalidate();
        }
        answered
    }
}

struct Supervisor {
    options: PgConnectOptions,
    work: PgPool,
    probe: PgPool,
    mode: MetadataMode,
    shared: Arc<Shared>,
    metrics: MetadataMetrics,
}

impl Supervisor {
    async fn run(self) {
        let mut retry_in = RETRY_MIN;
        while !self.work.is_closed() {
            let epoch = self.shared.epoch();
            let status = tokio::time::timeout(VALIDATION_TIMEOUT, self.validate())
                .await
                .unwrap_or(MetadataStatus::Unavailable);
            if self.work.is_closed() {
                break;
            }
            self.metrics.record(&status);
            if self.shared.publish(epoch, status) {
                retry_in = RETRY_MIN;
                self.watch_until_disrupted().await;
                self.shared.demote();
            } else {
                tokio::select! {
                    () = tokio::time::sleep(retry_in) => {}
                    () = self.shared.disrupted.notified() => {}
                    () = self.work.close_event() => {}
                }
                retry_in = (retry_in * 2).min(RETRY_MAX);
            }
        }
        self.shared.demote();
    }

    /// Inspects on a connection of its own, outside the work pool, so the
    /// verdict is published even when work connections are refused.
    async fn validate(&self) -> MetadataStatus {
        if self.mode == MetadataMode::Development
            && migrate(&self.options, DEVELOPMENT_MIGRATION_TIMEOUT)
                .await
                .is_err()
        {
            // The inspection below still names the precise schema problem.
            tracing::warn!(
                event_code = "metadata_migration_failed",
                "development migration did not complete"
            );
        }
        let Ok(mut connection) = PgConnection::connect_with(&self.options).await else {
            return MetadataStatus::Unavailable;
        };
        let status = assess(&mut connection, self.mode).await;
        connection.close().await.ok();
        status
    }

    /// Holds one idle connection: PostgreSQL closing it (restart, failover,
    /// termination) is noticed at once, without polling. Also returns when
    /// something else withdrew admission (a failed readiness probe, a work
    /// connection refused by its validation) or the pools close.
    async fn watch_until_disrupted(&self) {
        let connected = tokio::select! {
            connected = PgListener::connect_with(&self.probe) => connected,
            () = self.shared.disrupted.notified() => return,
            () = self.work.close_event() => return,
        };
        let Ok(mut sentinel) = connected else {
            return;
        };
        sentinel.eager_reconnect(false);
        loop {
            tokio::select! {
                received = sentinel.try_recv() => {
                    if !matches!(received, Ok(Some(_))) {
                        return;
                    }
                }
                () = self.shared.disrupted.notified() => return,
                () = self.work.close_event() => return,
            }
        }
    }
}
