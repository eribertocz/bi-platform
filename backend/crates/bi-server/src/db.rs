//! PostgreSQL metadata database: connection pool, embedded migrations and
//! the state that readiness reports.

use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use tokio::sync::watch;

pub static MIGRATOR: Migrator = sqlx::migrate!();

const MAX_CONNECTIONS: u32 = 10;
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);
/// Upper bound for a readiness probe, so a dead database never stalls `/health/ready`.
const PING_TIMEOUT: Duration = Duration::from_secs(1);
const MIGRATION_RETRY_MIN: Duration = Duration::from_millis(250);
const MIGRATION_RETRY_MAX: Duration = Duration::from_secs(10);

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

#[derive(Clone)]
pub struct Database {
    pool: PgPool,
    migrated: watch::Receiver<bool>,
}

impl Database {
    /// Wraps `pool` and applies the embedded migrations in the background,
    /// retrying with backoff until they succeed.
    #[must_use]
    pub fn start(pool: PgPool) -> Self {
        let (migrated_tx, migrated) = watch::channel(false);
        tokio::spawn(migrate_until_applied(pool.clone(), migrated_tx));
        Self { pool, migrated }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    #[must_use]
    pub fn migrations_applied(&self) -> bool {
        *self.migrated.borrow()
    }

    /// Resolves once migrations are applied. Returns `false` if the migration
    /// task ended without applying them.
    pub async fn wait_until_migrated(&self) -> bool {
        self.migrated
            .clone()
            .wait_for(|applied| *applied)
            .await
            .is_ok()
    }

    /// Whether PostgreSQL answers a trivial query within [`PING_TIMEOUT`].
    pub async fn ping(&self) -> bool {
        let query = sqlx::query("SELECT 1").execute(&self.pool);
        matches!(tokio::time::timeout(PING_TIMEOUT, query).await, Ok(Ok(_)))
    }
}

async fn migrate_until_applied(pool: PgPool, migrated: watch::Sender<bool>) {
    let mut retry_in = MIGRATION_RETRY_MIN;
    loop {
        match MIGRATOR.run(&pool).await {
            Ok(()) => {
                tracing::info!(
                    migrations = MIGRATOR.iter().count(),
                    "database migrations applied"
                );
                migrated.send_replace(true);
                return;
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    retry_in_ms = retry_in.as_millis(),
                    "database migrations not applied; retrying"
                );
            }
        }
        if pool.is_closed() {
            return;
        }
        tokio::time::sleep(retry_in).await;
        retry_in = (retry_in * 2).min(MIGRATION_RETRY_MAX);
    }
}
