//! bi-platform API server: Axum HTTP layer over the modular monolith.

pub mod analytics;
pub mod audit;
pub mod auth;
pub mod config;
pub mod db;
pub mod health;
pub mod metrics;
pub mod models;
pub mod oidc;
pub mod openapi;
pub mod problem;
pub mod query;
pub mod semantic_execution;
mod static_files;
pub mod telemetry;

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::Request;
use axum::routing::{get, post};
use tokio::net::TcpListener;
use tower_http::request_id::{
    MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};
use tower_http::trace::TraceLayer;

use crate::analytics::Analytics;
use crate::db::Database;

/// Largest accepted query request body.
const QUERY_BODY_LIMIT: usize = 64 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub auth: auth::AuthConfig,
    pub analytics: Analytics,
    pub query_timeout: Duration,
    pub metrics: metrics::QueryMetrics,
    pub models: models::Models,
}

impl AppState {
    /// API-only state: default authentication, no analytical runtime,
    /// default query deadline.
    #[must_use]
    pub fn new(db: Database) -> Self {
        Self {
            db,
            auth: auth::AuthConfig::default(),
            analytics: Analytics::disabled(),
            query_timeout: config::DEFAULT_QUERY_TIMEOUT,
            metrics: metrics::QueryMetrics::default(),
            models: models::Models::default(),
        }
    }
}

/// Builds the HTTP application: routes plus request-id and tracing layers.
pub fn router(state: AppState) -> Router {
    router_with_static(state, None)
}

/// Builds the same-origin application, optionally serving a frontend build.
pub fn router_with_static(state: AppState, static_dir: Option<PathBuf>) -> Router {
    let app = Router::new()
        .merge(auth::routes())
        .merge(oidc::routes())
        .route("/health/live", get(health::live))
        .route("/health/ready", get(health::ready))
        .route("/api/v1/models", get(models::list))
        .route(
            "/api/v1/queries",
            post(query::run).layer(DefaultBodyLimit::max(QUERY_BODY_LIMIT)),
        )
        .with_state(state.clone());
    let app = if let Some(directory) = static_dir {
        app.fallback(move |request| static_files::serve(directory.clone(), request))
    } else {
        app
    };
    app.layer(axum::middleware::from_fn_with_state(
        state,
        auth::enforce_session,
    ))
    // Layers run bottom-up on requests: the id is set before the trace span
    // reads it, and copied to the response on the way out.
    .layer(PropagateRequestIdLayer::x_request_id())
    .layer(
        TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
            let request_id = request
                .extensions()
                .get::<RequestId>()
                .and_then(|id| id.header_value().to_str().ok())
                .unwrap_or_default();
            tracing::info_span!(
                "http_request",
                method = %request.method(),
                path = %request.uri().path(),
                request_id,
            )
        }),
    )
    .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

/// Serves `app` on `listener` until `shutdown` resolves, then lets in-flight
/// requests finish.
///
/// # Errors
/// Returns an error if the server fails while accepting connections.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
}

/// Resolves on Ctrl+C, or on SIGTERM where available.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "cannot listen for Ctrl+C");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}
