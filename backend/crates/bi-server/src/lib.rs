//! bi-platform API server: Axum HTTP layer over the modular monolith.

pub mod auth;
pub mod config;
pub mod db;
pub mod health;
pub mod openapi;
pub mod problem;
mod static_files;
pub mod telemetry;

use std::future::Future;
use std::path::PathBuf;

use axum::Router;
use axum::http::Request;
use axum::routing::get;
use tokio::net::TcpListener;
use tower_http::request_id::{
    MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};
use tower_http::trace::TraceLayer;

use crate::db::Database;

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub auth: auth::AuthConfig,
}

impl AppState {
    #[must_use]
    pub fn new(db: Database) -> Self {
        Self {
            db,
            auth: auth::AuthConfig::default(),
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
        .route("/health/live", get(health::live))
        .route("/health/ready", get(health::ready))
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
    axum::serve(listener, app)
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
