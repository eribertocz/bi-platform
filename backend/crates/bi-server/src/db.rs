//! PostgreSQL metadata database: the connection pool, the administrative
//! migration step and the single admission state that readiness and every
//! `/api/v1` handler consult (#115).

use std::sync::Arc;
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Meter};
use sqlx::migrate::{MigrateError, Migrator};
use sqlx::postgres::{PgConnectOptions, PgListener, PgPool, PgPoolOptions};
use tokio::sync::{Notify, watch};
use tracing::instrument::WithSubscriber as _;

use crate::metadata::{self, Assessment, MetadataMode};

pub static MIGRATOR: Migrator = sqlx::migrate!();

/// Ten for requests plus the idle sentinel that notices a lost database.
const MAX_CONNECTIONS: u32 = 11;
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);
/// Upper bound for a readiness probe, so a dead database never stalls `/health/ready`.
const PING_TIMEOUT: Duration = Duration::from_secs(1);
/// Upper bound for one validation, so a hung server leaves the state bounded.
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_MIN: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(10);

/// Builds a pool that connects on first use, so the API can start (and stay
/// live) while PostgreSQL is unavailable.
///
/// # Errors
/// Returns an error when `url` is not a valid PostgreSQL connection URL.
pub fn lazy_pool(url: &str) -> Result<PgPool, sqlx::Error> {
    let options: PgConnectOptions = url.parse()?;
    Ok(lazy_pool_with(options))
}

#[must_use]
pub fn lazy_pool_with(options: PgConnectOptions) -> PgPool {
    PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .acquire_timeout(ACQUIRE_TIMEOUT)
        .connect_lazy_with(options)
}

/// Applies the embedded migrations with the lock and checksums of `SQLx`:
/// the administrative step (`bi-server migrate`, or development start-up).
///
/// `SQLx` names its history table without a schema; this pins it to
/// `public._sqlx_migrations`, where the restricted server reads it, through
/// a dedicated single-connection pool closed afterwards, so no connection of
/// `pool` ever carries the changed `search_path`.
///
/// # Errors
/// Returns the database or migration error.
pub async fn migrate(pool: &PgPool) -> Result<(), MigrateError> {
    let options = (*pool.connect_options())
        .clone()
        .options([("search_path", "public")]);
    let administrative = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(ACQUIRE_TIMEOUT)
        .connect_lazy_with(options);
    let applied = MIGRATOR.run(&administrative).await;
    administrative.close().await;
    applied
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

#[derive(Clone)]
pub struct Database {
    pool: PgPool,
    mode: MetadataMode,
    shared: Arc<Shared>,
}

impl Database {
    /// Wraps `pool` and validates it in the background; see [`Self::start_observed`].
    #[must_use]
    pub fn start(pool: PgPool, mode: MetadataMode) -> Self {
        Self::start_observed(pool, mode, MetadataMetrics::default())
    }

    /// Wraps `pool` and starts the supervisor that validates the schema and
    /// the role, publishes the admission state, notices a lost database and
    /// revalidates before admitting again. Only [`MetadataMode::Development`]
    /// migrates. The supervisor ends when the pool is closed.
    #[must_use]
    pub fn start_observed(pool: PgPool, mode: MetadataMode, metrics: MetadataMetrics) -> Self {
        let (status, _) = watch::channel(Observed {
            epoch: 0,
            status: MetadataStatus::Validating,
        });
        let shared = Arc::new(Shared {
            status,
            disrupted: Notify::new(),
        });
        // The supervisor logs through the subscriber of whoever started it.
        tokio::spawn(
            supervise(pool.clone(), mode, shared.clone(), metrics).with_current_subscriber(),
        );
        Self { pool, mode, shared }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    #[must_use]
    pub fn mode(&self) -> MetadataMode {
        self.mode
    }

    #[must_use]
    pub fn status(&self) -> MetadataStatus {
        if self.pool.is_closed() {
            return MetadataStatus::Unavailable;
        }
        self.shared.status.borrow().status.clone()
    }

    /// Whether application work may run now. Reads the prepared state; never
    /// queries PostgreSQL.
    #[must_use]
    pub fn admits(&self) -> bool {
        !self.pool.is_closed() && self.shared.status.borrow().status.admits()
    }

    /// Resolves once work is admitted; `false` if the pool closes first.
    pub async fn wait_until_ready(&self) -> bool {
        let mut status = self.shared.status.subscribe();
        tokio::select! {
            admitted = status.wait_for(|observed| observed.status.admits()) => admitted.is_ok(),
            () = self.pool.close_event() => false,
        }
    }

    /// Whether PostgreSQL answers a trivial query within [`PING_TIMEOUT`]. A
    /// failure stops admitted work and asks the supervisor to revalidate.
    pub async fn ping(&self) -> bool {
        let query = sqlx::query("SELECT 1").execute(&self.pool);
        let answered = matches!(tokio::time::timeout(PING_TIMEOUT, query).await, Ok(Ok(_)));
        if !answered {
            self.shared.invalidate();
        }
        answered
    }
}

async fn supervise(
    pool: PgPool,
    mode: MetadataMode,
    shared: Arc<Shared>,
    metrics: MetadataMetrics,
) {
    let mut retry_in = RETRY_MIN;
    while !pool.is_closed() {
        let epoch = shared.epoch();
        let status = tokio::time::timeout(VALIDATION_TIMEOUT, validate(&pool, mode))
            .await
            .unwrap_or(MetadataStatus::Unavailable);
        if pool.is_closed() {
            break;
        }
        metrics.record(&status);
        if shared.publish(epoch, status) {
            retry_in = RETRY_MIN;
            watch_until_disrupted(&pool, &shared).await;
            shared.demote();
        } else {
            tokio::select! {
                () = tokio::time::sleep(retry_in) => {}
                () = shared.disrupted.notified() => {}
                () = pool.close_event() => {}
            }
            retry_in = (retry_in * 2).min(RETRY_MAX);
        }
    }
    shared.demote();
}

async fn validate(pool: &PgPool, mode: MetadataMode) -> MetadataStatus {
    if mode == MetadataMode::Development && migrate(pool).await.is_err() {
        // The inspection below still names the precise schema problem.
        tracing::warn!(
            event_code = "metadata_migration_failed",
            "development migration did not complete"
        );
    }
    let facts = async {
        let mut connection = pool.acquire().await?;
        metadata::inspect::facts(&mut connection).await
    }
    .await;
    match facts {
        Ok(facts) => MetadataStatus::Assessed(Assessment::of(&facts, &embedded(), mode)),
        Err(_) => MetadataStatus::Unavailable,
    }
}

/// Holds one idle connection: PostgreSQL closing it (restart, failover,
/// termination) is noticed at once, without polling. Also returns when a
/// readiness probe fails or the pool closes.
async fn watch_until_disrupted(pool: &PgPool, shared: &Shared) {
    let Ok(mut sentinel) = PgListener::connect_with(pool).await else {
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
            () = shared.disrupted.notified() => return,
        }
    }
}
