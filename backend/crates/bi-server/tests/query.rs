//! `POST /api/v1/queries`: errors as problem details, and exact results
//! against the oracle on a real DuckLake (`--features postgres-tests`).

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post as post_route;
use bi_server::analytics::Analytics;
use bi_server::db::{self, Database};
use bi_server::{AppState, query};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use tower_http::request_id::{MakeRequestUuid, SetRequestIdLayer};

/// The query handler alone. Session enforcement for `/api/v1/**` belongs to
/// the full router: see `protection` below and `auth.rs`.
fn app(analytics: Analytics, query_timeout: Duration) -> Router {
    let db = Database::start(db::lazy_pool("postgres://bi:bi@127.0.0.1:1/bi").unwrap());
    Router::new()
        .route("/api/v1/queries", post_route(query::run))
        .with_state(AppState {
            analytics,
            query_timeout,
            ..AppState::new(db)
        })
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

async fn post(app: &Router, body: &str) -> (StatusCode, String, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/queries")
                .header("content-type", "application/json")
                .header("x-request-id", "req-query-test")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let content_type = response.headers()["content-type"]
        .to_str()
        .unwrap()
        .to_owned();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        content_type,
        serde_json::from_slice(&bytes).unwrap(),
    )
}

fn assert_problem(response: &(StatusCode, String, Value), status: StatusCode, code: &str) {
    let (actual, content_type, body) = response;
    assert_eq!(*actual, status, "{body}");
    assert_eq!(content_type, "application/problem+json");
    assert_eq!(body["status"], status.as_u16());
    assert_eq!(body["code"], code);
    assert_eq!(body["request_id"], "req-query-test");
}

#[tokio::test]
async fn malformed_requests_are_bad_requests() {
    let app = app(Analytics::disabled(), Duration::from_secs(5));
    assert_problem(
        &post(&app, "{not json").await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
    let response = post(
        &app,
        r#"{"dataset":"sales","measures":["row_count"],"sql":"SELECT 1"}"#,
    )
    .await;
    assert_problem(&response, StatusCode::BAD_REQUEST, "invalid_request");
    assert!(
        response.2["detail"].as_str().unwrap().contains("sql"),
        "{}",
        response.2
    );
}

#[tokio::test]
async fn queries_outside_the_catalog_are_unprocessable() {
    let app = app(Analytics::disabled(), Duration::from_secs(5));
    let response = post(&app, r#"{"dataset":"sales","measures":["salary"]}"#).await;
    assert_problem(&response, StatusCode::UNPROCESSABLE_ENTITY, "invalid_query");
    assert!(
        response.2["detail"]
            .as_str()
            .unwrap()
            .contains("unknown measure")
    );
}

#[tokio::test]
async fn without_the_analytical_runtime_the_service_is_unavailable() {
    let app = app(Analytics::disabled(), Duration::from_secs(5));
    assert_problem(
        &post(&app, r#"{"dataset":"sales","measures":["row_count"]}"#).await,
        StatusCode::SERVICE_UNAVAILABLE,
        "analytics_unavailable",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_failures_do_not_leak_internal_details() {
    // A runtime without a lake: the sales table does not exist.
    let runtime = bi_runtime::Bootstrap::open(bi_runtime::RuntimeConfig::in_memory(1))
        .unwrap()
        .finish()
        .unwrap();
    let app = app(Analytics::ready(runtime), Duration::from_secs(5));
    let response = post(&app, r#"{"dataset":"sales","measures":["row_count"]}"#).await;
    assert_problem(&response, StatusCode::INTERNAL_SERVER_ERROR, "query_failed");
    assert!(response.2.get("detail").is_none(), "{}", response.2);
}

#[cfg(feature = "postgres-tests")]
mod lake {
    use std::sync::atomic::{AtomicU32, Ordering};

    use bi_fixtures::oracle::Oracle;
    use bi_fixtures::sales::{self, FixtureSpec};
    use bi_runtime::config::DEV_LAKE_CATALOG;
    use bi_runtime::{Bootstrap, LakeConfig, RuntimeConfig, fixtures};

    use super::*;

    const SPEC: FixtureSpec = FixtureSpec {
        seed: 42,
        rows: 50_000,
    };

    /// A lake with the sales fixture, isolated by metadata schema and data dir.
    fn lake_app(data: &std::path::Path, query_timeout: Duration) -> Router {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let config = RuntimeConfig {
            lake: Some(LakeConfig {
                catalog: std::env::var("BI_TEST_LAKE_CATALOG")
                    .unwrap_or_else(|_| DEV_LAKE_CATALOG.to_owned()),
                metadata_schema: Some(format!(
                    "bi_test_q{}_{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                )),
                data_path: data.to_owned(),
            }),
            ..RuntimeConfig::in_memory(2)
        };
        let bootstrap = Bootstrap::open(config).unwrap();
        fixtures::load_sales(&bootstrap, SPEC).unwrap();
        app(Analytics::ready(bootstrap.finish().unwrap()), query_timeout)
    }

    fn oracle() -> Oracle {
        Oracle::from_rows(SPEC, sales::rows(SPEC))
    }

    fn row(values: &[Option<&str>]) -> Value {
        json!(values)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn totals_match_the_oracle_exactly() {
        let data = tempfile::tempdir().unwrap();
        let app = lake_app(data.path(), Duration::from_secs(10));
        let (status, _, body) = post(
            &app,
            r#"{"dataset":"sales","measures":["row_count","amount_sum","quantity_sum"]}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let totals = oracle().totals;
        assert_eq!(
            body["rows"],
            json!([[totals.row_count, totals.amount.sum, totals.quantity.sum]])
        );
        assert_eq!(body["row_count"], 1);
        assert_eq!(body["truncated"], false);
        assert_eq!(
            body["columns"][1],
            json!({"name": "amount_sum", "type": "decimal", "precision": 38, "scale": 2, "nullable": true})
        );
        assert_eq!(body["columns"][0]["type"], "integer");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn grouping_filtering_and_sorting_match_the_oracle() {
        let data = tempfile::tempdir().unwrap();
        let app = lake_app(data.path(), Duration::from_secs(10));
        let by_region = oracle().by_region;
        let expected = |region: Option<&str>| {
            let group = by_region
                .iter()
                .find(|g| g.region.as_deref() == region)
                .unwrap();
            row(&[
                region,
                Some(group.row_count.as_str()),
                group.quantity_sum.as_deref(),
                group.amount_sum.as_deref(),
            ])
        };

        // Default order: region ascending, NULL last, like the oracle.
        let (status, _, body) = post(
            &app,
            r#"{"dataset":"sales","dimensions":["region"],
                "measures":["row_count","quantity_sum","amount_sum"]}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let all: Vec<Value> = by_region
            .iter()
            .map(|g| expected(g.region.as_deref()))
            .collect();
        assert_eq!(body["rows"], json!(all));

        // IN with NULL matches the missing region too.
        let (_, _, body) = post(
            &app,
            r#"{"dataset":"sales","dimensions":["region"],
                "measures":["row_count","quantity_sum","amount_sum"],
                "filters":[{"field":"region","op":"in","values":["north",null]}]}"#,
        )
        .await;
        assert_eq!(
            body["rows"],
            json!([expected(Some("north")), expected(None)])
        );

        // Sorting by a decimal measure is numeric, and the limit truncates.
        let (_, _, body) = post(
            &app,
            r#"{"dataset":"sales","dimensions":["region"],
                "measures":["row_count","quantity_sum","amount_sum"],
                "sort":[{"field":"amount_sum","direction":"desc"}],"limit":2}"#,
        )
        .await;
        assert_eq!(body["row_count"], 2);
        assert_eq!(body["truncated"], true);
        // Row 0 of the fixture holds the largest DECIMAL(18,2) amount.
        let largest = by_region
            .iter()
            .max_by_key(|g| {
                g.amount_sum
                    .as_deref()
                    .map(|s| s.replace('.', "").parse::<i128>().unwrap())
            })
            .unwrap();
        assert_eq!(body["rows"][0], expected(largest.region.as_deref()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_query_past_its_deadline_is_cancelled() {
        let data = tempfile::tempdir().unwrap();
        let app = lake_app(data.path(), Duration::from_nanos(1));
        assert_problem(
            &post(
                &app,
                r#"{"dataset":"sales","dimensions":["region"],"measures":["amount_sum"]}"#,
            )
            .await,
            StatusCode::GATEWAY_TIMEOUT,
            "query_timeout",
        );
    }
}

/// An in-memory `lake` with the sales schema, filled by `rows` (SQL VALUES).
async fn memory_lake_app(rows: &'static str) -> Router {
    app(memory_lake(rows).await, Duration::from_secs(10))
}

async fn memory_lake(rows: &'static str) -> Analytics {
    let runtime = bi_runtime::Bootstrap::open(bi_runtime::RuntimeConfig::in_memory(1))
        .unwrap()
        .finish()
        .unwrap();
    runtime
        .run(move |conn| {
            conn.execute_batch(&format!(
                "ATTACH ':memory:' AS lake;
                 CREATE TABLE lake.main.sales (order_id BIGINT, ordered_at TIMESTAMPTZ,
                     region VARCHAR, quantity BIGINT, amount DECIMAL(18,2));
                 INSERT INTO lake.main.sales VALUES {rows};"
            ))
        })
        .await
        .unwrap();
    Analytics::ready(runtime)
}

#[tokio::test(flavor = "multi_thread")]
async fn results_over_the_byte_budget_are_refused() {
    // One 2 MiB region answering a 78-byte request (the reviewed scenario).
    let app = memory_lake_app(
        "(9007199254740993, '2026-01-01T00:00:00Z', repeat('x', 2 * 1024 * 1024), 1, 1.00)",
    )
    .await;
    let response = post(
        &app,
        r#"{"dataset":"sales","dimensions":["region"],"measures":["row_count"],"limit":1}"#,
    )
    .await;
    assert_problem(
        &response,
        StatusCode::UNPROCESSABLE_ENTITY,
        "result_too_large",
    );

    // A single value over the per-value limit, far below the total budget.
    let app =
        memory_lake_app("(1, '2026-01-01T00:00:00Z', repeat('y', 100 * 1024), 1, 1.00)").await;
    assert_problem(
        &post(
            &app,
            r#"{"dataset":"sales","dimensions":["region"],"measures":["row_count"]}"#,
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "result_too_large",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn results_within_budget_stay_exact_and_report_their_phases() {
    let app = memory_lake_app(
        "(9007199254740993, '2026-01-01T00:00:00Z', 'north', 5, 9999999999999999.99),
         (9007199254740994, '2026-01-01T00:00:01Z', NULL, NULL, -0.01)",
    )
    .await;
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/queries")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"dataset":"sales","dimensions":["region"],"measures":["row_count","quantity_sum","amount_sum"]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let timing = response.headers()["server-timing"]
        .to_str()
        .unwrap()
        .to_owned();
    for phase in ["plan;dur=", "queue;dur=", "execute;dur=", "serialize;dur="] {
        assert!(timing.contains(phase), "{timing}");
    }
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["rows"],
        json!([
            ["north", "1", "5", "9999999999999999.99"],
            [null, "1", null, "-0.01"]
        ])
    );
}

/// The query endpoint behind the real router: a session and, for this POST,
/// the configured Origin are required before any query reaches the engine.
#[cfg(all(feature = "postgres-tests", feature = "dev-auth"))]
mod protection {
    use bi_server::auth::AuthConfig;
    use bi_server::router;
    use sqlx::PgPool;

    use super::*;

    const ORIGIN: &str = "http://127.0.0.1:8080";
    const QUERY: &str = r#"{"dataset":"sales","measures":["row_count"]}"#;

    async fn send(
        app: &Router,
        path: &str,
        cookie: Option<&str>,
        origin: Option<&str>,
        body: &str,
    ) -> axum::response::Response {
        let mut request = Request::post(path).header("content-type", "application/json");
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        app.clone()
            .oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = false)]
    async fn queries_require_a_session_and_the_configured_origin(pool: PgPool) {
        let db = Database::start(pool);
        assert!(db.wait_until_migrated().await);
        let auth = AuthConfig::from_lookup(&|key| match key {
            "BI_DEV_USERS" => Some(r#"[{"id":"analyst","display_name":"Analista"}]"#.to_owned()),
            _ => None,
        })
        .unwrap();
        let app = router(AppState {
            db,
            auth,
            analytics: memory_lake("(1, '2026-01-01T00:00:00Z', 'north', 1, 1.00)").await,
            query_timeout: Duration::from_secs(10),
            metrics: bi_server::metrics::QueryMetrics::default(),
        });

        let anonymous = send(&app, "/api/v1/queries", None, Some(ORIGIN), QUERY).await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        let login = send(
            &app,
            "/api/v1/dev/login",
            None,
            Some(ORIGIN),
            r#"{"user_id":"analyst"}"#,
        )
        .await;
        assert_eq!(login.status(), StatusCode::OK);
        let cookie = login.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();

        // A cross-site POST carries the cookie but not our Origin.
        for origin in [None, Some("https://evil.example")] {
            let response = send(&app, "/api/v1/queries", Some(&cookie), origin, QUERY).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin:?}");
        }

        let allowed = send(&app, "/api/v1/queries", Some(&cookie), Some(ORIGIN), QUERY).await;
        assert_eq!(allowed.status(), StatusCode::OK);
        let bytes = allowed.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["rows"], json!([["1"]]));
    }
}
