//! Reproducible latency of the admitted G1 query route on a real DuckLake
//! (#118 keeps it open only for the local synthetic demo). The same requests
//! are reported two ways:
//! - `handler`: the phases the handler times itself (`Server-Timing`: plan,
//!   queue, execute, serialize);
//! - `http`: the whole request through the real router, with the session
//!   lookup in PostgreSQL, the Origin check and the admission.
//!
//! It also times the server's bootstrap of the fixture, fresh and again on
//! the same lake (where the fixture is compared row by row before it is
//! vouched for). Run explicitly, against a disposable PostgreSQL and catalog:
//! `cargo test --release -p bi-server --features dev-auth,postgres-tests --test query_latency -- --ignored --nocapture`
#![cfg(all(feature = "postgres-tests", feature = "dev-auth"))]

use std::time::{Duration, Instant};

use axum::{Router, body::Body, http::Request};
use bi_fixtures::{
    oracle::Oracle,
    sales::{self, FixtureSpec},
};
use bi_runtime::{LakeConfig, RuntimeConfig};
use bi_server::auth::AuthConfig;
use bi_server::metadata::MetadataMode;
use bi_server::{AppState, analytics::Analytics, db::Database, router};
use http_body_util::BodyExt;
use serde_json::json;
use sqlx::PgPool;
use sysinfo::{ProcessesToUpdate, System};
use tower::ServiceExt;

const SPEC: FixtureSpec = FixtureSpec {
    seed: 42,
    rows: 200_000,
};
const ORIGIN: &str = "http://127.0.0.1:8080";

fn config(data_path: &std::path::Path) -> RuntimeConfig {
    RuntimeConfig {
        lake: Some(LakeConfig {
            catalog: std::env::var("BI_TEST_LAKE_CATALOG")
                .unwrap_or_else(|_| bi_runtime::config::DEV_LAKE_CATALOG.to_owned()),
            metadata_schema: Some(format!("bi_bench_{}", std::process::id())),
            data_path: data_path.to_owned(),
        }),
        duckdb_threads: Some(4),
        query_memory_limit: std::env::var("BI_BENCH_QUERY_MEMORY_LIMIT").ok(),
        ..RuntimeConfig::in_memory(2)
    }
}

/// The server's own bootstrap of the fixture, and how long it took.
async fn bootstrap(data_path: &std::path::Path) -> (Analytics, f64) {
    let started = Instant::now();
    let analytics = Analytics::start(config(data_path), Some(SPEC));
    assert!(analytics.wait_until_ready().await);
    (analytics, started.elapsed().as_secs_f64() * 1000.0)
}

async fn sample(app: &Router, cookie: &str, oracle: &Oracle) -> (f64, String, usize) {
    let request = Request::post("/api/v1/queries")
        .header("content-type", "application/json")
        .header("cookie", cookie)
        .header("origin", ORIGIN)
        .body(Body::from(
            r#"{"dataset":"sales","measures":["row_count","amount_sum","quantity_sum"]}"#,
        ))
        .unwrap();
    let started = Instant::now();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    let timing = response.headers()["server-timing"]
        .to_str()
        .unwrap()
        .to_owned();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["rows"],
        json!([[
            oracle.totals.row_count,
            oracle.totals.amount.sum,
            oracle.totals.quantity.sum
        ]])
    );
    (elapsed, timing, body.len())
}

#[sqlx::test(migrations = false)]
#[ignore = "measurement, not a CI latency threshold"]
#[allow(clippy::too_many_lines)] // One protocol, kept in one place.
async fn query_latency(pool: PgPool) {
    let data = tempfile::tempdir().unwrap();
    let oracle = Oracle::from_rows(SPEC, sales::rows(SPEC));
    let (first, fresh_ms) = bootstrap(data.path()).await;
    drop(first);
    let (analytics, again_ms) = bootstrap(data.path()).await;

    let db = Database::start(&pool.connect_options(), MetadataMode::Development);
    assert!(db.wait_until_ready().await);
    let auth = AuthConfig::from_lookup(&|key| match key {
        "BI_PUBLIC_ORIGIN" => Some(ORIGIN.to_owned()),
        "BI_DEV_USERS" => Some(r#"[{"id":"analyst","display_name":"Analista"}]"#.to_owned()),
        _ => None,
    })
    .unwrap();
    let sdk = std::env::var("BI_BENCH_METRICS").is_ok_and(|mode| mode == "sdk");
    let provider = sdk.then(|| {
        opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_periodic_exporter(opentelemetry_sdk::metrics::InMemoryMetricExporter::default())
            .build()
    });
    let metrics =
        provider
            .as_ref()
            .map_or_else(bi_server::metrics::QueryMetrics::default, |provider| {
                use opentelemetry::metrics::MeterProvider;
                bi_server::metrics::QueryMetrics::new(&provider.meter("bench"), &analytics)
            });
    let app = router(AppState {
        db: db.clone(),
        auth,
        analytics,
        metrics,
        query_timeout: Duration::from_secs(10),
        models: bi_server::models::Models::default(),
        context_key: bi_server::semantic::context::ContextKey::generate(),
    });
    let login = app
        .clone()
        .oneshot(
            Request::post("/api/v1/dev/login")
                .header("origin", ORIGIN)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"user_id":"analyst"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    let mut samples = Vec::new();
    let mut phases = std::collections::BTreeMap::<String, Vec<f64>>::new();
    let mut system = System::new_all();
    let pid = sysinfo::get_current_pid().unwrap();
    let mut rss_peak = 0;
    let mut bytes = 0;
    let mut cold_ms = 0.0;
    let started_all = Instant::now();
    for i in 0..520 {
        let (elapsed, timing, size) = sample(&app, &cookie, &oracle).await;
        if i == 0 {
            cold_ms = elapsed;
        }
        bytes = size;
        if i >= 20 {
            samples.push(elapsed);
            let mut handler = 0.0;
            for phase in timing.split(", ") {
                let (name, value) = phase.split_once(";dur=").unwrap();
                let value: f64 = value.parse().unwrap();
                handler += value;
                phases.entry(name.to_owned()).or_default().push(value);
            }
            phases.entry("total".to_owned()).or_default().push(handler);
        }
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);
        rss_peak = rss_peak.max(system.process(pid).unwrap().memory());
    }
    let wall_seconds = started_all.elapsed().as_secs_f64();
    let summary = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        json!({"p50_ms": values[values.len()/2], "p95_ms": values[values.len()*95/100], "p99_ms": values[values.len()*99/100]})
    };
    let handler: std::collections::BTreeMap<_, _> =
        phases.into_iter().map(|(k, v)| (k, summary(v))).collect();
    println!(
        "{}",
        json!({"task": "118", "route": "router+session+origin+admission", "query_memory_limit": std::env::var("BI_BENCH_QUERY_MEMORY_LIMIT").ok(), "mode": if sdk { "sdk" } else { "noop" }, "os": System::long_os_version(), "cpu": system.cpus()[0].brand(), "ram_bytes": system.total_memory(), "logical_cpus": system.cpus().len(), "seed": SPEC.seed, "rows": SPEC.rows, "workers": 2, "duckdb_threads": 4, "warmup": 20, "samples": 500, "http": summary(samples), "handler": handler, "cold_ms": cold_ms, "bootstrap_fresh_ms": fresh_ms, "bootstrap_same_lake_ms": again_ms, "rss_peak_bytes_sampled": rss_peak, "response_bytes": bytes, "wall_seconds_including_assertions_and_rss": wall_seconds, "oracle": true})
    );
    if let Some(provider) = provider {
        tokio::task::spawn_blocking(move || provider.shutdown().unwrap())
            .await
            .unwrap();
    }
    db.close().await;
}
