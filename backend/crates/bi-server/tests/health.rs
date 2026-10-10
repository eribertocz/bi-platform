//! Health endpoints without a database: liveness must not depend on
//! PostgreSQL and readiness must report it (M15 §22).

mod common;

use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use bi_server::db::{self, Database};
use bi_server::health::{Health, HealthCheck, HealthStatus};
use bi_server::{AppState, router};
use common::{get, get_health};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

/// An app whose pool points at a port where nothing listens.
fn app_without_database() -> Router {
    let pool = db::lazy_pool("postgres://bi:bi@127.0.0.1:1/bi").unwrap();
    router(AppState::new(Database::start(pool)))
}

fn check(name: &str, status: HealthStatus) -> HealthCheck {
    HealthCheck {
        name: name.to_owned(),
        status,
    }
}

#[tokio::test]
async fn live_is_ok_while_database_is_down() {
    let (status, health) = get_health(&app_without_database(), "/health/live").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        health,
        Health {
            status: HealthStatus::Ok,
            checks: vec![],
        }
    );
}

#[tokio::test]
async fn ready_reports_unavailable_database_quickly() {
    let started = Instant::now();
    let (status, health) = get_health(&app_without_database(), "/health/ready").await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        health,
        Health {
            status: HealthStatus::Unavailable,
            checks: vec![
                check("postgres", HealthStatus::Unavailable),
                check("migrations", HealthStatus::Unavailable),
            ],
        }
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "readiness is bounded by its ping timeout, took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn generates_request_id() {
    let response = get(&app_without_database(), "/health/live").await;
    let id = response.headers()["x-request-id"].to_str().unwrap();
    assert_eq!(id.len(), 36, "UUID request id, got {id}");
}

#[tokio::test]
async fn propagates_client_request_id() {
    let response = app_without_database()
        .oneshot(
            Request::get("/health/live")
                .header("x-request-id", "req-test-001")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["x-request-id"], "req-test-001");
}

#[tokio::test]
async fn unknown_route_is_not_found() {
    let response = get(&app_without_database(), "/health/unknown").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn serves_over_tcp_and_shuts_down_gracefully() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(bi_server::serve(
        listener,
        app_without_database(),
        async move {
            stopped.await.ok();
        },
    ));

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /health/live HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(
        response.ends_with(r#"{"status":"ok","checks":[]}"#),
        "{response}"
    );

    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server stops after the shutdown signal")
        .unwrap()
        .unwrap();
}
