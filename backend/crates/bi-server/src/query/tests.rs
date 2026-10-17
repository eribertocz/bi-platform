//! The query handler once admitted (`answer`): problems, budgets and exact
//! values on in-memory runtimes. Admission itself, which closes the route
//! outside the local synthetic demo (#118), is tested through the real
//! router in `tests/query.rs`.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};

use super::*;
use crate::db::Database;
use crate::metadata::MetadataMode;

fn state() -> AppState {
    AppState::new(Database::start(
        &"postgres://bi:bi@127.0.0.1:1/bi".parse().unwrap(),
        MetadataMode::Development,
    ))
}

/// One request on the admitted path; `content_type` as sent by the client.
async fn answer_with(
    runtime: &bi_runtime::Runtime,
    content_type: Option<&str>,
    body: &str,
) -> (StatusCode, String, axum::http::HeaderMap, Value) {
    let mut request = Request::post("/api/v1/queries");
    if let Some(content_type) = content_type {
        request = request.header("content-type", content_type);
    }
    let response = answer(
        &state(),
        Some(runtime),
        "req-query-test",
        request.body(Body::from(body.to_owned())).unwrap(),
    )
    .await;
    let status = response.status();
    let headers = response.headers().clone();
    let content_type = headers
        .get("content-type")
        .map_or_else(String::new, |value| value.to_str().unwrap().to_owned());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        content_type,
        headers,
        serde_json::from_slice(&bytes).unwrap(),
    )
}

async fn post(runtime: &bi_runtime::Runtime, body: &str) -> (StatusCode, String, Value) {
    let (status, content_type, _, body) =
        answer_with(runtime, Some("application/json"), body).await;
    (status, content_type, body)
}

fn assert_problem(response: &(StatusCode, String, Value), status: StatusCode, code: &str) {
    let (actual, content_type, body) = response;
    assert_eq!(*actual, status, "{body}");
    assert_eq!(content_type, "application/problem+json");
    assert_eq!(body["status"], status.as_u16());
    assert_eq!(body["code"], code);
    assert_eq!(body["request_id"], "req-query-test");
}

/// A runtime without a lake: no `sales` table.
fn bare() -> bi_runtime::Runtime {
    bi_runtime::Bootstrap::open(bi_runtime::RuntimeConfig::in_memory(1))
        .unwrap()
        .finish()
        .unwrap()
}

/// An in-memory `lake` with the sales schema, filled by `rows` (SQL VALUES).
async fn memory_lake(rows: &'static str) -> bi_runtime::Runtime {
    let runtime = bare();
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
    runtime
}

#[test]
fn server_timing_lists_phases_in_milliseconds() {
    assert_eq!(
        server_timing(&[
            ("plan", Duration::from_micros(120)),
            ("execute", Duration::from_millis(15)),
        ]),
        "plan;dur=0.120, execute;dur=15.000"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_requests_are_bad_requests() {
    let runtime = bare();
    assert_problem(
        &post(&runtime, "{not json").await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
    let response = post(
        &runtime,
        r#"{"dataset":"sales","measures":["row_count"],"sql":"SELECT 1"}"#,
    )
    .await;
    assert_problem(&response, StatusCode::BAD_REQUEST, "invalid_request");
    assert!(
        response.2["detail"].as_str().unwrap().contains("sql"),
        "{}",
        response.2
    );
    let (status, _, _, body) = answer_with(&runtime, None, r#"{"dataset":"sales"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_request");
}

#[tokio::test(flavor = "multi_thread")]
async fn queries_outside_the_catalog_are_unprocessable() {
    let response = post(&bare(), r#"{"dataset":"sales","measures":["salary"]}"#).await;
    assert_problem(&response, StatusCode::UNPROCESSABLE_ENTITY, "invalid_query");
    assert!(
        response.2["detail"]
            .as_str()
            .unwrap()
            .contains("unknown measure")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_failures_do_not_leak_internal_details() {
    let response = post(&bare(), r#"{"dataset":"sales","measures":["row_count"]}"#).await;
    assert_problem(&response, StatusCode::INTERNAL_SERVER_ERROR, "query_failed");
    assert!(response.2.get("detail").is_none(), "{}", response.2);
}

#[tokio::test(flavor = "multi_thread")]
async fn results_over_the_byte_budget_are_refused() {
    // One 2 MiB region answering a 78-byte request (the reviewed scenario).
    let runtime = memory_lake(
        "(9007199254740993, '2026-01-01T00:00:00Z', repeat('x', 2 * 1024 * 1024), 1, 1.00)",
    )
    .await;
    let response = post(
        &runtime,
        r#"{"dataset":"sales","dimensions":["region"],"measures":["row_count"],"limit":1}"#,
    )
    .await;
    assert_problem(
        &response,
        StatusCode::UNPROCESSABLE_ENTITY,
        "result_too_large",
    );

    // A single value over the per-value limit, far below the total budget.
    let runtime =
        memory_lake("(1, '2026-01-01T00:00:00Z', repeat('y', 100 * 1024), 1, 1.00)").await;
    assert_problem(
        &post(
            &runtime,
            r#"{"dataset":"sales","dimensions":["region"],"measures":["row_count"]}"#,
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "result_too_large",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn results_within_budget_stay_exact_and_report_their_phases() {
    let runtime = memory_lake(
        "(9007199254740993, '2026-01-01T00:00:00Z', 'north', 5, 9999999999999999.99),
         (9007199254740994, '2026-01-01T00:00:01Z', NULL, NULL, -0.01)",
    )
    .await;
    let (status, _, headers, body) = answer_with(
        &runtime,
        Some("application/json"),
        r#"{"dataset":"sales","dimensions":["region"],"measures":["row_count","quantity_sum","amount_sum"]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let timing = headers["server-timing"].to_str().unwrap().to_owned();
    for phase in ["plan;dur=", "queue;dur=", "execute;dur=", "serialize;dur="] {
        assert!(timing.contains(phase), "{timing}");
    }
    assert_eq!(
        body["rows"],
        json!([
            ["north", "1", "5", "9999999999999999.99"],
            [null, "1", null, "-0.01"]
        ])
    );
}
