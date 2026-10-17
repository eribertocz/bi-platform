//! Reproducible latency of `GET /api/v1/models` (#124) through the real
//! router, with a dev session on PostgreSQL and the example project compiled
//! on a real DuckLake. Reported per request kind:
//! - `authorized`: the decision, reading and compiling `metrics/`, and the
//!   audit event;
//! - `denied`: the decision and its audit event, nothing of the catalog;
//! - `unconfigured`: no project, so the session check alone (the baseline);
//! - `decision`: loading `policies/` and deciding, called directly;
//! - `audit`: appending one event in its own transaction, as the route does.
//!
//! The same file runs before #124, where every session lists. Run
//! explicitly, against a disposable PostgreSQL and catalog:
//! `cargo test --release -p bi-server --features dev-auth,postgres-tests --test models_latency -- --ignored --nocapture`
#![cfg(all(feature = "postgres-tests", feature = "dev-auth"))]

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use axum::{Router, body::Body, http::Request};
use bi_runtime::{LakeConfig, RuntimeConfig};
use bi_server::auth::AuthConfig;
use bi_server::metadata::MetadataMode;
use bi_server::models::Models;
use bi_server::{AppState, analytics::Analytics, db::Database, router};
use http_body_util::BodyExt;
use serde_json::json;
use sqlx::PgPool;
use sysinfo::System;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:8080";
const WARMUP: usize = 20;
const SAMPLES: usize = 300;
const EXAMPLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../examples/models-project"
);

/// The example project's models, with a modeler and a viewer.
fn project() -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    std::fs::create_dir(p.path().join("metrics")).unwrap();
    std::fs::create_dir(p.path().join("policies")).unwrap();
    for entry in std::fs::read_dir(Path::new(EXAMPLE).join("metrics")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(
            entry.path(),
            p.path().join("metrics").join(entry.file_name()),
        )
        .unwrap();
    }
    std::fs::write(
        p.path().join("policies/members.yaml"),
        "version: 1\nusers:\n  modeler: [modeler]\n  viewer: [viewer]\n",
    )
    .unwrap();
    p
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
/// the status every request answered.
async fn measure(app: &Router, cookie: &str) -> (Vec<f64>, f64, u16) {
    let mut samples = Vec::with_capacity(SAMPLES);
    let mut cold = 0.0;
    let mut status = 0;
    for i in 0..WARMUP + SAMPLES {
        let request = Request::get("/api/v1/models")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let started = Instant::now();
        let response = app.clone().oneshot(request).await.unwrap();
        let answered = response.status().as_u16();
        response.into_body().collect().await.unwrap();
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        assert!(i == 0 || answered == status, "{answered} after {status}");
        status = answered;
        if i == 0 {
            cold = elapsed;
        }
        if i >= WARMUP {
            samples.push(elapsed);
        }
    }
    (samples, cold, status)
}

fn summary(mut values: Vec<f64>) -> serde_json::Value {
    values.sort_by(f64::total_cmp);
    json!({
        "p50_ms": values[values.len() / 2],
        "p95_ms": values[values.len() * 95 / 100],
        "max_ms": values[values.len() - 1],
    })
}

#[sqlx::test(migrations = false)]
#[ignore = "measurement, not a CI latency threshold"]
async fn models_latency(pool: PgPool) {
    let data = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let analytics = Analytics::start(
        RuntimeConfig {
            lake: Some(LakeConfig {
                catalog: std::env::var("BI_TEST_LAKE_CATALOG")
                    .unwrap_or_else(|_| bi_runtime::config::DEV_LAKE_CATALOG.to_owned()),
                metadata_schema: Some(format!("bi_bench_models_{}", std::process::id())),
                data_path: data.path().to_owned(),
            }),
            duckdb_threads: Some(4),
            ..RuntimeConfig::in_memory(2)
        },
        Some(bi_fixtures::sales::FixtureSpec {
            seed: 42,
            rows: 1_000,
        }),
    );
    assert!(analytics.wait_until_ready().await);
    let bootstrap_ms = started.elapsed().as_secs_f64() * 1000.0;
    let db = Database::start(&pool.connect_options(), MetadataMode::Development);
    assert!(db.wait_until_ready().await);
    let auth = AuthConfig::from_lookup(&|key| match key {
        "BI_PUBLIC_ORIGIN" => Some(ORIGIN.to_owned()),
        "BI_DEV_USERS" => Some(
            r#"[{"id":"modeler","display_name":"Modelador"},{"id":"viewer","display_name":"Lector"}]"#
                .to_owned(),
        ),
        _ => None,
    })
    .unwrap();
    let p = project();
    let app = |project: Option<&Path>| {
        router(AppState {
            db: db.clone(),
            auth: auth.clone(),
            analytics: analytics.clone(),
            query_timeout: Duration::from_secs(10),
            models: Models::new(project.map(Path::to_owned)),
            ..AppState::new(db.clone())
        })
    };
    let configured = app(Some(p.path()));
    let modeler = login(&configured, "modeler").await;
    let viewer = login(&configured, "viewer").await;
    let mut report = BTreeMap::new();
    for (kind, router, cookie) in [
        ("authorized", configured.clone(), &modeler),
        ("denied", configured.clone(), &viewer),
        ("unconfigured", app(None), &modeler),
    ] {
        let (samples, cold, status) = measure(&router, cookie).await;
        report.insert(
            kind,
            json!({"status": status, "cold_ms": cold, "warm": summary(samples)}),
        );
    }
    // The decision alone, on the same files. The resource is
    // `models::catalog::resource()`, pinned by its unit test; it is spelled
    // out so this file also runs before #124.
    let resource = sef_authz::Resource {
        kind: sef_authz::ResourceKind::Model,
        id: "project:catalog".to_owned(),
        attributes: BTreeMap::from([(
            "classification".to_owned(),
            sef_authz::AttrValue::String("restricted".to_owned()),
        )]),
        parents: Vec::new(),
    };
    let directory = p.path().to_owned();
    let decision = tokio::task::spawn_blocking(move || {
        (0..WARMUP + SAMPLES)
            .map(|_| {
                let started = Instant::now();
                let access = bi_server::semantic::access::load(&directory).unwrap();
                let principal =
                    bi_server::semantic::access::principal(&access.members, "modeler", &[]);
                let decided = sef_authz::authorize(
                    &access.snapshot,
                    &principal,
                    sef_authz::Action::View,
                    &resource,
                    &sef_authz::Context::new(),
                );
                assert!(decided.allowed);
                started.elapsed().as_secs_f64() * 1000.0
            })
            .skip(WARMUP)
            .collect::<Vec<_>>()
    })
    .await
    .unwrap();
    report.insert("decision", json!({"warm": summary(decision)}));
    // One audit event alone: transaction, append to the chain, commit.
    let mut audit = Vec::with_capacity(SAMPLES);
    for i in 0..WARMUP + SAMPLES {
        let event = bi_audit::Event {
            event_id: format!("bench-{i:04}-{}", "0".repeat(32)),
            occurred_at: std::time::SystemTime::now().into(),
            actor_id: "modeler".to_owned(),
            actor_type: bi_audit::ActorType::User,
            session_id: None,
            action: bi_audit::Action::Query,
            resource_type: "project_catalog".to_owned(),
            resource_id: "project:catalog".to_owned(),
            result: bi_audit::ResultCode::Success,
            request_id: "bench".to_owned(),
            trace_id: "a".repeat(32),
            policy_decision_id: Some("bench".to_owned()),
            source: bi_audit::Source::Http,
            metadata_redacted: bi_audit::MetadataRedacted::default(),
            schema_version: bi_audit::SCHEMA_VERSION,
        };
        let started = Instant::now();
        let mut transaction = db.begin().await.unwrap();
        bi_server::audit::append(&mut transaction, &event)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        if i >= WARMUP {
            audit.push(started.elapsed().as_secs_f64() * 1000.0);
        }
    }
    report.insert("audit", json!({"warm": summary(audit)}));
    let system = System::new_all();
    println!(
        "{}",
        json!({
            "task": "124",
            "route": "GET /api/v1/models, router+session",
            "os": System::long_os_version(),
            "cpu": system.cpus()[0].brand(),
            "logical_cpus": system.cpus().len(),
            "ram_bytes": system.total_memory(),
            "project": "examples/models-project/metrics (2 files)",
            "fixture_rows": 1_000,
            "warmup": WARMUP,
            "samples": SAMPLES,
            "bootstrap_ms": bootstrap_ms,
            "report": report,
        })
    );
    db.close().await;
}
