use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use bi_server::auth::AuthConfig;
use bi_server::db::{self, Database};
use bi_server::problem::Problem;
use bi_server::{AppState, router};
use http_body_util::BodyExt;
use tower::ServiceExt;

const USERS: &str =
    r#"[{"id":"analyst","display_name":"Analista"},{"id":"viewer","display_name":"Lector"}]"#;

fn config(origin: &str) -> AuthConfig {
    AuthConfig::from_lookup(&|key| match key {
        "BI_PUBLIC_ORIGIN" => Some(origin.to_owned()),
        "BI_DEV_USERS" => Some(USERS.to_owned()),
        _ => None,
    })
    .unwrap()
}

fn offline(config: AuthConfig) -> Router {
    router(AppState {
        auth: config,
        ..AppState::new(Database::start(
            db::lazy_pool("postgres://bi:bi@127.0.0.1:1/bi").unwrap(),
        ))
    })
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    origin: Option<&str>,
    body: &str,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

async fn assert_problem(response: axum::response::Response, status: StatusCode, code: &str) {
    assert_eq!(response.status(), status);
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    let id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_ne!(id, "");
    let body: Problem =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body.status, status.as_u16());
    assert_eq!(body.code, code);
    assert_eq!(body.request_id, id);
    if code == "origin_forbidden" {
        assert_eq!(body.public_origin.as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(
            body.detail.as_deref(),
            Some("Abre la aplicación desde http://127.0.0.1:8080.")
        );
    } else {
        assert!(body.public_origin.is_none());
    }
}

#[tokio::test]
async fn entire_api_namespace_requires_a_session_before_routing() {
    let app = offline(AuthConfig::default());
    for path in [
        "/api/v1",
        "/api/v1/session",
        "/api/v1/queries",
        "/api/v1/future/data",
    ] {
        assert_problem(
            send(&app, "GET", path, None, None, "").await,
            StatusCode::UNAUTHORIZED,
            "session_required",
        )
        .await;
    }
    for cookie in [
        "bi_session=bad".to_owned(),
        format!(
            "bi_session={}; bi_session={}",
            "a".repeat(64),
            "b".repeat(64)
        ),
    ] {
        assert_problem(
            send(&app, "GET", "/api/v1/session", Some(&cookie), None, "").await,
            StatusCode::UNAUTHORIZED,
            "session_required",
        )
        .await;
    }
}

#[tokio::test]
async fn dev_routes_disabled_without_configured_users() {
    let app = offline(AuthConfig::default());
    for (method, path) in [("GET", "/api/v1/dev/users"), ("POST", "/api/v1/dev/login")] {
        assert_problem(
            send(
                &app,
                method,
                path,
                None,
                Some("http://127.0.0.1:8080"),
                "{}",
            )
            .await,
            StatusCode::NOT_FOUND,
            "not_found",
        )
        .await;
    }
}

#[cfg(not(feature = "dev-auth"))]
#[tokio::test]
async fn production_build_cannot_enable_login_through_environment() {
    let app = offline(config("https://example.test"));
    for (method, path) in [("GET", "/api/v1/dev/users"), ("POST", "/api/v1/dev/login")] {
        assert_problem(
            send(
                &app,
                method,
                path,
                None,
                Some("https://example.test"),
                r#"{"user_id":"analyst"}"#,
            )
            .await,
            StatusCode::NOT_FOUND,
            "not_found",
        )
        .await;
    }
}

#[test]
fn auth_configuration_fails_closed_and_redacts_invalid_values() {
    for origin in [
        "http://remote.test",
        "https://example.test/",
        "https://user:private@example.test",
        "https://example.test/path",
        "null",
    ] {
        let error =
            AuthConfig::from_lookup(&|key| (key == "BI_PUBLIC_ORIGIN").then(|| origin.to_owned()))
                .unwrap_err();
        assert!(!error.to_string().contains(origin));
    }
    for ttl in ["0", "59", "86401", "invalid"] {
        assert!(
            AuthConfig::from_lookup(
                &|key| (key == "BI_SESSION_TTL_SECONDS").then(|| ttl.to_owned())
            )
            .is_err()
        );
    }
}

#[cfg(feature = "dev-auth")]
#[tokio::test]
async fn login_validates_origin_and_payload_before_database_access() {
    let app = offline(config("http://127.0.0.1:8080"));
    for origin in [
        None,
        Some("null"),
        Some("http://evil.test"),
        Some("http://localhost:8080"),
    ] {
        assert_problem(
            send(
                &app,
                "POST",
                "/api/v1/dev/login",
                None,
                origin,
                r#"{"user_id":"analyst"}"#,
            )
            .await,
            StatusCode::FORBIDDEN,
            "origin_forbidden",
        )
        .await;
    }
    for body in ["broken", r#"{"user_id":"analyst","role":"admin"}"#] {
        assert_problem(
            send(
                &app,
                "POST",
                "/api/v1/dev/login",
                None,
                Some("http://127.0.0.1:8080"),
                body,
            )
            .await,
            StatusCode::BAD_REQUEST,
            "invalid_request",
        )
        .await;
    }
    assert_problem(
        send(
            &app,
            "POST",
            "/api/v1/dev/login",
            None,
            Some("http://127.0.0.1:8080"),
            r#"{"user_id":"unknown"}"#,
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "unknown_dev_user",
    )
    .await;
    for headers in [
        vec![
            ("origin", "http://127.0.0.1:8080"),
            ("origin", "http://127.0.0.1:8080"),
        ],
        vec![
            ("origin", "http://127.0.0.1:8080"),
            ("sec-fetch-site", "cross-site"),
        ],
    ] {
        let mut request = Request::post("/api/v1/dev/login");
        for (key, value) in headers {
            request = request.header(key, value);
        }
        assert_problem(
            app.clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap(),
            StatusCode::FORBIDDEN,
            "origin_forbidden",
        )
        .await;
    }
}

#[cfg(feature = "dev-auth")]
#[tokio::test]
async fn origin_hint_ignores_host_proxy_and_referrer_and_never_sets_a_cookie() {
    let app = offline(config("http://127.0.0.1:8080"));
    for origin in ["http://localhost:8080", "https://attacker.test"] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/v1/dev/login")
                    .header("origin", origin)
                    .header("host", "attacker.test")
                    .header("x-forwarded-host", "attacker.test")
                    .header("x-forwarded-proto", "https")
                    .header("referer", "https://attacker.test/")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"user_id":"analyst"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(!response.headers().contains_key("set-cookie"));
        assert_problem(response, StatusCode::FORBIDDEN, "origin_forbidden").await;
    }
}

#[cfg(all(feature = "postgres-tests", feature = "dev-auth"))]
mod postgres {
    use super::{assert_problem, config, send};
    use axum::Router;
    use axum::http::StatusCode;
    use bi_server::auth::Session;
    use bi_server::db::Database;
    use bi_server::{AppState, router};
    use http_body_util::BodyExt;
    use sha2::{Digest, Sha256};
    use sqlx::PgPool;

    async fn app(pool: PgPool, origin: &str) -> Router {
        let db = Database::start(pool);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(30), db.wait_until_migrated())
                .await
                .unwrap()
        );
        router(AppState {
            auth: config(origin),
            ..AppState::new(db)
        })
    }

    async fn login(app: &Router, origin: &str, previous: Option<&str>, user: &str) -> String {
        let response = send(
            app,
            "POST",
            "/api/v1/dev/login",
            previous,
            Some(origin),
            &format!(r#"{{"user_id":"{user}"}}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(cookie.contains("HttpOnly; SameSite=Strict; Max-Age=3600"));
        assert!(cookie.contains("Path=/"));
        assert_eq!(cookie.contains("; Secure"), origin.starts_with("https:"));
        let session: Session =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(session.user.id, user);
        assert_eq!(session.expires_at.len(), 27);
        cookie.split(';').next().unwrap().to_owned()
    }

    #[sqlx::test(migrations = false)]
    async fn persistent_session_rotates_revokes_and_expires(pool: PgPool) {
        let origin = "http://127.0.0.1:8080";
        let first_app = app(pool.clone(), origin).await;
        let first = login(&first_app, origin, None, "analyst").await;
        let restarted = app(pool.clone(), origin).await;
        assert_eq!(
            send(&restarted, "GET", "/api/v1/session", Some(&first), None, "")
                .await
                .status(),
            StatusCode::OK
        );
        let second = login(&restarted, origin, Some(&first), "viewer").await;
        assert_ne!(first, second);
        assert_problem(
            send(&first_app, "GET", "/api/v1/session", Some(&first), None, "").await,
            StatusCode::UNAUTHORIZED,
            "session_required",
        )
        .await;
        for origin in [None, Some("http://evil.test")] {
            assert_problem(
                send(
                    &restarted,
                    "POST",
                    "/api/v1/logout",
                    Some(&second),
                    origin,
                    "",
                )
                .await,
                StatusCode::FORBIDDEN,
                "origin_forbidden",
            )
            .await;
        }
        let response = send(
            &restarted,
            "POST",
            "/api/v1/logout",
            Some(&second),
            Some(origin),
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response.headers()["set-cookie"],
            "bi_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"
        );
        assert_problem(
            send(
                &first_app,
                "GET",
                "/api/v1/session",
                Some(&second),
                None,
                "",
            )
            .await,
            StatusCode::UNAUTHORIZED,
            "session_required",
        )
        .await;
        let third = login(&restarted, origin, None, "analyst").await;
        let hash = Sha256::digest(third.strip_prefix("bi_session=").unwrap().as_bytes());
        sqlx::query("UPDATE bi_meta.sessions SET expires_at = clock_timestamp() - interval '1 second' WHERE token_hash = $1").bind(hash.as_slice()).execute(&pool).await.unwrap();
        assert_problem(
            send(&first_app, "GET", "/api/v1/session", Some(&third), None, "").await,
            StatusCode::UNAUTHORIZED,
            "session_required",
        )
        .await;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM bi_meta.sessions WHERE octet_length(token_hash) = 32 AND data_scope = 'synthetic'").fetch_one(&pool).await.unwrap();
        assert_eq!(count, 3);
    }

    #[sqlx::test(migrations = false)]
    async fn https_cookies_and_session_store_failure(pool: PgPool) {
        let origin = "https://example.test";
        let app = app(pool.clone(), origin).await;
        let cookie = login(&app, origin, None, "analyst").await;
        let response = send(
            &app,
            "POST",
            "/api/v1/logout",
            Some(&cookie),
            Some(origin),
            "",
        )
        .await;
        assert!(
            response.headers()["set-cookie"]
                .to_str()
                .unwrap()
                .contains("; Secure")
        );
        pool.close().await;
        assert_problem(
            send(&app, "GET", "/api/v1/session", Some(&cookie), None, "").await,
            StatusCode::SERVICE_UNAVAILABLE,
            "session_store_unavailable",
        )
        .await;
    }
}
