//! Reproducible HTTP-handler benchmark on a real DuckLake. Run explicitly:
//! `cargo test --release -p bi-server --features postgres-tests --test query_latency -- --ignored --nocapture`
#![cfg(feature = "postgres-tests")]

use std::time::{Duration, Instant};

use axum::{Router, body::Body, http::Request, routing::post};
use bi_fixtures::{
    oracle::Oracle,
    sales::{self, FixtureSpec},
};
use bi_runtime::{Bootstrap, LakeConfig, RuntimeConfig, fixtures};
use bi_server::{
    AppState,
    analytics::Analytics,
    db::{self, Database},
    query,
};
use http_body_util::BodyExt;
use serde_json::json;
use sysinfo::{ProcessesToUpdate, System};
use tower::ServiceExt;

const SPEC: FixtureSpec = FixtureSpec {
    seed: 42,
    rows: 200_000,
};

fn lake(data_path: std::path::PathBuf) -> (Analytics, Oracle) {
    let oracle = Oracle::from_rows(SPEC, sales::rows(SPEC));
    let bootstrap = Bootstrap::open(RuntimeConfig {
        lake: Some(LakeConfig {
            catalog: std::env::var("BI_TEST_LAKE_CATALOG")
                .unwrap_or_else(|_| bi_runtime::config::DEV_LAKE_CATALOG.to_owned()),
            metadata_schema: Some(format!("bi_bench_{}", std::process::id())),
            data_path,
        }),
        duckdb_threads: Some(4),
        ..RuntimeConfig::in_memory(2)
    })
    .unwrap();
    fixtures::load_sales(&bootstrap, SPEC).unwrap();
    (Analytics::ready(bootstrap.finish().unwrap()), oracle)
}

async fn sample(app: &Router, oracle: &Oracle) -> (f64, String, usize) {
    let request = Request::post("/api/v1/queries")
        .header("content-type", "application/json")
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "measurement, not a CI latency threshold"]
async fn query_latency() {
    let data = tempfile::tempdir().unwrap();
    let data_path = data.path().to_owned();
    let (analytics, oracle) = tokio::task::spawn_blocking(move || lake(data_path))
        .await
        .unwrap();
    let db = Database::start(db::lazy_pool("postgres://bi:bi@127.0.0.1:1/bi").unwrap());
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
    let app = Router::new()
        .route("/api/v1/queries", post(query::run))
        .with_state(AppState {
            analytics,
            metrics,
            query_timeout: Duration::from_secs(10),
            ..AppState::new(db)
        });
    let mut samples = Vec::new();
    let mut phases = std::collections::BTreeMap::<String, Vec<f64>>::new();
    let mut system = System::new_all();
    let pid = sysinfo::get_current_pid().unwrap();
    let mut rss_peak = 0;
    let mut bytes = 0;
    let mut cold_ms = 0.0;
    let started_all = Instant::now();
    for i in 0..520 {
        let (elapsed, timing, size) = sample(&app, &oracle).await;
        if i == 0 {
            cold_ms = elapsed;
        }
        bytes = size;
        if i >= 20 {
            samples.push(elapsed);
            for phase in timing.split(", ") {
                let (name, value) = phase.split_once(";dur=").unwrap();
                phases
                    .entry(name.to_owned())
                    .or_default()
                    .push(value.parse().unwrap());
            }
        }
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);
        rss_peak = rss_peak.max(system.process(pid).unwrap().memory());
    }
    let wall_seconds = started_all.elapsed().as_secs_f64();
    let summary = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        json!({"p50_ms": values[values.len()/2], "p95_ms": values[values.len()*95/100], "p99_ms": values[values.len()*99/100]})
    };
    let phase_report: std::collections::BTreeMap<_, _> =
        phases.into_iter().map(|(k, v)| (k, summary(v))).collect();
    println!(
        "{}",
        json!({"task": "23", "mode": if sdk { "sdk" } else { "noop" }, "os": System::long_os_version(), "cpu": system.cpus()[0].brand(), "ram_bytes": system.total_memory(), "logical_cpus": system.cpus().len(), "seed": SPEC.seed, "rows": SPEC.rows, "workers": 2, "duckdb_threads": 4, "warmup": 20, "samples": 500, "latency": summary(samples), "phases": phase_report, "cold_ms": cold_ms, "rss_peak_bytes_sampled": rss_peak, "response_bytes": bytes, "wall_seconds_including_assertions_and_rss": wall_seconds, "oracle": true})
    );
    if let Some(provider) = provider {
        tokio::task::spawn_blocking(move || provider.shutdown().unwrap())
            .await
            .unwrap();
    }
}
