//! `POST /api/v1/queries`: typed aggregate queries over allowlisted datasets
//! (M05). The server compiles SQL from the catalog, runs it on the DuckDB
//! pool with a deadline and returns exact, typed values.

pub mod catalog;
pub mod plan;

use std::time::Instant;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bi_runtime::RuntimeError;
use bi_runtime::duckdb::{self, Connection, types::Value};
use serde::Serialize;
use tower_http::request_id::RequestId;
use utoipa::ToSchema;

use crate::AppState;
use crate::problem::Problem;
use plan::{Param, QueryRequest, ResultColumn};

#[derive(Debug, Serialize, ToSchema)]
pub struct QueryResult {
    pub columns: Vec<ResultColumn>,
    /// Rows in column order. Every non-null value is a string in its column's
    /// type (`integer` and `decimal` are exact; `null` means missing).
    #[schema(value_type = Vec<Vec<Option<String>>>)]
    pub rows: Vec<Vec<Option<String>>>,
    pub row_count: usize,
    /// More rows matched than `limit` allowed.
    pub truncated: bool,
}

#[utoipa::path(
    post,
    path = "/api/v1/queries",
    tag = "queries",
    operation_id = "runQuery",
    summary = "Consulta agregada tipada sobre un dataset permitido; sin SQL del cliente.",
    request_body = QueryRequest,
    responses(
        (status = 200, description = "Resultado tipado y exacto", body = QueryResult),
        (status = 400, description = "JSON inválido o campos desconocidos", body = Problem, content_type = "application/problem+json"),
        (status = 422, description = "Consulta fuera del catálogo o de los límites", body = Problem, content_type = "application/problem+json"),
        (status = 429, description = "Cola de consultas llena; reintentar", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "Fallo interno al ejecutar", body = Problem, content_type = "application/problem+json"),
        (status = 503, description = "Motor analítico no disponible", body = Problem, content_type = "application/problem+json"),
        (status = 504, description = "Superó el tiempo máximo y se canceló", body = Problem, content_type = "application/problem+json")
    )
)]
pub async fn run(
    State(state): State<AppState>,
    request_id: Option<Extension<RequestId>>,
    body: Result<Json<QueryRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id
        .and_then(|Extension(id)| id.header_value().to_str().ok().map(str::to_owned))
        .unwrap_or_default();
    let problem = |status, code: &str, detail: Option<String>| {
        Problem::new(status, code, detail, request_id.clone()).into_response()
    };

    let request = match body {
        Ok(Json(request)) => request,
        Err(rejection) => {
            return problem(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                Some(rejection.body_text()),
            );
        }
    };
    let plan = match plan::compile(&request) {
        Ok(plan) => plan,
        Err(invalid) => {
            return problem(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_query",
                Some(invalid.0),
            );
        }
    };
    let Some(runtime) = state.analytics.runtime() else {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "analytics_unavailable",
            Some("El motor analítico no está listo.".to_owned()),
        );
    };

    let started = Instant::now();
    let columns = plan.columns.len();
    let fetch_limit = plan.limit + 1;
    let (sql, params) = (plan.sql, plan.params);
    // Dropping this future on timeout interrupts the query on its connection.
    let execution = runtime.run(move |conn| fetch(conn, &sql, &params, columns, fetch_limit));
    let outcome = tokio::time::timeout(state.query_timeout, execution).await;
    let elapsed_ms = started.elapsed().as_millis();

    let mut rows = match outcome {
        Ok(Ok(rows)) => rows,
        Err(_) => {
            tracing::warn!(dataset = %request.dataset, elapsed_ms, "query timed out and was cancelled");
            return problem(
                StatusCode::GATEWAY_TIMEOUT,
                "query_timeout",
                Some(format!(
                    "La consulta superó {} ms y se canceló.",
                    state.query_timeout.as_millis()
                )),
            );
        }
        Ok(Err(error)) => {
            let (status, code) = match &error {
                RuntimeError::Busy => (StatusCode::TOO_MANY_REQUESTS, "too_many_queries"),
                RuntimeError::Engine(_) | RuntimeError::Panicked => {
                    (StatusCode::INTERNAL_SERVER_ERROR, "query_failed")
                }
                _ => (StatusCode::SERVICE_UNAVAILABLE, "analytics_unavailable"),
            };
            tracing::error!(%error, dataset = %request.dataset, elapsed_ms, "query failed");
            let mut response = problem(status, code, None);
            if status == StatusCode::TOO_MANY_REQUESTS {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            }
            return response;
        }
    };

    let truncated = rows.len() > fetch_limit - 1;
    rows.truncate(fetch_limit - 1);
    tracing::info!(
        dataset = %request.dataset,
        rows = rows.len(),
        truncated,
        elapsed_ms,
        "query executed"
    );
    let mut response = Json(QueryResult {
        columns: plan.columns,
        row_count: rows.len(),
        rows,
        truncated,
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Runs on a pool thread. Every selected column is already `VARCHAR`, so
/// values leave DuckDB as exact text.
fn fetch(
    conn: &Connection,
    sql: &str,
    params: &[Param],
    columns: usize,
    limit: usize,
) -> duckdb::Result<Vec<Vec<Option<String>>>> {
    let values: Vec<Value> = params
        .iter()
        .map(|param| match param {
            Param::Text(text) => Value::Text(text.clone()),
            Param::BigInt(number) => Value::BigInt(*number),
        })
        .collect();
    let mut statement = conn.prepare(sql)?;
    let mut rows = statement.query(duckdb::params_from_iter(values.iter()))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        if out.len() == limit {
            break;
        }
        out.push(
            (0..columns)
                .map(|i| row.get::<_, Option<String>>(i))
                .collect::<duckdb::Result<_>>()?,
        );
    }
    Ok(out)
}
