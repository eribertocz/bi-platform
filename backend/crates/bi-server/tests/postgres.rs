//! Readiness and migrations against a real PostgreSQL. Run with
//! `cargo test --features postgres-tests` and `DATABASE_URL` pointing to a
//! server where the user may create databases (each test gets a fresh one).
#![cfg(feature = "postgres-tests")]

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use bi_server::db::{Database, MIGRATOR};
use bi_server::health::{Health, HealthCheck, HealthStatus};
use bi_server::{AppState, router};
use common::get_health;
use sqlx::PgPool;

const MIGRATION_TIMEOUT: Duration = Duration::from_secs(30);

async fn migrated_database(pool: PgPool) -> Database {
    let db = Database::start(pool);
    let migrated = tokio::time::timeout(MIGRATION_TIMEOUT, db.wait_until_migrated())
        .await
        .expect("migrations finish in time");
    assert!(migrated);
    db
}

fn health(status: HealthStatus, postgres: HealthStatus, migrations: HealthStatus) -> Health {
    let check = |name: &str, status| HealthCheck {
        name: name.to_owned(),
        status,
    };
    Health {
        status,
        checks: vec![check("postgres", postgres), check("migrations", migrations)],
    }
}

#[sqlx::test(migrations = false)]
async fn ready_after_migrations_are_applied(pool: PgPool) {
    let db = migrated_database(pool.clone()).await;
    let (status, body) = get_health(&router(AppState { db }), "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        health(HealthStatus::Ok, HealthStatus::Ok, HealthStatus::Ok)
    );

    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(applied, i64::try_from(MIGRATOR.iter().count()).unwrap());

    let schema_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.schemata WHERE schema_name = 'bi_meta')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(schema_exists);
}

#[sqlx::test(migrations = false)]
async fn migrations_are_idempotent_across_restarts(pool: PgPool) {
    migrated_database(pool.clone()).await;
    // A restarted API finds every migration applied and becomes ready.
    let db = migrated_database(pool).await;
    let (status, _) = get_health(&router(AppState { db }), "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test(migrations = false)]
async fn ready_turns_unavailable_when_database_is_lost(pool: PgPool) {
    let db = migrated_database(pool.clone()).await;
    let app = router(AppState { db });
    pool.close().await;

    let (status, body) = get_health(&app, "/health/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body,
        health(
            HealthStatus::Unavailable,
            HealthStatus::Unavailable,
            HealthStatus::Ok
        )
    );
    let (live, _) = get_health(&app, "/health/live").await;
    assert_eq!(live, StatusCode::OK);
}
