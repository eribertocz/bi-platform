#![allow(dead_code)] // Each test binary uses a different subset.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use bi_server::health::Health;
use http_body_util::BodyExt;
use tower::ServiceExt;

pub async fn get(app: &Router, uri: &str) -> Response<Body> {
    app.clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// GETs a health endpoint and checks the headers every health response carries.
pub async fn get_health(app: &Router, uri: &str) -> (StatusCode, Health) {
    let response = get(app, uri).await;
    let status = response.status();
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}
