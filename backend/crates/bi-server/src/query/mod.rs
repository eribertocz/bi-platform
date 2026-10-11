//! `POST /api/v1/queries`: typed aggregate queries over allowlisted datasets
//! (M05). The server compiles SQL from the catalog, runs it on the DuckDB
//! pool with a deadline and a result-size budget, and returns exact, typed
//! values. Each response reports its phases in a `Server-Timing` header.

pub mod catalog;
pub mod plan;
#[cfg(test)]
mod properties;

use std::fmt::Write as _;
use std::time::{Duration, Instant};

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
use crate::metrics::{Outcome, QueryMetrics, Rejected};
use crate::problem::Problem;
use plan::{Param, QueryRequest, ResultColumn};

/// Total bytes of all values a result may carry.
pub const MAX_RESULT_BYTES: usize = 4 * 1024 * 1024;
/// Bytes of any single value.
pub const MAX_CELL_BYTES: usize = 64 * 1024;

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
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Resultado tipado y exacto; fases en la cabecera `Server-Timing` (plan, queue, execute, serialize; ms)", body = QueryResult),
        (status = 400, description = "JSON inválido o campos desconocidos", body = Problem, content_type = "application/problem+json"),
        (status = 401, description = "Sin sesión válida (`session_required`)", body = Problem, content_type = "application/problem+json"),
        (status = 403, description = "Origin ausente o distinto al configurado (`origin_forbidden`)", body = Problem, content_type = "application/problem+json"),
        (status = 422, description = "Consulta fuera del catálogo o de los límites (`invalid_query`), o resultado mayor que el presupuesto de bytes (`result_too_large`)", body = Problem, content_type = "application/problem+json"),
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
    match handle(&state, body).await {
        Ok(response) => response,
        Err(rejection) => {
            let reason = match rejection.code {
                "invalid_query" => Some(Rejected::InvalidQuery),
                "too_many_queries" => Some(Rejected::Busy),
                "query_timeout" => Some(Rejected::Timeout),
                "result_too_large" => Some(Rejected::ResultTooLarge),
                "analytics_unavailable" => Some(Rejected::Unavailable),
                _ => None,
            };
            if let Some(reason) = reason {
                state.metrics.reject(reason);
            }
            rejection.into_response(&request_id)
        }
    }
}

/// Why a query request ends without a result; becomes a problem response.
struct Rejection {
    status: StatusCode,
    code: &'static str,
    detail: Option<String>,
}

impl Rejection {
    fn new(status: StatusCode, code: &'static str, detail: Option<String>) -> Self {
        Self {
            status,
            code,
            detail,
        }
    }

    fn into_response(self, request_id: &str) -> Response {
        let retry = self.status == StatusCode::TOO_MANY_REQUESTS;
        let mut response =
            Problem::new(self.status, self.code, self.detail, request_id).into_response();
        if retry {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}

/// Validates, plans, executes and serializes one query, timing each phase.
async fn handle(
    state: &AppState,
    body: Result<Json<QueryRequest>, JsonRejection>,
) -> Result<Response, Rejection> {
    let Json(request) = body.map_err(|rejection| {
        Rejection::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            Some(rejection.body_text()),
        )
    })?;
    let (plan, plan_time) = compile(&request, &state.metrics)?;
    let runtime = state.analytics.runtime().ok_or_else(|| {
        Rejection::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "analytics_unavailable",
            Some("El motor analítico no está listo.".to_owned()),
        )
    })?;

    let fetch_limit = plan.limit + 1;
    let (fetched, queue_time, execute_time) = execute(
        &runtime,
        state.query_timeout,
        plan.sql,
        plan.params,
        plan.columns.len(),
        fetch_limit,
        &state.metrics,
    )
    .await
    .map_err(|failure| match failure {
        Failure::Timeout => {
            tracing::warn!(dataset = %request.dataset, "query timed out and was cancelled");
            Rejection::new(
                StatusCode::GATEWAY_TIMEOUT,
                "query_timeout",
                Some(format!(
                    "La consulta superó {} ms y se canceló.",
                    state.query_timeout.as_millis()
                )),
            )
        }
        Failure::Runtime(error) => {
            tracing::error!(%error, dataset = %request.dataset, "query failed");
            let (status, code) = status_for(&error);
            Rejection::new(status, code, None)
        }
    })?;
    let Fetched::Rows(mut rows) = fetched else {
        tracing::warn!(dataset = %request.dataset, "query result over the byte budget");
        return Err(Rejection::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "result_too_large",
            Some(format!(
                "El resultado supera {MAX_RESULT_BYTES} bytes (o {MAX_CELL_BYTES} en un solo valor). Añade filtros, menos dimensiones o un límite menor."
            )),
        ));
    };
    let truncated = rows.len() > fetch_limit - 1;
    rows.truncate(fetch_limit - 1);
    let row_count = rows.len();

    let serializing = Instant::now();
    let result = QueryResult {
        columns: plan.columns,
        row_count,
        rows,
        truncated,
    };
    let body = serde_json::to_vec(&result).map_err(|error| {
        tracing::error!(%error, "cannot serialize a query result");
        Rejection::new(StatusCode::INTERNAL_SERVER_ERROR, "query_failed", None)
    })?;
    let serialize_time = serializing.elapsed();
    state.metrics.serialize(serialize_time);

    tracing::info!(
        dataset = %request.dataset,
        rows = row_count,
        bytes = body.len(),
        truncated,
        plan_us = plan_time.as_micros(),
        queue_us = queue_time.as_micros(),
        execute_us = execute_time.as_micros(),
        serialize_us = serialize_time.as_micros(),
        "query executed"
    );
    Ok(json_response(
        body,
        &server_timing(&[
            ("plan", plan_time),
            ("queue", queue_time),
            ("execute", execute_time),
            ("serialize", serialize_time),
        ]),
    ))
}

fn compile(
    request: &QueryRequest,
    metrics: &QueryMetrics,
) -> Result<(plan::Plan, Duration), Rejection> {
    let planning = Instant::now();
    let compiled = plan::compile(request);
    let elapsed = planning.elapsed();
    metrics.compile(
        elapsed,
        if compiled.is_ok() {
            Outcome::Ok
        } else {
            Outcome::Error
        },
    );
    compiled.map(|plan| (plan, elapsed)).map_err(|invalid| {
        Rejection::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_query",
            Some(invalid.0),
        )
    })
}

enum Failure {
    Timeout,
    Runtime(RuntimeError),
}

/// Runs the compiled query on the pool within `deadline`; returns the rows
/// with the time spent queued (waiting for a worker plus hand-off) and
/// executing. Dropping the inner future on timeout interrupts the query.
async fn execute(
    runtime: &bi_runtime::Runtime,
    deadline: Duration,
    sql: String,
    params: Vec<Param>,
    columns: usize,
    fetch_limit: usize,
    metrics: &QueryMetrics,
) -> Result<(Fetched, Duration, Duration), Failure> {
    let submitted = Instant::now();
    let timer = metrics.execution(submitted);
    let marker = timer
        .as_ref()
        .map(crate::metrics::ExecutionTimer::start_marker);
    let execution = runtime.run(move |conn| {
        let started = Instant::now();
        if let Some(marker) = &marker {
            marker.start(started);
        }
        let fetched = fetch(conn, &sql, &params, columns, fetch_limit);
        let execute_time = started.elapsed();
        if let Some(marker) = &marker {
            marker.end(started + execute_time);
        }
        fetched.map(|fetched| (fetched, execute_time))
    });
    let outcome = tokio::time::timeout(deadline, execution).await;
    let completed = submitted.elapsed();
    if let Some(timer) = timer {
        timer.finish(match &outcome {
            Err(_) => Outcome::Timeout,
            Ok(Ok(_)) => Outcome::Ok,
            Ok(Err(RuntimeError::Busy)) => Outcome::Busy,
            Ok(Err(RuntimeError::Engine(_) | RuntimeError::Panicked)) => Outcome::Error,
            Ok(Err(_)) => Outcome::Unavailable,
        });
    }
    match outcome {
        Err(_) => Err(Failure::Timeout),
        Ok(Err(error)) => Err(Failure::Runtime(error)),
        Ok(Ok((fetched, execute_time))) => Ok((
            fetched,
            completed.saturating_sub(execute_time),
            execute_time,
        )),
    }
}

/// Status and problem code for a runtime failure.
fn status_for(error: &RuntimeError) -> (StatusCode, &'static str) {
    match error {
        RuntimeError::Busy => (StatusCode::TOO_MANY_REQUESTS, "too_many_queries"),
        RuntimeError::Engine(_) | RuntimeError::Panicked => {
            (StatusCode::INTERNAL_SERVER_ERROR, "query_failed")
        }
        _ => (StatusCode::SERVICE_UNAVAILABLE, "analytics_unavailable"),
    }
}

fn json_response(body: Vec<u8>, timing: &str) -> Response {
    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(value) = HeaderValue::from_str(timing) {
        headers.insert("server-timing", value);
    }
    response
}

enum Fetched {
    Rows(Vec<Vec<Option<String>>>),
    /// The result exceeded the byte budget; reading stopped there.
    TooLarge,
}

/// W3C `Server-Timing` value, durations in milliseconds.
fn server_timing(phases: &[(&str, Duration)]) -> String {
    let mut value = String::new();
    for (name, duration) in phases {
        if !value.is_empty() {
            value.push_str(", ");
        }
        let _ = write!(value, "{name};dur={:.3}", duration.as_secs_f64() * 1000.0);
    }
    value
}

/// Runs on a pool thread. Every selected column is already `VARCHAR`, so
/// values leave DuckDB as exact text. Stops as soon as the byte budget is
/// exceeded instead of materializing the whole result.
fn fetch(
    conn: &Connection,
    sql: &str,
    params: &[Param],
    columns: usize,
    limit: usize,
) -> duckdb::Result<Fetched> {
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
    let mut bytes = 0;
    while let Some(row) = rows.next()? {
        if out.len() == limit {
            break;
        }
        let mut values = Vec::with_capacity(columns);
        for i in 0..columns {
            let value: Option<String> = row.get(i)?;
            let size = value.as_ref().map_or(0, String::len);
            bytes += size;
            if size > MAX_CELL_BYTES || bytes > MAX_RESULT_BYTES {
                return Ok(Fetched::TooLarge);
            }
            values.push(value);
        }
        out.push(values);
    }
    Ok(Fetched::Rows(out))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
