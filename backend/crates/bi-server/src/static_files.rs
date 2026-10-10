//! Static assets and the Svelte SPA shell. API namespaces never reach the filesystem.

use std::path::PathBuf;

use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tower_http::services::{ServeDir, ServeFile};

pub async fn serve(directory: PathBuf, request: Request) -> Response {
    let path = request.uri().path();
    if matches!(path.split('/').nth(1), Some("api" | "health")) {
        return StatusCode::NOT_FOUND.into_response();
    }

    // A fetch, missing asset or mutation must not receive the SPA shell.
    let navigation = matches!(*request.method(), Method::GET | Method::HEAD)
        && !path.starts_with("/_app/")
        && !path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .contains('.')
        && request.headers().get(header::ACCEPT).is_some_and(|value| {
            value.to_str().is_ok_and(|accept| {
                accept.split(',').any(|entry| {
                    let mut parts = entry.trim().split(';');
                    parts.next() == Some("text/html")
                        && !parts.any(|part| {
                            part.trim()
                                .strip_prefix("q=")
                                .is_some_and(|q| q.parse::<f32>().is_ok_and(|q| q <= 0.0))
                        })
                })
            })
        })
        && request
            .headers()
            .get("sec-fetch-mode")
            .is_none_or(|value| value == "navigate")
        && request
            .headers()
            .get("sec-fetch-dest")
            .is_none_or(|value| value == "document");

    let result = if navigation {
        ServeDir::new(&directory)
            .fallback(ServeFile::new(directory.join("200.html")))
            .try_call(request)
            .await
    } else {
        ServeDir::new(&directory).try_call(request).await
    };
    match result {
        Ok(response) => response.into_response(),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            StatusCode::NOT_FOUND.into_response()
        }
        Err(error) => {
            tracing::error!(%error, "cannot serve frontend file");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
