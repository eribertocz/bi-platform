//! `POST /api/v1/queries` through the real router (#118): closed outside the
//! local synthetic demo, with every authenticated refusal audited, and exact
//! on a real DuckLake when every condition holds. Run with
//! `--features dev-auth,postgres-tests`. The handler once admitted is tested
//! in `src/query/tests.rs`.
#![cfg(all(feature = "postgres-tests", feature = "dev-auth"))]

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use bi_fixtures::oracle::Oracle;
use bi_fixtures::sales::{self, FixtureSpec};
use bi_runtime::config::DEV_LAKE_CATALOG;
use bi_runtime::{LakeConfig, Runtime, RuntimeConfig};
use bi_server::analytics::Analytics;
use bi_server::auth::AuthConfig;
use bi_server::db::Database;
use bi_server::metadata::MetadataMode;
use bi_server::metrics::QueryMetrics;
use bi_server::{AppState, router};
use http_body_util::BodyExt;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:8080";
const QUERY: &str = r#"{"dataset":"sales","measures":["row_count","amount_sum"]}"#;
const SPEC: FixtureSpec = FixtureSpec {
    seed: 42,
    rows: 1_000,
};
const CLOSED: &str = "Esta consulta de demostración solo está disponible en desarrollo sintético local. El flujo semántico de consulta aún no está disponible.";
const READY: Duration = Duration::from_secs(60);

/// A DuckLake of its own: metadata schema `schema` and data under `data`.
fn lake(data: &Path, schema: &str) -> RuntimeConfig {
    RuntimeConfig {
        lake: Some(LakeConfig {
            catalog: std::env::var("BI_TEST_LAKE_CATALOG")
                .unwrap_or_else(|_| DEV_LAKE_CATALOG.to_owned()),
            metadata_schema: Some(schema.to_owned()),
            data_path: data.to_owned(),
        }),
        ..RuntimeConfig::in_memory(2)
    }
}

fn schema() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        "bi_test_q{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// The server's own bootstrap of `config` with the synthetic fixture.
async fn bootstrapped(config: RuntimeConfig) -> Analytics {
    let analytics = Analytics::start(config, Some(SPEC));
    assert!(
        tokio::time::timeout(READY, analytics.wait_until_ready())
            .await
            .unwrap()
    );
    analytics
}

/// A runtime the server did not bootstrap: same table, unknown provenance.
async fn unverified() -> Analytics {
    let runtime = tokio::task::spawn_blocking(|| {
        bi_runtime::Bootstrap::open(RuntimeConfig::in_memory(1))
            .unwrap()
            .finish()
            .unwrap()
    })
    .await
    .unwrap();
    runtime
        .run(|conn| {
            conn.execute_batch(
                "ATTACH ':memory:' AS lake;
                 CREATE TABLE lake.main.sales (order_id BIGINT, ordered_at TIMESTAMPTZ,
                     region VARCHAR, quantity BIGINT, amount DECIMAL(18,2));
                 INSERT INTO lake.main.sales VALUES (1, '2026-01-01T00:00:00Z', 'north', 1, 1.00);",
            )
        })
        .await
        .unwrap();
    Analytics::ready(runtime)
}

fn oracle() -> Oracle {
    Oracle::from_rows(SPEC, sales::rows(SPEC))
}

struct Stack {
    app: Router,
    db: Database,
    cookie: String,
    origin: &'static str,
    provider: SdkMeterProvider,
    exporter: InMemoryMetricExporter,
}

/// The real router in development metadata mode, with a synthetic dev
/// session already open.
async fn stack(
    pool: &PgPool,
    analytics: Analytics,
    origin: &'static str,
    query_timeout: Duration,
) -> Stack {
    let db = Database::start(&pool.connect_options(), MetadataMode::Development);
    assert!(
        tokio::time::timeout(READY, db.wait_until_ready())
            .await
            .unwrap()
    );
    let auth = AuthConfig::from_lookup(&|key| match key {
        "BI_PUBLIC_ORIGIN" => Some(origin.to_owned()),
        "BI_DEV_USERS" => Some(r#"[{"id":"analyst","display_name":"Analista"}]"#.to_owned()),
        _ => None,
    })
    .unwrap();
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter.clone())
        .build();
    let metrics = QueryMetrics::new(&provider.meter("test"), &analytics);
    let app = router(AppState {
        db: db.clone(),
        auth,
        analytics,
        query_timeout,
        metrics,
        models: bi_server::models::Models::default(),
        context_key: bi_server::semantic::context::ContextKey::generate(),
    });
    let login = app
        .clone()
        .oneshot(
            Request::post("/api/v1/dev/login")
                .header("origin", origin)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"user_id":"analyst"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    Stack {
        app,
        db,
        cookie,
        origin,
        provider,
        exporter,
    }
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    text: String,
    body: Value,
}

async fn send(app: &Router, request: axum::http::request::Builder, body: &str) -> Answer {
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    Answer {
        status,
        headers,
        body: serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    }
}

impl Stack {
    /// A query with the session, the configured Origin and a JSON body.
    async fn query(&self, body: &str) -> Answer {
        send(
            &self.app,
            Request::post("/api/v1/queries")
                .header("cookie", &self.cookie)
                .header("origin", self.origin)
                .header("content-type", "application/json")
                .header("x-request-id", "req-query-test"),
            body,
        )
        .await
    }

    /// Sum of the counts (histograms) or values (counters) of `name` whose
    /// attributes include every pair of `labels`.
    fn counted(&self, name: &str, labels: &[(&str, &str)]) -> u64 {
        self.provider.force_flush().unwrap();
        let exports: Vec<ResourceMetrics> = self.exporter.get_finished_metrics().unwrap();
        let matches = |attributes: &mut dyn Iterator<Item = &opentelemetry::KeyValue>| {
            let present: Vec<(String, String)> = attributes
                .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                .collect();
            labels
                .iter()
                .all(|(k, v)| present.iter().any(|(pk, pv)| pk == k && pv == v))
        };
        exports
            .last()
            .into_iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
            .filter(|metric| metric.name() == name)
            .map(|metric| match metric.data() {
                AggregatedMetrics::F64(MetricData::Histogram(histogram)) => histogram
                    .data_points()
                    .filter(|p| matches(&mut p.attributes()))
                    .map(opentelemetry_sdk::metrics::data::HistogramDataPoint::count)
                    .sum(),
                AggregatedMetrics::U64(MetricData::Sum(sum)) => sum
                    .data_points()
                    .filter(|p| matches(&mut p.attributes()))
                    .map(opentelemetry_sdk::metrics::data::SumDataPoint::value)
                    .sum(),
                _ => 0,
            })
            .sum()
    }

    /// No query was compiled or executed: the refusal did no analytical work.
    fn assert_no_analytical_work(&self) {
        for name in [
            "bi_semantic_compile_duration",
            "bi_query_wait",
            "bi_query_execute",
        ] {
            assert_eq!(self.counted(name, &[]), 0, "{name}");
        }
    }
}

/// The denials of the legacy route in the audit history, oldest first.
async fn denials(pool: &PgPool) -> Vec<Value> {
    sqlx::query_scalar::<_, String>(
        "SELECT payload FROM bi_meta.audit_events
         WHERE (payload::jsonb)->>'resource_id' = 'legacy_queries' ORDER BY sequence",
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .iter()
    .map(|payload| serde_json::from_str(payload).unwrap())
    .collect()
}

fn assert_closed(answer: &Answer) {
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert_eq!(answer.headers["content-type"], "application/problem+json");
    assert_eq!(answer.headers["cache-control"], "no-store");
    assert_eq!(answer.body["code"], "legacy_query_disabled");
    assert_eq!(answer.body["detail"], CLOSED);
    assert!(
        answer.body["request_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
}

fn assert_denial_event(event: &Value) {
    assert_eq!(event["action"], "denial");
    assert_eq!(event["result"], "denied");
    assert_eq!(event["resource_type"], "query_endpoint");
    assert_eq!(event["resource_id"], "legacy_queries");
    assert_eq!(event["actor_id"], "analyst");
    assert_eq!(event["policy_decision_id"], Value::Null);
    assert_eq!(event["metadata_redacted"]["query_shape"], Value::Null);
}

/// Swaps the amounts of two orders of one region: same schema, rows,
/// totals and per-region totals, different rows.
async fn swap_two_amounts(runtime: &Runtime) {
    runtime
        .run(|conn| {
            let (first, second): (i64, i64) = conn.query_row(
                "SELECT a.order_id, b.order_id FROM lake.main.sales a JOIN lake.main.sales b
                 ON a.region = b.region AND a.order_id < b.order_id AND a.amount <> b.amount
                 ORDER BY a.order_id, b.order_id LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            conn.execute(
                "UPDATE lake.main.sales SET amount = CASE order_id
                     WHEN ?1 THEN (SELECT amount FROM lake.main.sales WHERE order_id = ?2)
                     ELSE (SELECT amount FROM lake.main.sales WHERE order_id = ?1) END
                 WHERE order_id IN (?1, ?2)",
                [first, second],
            )
        })
        .await
        .unwrap();
}

#[sqlx::test(migrations = false)]
async fn the_local_synthetic_demo_answers_exactly_and_audits_nothing(pool: PgPool) {
    let data = tempfile::tempdir().unwrap();
    let stack = stack(
        &pool,
        bootstrapped(lake(data.path(), &schema())).await,
        ORIGIN,
        Duration::from_secs(10),
    )
    .await;
    let oracle = oracle();

    let totals = stack
        .query(r#"{"dataset":"sales","measures":["row_count","amount_sum","quantity_sum"]}"#)
        .await;
    assert_eq!(totals.status, StatusCode::OK, "{}", totals.text);
    assert_eq!(
        totals.body["rows"],
        json!([[
            oracle.totals.row_count,
            oracle.totals.amount.sum,
            oracle.totals.quantity.sum
        ]])
    );
    assert_eq!(
        totals.body["columns"][1],
        json!({"name": "amount_sum", "type": "decimal", "precision": 38, "scale": 2, "nullable": true})
    );
    let timing = totals.headers["server-timing"].to_str().unwrap();
    for phase in ["plan;dur=", "queue;dur=", "execute;dur=", "serialize;dur="] {
        assert!(timing.contains(phase), "{timing}");
    }

    // A present region and the missing one (NULL), like the oracle.
    let expected = |region: Option<&str>| {
        let group = oracle
            .by_region
            .iter()
            .find(|g| g.region.as_deref() == region)
            .unwrap();
        json!([
            region,
            group.row_count,
            group.quantity_sum,
            group.amount_sum
        ])
    };
    let filtered = stack
        .query(
            r#"{"dataset":"sales","dimensions":["region"],
                "measures":["row_count","quantity_sum","amount_sum"],
                "filters":[{"field":"region","op":"in","values":["north",null]}]}"#,
        )
        .await;
    assert_eq!(filtered.status, StatusCode::OK, "{}", filtered.text);
    assert_eq!(
        filtered.body["rows"],
        json!([expected(Some("north")), expected(None)])
    );
    let sorted = stack
        .query(
            r#"{"dataset":"sales","dimensions":["region"],"measures":["amount_sum"],
                "sort":[{"field":"amount_sum","direction":"desc"}],"limit":2}"#,
        )
        .await;
    assert_eq!(sorted.body["row_count"], 2);
    assert_eq!(sorted.body["truncated"], true);

    // Rejections of the existing contract, in the admitted case.
    let malformed = stack.query("{not json").await;
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
    assert_eq!(malformed.body["code"], "invalid_request");
    let unknown_field = stack
        .query(r#"{"dataset":"sales","measures":["row_count"],"sql":"SELECT 1"}"#)
        .await;
    assert_eq!(unknown_field.status, StatusCode::BAD_REQUEST);
    let outside = stack
        .query(r#"{"dataset":"sales","measures":["salary"]}"#)
        .await;
    assert_eq!(outside.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(outside.body["code"], "invalid_query");

    assert_eq!(denials(&pool).await.len(), 0);
    assert_eq!(stack.counted("bi_legacy_query_denied", &[]), 0);
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn an_admitted_query_past_its_deadline_is_cancelled(pool: PgPool) {
    let data = tempfile::tempdir().unwrap();
    let stack = stack(
        &pool,
        bootstrapped(lake(data.path(), &schema())).await,
        ORIGIN,
        Duration::from_nanos(1),
    )
    .await;
    let answer = stack
        .query(r#"{"dataset":"sales","dimensions":["region"],"measures":["amount_sum"]}"#)
        .await;
    assert_eq!(
        answer.status,
        StatusCode::GATEWAY_TIMEOUT,
        "{}",
        answer.text
    );
    assert_eq!(answer.body["code"], "query_timeout");
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn until_its_runtime_is_ready_the_demo_is_unavailable(pool: PgPool) {
    let stack = stack(&pool, Analytics::disabled(), ORIGIN, Duration::from_secs(5)).await;
    let answer = stack.query(QUERY).await;
    assert_eq!(answer.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(answer.body["code"], "analytics_unavailable");
    assert_eq!(denials(&pool).await.len(), 0);
    stack.assert_no_analytical_work();
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn an_unverified_runtime_closes_the_route_before_reading_the_body(pool: PgPool) {
    let stack = stack(&pool, unverified().await, ORIGIN, Duration::from_secs(5)).await;
    let leaky = r#"{"sql":"SELECT * FROM secrets","roles":["admin"],"synthetic":true}"#;
    let cases: [(Option<&str>, &str, &str); 6] = [
        (Some("application/json"), "/api/v1/queries", QUERY),
        (Some("application/json"), "/api/v1/queries", "{not json"),
        (Some("application/json"), "/api/v1/queries", leaky),
        (None, "/api/v1/queries", "{not json"),
        (Some("text/plain"), "/api/v1/queries", leaky),
        (
            Some("application/json"),
            "/api/v1/queries?synthetic=true&mode=development&role=admin",
            QUERY,
        ),
    ];
    for (content_type, path, body) in cases {
        let mut request = Request::post(path)
            .header("cookie", &stack.cookie)
            .header("origin", ORIGIN)
            // Proxy headers never widen anything.
            .header("host", "localhost")
            .header("x-forwarded-host", "127.0.0.1")
            .header("x-forwarded-proto", "http")
            .header("x-forwarded-for", "127.0.0.1");
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        let answer = send(&stack.app, request, body).await;
        assert_closed(&answer);
        for reflected in ["secrets", "SELECT", "roles", "admin", "not json"] {
            assert!(!answer.text.contains(reflected), "{}", answer.text);
        }
    }
    let events = denials(&pool).await;
    assert_eq!(events.len(), cases.len(), "one event per refusal");
    for event in &events {
        assert_denial_event(event);
    }
    assert_eq!(
        stack.counted(
            "bi_legacy_query_denied",
            &[("reason", "unverified_runtime"), ("audit", "recorded")]
        ),
        u64::try_from(cases.len()).unwrap()
    );
    stack.assert_no_analytical_work();
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn generated_headers_and_parameters_never_open_the_route(pool: PgPool) {
    use proptest::prelude::*;
    use proptest::strategy::ValueTree as _;
    use proptest::test_runner::TestRunner;

    let stack = stack(&pool, unverified().await, ORIGIN, Duration::from_secs(5)).await;
    let header = prop_oneof![
        Just("host"),
        Just("x-forwarded-host"),
        Just("x-forwarded-for"),
        Just("x-forwarded-proto"),
        Just("forwarded"),
        Just("x-bi-mode"),
        Just("x-synthetic"),
    ];
    let case = (
        header,
        "[a-zA-Z0-9:./=_-]{0,24}",
        "[a-z_]{1,12}=[a-z0-9_]{0,12}",
        prop_oneof![
            Just(QUERY.to_owned()),
            "[ -~]{0,64}",
            "[a-z_]{1,12}".prop_map(|field| format!(
                r#"{{"dataset":"sales","measures":["row_count"],"{field}":true}}"#
            )),
        ],
    );
    let mut runner = TestRunner::deterministic();
    let mut sent = 0;
    for _ in 0..48 {
        let (header, value, parameter, body) = case.new_tree(&mut runner).unwrap().current();
        let request = Request::post(format!("/api/v1/queries?{parameter}"))
            .header("cookie", &stack.cookie)
            .header("origin", ORIGIN)
            .header("content-type", "application/json")
            .header(header, value.as_str());
        assert_closed(&send(&stack.app, request, &body).await);
        sent += 1;
    }
    assert_eq!(denials(&pool).await.len(), sent);
    stack.assert_no_analytical_work();
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_lake_whose_fixture_rows_changed_is_not_vouched_for(pool: PgPool) {
    let data = tempfile::tempdir().unwrap();
    let schema = schema();
    let first = bootstrapped(lake(data.path(), &schema)).await;
    let runtime = first.runtime().unwrap();
    let stack_before = stack(&pool, first, ORIGIN, Duration::from_secs(10)).await;
    assert_eq!(stack_before.query(QUERY).await.status, StatusCode::OK);
    swap_two_amounts(&runtime).await;
    drop(runtime);
    stack_before.db.close().await;
    drop(stack_before);

    // The same lake bootstrapped again: marker intact, rows not the fixture's.
    let second = bootstrapped(lake(data.path(), &schema)).await;
    let stack = stack(&pool, second, ORIGIN, Duration::from_secs(10)).await;
    assert_closed(&stack.query(QUERY).await);
    assert_eq!(
        stack.counted(
            "bi_legacy_query_denied",
            &[("reason", "unverified_runtime")]
        ),
        1
    );
    stack.assert_no_analytical_work();
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_public_origin_closes_the_route(pool: PgPool) {
    let data = tempfile::tempdir().unwrap();
    let stack = stack(
        &pool,
        bootstrapped(lake(data.path(), &schema())).await,
        "https://bi.example.test",
        Duration::from_secs(5),
    )
    .await;
    assert_closed(&stack.query(QUERY).await);
    assert_eq!(
        stack.counted("bi_legacy_query_denied", &[("reason", "public_origin")]),
        1
    );
    stack.assert_no_analytical_work();
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn session_origin_and_metadata_checks_come_first(pool: PgPool) {
    let stack = stack(&pool, unverified().await, ORIGIN, Duration::from_secs(5)).await;
    let anonymous = send(
        &stack.app,
        Request::post("/api/v1/queries")
            .header("origin", ORIGIN)
            .header("content-type", "application/json"),
        "{not json",
    )
    .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert_eq!(anonymous.body["code"], "session_required");
    for origin in [None, Some("https://evil.example")] {
        let mut request = Request::post("/api/v1/queries")
            .header("cookie", &stack.cookie)
            .header("content-type", "application/json");
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let answer = send(&stack.app, request, "{not json").await;
        assert_eq!(answer.status, StatusCode::FORBIDDEN, "{origin:?}");
        assert_eq!(answer.body["code"], "origin_forbidden");
    }
    stack.db.close().await;
    let unready = stack.query("{not json").await;
    assert_eq!(unready.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(unready.body["code"], "metadata_not_ready");
    assert_eq!(denials(&pool).await.len(), 0);
    assert_eq!(stack.counted("bi_legacy_query_denied", &[]), 0);
    stack.assert_no_analytical_work();
}

#[sqlx::test(migrations = false)]
async fn a_held_audit_head_ends_the_refusal_within_its_deadline(pool: PgPool) {
    let deadline = Duration::from_secs(1);
    let stack = stack(&pool, unverified().await, ORIGIN, deadline).await;
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE bi_meta.audit_head IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *holder)
        .await
        .unwrap();

    let started = Instant::now();
    let answer = stack.query(QUERY).await;
    let elapsed = started.elapsed();
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );
    assert_eq!(answer.body["code"], "audit_unavailable");
    assert!(
        elapsed < deadline + Duration::from_secs(2),
        "{elapsed:?} for a {deadline:?} deadline"
    );
    // Nothing keeps waiting for the lock once the request has answered.
    let waiting_until = Instant::now() + Duration::from_secs(5);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
             WHERE datname = current_database() AND wait_event_type = 'Lock'
               AND pid <> pg_backend_pid()",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if waiting == 0 {
            break;
        }
        assert!(
            Instant::now() < waiting_until,
            "{waiting} backends still wait"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        stack.counted(
            "bi_legacy_query_denied",
            &[("reason", "unverified_runtime"), ("audit", "unavailable")]
        ),
        1
    );
    stack.assert_no_analytical_work();
    holder.commit().await.unwrap();
    assert_eq!(denials(&pool).await.len(), 0, "no event without commit");

    assert_closed(&stack.query(QUERY).await);
    let events = denials(&pool).await;
    assert_eq!(events.len(), 1, "recorded once after the release");
    assert_denial_event(&events[0]);
    stack.db.close().await;
}

/// Rows of the lake's `sales` table, read through `analytics`' own runtime.
async fn sales_rows(analytics: &Analytics) -> i64 {
    analytics
        .runtime()
        .unwrap()
        .run(|conn| conn.query_row("SELECT count(*) FROM lake.main.sales", [], |row| row.get(0)))
        .await
        .unwrap()
}

/// A lake whose `sales` table the server never wrote (no fixture marker):
/// the bootstrap keeps its rows, and the route stays closed (#118 review).
#[sqlx::test(migrations = false)]
async fn an_unknown_lake_table_is_preserved_and_the_route_closed(pool: PgPool) {
    let data = tempfile::tempdir().unwrap();
    let config = lake(data.path(), &schema());
    let writer = config.clone();
    let runtime = tokio::task::spawn_blocking(move || {
        bi_runtime::Bootstrap::open(writer)
            .unwrap()
            .finish()
            .unwrap()
    })
    .await
    .unwrap();
    runtime
        .run(|conn| {
            conn.execute_batch(
                "CREATE TABLE lake.main.sales (order_id BIGINT, ordered_at TIMESTAMPTZ,
                     region VARCHAR, quantity BIGINT, amount DECIMAL(18,2));
                 INSERT INTO lake.main.sales VALUES
                     (9000001, '2026-01-01T00:00:00Z', 'private-region', 1, 123.45);",
            )
        })
        .await
        .unwrap();
    drop(runtime);

    let analytics = bootstrapped(config).await;
    assert_eq!(
        sales_rows(&analytics).await,
        1,
        "someone else's rows survive"
    );
    let stack = stack(&pool, analytics, ORIGIN, Duration::from_secs(10)).await;
    let answer = stack.query(QUERY).await;
    assert_closed(&answer);
    assert!(!answer.text.contains("private-region"), "{}", answer.text);
    assert_eq!(
        stack.counted(
            "bi_legacy_query_denied",
            &[("reason", "unverified_runtime")]
        ),
        1
    );
    stack.assert_no_analytical_work();
    stack.db.close().await;
}

/// A lake marked with another fixture whose rows are no longer that
/// fixture: not replaced by the configured one, and not vouched for.
#[sqlx::test(migrations = false)]
async fn a_lake_marked_with_another_fixture_over_other_rows_is_preserved_and_closed(pool: PgPool) {
    let data = tempfile::tempdir().unwrap();
    let schema = schema();
    let marked = Analytics::start(
        lake(data.path(), &schema),
        Some(FixtureSpec { seed: 7, rows: 500 }),
    );
    assert!(
        tokio::time::timeout(READY, marked.wait_until_ready())
            .await
            .unwrap()
    );
    swap_two_amounts(&marked.runtime().unwrap()).await;
    drop(marked);

    let analytics = bootstrapped(lake(data.path(), &schema)).await;
    assert_eq!(
        sales_rows(&analytics).await,
        500,
        "not replaced by the configured fixture"
    );
    let stack = stack(&pool, analytics, ORIGIN, Duration::from_secs(10)).await;
    assert_closed(&stack.query(QUERY).await);
    stack.assert_no_analytical_work();
    stack.db.close().await;
}
