//! M15 contract against the SDK and a real local OTLP/HTTP receiver.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    body::{Body, Bytes},
    http::{Request, StatusCode},
    routing::post,
};
use bi_runtime::{Bootstrap, Runtime, RuntimeConfig};
use bi_server::{
    AppState,
    analytics::Analytics,
    db::{self, Database},
    metrics::{self, QueryMetrics},
    query,
};
use http_body_util::BodyExt;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use prost::Message;
use tower::ServiceExt;

const QUERY: &str = r#"{"dataset":"sales","measures":["row_count"]}"#;

fn install_tls_provider() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .unwrap();
    });
}

fn app(analytics: Analytics, metrics: QueryMetrics, timeout: Duration) -> Router {
    let db = Database::start(db::lazy_pool("postgres://bi:bi@127.0.0.1:1/bi").unwrap());
    Router::new()
        .route("/api/v1/queries", post(query::run))
        .with_state(AppState {
            analytics,
            metrics,
            query_timeout: timeout,
            ..AppState::new(db)
        })
}

async fn request(app: &Router, query: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/queries")
                .header("content-type", "application/json")
                .header("x-request-id", "private-request-id")
                .body(Body::from(query.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

async fn runtime() -> Runtime {
    let runtime = tokio::task::spawn_blocking(|| {
        Bootstrap::open(RuntimeConfig {
            queue_capacity: 1,
            ..RuntimeConfig::in_memory(1)
        })
        .unwrap()
        .finish()
        .unwrap()
    })
    .await
    .unwrap();
    runtime.run(|conn| conn.execute_batch("ATTACH ':memory:' AS lake;
        CREATE TABLE lake.main.sales (order_id BIGINT, ordered_at TIMESTAMPTZ,
        region VARCHAR, quantity BIGINT, amount DECIMAL(18,2));
        INSERT INTO lake.main.sales VALUES (9007199254740993, '2026-01-01T00:00:00Z', 'private-region', NULL, 9999999999999999.99)")).await.unwrap();
    runtime
}

struct Capture {
    provider: SdkMeterProvider,
    exporter: InMemoryMetricExporter,
    app: Router,
}

impl Capture {
    fn new(analytics: Analytics, timeout: Duration) -> Self {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let metrics = QueryMetrics::new(&provider.meter("test"), &analytics);
        Self {
            provider,
            exporter,
            app: app(analytics, metrics, timeout),
        }
    }

    fn points(&self) -> Vec<Point> {
        self.provider.force_flush().unwrap();
        points(&self.exporter.get_finished_metrics().unwrap())
    }
}

#[derive(Debug)]
struct Point {
    name: String,
    unit: String,
    attributes: BTreeMap<String, String>,
    count: u64,
    value: f64,
}

fn attributes<'a>(
    values: impl Iterator<Item = &'a opentelemetry::KeyValue>,
) -> BTreeMap<String, String> {
    values
        .map(|kv| (kv.key.to_string(), kv.value.to_string()))
        .collect()
}

fn expected_unit(name: &str) -> &'static str {
    if name == "bi_oidc_login" {
        "{login}"
    } else if [
        "bi_semantic_compile_duration",
        "bi_query_wait",
        "bi_query_execute",
        "bi_result_serialize",
    ]
    .contains(&name)
    {
        "s"
    } else {
        "{query}"
    }
}

#[allow(clippy::cast_precision_loss)] // Test counts/gauges are <= 4.
fn points(exports: &[ResourceMetrics]) -> Vec<Point> {
    let mut points = Vec::new();
    for metric in exports
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
    {
        let point = |attributes, count, value| Point {
            name: metric.name().to_owned(),
            unit: metric.unit().to_owned(),
            attributes,
            count,
            value,
        };
        match metric.data() {
            AggregatedMetrics::F64(MetricData::Histogram(histogram)) => {
                for p in histogram.data_points() {
                    assert!(p.sum().is_finite() && p.sum() >= 0.0);
                    assert_eq!(p.bucket_counts().sum::<u64>(), p.count());
                    assert!(p.bounds().any(|b| b < 0.001));
                    points.push(point(attributes(p.attributes()), p.count(), p.sum()));
                }
            }
            AggregatedMetrics::U64(MetricData::Sum(sum)) => {
                assert!(sum.is_monotonic());
                for p in sum.data_points() {
                    points.push(point(
                        attributes(p.attributes()),
                        p.value(),
                        p.value() as f64,
                    ));
                }
            }
            AggregatedMetrics::U64(MetricData::Gauge(gauge)) => {
                for p in gauge.data_points() {
                    points.push(point(attributes(p.attributes()), 1, p.value() as f64));
                }
            }
            data => panic!("unexpected metric data: {data:?}"),
        }
    }
    for p in &points {
        let expected_keys = match p.name.as_str() {
            "bi_semantic_compile_duration" => vec!["mode", "result"],
            "bi_query_wait" => vec!["result", "workload"],
            "bi_query_execute" => vec!["engine", "result", "workload"],
            "bi_result_serialize" => vec!["format"],
            "bi_query_rejected" => vec!["reason", "workload"],
            "bi_oidc_login" => vec!["code"],
            "bi_query_inflight"
            | "bi_query_queued"
            | "bi_query_capacity"
            | "bi_query_queue_capacity" => vec!["workload"],
            name => panic!("unexpected instrument: {name}"),
        };
        assert_eq!(
            p.attributes.keys().map(String::as_str).collect::<Vec<_>>(),
            expected_keys
        );
        assert_eq!(p.unit, expected_unit(&p.name));
        for (key, value) in &p.attributes {
            let allowed: &[&str] = match key.as_str() {
                "mode" => &["catalog"],
                "workload" => &["interactive"],
                "engine" => &["duckdb"],
                "format" => &["json"],
                "code" if p.name == "bi_oidc_login" => &["oidc_disabled", "rate_limited"],
                "result" => &["ok", "error", "busy", "timeout", "unavailable", "cancelled"],
                "reason" => &[
                    "invalid_query",
                    "busy",
                    "timeout",
                    "result_too_large",
                    "unavailable",
                ],
                _ => panic!("unsafe metric label: {key}"),
            };
            assert!(allowed.contains(&value.as_str()), "{p:?}");
        }
    }
    points
}

fn count(points: &[Point], name: &str, label: Option<(&str, &str)>) -> u64 {
    points
        .iter()
        .filter(|p| {
            p.name == name
                && label.is_none_or(|(k, v)| p.attributes.get(k).is_some_and(|actual| actual == v))
        })
        .map(|p| p.count)
        .sum()
}

#[tokio::test]
async fn oidc_denial_exports_a_closed_outcome_without_private_labels() {
    let capture = Capture::new(Analytics::disabled(), Duration::from_secs(5));
    let db = Database::start(db::lazy_pool("postgres://bi:bi@127.0.0.1:1/bi").unwrap());
    let app = bi_server::router(AppState {
        metrics: QueryMetrics::new(&capture.provider.meter("test"), &Analytics::disabled()),
        ..AppState::new(db)
    });
    let response = app
        .oneshot(
            Request::post("/api/v1/oidc/start?token=private-token")
                .header("origin", "http://127.0.0.1:8080")
                .header("x-request-id", "private-request-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()["x-request-id"], "private-request-id");
    let points = capture.points();
    assert_eq!(
        count(&points, "bi_oidc_login", Some(("code", "oidc_disabled"))),
        1
    );
    assert!(!format!("{points:?}").contains("private-"));
}

#[tokio::test(flavor = "multi_thread")]
async fn success_records_each_phase_and_exact_result_without_private_labels() {
    let capture = Capture::new(Analytics::ready(runtime().await), Duration::from_secs(5));
    let (status, body) = request(
        &capture.app,
        r#"{"dataset":"sales","measures":["row_count","amount_sum","quantity_sum"]}"#,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        body["rows"],
        serde_json::json!([["1", "9999999999999999.99", null]])
    );
    let points = capture.points();
    for name in [
        "bi_semantic_compile_duration",
        "bi_query_wait",
        "bi_query_execute",
        "bi_result_serialize",
    ] {
        assert_eq!(count(&points, name, None), 1, "{points:?}");
    }
    assert_eq!(count(&points, "bi_query_rejected", None), 0);
    for (name, value) in [
        ("bi_query_inflight", 0.0),
        ("bi_query_queued", 0.0),
        ("bi_query_capacity", 1.0),
        ("bi_query_queue_capacity", 1.0),
    ] {
        assert_eq!(points.iter().find(|p| p.name == name).unwrap().value, value);
    }
}

#[tokio::test]
async fn validation_and_unavailability_record_only_phases_reached() {
    for (query, status, reason, result) in [
        (
            r#"{"dataset":"sales","measures":["private-invalid-measure"]}"#,
            422,
            "invalid_query",
            "error",
        ),
        (QUERY, 503, "unavailable", "ok"),
    ] {
        let capture = Capture::new(Analytics::disabled(), Duration::from_secs(5));
        assert_eq!(request(&capture.app, query).await.0, status);
        let points = capture.points();
        assert_eq!(
            count(
                &points,
                "bi_semantic_compile_duration",
                Some(("result", result))
            ),
            1
        );
        assert_eq!(
            count(&points, "bi_query_rejected", Some(("reason", reason))),
            1
        );
        for name in ["bi_query_wait", "bi_query_execute", "bi_result_serialize"] {
            assert_eq!(count(&points, name, None), 0);
        }
    }
}

/// Holds native work after interruption too; release by dropping/sending.
async fn block(runtime: &Runtime) -> (std::sync::mpsc::Sender<()>, tokio::task::JoinHandle<()>) {
    let (release, wait) = std::sync::mpsc::channel();
    let (started, ready) = tokio::sync::oneshot::channel();
    let runtime = runtime.clone();
    let task = tokio::spawn(async move {
        runtime
            .run(move |_| {
                started.send(()).ok();
                wait.recv().ok();
                Ok(())
            })
            .await
            .ok();
    });
    ready.await.unwrap();
    (release, task)
}

async fn queued(runtime: &Runtime) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while runtime.load().queued != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn full_queue_records_busy_and_observes_native_pool_occupancy() {
    let runtime = runtime().await;
    let capture = Capture::new(Analytics::ready(runtime.clone()), Duration::from_secs(5));
    let (release, blocker) = block(&runtime).await;
    let waiting = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.run(|_| Ok(())).await }
    });
    queued(&runtime).await;
    assert_eq!(request(&capture.app, QUERY).await.0, 429);
    let points = capture.points();
    assert_eq!(
        count(&points, "bi_query_rejected", Some(("reason", "busy"))),
        1
    );
    assert_eq!(count(&points, "bi_query_wait", Some(("result", "busy"))), 1);
    assert_eq!(count(&points, "bi_query_execute", None), 0);
    for name in ["bi_query_queued", "bi_query_inflight"] {
        assert_eq!(points.iter().find(|p| p.name == name).unwrap().value, 1.0);
    }
    drop(release);
    blocker.await.unwrap();
    waiting.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_timeout_records_wait_without_inventing_execution() {
    let runtime = runtime().await;
    let capture = Capture::new(Analytics::ready(runtime.clone()), Duration::from_millis(30));
    let (release, blocker) = block(&runtime).await;
    assert_eq!(request(&capture.app, QUERY).await.0, 504);
    let points = capture.points();
    assert_eq!(
        count(&points, "bi_query_wait", Some(("result", "timeout"))),
        1
    );
    assert_eq!(count(&points, "bi_query_execute", None), 0);
    assert_eq!(
        count(&points, "bi_query_rejected", Some(("reason", "timeout"))),
        1
    );
    assert_eq!(runtime.load().queued, 1, "cancelled slot awaits dequeuing");
    drop(release);
    blocker.await.unwrap();
    runtime.run(|_| Ok(())).await.unwrap();
    assert_eq!(runtime.load().running, 0);
    assert_eq!(runtime.load().queued, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn running_timeout_records_execution_and_worker_recovers() {
    let runtime = runtime().await;
    runtime.run(|conn| conn.execute_batch("DROP TABLE lake.main.sales;
        CREATE VIEW lake.main.sales AS SELECT range::BIGINT AS order_id, NULL::TIMESTAMPTZ AS ordered_at,
        NULL::VARCHAR AS region, NULL::BIGINT AS quantity, (range % 100)::DECIMAL(18,2) AS amount FROM range(1000000000)")).await.unwrap();
    let capture = Capture::new(
        Analytics::ready(runtime.clone()),
        Duration::from_millis(100),
    );
    assert_eq!(
        request(
            &capture.app,
            r#"{"dataset":"sales","measures":["amount_sum"]}"#
        )
        .await
        .0,
        504
    );
    let points = capture.points();
    assert_eq!(count(&points, "bi_query_wait", Some(("result", "ok"))), 1);
    assert_eq!(
        count(&points, "bi_query_execute", Some(("result", "timeout"))),
        1
    );
    assert_eq!(
        count(&points, "bi_query_rejected", Some(("reason", "timeout"))),
        1
    );
    assert_eq!(count(&points, "bi_result_serialize", None), 0);
    tokio::time::timeout(Duration::from_secs(5), runtime.run(|_| Ok(())))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(runtime.load().running, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_results_and_engine_errors_keep_phase_and_rejection_semantics() {
    for oversized in [true, false] {
        let runtime = runtime().await;
        runtime
            .run(move |conn| {
                conn.execute_batch(if oversized {
                    "UPDATE lake.main.sales SET region = repeat('x', 100000)"
                } else {
                    "DROP TABLE lake.main.sales"
                })
            })
            .await
            .unwrap();
        let capture = Capture::new(Analytics::ready(runtime), Duration::from_secs(5));
        assert_eq!(
            request(
                &capture.app,
                r#"{"dataset":"sales","dimensions":["region"],"measures":["row_count"]}"#
            )
            .await
            .0,
            if oversized { 422 } else { 500 }
        );
        let points = capture.points();
        assert_eq!(
            count(
                &points,
                "bi_query_execute",
                Some(("result", if oversized { "ok" } else { "error" }))
            ),
            1
        );
        assert_eq!(
            count(
                &points,
                "bi_query_rejected",
                Some(("reason", "result_too_large"))
            ),
            u64::from(oversized)
        );
        assert_eq!(count(&points, "bi_result_serialize", None), 0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn real_otlp_http_export_decodes_metrics_at_a_local_receiver() {
    use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
    install_tls_provider();
    let (deliver, mut requests) = tokio::sync::mpsc::channel(4);
    let receiver = Router::new().route(
        "/v1/metrics",
        post(move |headers: axum::http::HeaderMap, body: Bytes| {
            let deliver = deliver.clone();
            async move {
                assert_eq!(headers["content-type"], "application/x-protobuf");
                deliver
                    .send(ExportMetricsServiceRequest::decode(body).unwrap())
                    .await
                    .unwrap();
                (
                    [("content-type", "application/x-protobuf")],
                    Vec::<u8>::new(),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(bi_server::serve(listener, receiver, async {
        stopped.await.ok();
    }));
    let analytics = Analytics::ready(runtime().await);
    let (metrics, provider) = tokio::task::spawn_blocking({
        let analytics = analytics.clone();
        move || metrics::otlp(&endpoint, &analytics).unwrap()
    })
    .await
    .unwrap();
    let app = app(analytics, metrics, Duration::from_secs(5));
    assert_eq!(request(&app, QUERY).await.0, 200);
    assert_eq!(
        request(
            &app,
            r#"{"dataset":"sales","measures":["private-invalid"]}"#
        )
        .await
        .0,
        422
    );
    let provider = tokio::task::spawn_blocking(move || {
        provider.force_flush().unwrap();
        provider
    })
    .await
    .unwrap();
    let export = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap();
    let metrics: Vec<_> = export
        .resource_metrics
        .iter()
        .flat_map(|r| &r.scope_metrics)
        .flat_map(|s| &s.metrics)
        .collect();
    for name in [
        "bi_semantic_compile_duration",
        "bi_query_wait",
        "bi_query_execute",
        "bi_result_serialize",
        "bi_query_inflight",
        "bi_query_queued",
        "bi_query_capacity",
        "bi_query_queue_capacity",
        "bi_query_rejected",
    ] {
        assert!(
            metrics.iter().any(|m| m.name == name),
            "missing {name}: {export:?}"
        );
    }
    for metric in &metrics {
        assert_eq!(metric.unit, expected_unit(&metric.name));
    }
    let resource = export.resource_metrics[0].resource.as_ref().unwrap();
    assert_eq!(resource.attributes.len(), 1);
    assert_eq!(resource.attributes[0].key, "service.name");
    assert!(!format!("{export:?}").contains("private-"));
    tokio::task::spawn_blocking(move || provider.shutdown().unwrap())
        .await
        .unwrap();
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_export_is_isolated_from_queries() {
    install_tls_provider();
    let exporting = Arc::new(tokio::sync::Notify::new());
    let receiver = Router::new().route(
        "/v1/metrics",
        post({
            let exporting = Arc::clone(&exporting);
            move || {
                let exporting = Arc::clone(&exporting);
                async move {
                    exporting.notify_one();
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    StatusCode::BAD_REQUEST
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(bi_server::serve(listener, receiver, async {
        stopped.await.ok();
    }));
    let analytics = Analytics::ready(runtime().await);
    let (metrics, provider) = tokio::task::spawn_blocking({
        let analytics = analytics.clone();
        move || metrics::otlp(&endpoint, &analytics).unwrap()
    })
    .await
    .unwrap();
    let app = app(analytics, metrics, Duration::from_secs(5));
    assert_eq!(request(&app, QUERY).await.0, 200);
    let provider = Arc::new(provider);
    let failed = tokio::task::spawn_blocking({
        let provider = Arc::clone(&provider);
        move || provider.force_flush()
    });
    tokio::time::timeout(Duration::from_secs(5), exporting.notified())
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(100), request(&app, QUERY))
            .await
            .expect("queries do not wait for the exporter")
            .0,
        200
    );
    assert!(failed.await.unwrap().is_err());
    assert_eq!(request(&app, QUERY).await.0, 200);
    tokio::task::spawn_blocking(move || {
        provider.shutdown().ok();
    })
    .await
    .unwrap();
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_exporter_configuration_can_fall_back_to_noop() {
    install_tls_provider();
    assert!(
        tokio::task::spawn_blocking(|| metrics::otlp("http://[invalid", &Analytics::disabled()))
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(
        request(
            &app(
                Analytics::ready(runtime().await),
                QueryMetrics::default(),
                Duration::from_secs(5)
            ),
            QUERY
        )
        .await
        .0,
        200
    );
}
