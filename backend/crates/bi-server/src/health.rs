//! Liveness and readiness endpoints (M15 §22): liveness never touches
//! dependencies; readiness reports whether the API can serve.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Ok,
    Degraded,
    Unavailable,
}

impl From<bool> for HealthStatus {
    fn from(ok: bool) -> Self {
        if ok { Self::Ok } else { Self::Unavailable }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthCheck {
    #[schema(examples("postgres", "migrations"))]
    pub name: String,
    pub status: HealthStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Health {
    pub status: HealthStatus,
    pub checks: Vec<HealthCheck>,
}

#[utoipa::path(
    get,
    path = "/health/live",
    tag = "health",
    operation_id = "getLiveness",
    summary = "Liveness: el proceso responde. No consulta dependencias.",
    responses((status = 200, description = "Proceso activo", body = Health))
)]
pub async fn live() -> Response {
    health_response(
        StatusCode::OK,
        Health {
            status: HealthStatus::Ok,
            checks: Vec::new(),
        },
    )
}

#[utoipa::path(
    get,
    path = "/health/ready",
    tag = "health",
    operation_id = "getReadiness",
    summary = "Readiness: PostgreSQL accesible, migraciones aplicadas y, si está habilitado, el lago analítico listo.",
    responses(
        (status = 200, description = "Listo para servir", body = Health),
        (status = 503, description = "No listo; `checks` indica qué dependencia falla", body = Health)
    )
)]
pub async fn ready(State(state): State<AppState>) -> Response {
    let mut checks = vec![
        HealthCheck {
            name: "postgres".to_owned(),
            status: HealthStatus::from(state.db.ping().await),
        },
        HealthCheck {
            name: "migrations".to_owned(),
            status: HealthStatus::from(state.db.migrations_applied()),
        },
    ];
    // Queries need the analytical runtime (DuckDB + DuckLake) when enabled.
    if state.analytics.enabled() {
        checks.push(HealthCheck {
            name: "lake".to_owned(),
            status: HealthStatus::from(state.analytics.runtime().is_some()),
        });
    }
    let ready = checks.iter().all(|check| check.status == HealthStatus::Ok);
    let health = Health {
        status: HealthStatus::from(ready),
        checks,
    };
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    health_response(status, health)
}

fn health_response(status: StatusCode, health: Health) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(health),
    )
        .into_response()
}
