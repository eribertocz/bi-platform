//! `GET /api/v1/models` through the real router (#124): `view` on
//! `Model::"project:catalog"`, decided with the project's policies and
//! members before anything of `metrics/` is read, and every decision
//! recorded before the answer leaves. Real sessions and audit chain on
//! PostgreSQL, real DuckDB (DuckLake for the full listing). Run with
//! `--features dev-auth,postgres-tests`. The listing once allowed is tested
//! in `src/models/tests.rs`.
#![cfg(all(feature = "postgres-tests", feature = "dev-auth"))]

use std::path::Path;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use bi_server::analytics::Analytics;
use bi_server::auth::AuthConfig;
use bi_server::db::Database;
use bi_server::metadata::MetadataMode;
use bi_server::models::Models;
use bi_server::{AppState, router};
use http_body_util::BodyExt;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:8080";
const READY: Duration = Duration::from_secs(60);
const VALID: &str = include_str!("../../../../examples/models-project/metrics/ventas.yaml");
const INVALID: &str =
    include_str!("../../../../examples/models-project/metrics/origen-inexistente.yaml");

/// Metadata that only a granted listing may carry.
const SENTINEL_FIELD: &str = "      - name: campo_reservado
        description: descripcion_reservada
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: quantity
";
const RESTRICTED_METRIC: &str = r#"  - name: coste
    description: descripcion_reservada
    custom_extensions:
      - vendor_name: bi-platform
        data: '{"version": 1, "sensitivity": "restricted"}'
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(sales.campo_reservado)
"#;
/// Anything of the catalog: none of it may reach a refused session.
const CATALOG: [&str; 11] = [
    "campo_reservado",
    "descripcion_reservada",
    "Ventas",
    "ventas.yaml",
    "invalido.yaml",
    "Origen inexistente",
    "importe_total",
    "coste",
    "unknown_source",
    "sef-sha256",
    "metrics/",
];
const FORBIDDEN: &str = "No tienes permiso para revisar las definiciones y los errores de este \
                         proyecto. Solicita acceso a quien lo administra.";
const POLICIES_INVALID: &str = "Las políticas o los miembros del proyecto no son válidos: nadie \
                                puede revisar los modelos hasta corregirlos.";

const MEMBERS: &str = "version: 1
users:
  catalog_author: [modeler]
  administrator: [admin]
  reader: [viewer]
  analyst: [analyst]
  inspector: [catalog_reader]
groups:
  catalog_authors: [modeler]
";
const USERS: [&str; 7] = [
    "catalog_author",
    "administrator",
    "reader",
    "analyst",
    "inspector",
    "unmapped",
    "admin",
];
/// Sessions the base policies refuse: other roles, no membership, and an
/// identifier that only looks privileged.
const REFUSED: [&str; 4] = ["reader", "analyst", "unmapped", "admin"];

fn model() -> String {
    let with_field = VALID.replacen("metrics:\n", &format!("{SENTINEL_FIELD}metrics:\n"), 1);
    assert_ne!(with_field, VALID);
    with_field + RESTRICTED_METRIC
}

/// A project with a valid model carrying sentinels, an invalid one, and
/// the members above under the base policies.
fn project() -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    std::fs::create_dir(p.path().join("metrics")).unwrap();
    std::fs::create_dir(p.path().join("policies")).unwrap();
    std::fs::write(p.path().join("metrics/ventas.yaml"), model()).unwrap();
    std::fs::write(p.path().join("metrics/invalido.yaml"), INVALID).unwrap();
    std::fs::write(p.path().join("policies/members.yaml"), MEMBERS).unwrap();
    p
}

/// The base policies plus `extra`, which then replace the defaults.
fn policies(project: &Path, extra: &str) {
    std::fs::write(
        project.join("policies/project.cedar"),
        format!("{}\n{extra}", sef_authz::BASE_POLICIES),
    )
    .unwrap();
}

async fn analytics() -> Analytics {
    let runtime = tokio::task::spawn_blocking(|| {
        bi_runtime::Bootstrap::open(bi_runtime::RuntimeConfig::in_memory(1))
            .unwrap()
            .finish()
            .unwrap()
    })
    .await
    .unwrap();
    runtime
        .run(|conn| {
            conn.execute_batch(
                "ATTACH ':memory:' AS lake; CREATE TABLE lake.main.sales (order_id BIGINT NOT \
                 NULL, ordered_at TIMESTAMPTZ, region VARCHAR, quantity INTEGER, amount \
                 DECIMAL(18,2));",
            )
        })
        .await
        .unwrap();
    Analytics::ready(runtime)
}

/// One PostgreSQL, sessions and meter shared by every router of a test.
struct Stack {
    db: Database,
    pool: PgPool,
    auth: AuthConfig,
    provider: SdkMeterProvider,
    exporter: InMemoryMetricExporter,
    timeout: Duration,
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    text: String,
    body: Value,
}

impl Stack {
    async fn start(pool: PgPool, timeout: Duration) -> Self {
        let db = Database::start(&pool.connect_options(), MetadataMode::Development);
        assert!(
            tokio::time::timeout(READY, db.wait_until_ready())
                .await
                .unwrap()
        );
        let users: Vec<Value> = USERS
            .iter()
            .map(|id| json!({ "id": id, "display_name": id }))
            .collect();
        let users = Value::from(users).to_string();
        let auth = AuthConfig::from_lookup(&|key| match key {
            "BI_PUBLIC_ORIGIN" => Some(ORIGIN.to_owned()),
            "BI_DEV_USERS" => Some(users.clone()),
            _ => None,
        })
        .unwrap();
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        Self {
            db,
            pool,
            auth,
            provider,
            exporter,
            timeout,
        }
    }

    fn app(&self, project: Option<&Path>, analytics: Analytics) -> Router {
        router(AppState {
            db: self.db.clone(),
            auth: self.auth.clone(),
            analytics,
            query_timeout: self.timeout,
            models: Models::new(project.map(Path::to_owned))
                .with_meter(&self.provider.meter("test")),
            ..AppState::new(self.db.clone())
        })
    }

    async fn login(app: &Router, user: &str) -> String {
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/v1/dev/login")
                    .header("origin", ORIGIN)
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "user_id": user }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{user}");
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }

    /// Sum of the counts (histograms) or values (counters) of `name` whose
    /// attributes include every pair of `labels`.
    fn counted(&self, name: &str, labels: &[(&str, &str)]) -> u64 {
        self.provider.force_flush().unwrap();
        let exports: Vec<ResourceMetrics> = self.exporter.get_finished_metrics().unwrap();
        let matches = |attributes: &mut dyn Iterator<Item = &opentelemetry::KeyValue>| {
            let present: Vec<(String, String)> = attributes
                .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                .collect();
            labels
                .iter()
                .all(|(k, v)| present.iter().any(|(pk, pv)| pk == k && pv == v))
        };
        exports
            .last()
            .into_iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
            .filter(|metric| metric.name() == name)
            .map(|metric| match metric.data() {
                AggregatedMetrics::F64(MetricData::Histogram(histogram)) => histogram
                    .data_points()
                    .filter(|p| matches(&mut p.attributes()))
                    .map(opentelemetry_sdk::metrics::data::HistogramDataPoint::count)
                    .sum(),
                AggregatedMetrics::U64(MetricData::Sum(sum)) => sum
                    .data_points()
                    .filter(|p| matches(&mut p.attributes()))
                    .map(opentelemetry_sdk::metrics::data::SumDataPoint::value)
                    .sum(),
                _ => 0,
            })
            .sum()
    }

    /// Nothing of `metrics/` was read and nothing was compiled.
    fn assert_no_catalog_work(&self) {
        for step in ["read", "compile"] {
            assert_eq!(
                self.counted("bi_models_catalog_work", &[("step", step)]),
                0,
                "{step}"
            );
        }
    }

    /// The catalog's audit events, oldest first.
    async fn events(&self) -> Vec<Value> {
        sqlx::query_scalar::<_, String>(
            "SELECT payload FROM bi_meta.audit_events
             WHERE (payload::jsonb)->>'resource_type' = 'project_catalog' ORDER BY sequence",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap()
        .iter()
        .map(|payload| serde_json::from_str(payload).unwrap())
        .collect()
    }
}

async fn list(app: &Router, cookie: Option<&str>) -> Answer {
    send(app, "/api/v1/models", cookie, &[]).await
}

async fn send(
    app: &Router,
    uri: &str,
    cookie: Option<&str>,
    headers: &[(&'static str, &str)],
) -> Answer {
    let mut request = Request::get(uri).header("x-request-id", "req-models-test");
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    Answer {
        status,
        headers,
        body: serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    }
}

fn assert_nothing_of_the_catalog(answer: &Answer) {
    for sentinel in CATALOG {
        assert!(
            !answer.text.contains(sentinel),
            "{sentinel}: {}",
            answer.text
        );
    }
}

fn assert_forbidden(answer: &Answer) {
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert_eq!(answer.headers["content-type"], "application/problem+json");
    assert_eq!(answer.headers["cache-control"], "no-store");
    assert_eq!(answer.body["code"], "models_forbidden");
    assert_eq!(answer.body["detail"], FORBIDDEN);
    assert_eq!(answer.body["request_id"], "req-models-test");
    assert_nothing_of_the_catalog(answer);
}

fn assert_full_listing(answer: &Answer) {
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(answer.headers["cache-control"], "no-store");
    for sentinel in ["campo_reservado", "descripcion_reservada", "coste"] {
        assert!(
            answer.text.contains(sentinel),
            "{sentinel}: {}",
            answer.text
        );
    }
    let files: Vec<&Value> = answer.body["models"].as_array().unwrap().iter().collect();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["file"], "metrics/invalido.yaml");
    assert_eq!(files[0]["status"], "invalid");
    assert_eq!(files[1]["file"], "metrics/ventas.yaml");
    assert_eq!(files[1]["status"], "valid");
}

/// One event of a decision: who, what was decided, and the fingerprint
/// only when a decision was taken.
fn assert_event(event: &Value, actor: &str, action: &str, result: &str, decided: bool) {
    assert_eq!(event["actor_id"], actor);
    assert_eq!(event["action"], action);
    assert_eq!(event["result"], result);
    assert_eq!(event["resource_type"], "project_catalog");
    assert_eq!(event["resource_id"], "project:catalog");
    assert_eq!(event["source"], "http");
    assert_eq!(event["request_id"], "req-models-test");
    assert_eq!(event["metadata_redacted"]["query_shape"], Value::Null);
    let fingerprint = event["policy_decision_id"].as_str();
    assert_eq!(
        fingerprint.is_some_and(|f| !f.is_empty()),
        decided,
        "{event}"
    );
    for sentinel in CATALOG {
        assert!(!event.to_string().contains(sentinel), "{sentinel}");
    }
}

#[sqlx::test(migrations = false)]
async fn catalog_author_inspects_the_whole_project(pool: PgPool) {
    let data = tempfile::tempdir().unwrap();
    let lake = bi_runtime::RuntimeConfig {
        lake: Some(bi_runtime::LakeConfig {
            catalog: std::env::var("BI_TEST_LAKE_CATALOG")
                .unwrap_or_else(|_| bi_runtime::config::DEV_LAKE_CATALOG.to_owned()),
            metadata_schema: Some(format!("bi_test_models_{}", std::process::id())),
            data_path: data.path().to_owned(),
        }),
        ..bi_runtime::RuntimeConfig::in_memory(1)
    };
    let analytics = Analytics::start(
        lake,
        Some(bi_fixtures::sales::FixtureSpec {
            seed: 42,
            rows: 1_000,
        }),
    );
    assert!(
        tokio::time::timeout(READY, analytics.wait_until_ready())
            .await
            .unwrap()
    );
    let p = project();
    let stack = Stack::start(pool, Duration::from_secs(10)).await;
    let app = stack.app(Some(p.path()), analytics);
    for user in ["catalog_author", "administrator"] {
        let answer = list(&app, Some(&Stack::login(&app, user).await)).await;
        assert_full_listing(&answer);
        // The oracle is the fixture's exact metadata, invalid file included.
        let invalid = &answer.body["models"][0];
        assert_eq!(invalid["name"], "Origen inexistente");
        assert_eq!(invalid["diagnostics"][0]["code"], "unknown_source");
        let valid = &answer.body["models"][1];
        assert_eq!(valid["name"], "Ventas");
        assert!(
            valid["fingerprint"]
                .as_str()
                .unwrap()
                .starts_with("sef-sha256-v1:")
        );
        let metrics: Vec<(&str, &str)> = valid["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                (
                    m["name"].as_str().unwrap(),
                    m["data_type"].as_str().unwrap(),
                )
            })
            .collect();
        // Deterministic order; exact types of the DuckLake fixture.
        assert_eq!(
            metrics,
            vec![("coste", "INT64"), ("importe_total", "DECIMAL(18,2)")]
        );
        let dimension = |name: &str| {
            valid["dimensions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["name"] == name)
                .unwrap()
                .clone()
        };
        assert_eq!(dimension("sales.region")["data_type"], "VARCHAR");
        assert_eq!(dimension("sales.region")["nullable"], true);
        assert_eq!(
            dimension("sales.ordered_at")["data_type"],
            "TIMESTAMPTZ (instante)"
        );
        let field = valid["datasets"][0]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == "campo_reservado")
            .unwrap();
        assert_eq!(field["annotation"]["description"], "descripcion_reservada");
    }
    let events = stack.events().await;
    assert_eq!(events.len(), 2);
    assert_event(&events[0], "catalog_author", "query", "success", true);
    assert_event(&events[1], "administrator", "query", "success", true);
    assert_eq!(
        stack.counted("bi_models_catalog_work", &[("step", "read")]),
        2
    );
    assert_eq!(
        stack.counted("bi_models_catalog_work", &[("step", "compile")]),
        2
    );
    assert_eq!(stack.counted("bi_models_requests", &[("outcome", "ok")]), 2);
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn roles_without_inspection_are_refused_without_catalog_work(pool: PgPool) {
    let p = project();
    let stack = Stack::start(pool, Duration::from_secs(10)).await;
    let app = stack.app(Some(p.path()), analytics().await);
    for user in REFUSED {
        assert_forbidden(&list(&app, Some(&Stack::login(&app, user).await)).await);
    }
    let events = stack.events().await;
    assert_eq!(events.len(), REFUSED.len());
    for (event, user) in events.iter().zip(REFUSED) {
        assert_event(event, user, "denial", "denied", true);
    }
    stack.assert_no_catalog_work();
    for name in ["bi_models_requests", "bi_models_load"] {
        assert_eq!(
            stack.counted(name, &[("outcome", "forbidden")]),
            4,
            "{name}"
        );
        assert_eq!(stack.counted(name, &[("outcome", "ok")]), 0, "{name}");
    }
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn an_explicit_catalog_grant_inspects_but_does_not_query(pool: PgPool) {
    let p = project();
    policies(
        p.path(),
        r#"@id("catalog_reader.inspect")
permit (
  principal in Role::"catalog_reader",
  action == Action::"view",
  resource == Model::"project:catalog"
);"#,
    );
    let stack = Stack::start(pool, Duration::from_secs(10)).await;
    let app = stack.app(Some(p.path()), analytics().await);
    let inspector = Stack::login(&app, "inspector").await;
    assert_full_listing(&list(&app, Some(&inspector)).await);
    // Inspecting the catalog grants nothing on its members' data.
    let explain = app
        .clone()
        .oneshot(
            Request::post("/api/v1/semantic/explain")
                .header("origin", ORIGIN)
                .header("cookie", &inspector)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model": "metrics/ventas.yaml",
                        "query": { "selections": [
                            { "kind": "metric", "id": "x", "name": "coste" }
                        ] }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(explain.status(), StatusCode::FORBIDDEN);
    let body: Value =
        serde_json::from_slice(&explain.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["code"], "semantic_access_denied", "{body:#}");
    assert_eq!(body["denied"]["name"], "coste");
    // Still refused to the roles the grant does not name.
    assert_forbidden(&list(&app, Some(&Stack::login(&app, "reader").await)).await);
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn member_or_masked_grants_do_not_open_the_catalog(pool: PgPool) {
    let p = project();
    let stack = Stack::start(pool, Duration::from_secs(10)).await;
    let app = stack.app(Some(p.path()), analytics().await);
    let inspector = Stack::login(&app, "inspector").await;
    for grant in [
        "permit (principal in Role::\"catalog_reader\", action == Action::\"view\", resource is Metric);",
        "permit (principal in Role::\"catalog_reader\", action == Action::\"view\", resource is Dataset);",
        "permit (principal in Role::\"catalog_reader\", action == Action::\"view\", resource is Dimension);",
        "permit (principal in Role::\"catalog_reader\", action == Action::\"view_masked\", resource);",
        // Another model, named like a file or like the catalog's own scope.
        "permit (principal in Role::\"catalog_reader\", action == Action::\"view\", resource == Model::\"ventas\");",
        "permit (principal in Role::\"catalog_reader\", action == Action::\"view\", resource == Model::\"project:catalog:metrics/ventas.yaml\");",
    ] {
        policies(p.path(), grant);
        assert_forbidden(&list(&app, Some(&inspector)).await);
    }
    stack.assert_no_catalog_work();
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_forbid_beats_admin(pool: PgPool) {
    let p = project();
    policies(
        p.path(),
        r#"@id("catalog.closed")
forbid (principal, action == Action::"view", resource == Model::"project:catalog");"#,
    );
    let stack = Stack::start(pool, Duration::from_secs(10)).await;
    let app = stack.app(Some(p.path()), analytics().await);
    for user in ["administrator", "catalog_author"] {
        assert_forbidden(&list(&app, Some(&Stack::login(&app, user).await)).await);
    }
    for event in stack.events().await {
        assert_eq!(event["action"], "denial");
        assert_eq!(event["result"], "denied");
    }
    stack.assert_no_catalog_work();
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn an_evaluation_error_denies(pool: PgPool) {
    let p = project();
    // Cedar alone would skip the failing forbid and allow.
    std::fs::write(
        p.path().join("policies/project.cedar"),
        r#"@id("all")
permit (principal, action, resource);

@id("overflow")
forbid (principal, action, resource) when { 9223372036854775807 + 1 > 0 };"#,
    )
    .unwrap();
    let stack = Stack::start(pool, Duration::from_secs(10)).await;
    let app = stack.app(Some(p.path()), analytics().await);
    assert_forbidden(&list(&app, Some(&Stack::login(&app, "administrator").await)).await);
    let events = stack.events().await;
    assert_eq!(events.len(), 1);
    assert_event(&events[0], "administrator", "denial", "denied", true);
    stack.assert_no_catalog_work();
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn generated_requests_never_widen_the_grant(pool: PgPool) {
    use proptest::prelude::*;
    use proptest::strategy::ValueTree as _;
    use proptest::test_runner::TestRunner;

    let p = project();
    let stack = Stack::start(pool, Duration::from_secs(10)).await;
    let app = stack.app(Some(p.path()), analytics().await);
    let mut cookies = Vec::new();
    for user in REFUSED {
        cookies.push(Stack::login(&app, user).await);
    }
    let header = prop_oneof![
        Just("x-roles"),
        Just("x-bi-roles"),
        Just("x-user"),
        Just("x-forwarded-user"),
        Just("x-forwarded-groups"),
        Just("x-auth-request-groups"),
        Just("x-remote-user"),
        Just("x-bi-project"),
    ];
    let value = prop_oneof![
        Just("admin".to_owned()),
        Just("modeler".to_owned()),
        Just("catalog_author".to_owned()),
        Just("catalog_authors".to_owned()),
        Just("Role::\"admin\"".to_owned()),
        "[a-z_:,]{1,24}",
    ];
    let parameter = prop_oneof![
        Just("role=admin".to_owned()),
        Just("roles=modeler".to_owned()),
        Just("user=catalog_author".to_owned()),
        Just("groups=catalog_authors".to_owned()),
        Just("resource=project:catalog".to_owned()),
        Just("project=other".to_owned()),
        "[a-z_]{1,10}=[a-z_:]{0,16}",
    ];
    let case = (0..cookies.len(), header, value, parameter);
    let mut runner = TestRunner::deterministic();
    let mut sent = 0;
    for _ in 0..32 {
        let (who, header, value, parameter) = case.new_tree(&mut runner).unwrap().current();
        let answer = send(
            &app,
            &format!("/api/v1/models?{parameter}"),
            Some(&cookies[who]),
            &[(header, &value)],
        )
        .await;
        assert_forbidden(&answer);
        sent += 1;
    }
    let events = stack.events().await;
    assert_eq!(events.len(), sent);
    assert!(events.iter().all(|e| e["action"] == "denial"));
    stack.assert_no_catalog_work();
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn revocation_and_restoration_apply_to_the_next_request(pool: PgPool) {
    let p = project();
    let stack = Stack::start(pool, Duration::from_secs(10)).await;
    let app = stack.app(Some(p.path()), analytics().await);
    let author = Stack::login(&app, "catalog_author").await;
    assert_full_listing(&list(&app, Some(&author)).await);
    let members = p.path().join("policies/members.yaml");
    std::fs::write(
        &members,
        MEMBERS.replace("  catalog_author: [modeler]\n", ""),
    )
    .unwrap();
    assert_forbidden(&list(&app, Some(&author)).await);
    std::fs::write(&members, MEMBERS).unwrap();
    assert_full_listing(&list(&app, Some(&author)).await);
    // The same with a change of policy.
    policies(
        p.path(),
        r#"forbid (principal, action == Action::"view", resource == Model::"project:catalog");"#,
    );
    assert_forbidden(&list(&app, Some(&author)).await);
    std::fs::remove_file(p.path().join("policies/project.cedar")).unwrap();
    assert_full_listing(&list(&app, Some(&author)).await);
    let events: Vec<(String, String)> = stack
        .events()
        .await
        .iter()
        .map(|e| {
            assert_eq!(e["actor_id"], "catalog_author");
            (
                e["action"].as_str().unwrap().to_owned(),
                e["result"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let event = |action: &str, result: &str| (action.to_owned(), result.to_owned());
    assert_eq!(
        events,
        vec![
            event("query", "success"),
            event("denial", "denied"),
            event("query", "success"),
            event("denial", "denied"),
            event("query", "success"),
        ]
    );
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn invalid_or_unreadable_access_closes_without_details(pool: PgPool) {
    let p = project();
    let stack = Stack::start(pool, Duration::from_secs(10)).await;
    let app = stack.app(Some(p.path()), analytics().await);
    let author = Stack::login(&app, "catalog_author").await;
    let unmapped = Stack::login(&app, "unmapped").await;
    assert_full_listing(&list(&app, Some(&author)).await);
    let closed = |answer: &Answer| {
        assert_eq!(
            answer.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            answer.text
        );
        assert_eq!(answer.headers["cache-control"], "no-store");
        assert_eq!(answer.body["code"], "policies_invalid");
        assert_eq!(answer.body["detail"], POLICIES_INVALID);
        for detail in [
            "SECRETO",
            "policies/",
            "members.yaml",
            "cedar",
            "catalog_author",
        ] {
            assert!(!answer.text.contains(detail), "{detail}: {}", answer.text);
        }
        assert_nothing_of_the_catalog(answer);
    };
    let members = p.path().join("policies/members.yaml");
    let cedar = p.path().join("policies/project.cedar");
    let broken: [(&Path, &str); 3] = [
        (&members, "version: 2\nusers:\n  SECRETO_MIEMBRO: [admin]\n"),
        (&members, "version: 1\nusers: [SECRETO_MIEMBRO\n"),
        (
            &cedar,
            "permit (principal, action, resource) when { SECRETO_POLITICA };",
        ),
    ];
    let mut failures = 0;
    for (file, text) in broken {
        std::fs::write(file, text).unwrap();
        for cookie in [&author, &unmapped] {
            closed(&list(&app, Some(cookie)).await);
            failures += 1;
        }
        std::fs::write(&members, MEMBERS).unwrap();
        let _ = std::fs::remove_file(&cedar);
    }
    // Unreadable: `policies` is not a directory.
    let saved = p.path().join("policies-saved");
    std::fs::rename(p.path().join("policies"), &saved).unwrap();
    std::fs::write(p.path().join("policies"), "SECRETO_ARCHIVO").unwrap();
    closed(&list(&app, Some(&author)).await);
    failures += 1;
    std::fs::remove_file(p.path().join("policies")).unwrap();
    std::fs::rename(&saved, p.path().join("policies")).unwrap();
    // A valid configuration opens again; nothing earlier was kept.
    assert_full_listing(&list(&app, Some(&author)).await);
    let events = stack.events().await;
    assert_eq!(events.len(), failures + 2);
    for event in &events[1..=failures] {
        let actor = event["actor_id"].as_str().unwrap();
        assert_event(event, actor, "query", "failure", false);
    }
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn refusals_are_identical_whatever_the_catalog_state(pool: PgPool) {
    let stack = Stack::start(pool, Duration::from_secs(10)).await;
    let refusal = |answer: &Answer| {
        assert_forbidden(answer);
        let mut body = answer.body.clone();
        body.as_object_mut().unwrap().remove("request_id");
        (body, answer.headers["content-type"].clone())
    };
    let p = project();
    let reference = {
        let app = stack.app(Some(p.path()), analytics().await);
        refusal(&list(&app, Some(&Stack::login(&app, "reader").await)).await)
    };
    let broken = project();
    std::fs::write(broken.path().join("metrics/ventas.yaml"), "version: [\n").unwrap();
    let other = project();
    std::fs::write(
        other.path().join("metrics/ventas.yaml"),
        model().replace("campo_reservado", "otro_campo"),
    )
    .unwrap();
    let unreadable = project();
    std::fs::remove_dir_all(unreadable.path().join("metrics")).unwrap();
    std::fs::write(unreadable.path().join("metrics"), "not a directory").unwrap();
    let empty = project();
    for file in ["ventas.yaml", "invalido.yaml"] {
        std::fs::remove_file(empty.path().join("metrics").join(file)).unwrap();
    }
    let states: [(&Path, Analytics); 5] = [
        (p.path(), Analytics::disabled()),
        (broken.path(), analytics().await),
        (other.path(), analytics().await),
        (unreadable.path(), analytics().await),
        (empty.path(), analytics().await),
    ];
    for (project, analytics) in states {
        let app = stack.app(Some(project), analytics);
        for user in ["reader", "unmapped"] {
            let cookie = Stack::login(&app, user).await;
            assert_eq!(refusal(&list(&app, Some(&cookie)).await), reference);
        }
    }
    stack.assert_no_catalog_work();
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn session_and_metadata_checks_come_first(pool: PgPool) {
    let p = project();
    let stack = Stack::start(pool, Duration::from_secs(10)).await;
    let app = stack.app(Some(p.path()), analytics().await);
    let anonymous = list(&app, None).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert_eq!(anonymous.body["code"], "session_required");
    let author = Stack::login(&app, "catalog_author").await;
    let logout = app
        .clone()
        .oneshot(
            Request::post("/api/v1/logout")
                .header("cookie", &author)
                .header("origin", ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let revoked = list(&app, Some(&author)).await;
    assert_eq!(revoked.status, StatusCode::UNAUTHORIZED);
    assert_eq!(revoked.body["code"], "session_required");
    assert_nothing_of_the_catalog(&revoked);
    let author = Stack::login(&app, "catalog_author").await;
    assert_eq!(stack.events().await.len(), 0);
    stack.db.close().await;
    let unready = list(&app, Some(&author)).await;
    assert_eq!(unready.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(unready.body["code"], "metadata_not_ready");
    assert_nothing_of_the_catalog(&unready);
    stack.assert_no_catalog_work();
}

#[sqlx::test(migrations = false)]
async fn an_unconfigured_instance_answers_not_configured_without_a_decision(pool: PgPool) {
    let stack = Stack::start(pool, Duration::from_secs(10)).await;
    let app = stack.app(None, Analytics::disabled());
    for user in ["unmapped", "catalog_author"] {
        let answer = list(&app, Some(&Stack::login(&app, user).await)).await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
        assert_eq!(answer.headers["cache-control"], "no-store");
        assert_eq!(
            answer.body,
            json!({"project_status": "not_configured", "models": [], "diagnostics": []})
        );
    }
    assert_eq!(stack.events().await.len(), 0);
    stack.assert_no_catalog_work();
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_held_audit_head_ends_within_the_deadline(pool: PgPool) {
    let deadline = Duration::from_secs(1);
    let p = project();
    let stack = Stack::start(pool.clone(), deadline).await;
    let app = stack.app(Some(p.path()), analytics().await);
    let author = Stack::login(&app, "catalog_author").await;
    let reader = Stack::login(&app, "reader").await;
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE bi_meta.audit_head IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *holder)
        .await
        .unwrap();
    for cookie in [&author, &reader] {
        let started = Instant::now();
        let answer = list(&app, Some(cookie)).await;
        let elapsed = started.elapsed();
        assert_eq!(
            answer.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            answer.text
        );
        assert_eq!(answer.body["code"], "audit_unavailable");
        assert_nothing_of_the_catalog(&answer);
        assert!(
            elapsed < deadline + Duration::from_secs(2),
            "{elapsed:?} for a {deadline:?} deadline"
        );
    }
    // Nothing keeps waiting for the lock once the request has answered.
    let waiting_until = Instant::now() + Duration::from_secs(5);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
             WHERE datname = current_database() AND wait_event_type = 'Lock'
               AND pid <> pg_backend_pid()",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if waiting == 0 {
            break;
        }
        assert!(
            Instant::now() < waiting_until,
            "{waiting} backends still wait"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    holder.rollback().await.unwrap();
    assert_eq!(stack.events().await.len(), 0);
    assert_full_listing(&list(&app, Some(&author)).await);
    let events = stack.events().await;
    assert_eq!(events.len(), 1);
    assert_event(&events[0], "catalog_author", "query", "success", true);
    stack.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn authorized_failures_are_recorded_as_failures(pool: PgPool) {
    let p = project();
    let stack = Stack::start(pool, Duration::from_secs(10)).await;
    let down = stack.app(Some(p.path()), Analytics::disabled());
    let author = Stack::login(&down, "catalog_author").await;
    let answer = list(&down, Some(&author)).await;
    assert_eq!(answer.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(answer.body["code"], "analytics_unavailable");
    assert_nothing_of_the_catalog(&answer);
    // A global limit is still the 200 contract, with its diagnostic.
    for i in 0..=bi_server::models::MAX_FILES {
        std::fs::write(p.path().join(format!("metrics/m{i}.yaml")), VALID).unwrap();
    }
    let limited = list(&down, Some(&author)).await;
    assert_eq!(limited.status, StatusCode::OK);
    assert_eq!(limited.body["project_status"], "unavailable");
    assert_eq!(limited.body["diagnostics"][0]["code"], "too_many_files");
    let events = stack.events().await;
    assert_eq!(events.len(), 2);
    assert_event(&events[0], "catalog_author", "query", "failure", true);
    assert_event(&events[1], "catalog_author", "query", "success", true);
    assert_eq!(
        stack.counted("bi_models_requests", &[("outcome", "error")]),
        1
    );
    stack.db.close().await;
}
