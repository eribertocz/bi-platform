//! RFC 9457 responses shared by authentication and data endpoints.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const CONTENT_TYPE: &str = "application/problem+json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Problem {
    /// Problem type URI; `about:blank` means the HTTP status says it all.
    #[serde(rename = "type")]
    #[schema(format = "uri")]
    pub problem_type: String,
    pub title: String,
    #[schema(minimum = 400, maximum = 599)]
    pub status: u16,
    /// Human-readable explanation, safe to show; never internal errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Stable identifier for clients, e.g. `invalid_query`.
    #[schema(pattern = "^[a-z][a-z0-9_]+$")]
    pub code: String,
    /// Correlates with server logs (`x-request-id`).
    #[schema(min_length = 1)]
    pub request_id: String,
}

impl Problem {
    #[must_use]
    pub fn new(
        status: StatusCode,
        code: &str,
        detail: Option<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            problem_type: "about:blank".to_owned(),
            title: status.canonical_reason().unwrap_or("Error").to_owned(),
            status: status.as_u16(),
            detail,
            code: code.to_owned(),
            request_id: request_id.into(),
        }
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut response = (status, Json(self)).into_response();
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE));
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}
