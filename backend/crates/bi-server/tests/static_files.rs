use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use bi_server::db::{self, Database};
use bi_server::{AppState, router_with_static};
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

fn fixture() -> (TempDir, Router) {
    let directory = tempfile::tempdir().unwrap();
    for (path, contents) in [
        ("200.html", "<!doctype html><title>SPA shell</title>"),
        ("index.html", "<!doctype html><title>Home</title>"),
        ("app.js", "console.log('asset');"),
        ("api/hidden.html", "<!doctype html>must never be served"),
        ("health/hidden.html", "<!doctype html>must never be served"),
    ] {
        let path = directory.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    let db = Database::start(db::lazy_pool("postgres://bi:bi@127.0.0.1:1/bi").unwrap());
    let app = router_with_static(AppState { db }, Some(directory.path().to_owned()));
    (directory, app)
}

#[tokio::test]
async fn serves_assets_and_spa_navigation_with_request_ids() {
    let (_directory, app) = fixture();
    for (path, expected, content_type) in [
        ("/", "Home", "text/html"),
        ("/app.js", "asset", "javascript"),
        ("/estado", "SPA shell", "text/html"),
        ("/explorar/nuevo?filter=abc", "SPA shell", "text/html"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(path)
                    .header("accept", "text/html,application/xhtml+xml;q=0.9")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .contains(content_type)
        );
        assert!(response.headers().contains_key("x-request-id"));
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains(expected)
        );
    }
    let response = app
        .oneshot(
            Request::head("/estado")
                .header("accept", "text/html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );
}

#[tokio::test]
async fn namespaces_assets_and_fetches_never_fall_back_to_html() {
    let (_directory, app) = fixture();
    for path in [
        "/api",
        "/api/unknown",
        "/api/hidden.html",
        "/health",
        "/health/unknown",
        "/health/hidden.html",
        "/missing.js",
        "/missing.css",
        "/_app/missing",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(path)
                    .header("accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(
            response
                .headers()
                .get("content-type")
                .is_none_or(|value| value != "text/html")
        );
        assert!(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty()
        );
    }
    for accept in [
        None,
        Some("application/json"),
        Some("*/*"),
        Some("text/html;q=0"),
    ] {
        let mut request = Request::get("/estado");
        if let Some(accept) = accept {
            request = request.header("accept", accept);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    for (method, mode, dest, expected) in [
        (
            "POST",
            "navigate",
            "document",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        ("GET", "cors", "empty", StatusCode::NOT_FOUND),
        ("GET", "no-cors", "script", StatusCode::NOT_FOUND),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/estado")
                    .header("accept", "text/html")
                    .header("sec-fetch-mode", mode)
                    .header("sec-fetch-dest", dest)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert!(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn health_stays_json_with_frontend_enabled() {
    let (_directory, app) = fixture();
    for (path, status) in [
        ("/health/live", StatusCode::OK),
        ("/health/ready", StatusCode::SERVICE_UNAVAILABLE),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(path)
                    .header("accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()["content-type"], "application/json");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_ok());
    }
}

#[tokio::test]
async fn missing_build_does_not_synthesize_a_shell() {
    let (directory, _app) = fixture();
    let db = Database::start(db::lazy_pool("postgres://bi:bi@127.0.0.1:1/bi").unwrap());
    let app = router_with_static(AppState { db }, Some(directory.path().join("missing")));
    let response = app
        .oneshot(
            Request::get("/estado")
                .header("accept", "text/html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
