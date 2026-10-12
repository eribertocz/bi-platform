//! OIDC HTTP adapter. Login actions operate on the caller's session only;
//! no data permission is inferred from an identity or a provider group.

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{ConnectInfo, Extension, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreGenderClaim, CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm, CoreProviderMetadata,
};
use openidconnect::{
    AccessTokenHash, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, HttpRequest, HttpResponse, IdToken, IssuerUrl, Nonce,
    OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tower_http::request_id::RequestId;
use utoipa::ToSchema;

use crate::{AppState, auth, config::ConfigError, problem::Problem};

const BINDING_COOKIE: &str = "bi_oidc";
const ATTEMPT_SECONDS: u64 = 300;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_LIVE_ATTEMPTS: i64 = 1024;
const GLOBAL_BURST: f64 = 120.0;
const CLIENT_BURST: f64 = 20.0;
const MAX_CLIENTS: usize = 4096;

struct Bucket {
    tokens: f64,
    updated: Instant,
}
impl Bucket {
    fn refill(&mut self, now: Instant, burst: f64, rate: f64) {
        self.tokens =
            (self.tokens + now.duration_since(self.updated).as_secs_f64() * rate).min(burst);
        self.updated = now;
    }
}
pub(crate) struct Admission {
    global: Bucket,
    clients: BTreeMap<IpAddr, Bucket>,
    rejected: u64,
    last_log: Option<Instant>,
}
impl Admission {
    fn new(now: Instant) -> Self {
        Self {
            global: Bucket {
                tokens: GLOBAL_BURST,
                updated: now,
            },
            clients: BTreeMap::new(),
            rejected: 0,
            last_log: None,
        }
    }
    fn allow(&mut self, ip: IpAddr, now: Instant) -> bool {
        self.global.refill(now, GLOBAL_BURST, 2.0);
        if self.global.tokens < 1.0 {
            return false;
        }
        if !self.clients.contains_key(&ip) && self.clients.len() >= MAX_CLIENTS {
            // Only discard fully replenished buckets; rotating IPs cannot reset an active limit.
            self.clients
                .retain(|_, bucket| now.duration_since(bucket.updated) < Duration::from_secs(60));
            if self.clients.len() >= MAX_CLIENTS {
                return false;
            }
        }
        let client = self.clients.entry(ip).or_insert(Bucket {
            tokens: CLIENT_BURST,
            updated: now,
        });
        client.refill(now, CLIENT_BURST, 1.0 / 3.0);
        if client.tokens < 1.0 {
            return false;
        }
        client.tokens -= 1.0;
        self.global.tokens -= 1.0;
        true
    }
}
type AdmissionLimit = Arc<Mutex<Admission>>;

fn admit(
    state: &AppState,
    id: &RequestId,
    limit: &AdmissionLimit,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
) -> Option<Response> {
    // Never trust Forwarded/X-Forwarded-For from an anonymous caller. Without
    // transport information, all callers share one conservative client bucket.
    let ip = peer.map_or(IpAddr::from([0, 0, 0, 0]), |Extension(peer)| peer.0.ip());
    let mut admission = limit
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = Instant::now();
    if admission.allow(ip, now) {
        return None;
    }
    state.metrics.oidc_login("rate_limited");
    admission.rejected = admission.rejected.saturating_add(1);
    if admission
        .last_log
        .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(60))
    {
        tracing::warn!(
            event_code = "rate_limited",
            rejected = admission.rejected,
            "OIDC admission rejections in interval"
        );
        admission.rejected = 0;
        admission.last_log = Some(now);
    }
    Some(
        (
            [(header::REFERRER_POLICY, "no-referrer")],
            auth::problem(StatusCode::TOO_MANY_REQUESTS, "rate_limited", id),
        )
            .into_response(),
    )
}

type DiscoveredClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;
type CompanyIdToken = IdToken<
    CompanyClaims,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;

#[derive(Clone)]
pub(crate) struct OidcConfig {
    issuer: IssuerUrl,
    client_id: String,
    secret: Option<String>,
    redirect: RedirectUrl,
    attribute_claims: Vec<String>,
}

impl fmt::Debug for OidcConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OidcConfig { [redacted] }")
    }
}

impl OidcConfig {
    pub(crate) fn from_lookup(
        lookup: &impl Fn(&str) -> Option<String>,
        origin: &str,
    ) -> Result<Option<Self>, ConfigError> {
        let issuer = lookup("BI_OIDC_ISSUER");
        let client_id = lookup("BI_OIDC_CLIENT_ID");
        let secret = lookup("BI_OIDC_CLIENT_SECRET");
        let attributes = lookup("BI_OIDC_ATTRIBUTE_CLAIMS");
        if issuer.is_none() && client_id.is_none() && secret.is_none() && attributes.is_none() {
            return Ok(None);
        }
        let issuer = issuer.ok_or(ConfigError::InvalidAuthConfig)?;
        let client_id = client_id.ok_or(ConfigError::InvalidAuthConfig)?;
        let url = reqwest::Url::parse(&issuer).map_err(|_| ConfigError::InvalidAuthConfig)?;
        if !safe_transport(&url)
            || url.query().is_some()
            || url.fragment().is_some()
            || client_id.trim().is_empty()
            || client_id.len() > 200
            || secret.as_ref().is_some_and(String::is_empty)
        {
            return Err(ConfigError::InvalidAuthConfig);
        }
        let attribute_claims: Vec<String> = attributes
            .map(|claims| {
                claims
                    .split(',')
                    .map(|claim| claim.trim().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        if attribute_claims.len() > 20
            || attribute_claims.iter().any(|claim| {
                claim.is_empty()
                    || claim.len() > 100
                    || matches!(claim.as_str(), "nonce" | "at_hash" | "c_hash")
            })
        {
            return Err(ConfigError::InvalidAuthConfig);
        }
        Ok(Some(Self {
            issuer: IssuerUrl::new(issuer).map_err(|_| ConfigError::InvalidAuthConfig)?,
            client_id,
            secret,
            redirect: RedirectUrl::new(format!("{origin}/api/v1/oidc/callback"))
                .map_err(|_| ConfigError::InvalidAuthConfig)?,
            attribute_claims,
        }))
    }

    async fn client(&self, http: &OidcHttp) -> Result<DiscoveredClient, ()> {
        // Fetch JWKS for every login/callback: rotation cannot leave a stale key
        // accepted indefinitely. A provider outage fails closed, including cache misses.
        let metadata = CoreProviderMetadata::discover_async(self.issuer.clone(), http)
            .await
            .map_err(|_| ())?;
        let auth_url =
            reqwest::Url::parse(metadata.authorization_endpoint().as_str()).map_err(|_| ())?;
        if !safe_transport(&auth_url) {
            return Err(());
        }
        Ok(CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(self.client_id.clone()),
            self.secret.clone().map(ClientSecret::new),
        )
        .set_redirect_uri(self.redirect.clone()))
    }
}

fn safe_transport(url: &reqwest::Url) -> bool {
    url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && (url.scheme() == "https"
            || (url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
}

fn http_client() -> Result<OidcHttp, ()> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .map(OidcHttp)
        .map_err(|_| ())
}

struct OidcHttp(reqwest::Client);

impl std::ops::Deref for OidcHttp {
    type Target = reqwest::Client;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'c> openidconnect::AsyncHttpClient<'c> for OidcHttp {
    type Error = std::io::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, Self::Error>> + Send + 'c>,
    >;
    fn call(&'c self, request: HttpRequest) -> Self::Future {
        let http = self.0.clone();
        Box::pin(send_http(http, request))
    }
}

async fn send_http(
    http: reqwest::Client,
    request: HttpRequest,
) -> Result<HttpResponse, std::io::Error> {
    let error = || std::io::Error::other("oidc_provider_unavailable");
    let url = reqwest::Url::parse(&request.uri().to_string()).map_err(|_| error())?;
    if !safe_transport(&url) {
        return Err(error());
    }
    let mut response = http
        .request(request.method().clone(), url)
        .headers(request.headers().clone())
        .body(request.body().clone())
        .send()
        .await
        .map_err(|_| error())?;
    let mut builder = axum::http::Response::builder().status(response.status());
    *builder.headers_mut().ok_or_else(error)? = response.headers().clone();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| error())? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(error());
        }
        body.extend_from_slice(&chunk);
    }
    builder.body(body).map_err(|_| error())
}

/// Internal identity contract for a future authorization adapter, version 1.
/// Only signed, explicitly selected attributes survive token verification.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederatedIdentity {
    pub version: u8,
    pub issuer: String,
    pub subject: String,
    pub groups: Vec<String>,
    pub attributes: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CompanyClaims {
    #[serde(flatten)]
    values: BTreeMap<String, serde_json::Value>,
}
impl openidconnect::AdditionalClaims for CompanyClaims {}

fn now_seconds() -> Result<i64, ()> {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ())?
            .as_secs(),
    )
    .map_err(|_| ())
}

fn verified_identity(
    client: &DiscoveredClient,
    config: &OidcConfig,
    tokens: &openidconnect::core::CoreTokenResponse,
    nonce: &str,
) -> Result<(auth::User, FederatedIdentity), ()> {
    verify_identity_at(client, config, tokens, nonce, now_seconds()?)
}

fn verify_identity_at(
    client: &DiscoveredClient,
    config: &OidcConfig,
    tokens: &openidconnect::core::CoreTokenResponse,
    nonce: &str,
    now: i64,
) -> Result<(auth::User, FederatedIdentity), ()> {
    let raw_token = tokens.id_token().ok_or(())?.to_string();
    let token: CompanyIdToken =
        serde_json::from_value(serde_json::Value::String(raw_token)).map_err(|_| ())?;
    let verifier = client
        .id_token_verifier()
        .set_time_fn(move || {
            chrono::DateTime::from_timestamp(now, 0).expect("system time in range")
        })
        .set_allowed_algs([
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            CoreJwsSigningAlgorithm::RsaSsaPssSha256,
            CoreJwsSigningAlgorithm::EcdsaP256Sha256,
            CoreJwsSigningAlgorithm::EdDsa,
        ])
        .set_issue_time_verifier_fn(move |iat| {
            if iat.timestamp() > now + 30 || iat.timestamp() < now - 330 {
                Err("invalid_issue_time".to_owned())
            } else {
                Ok(())
            }
        });
    let expected_nonce = Nonce::new(nonce.to_owned());
    let claims = token.claims(&verifier, &expected_nonce).map_err(|_| ())?;
    if claims
        .authorized_party()
        .is_some_and(|azp| azp.as_str() != config.client_id)
        || claims.expiration() <= claims.issue_time()
    {
        return Err(());
    }
    if let Some(expected) = claims.access_token_hash() {
        let actual = AccessTokenHash::from_token(
            tokens.access_token(),
            token.signing_alg().map_err(|_| ())?,
            token.signing_key(&verifier).map_err(|_| ())?,
        )
        .map_err(|_| ())?;
        if actual != *expected {
            return Err(());
        }
    }
    let subject = claims.subject().as_str();
    if subject.is_empty() || subject.len() > 255 {
        return Err(());
    }
    let extra = &claims.additional_claims().values;
    if extra
        .get("nbf")
        .is_some_and(|nbf| nbf.as_i64().is_none_or(|nbf| nbf > now))
    {
        return Err(());
    }
    let groups: Vec<String> = extra
        .get("groups")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|_| ())?
        .unwrap_or_default();
    if groups.len() > 200
        || groups
            .iter()
            .any(|group| group.is_empty() || group.len() > 255)
    {
        return Err(());
    }
    let attributes = config
        .attribute_claims
        .iter()
        .filter_map(|name| extra.get(name).map(|value| (name.clone(), value.clone())))
        .collect();
    // Length-delimited pair prevents issuer/sub collisions; emails never link accounts.
    let key = serde_json::to_vec(&(config.issuer.as_str(), subject)).map_err(|_| ())?;
    let user_id = format!("oidc:{}", hexadecimal(&Sha256::digest(key)));
    let user = auth::User {
        id: user_id,
        display_name: claims
            .name()
            .and_then(|name| name.get(None))
            .map_or(subject, |name| name.as_str())
            .chars()
            .take(200)
            .collect(),
    };
    Ok((
        user,
        FederatedIdentity {
            version: 1,
            issuer: config.issuer.as_str().to_owned(),
            subject: subject.to_owned(),
            groups,
            attributes,
        },
    ))
}

#[derive(Serialize, ToSchema)]
pub struct LoginConfig {
    pub enabled: bool,
}
#[derive(Serialize, Deserialize, ToSchema)]
pub struct LoginStart {
    pub authorization_url: String,
}

#[utoipa::path(get, path = "/api/v1/oidc/config", tag = "session", operation_id = "getOidcConfig",
    description = "Acción session:login, recurso sesión propia; público, no revela secretos.",
    responses((status = 200, description = "Disponibilidad del login corporativo", body = LoginConfig)))]
pub(crate) async fn available(State(state): State<AppState>) -> Json<LoginConfig> {
    Json(LoginConfig {
        enabled: state.auth.oidc.is_some(),
    })
}

fn failure(state: &AppState, status: StatusCode, code: &'static str, id: &RequestId) -> Response {
    tracing::warn!(event_code = code, "OIDC login rejected");
    state.metrics.oidc_login(code);
    auth::problem(status, code, id)
}

fn binding_cookie(state: &AppState, value: &str, ttl: u64) -> String {
    format!(
        "{BINDING_COOKIE}={value}; Path=/api/v1/oidc; HttpOnly; SameSite=Lax; Max-Age={ttl}{}",
        if state.auth.secure { "; Secure" } else { "" }
    )
}

fn browser_binding(headers: &HeaderMap) -> Option<Vec<u8>> {
    let mut token = None;
    for cookie in headers.get_all(header::COOKIE) {
        for part in cookie.to_str().ok()?.split(';') {
            if let Some(value) = part.trim().strip_prefix("bi_oidc=") {
                if token.is_some()
                    || value.len() != 64
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                {
                    return None;
                }
                token = Some(value);
            }
        }
    }
    token.map(|value| Sha256::digest(value.as_bytes()).to_vec())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoParameters {}

#[utoipa::path(post, path = "/api/v1/oidc/start", tag = "session", operation_id = "startOidcLogin",
    description = "Acción session:login, recurso sesión propia. Origin obligatorio. PKCE S256, nonce y state con vida de cinco minutos. Redirección fija.",
    responses((status = 200, description = "URL del proveedor y cookie temporal HttpOnly", body = LoginStart),
        (status = 400, description = "Parámetros no permitidos", body = Problem),
        (status = 403, description = "Origin inválido", body = Problem),
        (status = 404, description = "OIDC deshabilitado", body = Problem),
        (status = 429, description = "Admisión OIDC limitada", body = Problem),
        (status = 503, description = "Proveedor, PostgreSQL o capacidad de intentos no disponible", body = Problem)))]
pub(crate) async fn start(
    State(state): State<AppState>,
    Extension(id): Extension<RequestId>,
    Extension(limit): Extension<AdmissionLimit>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    query: Result<Query<NoParameters>, axum::extract::rejection::QueryRejection>,
) -> Response {
    if let Some(response) = admit(&state, &id, &limit, peer) {
        return response;
    }
    let Some(config) = &state.auth.oidc else {
        return failure(&state, StatusCode::NOT_FOUND, "oidc_disabled", &id);
    };
    if query.is_err() {
        return failure(&state, StatusCode::BAD_REQUEST, "invalid_request", &id);
    }
    let Ok(http) = http_client() else {
        return failure(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "oidc_unavailable",
            &id,
        );
    };
    let Ok(client) = config.client(&http).await else {
        return failure(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "oidc_unavailable",
            &id,
        );
    };
    let (Ok(state_token), Ok(nonce), Ok(browser), Ok(verifier)) = (
        auth::random_token().await,
        auth::random_token().await,
        auth::random_token().await,
        auth::random_token().await,
    ) else {
        return failure(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "oidc_unavailable",
            &id,
        );
    };
    let challenge =
        PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(verifier.clone()));
    let url_state = state_token.clone();
    let url_nonce = nonce.clone();
    let (url, _, _) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            move || CsrfToken::new(url_state),
            move || Nonce::new(url_nonce),
        )
        .add_scope(Scope::new("profile".to_owned()))
        .set_pkce_challenge(challenge)
        .url();
    // Bounded lifetime, opportunistic cleanup; plaintext nonce/verifier never leave the server.
    let previous = auth::token_hash(&headers);
    let result = save_attempt(
        state.db.pool().clone(),
        Sha256::digest(state_token.as_bytes()).to_vec(),
        Sha256::digest(browser.as_bytes()).to_vec(),
        nonce,
        verifier,
        previous,
    )
    .await;
    if !matches!(result, Ok(true)) {
        return failure(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            if matches!(result, Ok(false)) {
                "oidc_attempt_capacity"
            } else {
                "session_store_unavailable"
            },
            &id,
        );
    }
    (
        [(
            header::SET_COOKIE,
            binding_cookie(&state, &browser, ATTEMPT_SECONDS),
        )],
        Json(LoginStart {
            authorization_url: url.to_string(),
        }),
    )
        .into_response()
}

async fn save_attempt(
    pool: sqlx::PgPool,
    state_hash: Vec<u8>,
    browser_hash: Vec<u8>,
    nonce: String,
    verifier: String,
    previous: Option<Vec<u8>>,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    // Serialize cleanup/count/insert across processes sharing this database.
    sqlx::query("LOCK TABLE bi_meta.oidc_attempts IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM bi_meta.oidc_attempts WHERE expires_at <= clock_timestamp()")
        .execute(&mut *tx)
        .await?;
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM bi_meta.oidc_attempts WHERE expires_at > clock_timestamp()",
    )
    .fetch_one(&mut *tx)
    .await?;
    if live >= MAX_LIVE_ATTEMPTS {
        tx.commit().await?;
        return Ok(false);
    }
    sqlx::query("INSERT INTO bi_meta.oidc_attempts (state_hash, browser_hash, nonce, verifier, previous_session) VALUES ($1, $2, $3, $4, $5)")
        .bind(state_hash).bind(browser_hash).bind(nonce).bind(verifier).bind(previous).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(true)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Callback {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
    iss: Option<String>,
    #[serde(rename = "session_state")]
    _session_state: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/oidc/callback", tag = "session", operation_id = "completeOidcLogin",
    description = "Acción session:login, recurso sesión propia. Requiere state, code y cookie temporal; consume el intento atómicamente antes del intercambio. Solo redirige a /.",
    params(("state" = String, Query, description = "State de un solo uso"), ("code" = String, Query, description = "Código del proveedor")),
    responses((status = 303, description = "Sesión de servidor creada; cookie HttpOnly SameSite=Strict"),
        (status = 401, description = "Intento o token inválido", body = Problem),
        (status = 404, description = "OIDC deshabilitado", body = Problem),
        (status = 429, description = "Admisión OIDC limitada", body = Problem),
        (status = 503, description = "Proveedor, PostgreSQL o capacidad de intentos no disponible", body = Problem)))]
pub(crate) async fn callback(
    State(state): State<AppState>,
    Extension(id): Extension<RequestId>,
    Extension(limit): Extension<AdmissionLimit>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    query: Result<Query<Callback>, axum::extract::rejection::QueryRejection>,
) -> Response {
    if let Some(response) = admit(&state, &id, &limit, peer) {
        return response;
    }
    let mut response = complete(&state, &id, &headers, query).await;
    if response.status() == StatusCode::UNAUTHORIZED {
        let result = async {
            let event = login_event(&id, None).await?;
            let mut tx = state.db.pool().begin().await.map_err(|_| ())?;
            crate::audit::append(&mut tx, &event)
                .await
                .map_err(|_| ())?;
            tx.commit().await.map_err(|_| ())
        }
        .await;
        if result.is_err() {
            response = failure(
                &state,
                StatusCode::SERVICE_UNAVAILABLE,
                "audit_store_unavailable",
                &id,
            );
        }
    }
    response.headers_mut().append(
        header::SET_COOKIE,
        binding_cookie(&state, "", 0)
            .parse()
            .expect("static cookie"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        "no-referrer".parse().expect("static header"),
    );
    response
}

async fn complete(
    state: &AppState,
    id: &RequestId,
    headers: &HeaderMap,
    query: Result<Query<Callback>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Some(config) = &state.auth.oidc else {
        return failure(state, StatusCode::NOT_FOUND, "oidc_disabled", id);
    };
    let Ok(Query(query)) = query else {
        return failure(state, StatusCode::UNAUTHORIZED, "oidc_attempt_invalid", id);
    };
    let Some(state_token) = query.state.filter(|s| s.len() == 64) else {
        return failure(state, StatusCode::UNAUTHORIZED, "oidc_attempt_invalid", id);
    };
    let Some(browser) = browser_binding(headers) else {
        return failure(state, StatusCode::UNAUTHORIZED, "oidc_attempt_invalid", id);
    };
    let attempt = sqlx::query("DELETE FROM bi_meta.oidc_attempts WHERE state_hash = $1 AND browser_hash = $2 AND expires_at > clock_timestamp() RETURNING nonce, verifier, previous_session")
        .bind(Sha256::digest(state_token.as_bytes()).to_vec()).bind(browser)
        .fetch_optional(state.db.pool()).await;
    let row = match attempt {
        Ok(Some(row)) => row,
        Ok(None) => return failure(state, StatusCode::UNAUTHORIZED, "oidc_attempt_invalid", id),
        Err(_) => {
            return failure(
                state,
                StatusCode::SERVICE_UNAVAILABLE,
                "session_store_unavailable",
                id,
            );
        }
    };
    if query.error.is_some() || query.iss.is_some_and(|iss| iss != config.issuer.as_str()) {
        return failure(state, StatusCode::UNAUTHORIZED, "oidc_attempt_invalid", id);
    }
    let Some(code) = query
        .code
        .filter(|code| !code.is_empty() && code.len() <= 4096)
    else {
        return failure(state, StatusCode::UNAUTHORIZED, "oidc_attempt_invalid", id);
    };
    let Ok(http) = http_client() else {
        return failure(
            state,
            StatusCode::SERVICE_UNAVAILABLE,
            "oidc_unavailable",
            id,
        );
    };
    let Ok(client) = config.client(&http).await else {
        return failure(
            state,
            StatusCode::SERVICE_UNAVAILABLE,
            "oidc_unavailable",
            id,
        );
    };
    let Ok(exchange) = client.exchange_code(AuthorizationCode::new(code)) else {
        return failure(
            state,
            StatusCode::SERVICE_UNAVAILABLE,
            "oidc_unavailable",
            id,
        );
    };
    let tokens = exchange
        .set_pkce_verifier(PkceCodeVerifier::new(row.get("verifier")))
        .request_async(&http)
        .await;
    let Ok(tokens) = tokens else {
        return failure(state, StatusCode::UNAUTHORIZED, "oidc_token_invalid", id);
    };
    let Ok((user, identity)) =
        verified_identity(&client, config, &tokens, &row.get::<String, _>("nonce"))
    else {
        return failure(state, StatusCode::UNAUTHORIZED, "oidc_token_invalid", id);
    };
    create_session(state, id, row.get("previous_session"), user, identity).await
}

async fn create_session(
    state: &AppState,
    id: &RequestId,
    previous: Option<Vec<u8>>,
    user: auth::User,
    identity: FederatedIdentity,
) -> Response {
    let Ok(token) = auth::random_token().await else {
        return failure(
            state,
            StatusCode::SERVICE_UNAVAILABLE,
            "oidc_unavailable",
            id,
        );
    };
    let Ok(mut event) = login_event(id, Some(&user.id)).await else {
        return failure(
            state,
            StatusCode::SERVICE_UNAVAILABLE,
            "oidc_unavailable",
            id,
        );
    };
    // A non-bearer reference supports correlation without exposing a session token.
    event.session_id = Some(format!(
        "session:{}",
        hexadecimal(&Sha256::digest(Sha256::digest(token.as_bytes())))
    ));
    let Ok(identity) = serde_json::to_string(&identity) else {
        return failure(state, StatusCode::UNAUTHORIZED, "oidc_token_invalid", id);
    };
    let result = async {
        let mut tx = state.db.pool().begin().await?;
        if let Some(previous) = previous {
            sqlx::query("UPDATE bi_meta.sessions SET revoked_at = clock_timestamp() WHERE token_hash = $1")
                .bind(previous).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO bi_meta.sessions (token_hash, user_id, display_name, data_scope, expires_at, federated_identity) VALUES ($1, $2, $3, 'synthetic', clock_timestamp() + make_interval(secs => $4), $5::jsonb)")
            .bind(Sha256::digest(token.as_bytes()).to_vec()).bind(user.id).bind(user.display_name)
            .bind(f64::from(state.auth.ttl_seconds)).bind(identity).execute(&mut *tx).await?;
        crate::audit::append(&mut tx, &event).await.map_err(|_| sqlx::Error::Protocol("audit_store_unavailable".to_owned()))?;
        tx.commit().await
    }.await;
    if result.is_err() {
        return failure(
            state,
            StatusCode::SERVICE_UNAVAILABLE,
            "session_store_unavailable",
            id,
        );
    }
    tracing::info!(event_code = "oidc_login_succeeded", "OIDC session created");
    state.metrics.oidc_login("success");
    (
        [(
            header::SET_COOKIE,
            state.auth.cookie(&token, state.auth.ttl_seconds),
        )],
        Redirect::to("/"),
    )
        .into_response()
}

fn hexadecimal(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(value, "{byte:02x}").expect("writing to String cannot fail");
    }
    value
}

async fn login_event(id: &RequestId, user: Option<&str>) -> Result<bi_audit::Event, ()> {
    let event_id = auth::random_token().await?;
    let raw_id = id.header_value().to_str().unwrap_or_default();
    let request_id = if !raw_id.is_empty()
        && raw_id.len() <= 128
        && raw_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
    {
        raw_id.to_owned()
    } else {
        format!(
            "request:{}",
            hexadecimal(&Sha256::digest(raw_id.as_bytes()))
        )
    };
    // Correlation reference until distributed tracing supplies a real trace id.
    let trace_id = hexadecimal(&Sha256::digest(request_id.as_bytes()))[..32].to_owned();
    Ok(bi_audit::Event {
        event_id,
        occurred_at: SystemTime::now().into(),
        actor_id: user.unwrap_or("anonymous").to_owned(),
        actor_type: if user.is_some() {
            bi_audit::ActorType::User
        } else {
            bi_audit::ActorType::Anonymous
        },
        session_id: None,
        action: bi_audit::Action::Login,
        resource_type: "session".to_owned(),
        resource_id: "own".to_owned(),
        result: if user.is_some() {
            bi_audit::ResultCode::Success
        } else {
            bi_audit::ResultCode::Denied
        },
        request_id,
        trace_id,
        policy_decision_id: None,
        source: bi_audit::Source::Http,
        metadata_redacted: bi_audit::MetadataRedacted::default(),
        schema_version: bi_audit::SCHEMA_VERSION,
    })
}

pub(crate) fn routes() -> axum::Router<AppState> {
    routes_with_limit(Arc::new(Mutex::new(Admission::new(Instant::now()))))
}

fn routes_with_limit(limit: AdmissionLimit) -> axum::Router<AppState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/v1/oidc/config", get(available))
        .route("/api/v1/oidc/start", post(start))
        .route("/api/v1/oidc/callback", get(callback))
        .layer(Extension(limit))
}

#[cfg(test)]
mod tests;
