//! Server sessions for development and verified federated identities.
//! Data access remains restricted to synthetic data until semantic policies exist.

use std::fmt::Write as _;

use axum::Json;
use axum::extract::{Extension, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tower_http::request_id::RequestId;
use utoipa::ToSchema;

use crate::{AppState, config::ConfigError, problem::Problem};

const COOKIE_NAME: &str = "bi_session";
const DEFAULT_ORIGIN: &str = "http://127.0.0.1:8080";
const DEFAULT_TTL: u32 = 3600;

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub(crate) origin: String,
    pub(crate) secure: bool,
    pub(crate) ttl_seconds: u32,
    users: Vec<User>,
    pub(crate) oidc: Option<crate::oidc::OidcConfig>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            origin: DEFAULT_ORIGIN.to_owned(),
            secure: false,
            ttl_seconds: DEFAULT_TTL,
            users: Vec::new(),
            oidc: None,
        }
    }
}

impl AuthConfig {
    /// Validates the public origin, independently of untrusted Host/proxy headers.
    /// HTTP is permitted only on loopback; HTTPS always sets Secure cookies.
    ///
    /// # Errors
    /// Returns a redacted error for invalid origins, lifetimes or dev identities.
    pub fn from_lookup(lookup: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let mut config = Self::default();
        config.origin = lookup("BI_PUBLIC_ORIGIN").unwrap_or(config.origin);
        let uri: Uri = config
            .origin
            .parse()
            .map_err(|_| ConfigError::InvalidAuthConfig)?;
        config.secure = uri.scheme_str() == Some("https");
        if (!config.secure && uri.scheme_str() != Some("http"))
            || uri
                .authority()
                .is_none_or(|authority| authority.as_str().contains('@'))
            || uri
                .path_and_query()
                .is_some_and(|path| path.as_str() != "/")
            || config.origin.ends_with('/')
            || (!config.secure && !matches!(uri.host(), Some("localhost" | "127.0.0.1" | "[::1]")))
        {
            return Err(ConfigError::InvalidAuthConfig);
        }
        if let Some(ttl) = lookup("BI_SESSION_TTL_SECONDS") {
            config.ttl_seconds = ttl.parse().map_err(|_| ConfigError::InvalidAuthConfig)?;
            if !(60..=86400).contains(&config.ttl_seconds) {
                return Err(ConfigError::InvalidAuthConfig);
            }
        }
        // Even an environment variable cannot enable dev login without the feature.
        #[cfg(feature = "dev-auth")]
        if let Some(users) = lookup("BI_DEV_USERS") {
            config.users =
                serde_json::from_str(&users).map_err(|_| ConfigError::InvalidAuthConfig)?;
            for (index, user) in config.users.iter().enumerate() {
                if user.id.is_empty()
                    || user.id.len() > 100
                    || user.display_name.trim().is_empty()
                    || user.display_name.len() > 200
                    || config.users[..index]
                        .iter()
                        .any(|previous| previous.id == user.id)
                {
                    return Err(ConfigError::InvalidAuthConfig);
                }
            }
        }
        config.oidc = crate::oidc::OidcConfig::from_lookup(lookup, &config.origin)?;
        Ok(config)
    }

    fn development_enabled(&self) -> bool {
        cfg!(feature = "dev-auth") && self.has_synthetic_users()
    }

    /// Synthetic identities are configured (only possible with `dev-auth`).
    pub(crate) fn has_synthetic_users(&self) -> bool {
        !self.users.is_empty()
    }

    /// The public origin is a loopback address.
    pub(crate) fn loopback_origin(&self) -> bool {
        self.origin
            .parse::<Uri>()
            .is_ok_and(|uri| matches!(uri.host(), Some("localhost" | "127.0.0.1" | "[::1]")))
    }

    /// The only setup where `BI_METADATA_MODE=development` is accepted:
    /// synthetic dev login compiled in and configured, a loopback public
    /// origin and no federated login. Never inferred from the database.
    pub(crate) fn allows_metadata_development(&self) -> bool {
        self.development_enabled() && self.oidc.is_none() && self.loopback_origin()
    }

    pub(crate) fn cookie(&self, token: &str, max_age: u32) -> String {
        // The separate short-lived OIDC binding cookie handles the cross-site callback.
        // Origin validation remains mandatory: SameSite alone is not CSRF protection.
        format!(
            "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
            if self.secure { "; Secure" } else { "" }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct User {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DataScope {
    Synthetic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub user: User,
    /// Server-derived restriction. This session grants no access to real sources.
    pub data_scope: DataScope,
    #[schema(format = DateTime)]
    pub expires_at: String,
}

#[derive(Clone)]
pub struct Principal {
    pub session: Session,
    pub federated_identity: Option<crate::oidc::FederatedIdentity>,
    token_hash: Vec<u8>,
}

pub(crate) fn problem(status: StatusCode, code: &str, request_id: &RequestId) -> Response {
    Problem::new(
        status,
        code,
        None,
        request_id
            .header_value()
            .to_str()
            .ok()
            .filter(|id| !id.is_empty())
            .unwrap_or("unavailable"),
    )
    .into_response()
}

pub(crate) fn token_hash(headers: &HeaderMap) -> Option<Vec<u8>> {
    let mut token = None;
    for cookie in headers.get_all(header::COOKIE) {
        for item in cookie.to_str().ok()?.split(';') {
            if let Some(value) = item.trim().strip_prefix("bi_session=") {
                // Reject duplicate cookies, malformed values and unbounded input.
                if token.is_some()
                    || value.len() != 64
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                {
                    return None;
                }
                token = Some(value);
            }
        }
    }
    token.map(|value| Sha256::digest(value.as_bytes()).to_vec())
}

/// Covers the entire API namespace, including unknown/future endpoints.
pub(crate) async fn enforce_session(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if path != "/api/v1" && !path.starts_with("/api/v1/") {
        return next.run(request).await;
    }
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .expect("request id layer runs first");
    let development_route = matches!(path, "/api/v1/dev/login" | "/api/v1/dev/users");
    if development_route && !state.auth.development_enabled() {
        return problem(StatusCode::NOT_FOUND, "not_found", request_id);
    }
    let oidc_route = matches!(
        path,
        "/api/v1/oidc/config" | "/api/v1/oidc/start" | "/api/v1/oidc/callback"
    );
    // Only configuration answers, and OIDC routes while OIDC is disabled,
    // touch no metadata state.
    let uses_metadata = !matches!(path, "/api/v1/dev/users" | "/api/v1/oidc/config")
        && (!oidc_route || state.auth.oidc.is_some());
    let session = if !development_route && !oidc_route {
        let Some(hash) = token_hash(request.headers()) else {
            return problem(StatusCode::UNAUTHORIZED, "session_required", request_id);
        };
        Some(hash)
    } else {
        None
    };
    if !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        // Fail closed for missing/null/duplicate/foreign Origin. Never trust Host,
        // X-Forwarded-* or Referer to widen the configured origin; no CORS enabled.
        let mut origins = request.headers().get_all(header::ORIGIN).iter();
        let origin_ok = origins
            .next()
            .is_some_and(|origin| origin == state.auth.origin.as_str())
            && origins.next().is_none();
        let site_ok = request
            .headers()
            .get("sec-fetch-site")
            .is_none_or(|site| site == "same-origin");
        if !origin_ok || !site_ok {
            let mut rejection = Problem::new(
                StatusCode::FORBIDDEN,
                "origin_forbidden",
                Some(format!("Abre la aplicación desde {}.", state.auth.origin)),
                request_id.header_value().to_str().unwrap_or("unavailable"),
            );
            rejection.public_origin = Some(state.auth.origin.clone());
            return rejection.into_response();
        }
    }
    // One admission point for every handler that reads or writes metadata:
    // the prepared state decides before any work, without querying PostgreSQL.
    if uses_metadata && !state.db.admits() {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "metadata_not_ready",
            request_id,
        );
    }
    if let Some(hash) = session {
        match principal(&state, hash).await {
            Ok(principal) => {
                request.extensions_mut().insert(principal);
            }
            Err((status, code)) => return problem(status, code, request_id),
        }
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static header"),
    );
    response
}

/// The live session behind `hash`, or the status and problem code to answer instead.
async fn principal(
    state: &AppState,
    hash: Vec<u8>,
) -> Result<Principal, (StatusCode, &'static str)> {
    const UNAVAILABLE: &str = "session_store_unavailable";
    let mut connection = state.db.acquire().await.map_err(|error| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            error.problem_code(UNAVAILABLE),
        )
    })?;
    let row = sqlx::query(
        "SELECT user_id, display_name, federated_identity::text AS federated_identity, to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS expires_at \
         FROM bi_meta.sessions WHERE token_hash = $1 AND expires_at > clock_timestamp() AND revoked_at IS NULL AND data_scope = 'synthetic'",
    ).bind(&hash).fetch_optional(&mut *connection).await;
    match row {
        Ok(Some(row)) => {
            let identity: Option<String> = row.get("federated_identity");
            let Ok(identity) = identity.map(|json| serde_json::from_str(&json)).transpose() else {
                return Err((StatusCode::UNAUTHORIZED, "session_required"));
            };
            Ok(Principal {
                session: Session {
                    user: User {
                        id: row.get("user_id"),
                        display_name: row.get("display_name"),
                    },
                    data_scope: DataScope::Synthetic,
                    expires_at: row.get("expires_at"),
                },
                federated_identity: identity,
                token_hash: hash,
            })
        }
        Ok(None) => Err((StatusCode::UNAUTHORIZED, "session_required")),
        Err(_) => Err((StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE)),
    }
}

#[utoipa::path(get, path = "/api/v1/session", tag = "session", operation_id = "getSession",
    security(("sessionCookie" = [])),
    responses((status = 200, description = "Sesión verificada por el servidor", body = Session),
        (status = 401, description = "Sin sesión válida", body = Problem, content_type = "application/problem+json"),
        (status = 503, description = "PostgreSQL no disponible, o metadatos no listos (`metadata_not_ready`)", body = Problem, content_type = "application/problem+json")))]
pub(crate) async fn current(Extension(principal): Extension<Principal>) -> Json<Session> {
    Json(principal.session)
}

#[utoipa::path(post, path = "/api/v1/logout", tag = "session", operation_id = "logout",
    security(("sessionCookie" = [])),
    responses((status = 204, description = "Sesión revocada y cookie eliminada"),
        (status = 401, description = "Sin sesión válida", body = Problem, content_type = "application/problem+json"),
        (status = 403, description = "Origin ausente o distinto al configurado", body = Problem, content_type = "application/problem+json"),
        (status = 503, description = "PostgreSQL no disponible, o metadatos no listos (`metadata_not_ready`)", body = Problem, content_type = "application/problem+json")))]
pub(crate) async fn logout(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(id): Extension<RequestId>,
) -> Response {
    let mut connection = match state.db.acquire().await {
        Ok(connection) => connection,
        Err(error) => {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                error.problem_code("session_store_unavailable"),
                &id,
            );
        }
    };
    if sqlx::query(
        "UPDATE bi_meta.sessions SET revoked_at = clock_timestamp() WHERE token_hash = $1",
    )
    .bind(principal.token_hash)
    .execute(&mut *connection)
    .await
    .is_err()
    {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "session_store_unavailable",
            &id,
        );
    }
    (
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, state.auth.cookie("", 0))],
    )
        .into_response()
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DevLogin {
    /// Selects a configured synthetic identity; this is not password authentication.
    pub user_id: String,
}

// Contract includes unavailable dev operations consistently in every build.
#[utoipa::path(get, path = "/api/v1/dev/users", tag = "development", operation_id = "getDevUsers",
    description = "Solo feature dev-auth con BI_DEV_USERS configurado; únicamente fixtures sintéticos.",
    responses((status = 200, description = "Identidades sintéticas configuradas", body = Vec<User>),
        (status = 404, description = "Login de desarrollo deshabilitado", body = Problem, content_type = "application/problem+json")))]
pub(crate) async fn dev_users(State(state): State<AppState>) -> Json<Vec<User>> {
    Json(state.auth.users)
}

#[utoipa::path(post, path = "/api/v1/dev/login", tag = "development", operation_id = "devLogin",
    description = "Solo feature dev-auth. Requiere Origin configurado; selecciona identidad sintética sin contraseña. Rota la sesión anterior.",
    request_body = DevLogin,
    responses((status = 200, description = "Sesión creada; cookie HttpOnly SameSite=Strict y Secure con HTTPS", body = Session),
        (status = 400, description = "Solicitud JSON inválida", body = Problem, content_type = "application/problem+json"),
        (status = 401, description = "Usuario no configurado", body = Problem, content_type = "application/problem+json"),
        (status = 403, description = "Origin ausente o distinto al configurado", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "Login de desarrollo deshabilitado", body = Problem, content_type = "application/problem+json"),
        (status = 503, description = "No se pudo crear la sesión, o metadatos no listos (`metadata_not_ready`)", body = Problem, content_type = "application/problem+json")))]
pub(crate) async fn dev_login(
    State(state): State<AppState>,
    Extension(id): Extension<RequestId>,
    headers: HeaderMap,
    body: Result<Json<DevLogin>, axum::extract::rejection::JsonRejection>,
) -> Response {
    #[cfg(feature = "dev-auth")]
    {
        let Ok(Json(login)) = body else {
            return problem(StatusCode::BAD_REQUEST, "invalid_request", &id);
        };
        let Some(user) = state
            .auth
            .users
            .iter()
            .find(|user| user.id == login.user_id)
        else {
            return problem(StatusCode::UNAUTHORIZED, "unknown_dev_user", &id);
        };
        let Ok(token) = random_token().await else {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "session_creation_failed",
                &id,
            );
        };
        let mut transaction = match state.db.begin().await {
            Ok(transaction) => transaction,
            Err(error) => {
                return problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    error.problem_code("session_store_unavailable"),
                    &id,
                );
            }
        };
        if let Some(previous) = token_hash(&headers)
            && sqlx::query(
                "UPDATE bi_meta.sessions SET revoked_at = clock_timestamp() WHERE token_hash = $1",
            )
            .bind(previous)
            .execute(&mut *transaction)
            .await
            .is_err()
        {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "session_store_unavailable",
                &id,
            );
        }
        let inserted = sqlx::query_scalar::<_, String>(
            "INSERT INTO bi_meta.sessions (token_hash, user_id, display_name, data_scope, expires_at) \
             VALUES ($1, $2, $3, 'synthetic', clock_timestamp() + make_interval(secs => $4)) \
             RETURNING to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')",
        ).bind(Sha256::digest(token.as_bytes()).as_slice()).bind(&user.id).bind(&user.display_name)
            .bind(f64::from(state.auth.ttl_seconds)).fetch_one(&mut *transaction).await;
        let Ok(expires_at) = inserted else {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "session_store_unavailable",
                &id,
            );
        };
        if transaction.commit().await.is_err() {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "session_store_unavailable",
                &id,
            );
        }
        (
            [(
                header::SET_COOKIE,
                state.auth.cookie(&token, state.auth.ttl_seconds),
            )],
            Json(Session {
                user: user.clone(),
                data_scope: DataScope::Synthetic,
                expires_at,
            }),
        )
            .into_response()
    }
    #[cfg(not(feature = "dev-auth"))]
    {
        let _ = (state, headers, body);
        problem(StatusCode::NOT_FOUND, "not_found", &id)
    }
}

pub(crate) fn routes() -> axum::Router<AppState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/v1/session", get(current))
        .route("/api/v1/logout", post(logout))
        .route("/api/v1/dev/users", get(dev_users))
        .route("/api/v1/dev/login", post(dev_login))
}

/// OS entropy is acquired outside Tokio's executor; no token is logged.
pub(crate) async fn random_token() -> Result<String, ()> {
    tokio::task::spawn_blocking(|| {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| ())?;
        let mut token = String::with_capacity(64);
        for byte in bytes {
            write!(token, "{byte:02x}").expect("writing to String cannot fail");
        }
        Ok(token)
    })
    .await
    .map_err(|_| ())?
}
