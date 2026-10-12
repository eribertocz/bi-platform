//! M15 query metrics. `OTel` names omit Prometheus' unit/type suffixes:
//! `bi_query_execute` with unit `s` becomes `bi_query_execute_seconds`,
//! `bi_query_rejected` (counter, unit `{query}`) becomes `bi_query_rejected_total`.
//! No endpoint means no SDK, reader thread, callbacks or per-query allocation.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter, MeterProvider, ObservableGauge};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    error::OTelSdkResult,
    metrics::{
        PeriodicReader, SdkMeterProvider, Temporality, data::ResourceMetrics,
        exporter::PushMetricExporter,
    },
};

use crate::analytics::Analytics;

const WORKLOAD: &str = "interactive";

/// All attribute values originate here, never from the request or engine text.
#[derive(Clone, Copy)]
pub(crate) enum Outcome {
    Ok,
    Error,
    Busy,
    Timeout,
    Unavailable,
    Cancelled,
}

impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Busy => "busy",
            Self::Timeout => "timeout",
            Self::Unavailable => "unavailable",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Rejected {
    InvalidQuery,
    Busy,
    Timeout,
    ResultTooLarge,
    Unavailable,
}

impl Rejected {
    fn label(self) -> &'static str {
        match self {
            Self::InvalidQuery => "invalid_query",
            Self::Busy => "busy",
            Self::Timeout => "timeout",
            Self::ResultTooLarge => "result_too_large",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Default)]
pub struct QueryMetrics(Option<Arc<Instruments>>);

struct Instruments {
    compile: Histogram<f64>,
    wait: Histogram<f64>,
    execute: Histogram<f64>,
    serialize: Histogram<f64>,
    rejected: Counter<u64>,
    oidc_logins: Counter<u64>,
    _pool: Vec<ObservableGauge<u64>>,
}

impl QueryMetrics {
    /// Uses an explicit meter so tests and embedders need no global provider.
    #[must_use]
    pub fn new(meter: &Meter, analytics: &Analytics) -> Self {
        let histogram = |name: &'static str, description: &'static str| {
            meter
                .f64_histogram(name)
                .with_unit("s")
                .with_description(description)
                .with_boundaries(vec![
                    0.0001, 0.0005, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
                    10.0, 30.0, 60.0,
                ])
                .build()
        };
        let mut pool = Vec::new();
        for (name, description, field) in [
            (
                "bi_query_inflight",
                "Jobs executing, including cancellation recovery",
                0,
            ),
            (
                "bi_query_queued",
                "Admitted jobs waiting for a worker, including cancelled slots",
                1,
            ),
            ("bi_query_queue_capacity", "Maximum waiting jobs", 2),
            ("bi_query_capacity", "Live worker slots", 3),
        ] {
            let analytics = analytics.clone();
            pool.push(
                meter
                    .u64_observable_gauge(name)
                    // UCUM annotations count objects; unit "1" would make
                    // Prometheus append _ratio to these occupancy gauges.
                    .with_unit("{query}")
                    .with_description(description)
                    .with_callback(move |observer| {
                        let value = analytics.runtime().map_or(0, |runtime| {
                            let load = runtime.load();
                            match field {
                                0 => load.running,
                                1 => load.queued,
                                2 => load.queue_capacity,
                                _ => load.workers,
                            }
                        });
                        observer.observe(
                            u64::try_from(value).unwrap_or(u64::MAX),
                            &[KeyValue::new("workload", WORKLOAD)],
                        );
                    })
                    .build(),
            );
        }
        Self(Some(Arc::new(Instruments {
            compile: histogram(
                "bi_semantic_compile_duration",
                "Query validation and planning",
            ),
            wait: histogram(
                "bi_query_wait",
                "Admission and worker hand-off; no execution time",
            ),
            execute: histogram(
                "bi_query_execute",
                "Native execution and fetch until completion or request deadline",
            ),
            serialize: histogram("bi_result_serialize", "Exact JSON result serialization"),
            rejected: meter
                .u64_counter("bi_query_rejected")
                .with_unit("{query}")
                .with_description("Rejected query requests")
                .build(),
            oidc_logins: meter
                .u64_counter("bi_oidc_login")
                .with_unit("{login}")
                .with_description("OIDC login outcomes; no identity or token labels")
                .build(),
            _pool: pool,
        })))
    }

    pub(crate) fn oidc_login(&self, result: &'static str) {
        if let Some(metrics) = &self.0 {
            metrics
                .oidc_logins
                .add(1, &[KeyValue::new("result", result)]);
        }
    }

    pub(crate) fn compile(&self, duration: Duration, outcome: Outcome) {
        if let Some(metrics) = &self.0 {
            metrics.compile.record(
                duration.as_secs_f64(),
                &[
                    KeyValue::new("mode", "catalog"),
                    KeyValue::new("result", outcome.label()),
                ],
            );
        }
    }

    pub(crate) fn serialize(&self, duration: Duration) {
        if let Some(metrics) = &self.0 {
            metrics
                .serialize
                .record(duration.as_secs_f64(), &[KeyValue::new("format", "json")]);
        }
    }

    pub(crate) fn reject(&self, reason: Rejected) {
        if let Some(metrics) = &self.0 {
            metrics.rejected.add(
                1,
                &[
                    KeyValue::new("reason", reason.label()),
                    KeyValue::new("workload", WORKLOAD),
                ],
            );
        }
    }

    pub(crate) fn execution(&self, submitted: Instant) -> Option<ExecutionTimer> {
        self.0.as_ref().map(|metrics| ExecutionTimer {
            metrics: Arc::clone(metrics),
            submitted,
            native: Arc::new(Mutex::new(NativeTiming::default())),
            finished: false,
        })
    }
}

/// Shared start instant only when metrics are enabled. The request records
/// phases once, including a deadline while queued or executing. Pool gauges
/// separately report native work still running after that deadline.
pub(crate) struct ExecutionTimer {
    metrics: Arc<Instruments>,
    submitted: Instant,
    native: Arc<Mutex<NativeTiming>>,
    finished: bool,
}

#[derive(Default)]
struct NativeTiming {
    started: Option<Instant>,
    ended: Option<Instant>,
}

pub(crate) struct ExecutionMarker(Arc<Mutex<NativeTiming>>);

impl ExecutionMarker {
    pub(crate) fn start(&self, now: Instant) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .started = Some(now);
    }

    pub(crate) fn end(&self, now: Instant) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).ended = Some(now);
    }
}

impl ExecutionTimer {
    pub(crate) fn start_marker(&self) -> ExecutionMarker {
        ExecutionMarker(Arc::clone(&self.native))
    }

    pub(crate) fn finish(mut self, outcome: Outcome) {
        self.record(outcome);
        self.finished = true;
    }

    fn record(&self, outcome: Outcome) {
        let ended = Instant::now();
        let native = self.native.lock().unwrap_or_else(PoisonError::into_inner);
        let started = native.started.map(|started| started.min(ended));
        let wait = started
            .unwrap_or(ended)
            .saturating_duration_since(self.submitted);
        self.metrics.wait.record(
            wait.as_secs_f64(),
            &[
                KeyValue::new("workload", WORKLOAD),
                KeyValue::new(
                    "result",
                    if started.is_some() {
                        "ok"
                    } else {
                        outcome.label()
                    },
                ),
            ],
        );
        if let Some(started) = started {
            self.metrics.execute.record(
                native
                    .ended
                    .unwrap_or(ended)
                    .min(ended)
                    .saturating_duration_since(started)
                    .as_secs_f64(),
                &[
                    KeyValue::new("engine", "duckdb"),
                    KeyValue::new("workload", WORKLOAD),
                    KeyValue::new("result", outcome.label()),
                ],
            );
        }
    }
}

impl Drop for ExecutionTimer {
    fn drop(&mut self) {
        if !self.finished {
            self.record(Outcome::Cancelled);
        }
    }
}

/// The SDK's internal error logs are disabled: exporter errors may include
/// URLs/headers. Report only a fixed event on the background export thread.
struct SafeExporter(opentelemetry_otlp::MetricExporter);

impl PushMetricExporter for SafeExporter {
    async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
        let result = self.0.export(metrics).await;
        if result.is_err() {
            tracing::warn!(
                event_code = "metrics_export_failed",
                signal = "metrics",
                "optional metrics export failed"
            );
        }
        result
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.0.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.0.shutdown_with_timeout(timeout)
    }

    fn temporality(&self) -> Temporality {
        self.0.temporality()
    }
}

/// Builds OTLP/HTTP protobuf metrics with background collection/export.
/// Call on a blocking thread; shutdown the returned provider there too.
/// The application must install its rustls crypto provider before calling this.
/// The endpoint is an OTLP base URL, e.g. `http://localhost:4318`.
///
/// # Errors
/// Invalid exporter configuration; callers should fall back to no-op metrics.
pub fn otlp(
    endpoint: &str,
    analytics: &Analytics,
) -> Result<(QueryMetrics, SdkMeterProvider), opentelemetry_otlp::ExporterBuildError> {
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
        .with_endpoint(format!("{}/v1/metrics", endpoint.trim_end_matches('/')))
        .with_timeout(Duration::from_secs(2))
        .build()?;
    let provider = SdkMeterProvider::builder()
        .with_resource(
            Resource::builder_empty()
                .with_service_name("bi-server")
                .build(),
        )
        .with_reader(PeriodicReader::builder(SafeExporter(exporter)).build())
        .build();
    let metrics = QueryMetrics::new(&provider.meter("bi-server"), analytics);
    Ok((metrics, provider))
}

/// No environment configuration means no SDK construction. Exporter setup
/// failure is optional telemetry loss and never prevents serving queries.
pub async fn from_env(analytics: Analytics) -> (QueryMetrics, Option<SdkMeterProvider>) {
    let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") else {
        return (QueryMetrics::default(), None);
    };
    if let Ok(Ok((metrics, provider))) =
        tokio::task::spawn_blocking(move || otlp(&endpoint, &analytics)).await
    {
        (metrics, Some(provider))
    } else {
        // Do not log exporter error text: it may contain endpoint credentials.
        tracing::warn!(
            event_code = "metrics_exporter_setup_failed",
            "optional query metrics disabled"
        );
        (QueryMetrics::default(), None)
    }
}
