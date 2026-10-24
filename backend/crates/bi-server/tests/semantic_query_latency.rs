//! Reproducible latency of `POST /api/v1/semantic/query` (#138) through the
//! real router, with a dev session on PostgreSQL and the Issue's rows on
//! in-memory DuckDB. Reported per request:
//! - `explain`: the same preparation and the same confirmed audit event,
//!   without executing — the honest baseline of the route;
//! - `query_by_region` and `query_10000_rows`: preparation, execution,
//!   encoding, the confirmed audit event and the response.
//!
//! The difference between `explain` and a query is execution and encoding;
//! a refusal is never compared with an answer. Run explicitly, against a
//! disposable PostgreSQL:
//! `cargo test --release -p bi-server --no-default-features --features dev-auth,postgres-tests --test semantic_query_latency -- --ignored --nocapture`
#![cfg(all(feature = "postgres-tests", feature = "dev-auth"))]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use axum::{Router, body::Body, http::Request};
use bi_runtime::RuntimeConfig;
use bi_server::auth::AuthConfig;
use bi_server::metadata::MetadataMode;
use bi_server::models::Models;
use bi_server::{AppState, analytics::Analytics, db::Database, router};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use sysinfo::System;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:8080";
const WARMUP: usize = 20;
const SAMPLES: usize = 300;

fn field(name: &str, extra: &str) -> String {
    format!(
        "      - name: {name}\n{extra}        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: {name}\n"
    )
}

/// The Issue's model: `ventas` with its five members and three metrics.
fn model() -> String {
    format!(
        "version: '0.2.0.dev0'\nname: Ventas\ndatasets:\n  - name: ventas\n    source: lake.main.ventas\n    primary_key: [id]\n    fields:\n{}{}{}{}{}metrics:\n  - name: importe_total\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: SUM(ventas.importe)\n  - name: filas\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: COUNT(ventas.id)\n",
        field("id", ""),
        field("region", "        dimension: {}\n"),
        field("ciudad", "        dimension: {}\n"),
        field("importe", ""),
        field(
            "instante",
            "        datatype: DateTimeTz\n        dimension: {}\n"
        ),
    )
}

fn project() -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    std::fs::create_dir(p.path().join("metrics")).unwrap();
    std::fs::create_dir(p.path().join("policies")).unwrap();
    std::fs::write(p.path().join("metrics/ventas.yaml"), model()).unwrap();
    std::fs::write(
        p.path().join("policies/members.yaml"),
        "version: 1\nusers:\n  viewer: [viewer]\n",
    )
    .unwrap();
    p
}

/// The Issue's four rows, and 10 500 more cities for the largest answer.
async fn analytics() -> Analytics {
    let runtime = tokio::task::spawn_blocking(|| {
        bi_runtime::Bootstrap::open(RuntimeConfig::in_memory(2))
            .unwrap()
            .finish()
            .unwrap()
    })
    .await
    .unwrap();
    runtime
        .run(|conn| {
            conn.execute_batch(
                "ATTACH ':memory:' AS lake; CREATE TABLE lake.main.ventas (id BIGINT NOT NULL, \
                 region VARCHAR NOT NULL, ciudad VARCHAR NOT NULL, importe DECIMAL(18,2) NOT NULL, \
                 instante TIMESTAMPTZ NOT NULL, nota VARCHAR); INSERT INTO lake.main.ventas VALUES \
                 (1, 'Norte', 'La Paz', 100.10, '2026-01-01T00:00:00Z', NULL), \
                 (2, 'Norte', 'El Alto', 0.20, '2026-01-02T00:00:00Z', 'n'), \
                 (3, 'Sur', 'Tarija', 200.20, '2026-01-01T00:00:00Z', 's'), \
                 (4, 'Sur', 'Sucre', -0.20, '2026-01-02T00:00:00Z', NULL); \
                 INSERT INTO lake.main.ventas SELECT 100 + i, 'Norte', 'c' || i, 1.00, \
                 TIMESTAMPTZ '2026-01-03 00:00:00+00', NULL FROM range(10500) t(i)",
            )
        })
        .await
        .unwrap();
    Analytics::ready(runtime)
}

async fn login(app: &Router, user: &str) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/dev/login")
                .header("origin", ORIGIN)
                .header("content-type", "application/json")
                .body(Body::from(json!({ "user_id": user }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

/// Milliseconds of each request after the warm-up, the first one's, and
/// the bytes of the last body.
async fn measure(app: &Router, cookie: &str, path: &str, body: &Value) -> (Vec<f64>, f64, usize) {
    let mut samples = Vec::with_capacity(SAMPLES);
    let mut cold = 0.0;
    let mut bytes = 0;
    for i in 0..WARMUP + SAMPLES {
        let request = Request::post(path)
            .header("origin", ORIGIN)
            .header("content-type", "application/json")
            .header("cookie", cookie)
            .body(Body::from(body.to_string()))
            .unwrap();
        let started = Instant::now();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
        bytes = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .len();
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if i == 0 {
            cold = elapsed;
        }
        if i >= WARMUP {
            samples.push(elapsed);
        }
    }
    (samples, cold, bytes)
}

fn summary(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    json!({
        "p50_ms": values[values.len() / 2],
        "p95_ms": values[values.len() * 95 / 100],
        "max_ms": values[values.len() - 1],
    })
}

fn selections(selections: &Value, limit: Option<u64>) -> Value {
    let mut query = json!({ "selections": selections });
    if let Some(limit) = limit {
        query["limit"] = json!(limit);
        query["order_by"] = json!([{ "selection": "c" }]);
    }
    json!({ "model": "metrics/ventas.yaml", "query": query })
}

#[sqlx::test(migrations = false)]
#[ignore = "measurement, not a CI latency threshold"]
async fn semantic_query_latency(pool: PgPool) {
    let db = Database::start(&pool.connect_options(), MetadataMode::Development);
    assert!(db.wait_until_ready().await);
    let auth = AuthConfig::from_lookup(&|key| match key {
        "BI_PUBLIC_ORIGIN" => Some(ORIGIN.to_owned()),
        "BI_DEV_USERS" => Some(r#"[{"id":"viewer","display_name":"Lector"}]"#.to_owned()),
        _ => None,
    })
    .unwrap();
    let p = project();
    let app = router(AppState {
        db: db.clone(),
        auth,
        analytics: analytics().await,
        query_timeout: Duration::from_secs(10),
        models: Models::new(Some(p.path().to_owned())),
        ..AppState::new(db.clone())
    });
    let viewer = login(&app, "viewer").await;
    let metric = |id: &str, name: &str| json!({ "kind": "metric", "id": id, "name": name });
    let dimension = |id: &str, name: &str| json!({ "kind": "dimension", "id": id, "name": name });
    let by_region = selections(
        &json!([
            dimension("r", "ventas.region"),
            metric("t", "importe_total"),
            metric("f", "filas")
        ]),
        None,
    );
    let by_city = selections(
        &json!([dimension("c", "ventas.ciudad"), metric("f", "filas")]),
        Some(10_000),
    );
    let mut report = BTreeMap::new();
    for (kind, path, body) in [
        ("explain_by_region", "/api/v1/semantic/explain", &by_region),
        ("query_by_region", "/api/v1/semantic/query", &by_region),
        ("query_10000_rows", "/api/v1/semantic/query", &by_city),
    ] {
        let (samples, cold, bytes) = measure(&app, &viewer, path, body).await;
        report.insert(
            kind,
            json!({"cold_ms": cold, "warm": summary(samples), "body_bytes": bytes}),
        );
    }
    let system = System::new_all();
    println!(
        "{}",
        json!({
            "task": "138",
            "route": "POST /api/v1/semantic/query, router+session+audit",
            "os": System::long_os_version(),
            "cpu": system.cpus()[0].brand(),
            "logical_cpus": system.cpus().len(),
            "ram_bytes": system.total_memory(),
            "fixture_rows": 10_504,
            "warmup": WARMUP,
            "samples": SAMPLES,
            "report": report,
        })
    );
    db.close().await;
}
