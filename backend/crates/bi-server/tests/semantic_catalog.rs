//! `GET /api/v1/semantic/catalog` (#140) through the real router: sessions
//! and the audit chain on PostgreSQL (also under the restricted runtime role
//! of #115), a project on disk and DuckDB compiling it. Every answer is
//! checked against the catalog written out by hand and against the single
//! audit event it must leave, or not leave. The projection's own visibility
//! matrix lives in `src/semantic/catalog/tests.rs`; this file covers HTTP.
#![cfg(all(feature = "postgres-tests", feature = "dev-auth"))]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use axum::{
    Router,
    body::{Body, Bytes},
    http::{HeaderMap, Request, StatusCode},
};
use bi_runtime::{Runtime, RuntimeConfig};
use bi_server::db::{self, Database};
use bi_server::metadata::MetadataMode;
use bi_server::semantic::run::{Budgets, INITIAL, Profile, Runs};
use bi_server::{AppState, analytics::Analytics, models::Models};
use http_body_util::BodyExt;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use serde_json::{Value, json};
use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, PgPool};
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:8080";
const CATALOG: &str = "/api/v1/semantic/catalog";

/// The rows of the Issue: id, region, city, amount, instant, note.
const ROWS: &str = "(1, 'Norte', 'La Paz', 100.10, '2026-01-01T00:00:00Z', NULL),
    (2, 'Norte', 'El Alto', 0.20, '2026-01-02T00:00:00Z', 'n'),
    (3, 'Sur', 'Tarija', 200.20, '2026-01-01T00:00:00Z', 's'),
    (4, 'Sur', 'Sucre', -0.20, '2026-01-02T00:00:00Z', NULL)";

/// The Issue's model: `nota` restricted, a restricted field and a
/// restricted metric whose names are sentinels, `margen` confidential, and
/// free text that must never leave.
const MODEL: &str = r#"version: '0.2.0.dev0'
name: Ventas
datasets:
  - name: ventas
    source: lake.main.ventas
    primary_key: [id]
    fields:
      - name: id
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: id
      - name: region
        dimension: {}
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: region
      - name: ciudad
        dimension: {}
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: ciudad
      - name: importe
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: importe
      - name: instante
        datatype: DateTimeTz
        dimension: {}
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: instante
      - name: nota
        dimension: {}
        custom_extensions:
          - vendor_name: bi-platform
            data: '{"version": 1, "sensitivity": "restricted"}'
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: nota
      - name: secreto_centinela
        dimension: {}
        custom_extensions:
          - vendor_name: bi-platform
            data: '{"version": 1, "sensitivity": "restricted"}'
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: nota
metrics:
  - name: importe_total
    description: descripcion_centinela
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(ventas.importe)
  - name: filas
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: COUNT(ventas.id)
  - name: margen
    custom_extensions:
      - vendor_name: bi-platform
        data: '{"version": 1, "sensitivity": "confidential"}'
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(ventas.importe)
  - name: reserva_centinela
    custom_extensions:
      - vendor_name: bi-platform
        data: '{"version": 1, "sensitivity": "restricted"}'
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(ventas.id)
"#;

const MEMBERS: &str = "
version: 1
users:
  ana: [viewer]
  bea: [viewer]
  analista: [analyst]
";

/// Every dev identity of these tests; `nadie` has no membership.
const USERS: &str = r#"[{"id":"ana","display_name":"Ana"},{"id":"bea","display_name":"Bea"},{"id":"analista","display_name":"Analista"},{"id":"nadie","display_name":"Nadie"}]"#;

/// What must never leave in a catalog or a problem: hidden names, free
/// text, SQL, sources, roles, classifications and fingerprints.
const SENTINELS: [&str; 12] = [
    "centinela",
    "nota",
    "SUM(",
    "COUNT(",
    "lake.main",
    "viewer",
    "restricted",
    "confidential",
    "fingerprint",
    "sha256",
    "policies/",
    ".cedar",
];

const DECISION_PREFIX: &str = "catalog-decisions-sha256:";

const AUDIT_FAILS: &str =
    "ALTER TABLE bi_meta.audit_events ADD CONSTRAINT append_fails CHECK (false) NOT VALID";

fn project() -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    std::fs::create_dir(p.path().join("metrics")).unwrap();
    std::fs::create_dir(p.path().join("policies")).unwrap();
    std::fs::write(p.path().join("metrics/ventas.yaml"), MODEL).unwrap();
    std::fs::write(p.path().join("policies/members.yaml"), MEMBERS).unwrap();
    p
}

/// A DuckDB runtime of `config` with the Issue's rows behind a view that
/// fails as soon as anything reads a row: compiling describes it, a scan
/// does not. The catalog must never need a row.
async fn trapped(config: RuntimeConfig) -> Analytics {
    let runtime = tokio::task::spawn_blocking(move || {
        bi_runtime::Bootstrap::open(config)
            .unwrap()
            .finish()
            .unwrap()
    })
    .await
    .unwrap();
    runtime
        .run(|conn| {
            conn.execute_batch(&format!(
                "ATTACH ':memory:' AS lake; CREATE TABLE lake.main.filas (id BIGINT NOT NULL, \
                 region VARCHAR NOT NULL, ciudad VARCHAR NOT NULL, importe DECIMAL(18,2) NOT NULL, \
                 instante TIMESTAMPTZ NOT NULL, nota VARCHAR); INSERT INTO lake.main.filas VALUES {ROWS}; \
                 CREATE VIEW lake.main.ventas AS SELECT * FROM lake.main.filas \
                 WHERE error('fila leida') IS NULL;"
            ))
        })
        .await
        .unwrap();
    Analytics::ready(runtime)
}

async fn development(pool: &PgPool) -> Database {
    let db = Database::start(&pool.connect_options(), MetadataMode::Development);
    assert!(
        tokio::time::timeout(Duration::from_secs(30), db.wait_until_ready())
            .await
            .unwrap()
    );
    db
}

/// What a harness is built from.
struct Setup {
    db: Database,
    analytics: Analytics,
    timeout: Duration,
    /// `false`: this instance serves no project.
    configured: bool,
    runs: Option<Runs>,
}

struct Harness {
    app: Router,
    pool: PgPool,
    project: tempfile::TempDir,
    db: Database,
    analytics: Analytics,
}

/// One answer as the client reads it.
struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    bytes: Bytes,
    body: Value,
}

/// One audit event, as the chain stores it.
#[derive(Debug, Clone, PartialEq)]
struct Event {
    action: String,
    result: String,
    resource_type: String,
    resource_id: String,
    decision: Option<String>,
    metadata: Value,
    schema: String,
    actor: String,
    payload: String,
}

async fn harness(pool: PgPool) -> Harness {
    let db = development(&pool).await;
    let analytics = trapped(RuntimeConfig::in_memory(2)).await;
    build(
        pool,
        Setup {
            db,
            analytics,
            timeout: INITIAL.timeout,
            configured: true,
            runs: None,
        },
    )
}

fn build(pool: PgPool, setup: Setup) -> Harness {
    let project = project();
    let auth = bi_server::auth::AuthConfig::from_lookup(&|key| match key {
        "BI_PUBLIC_ORIGIN" => Some(ORIGIN.into()),
        "BI_DEV_USERS" => Some(USERS.into()),
        _ => None,
    })
    .unwrap();
    let runs = setup.runs.unwrap_or_else(|| {
        Runs::new(
            Profile::new(Budgets {
                timeout: setup.timeout,
                ..INITIAL
            })
            .unwrap(),
        )
    });
    let app = bi_server::router(AppState {
        auth,
        analytics: setup.analytics.clone(),
        models: Models::new(setup.configured.then(|| project.path().into())),
        semantic_runs: runs,
        ..AppState::new(setup.db.clone())
    });
    Harness {
        app,
        pool,
        project,
        db: setup.db,
        analytics: setup.analytics,
    }
}

impl Harness {
    async fn login(&self, user: &str) -> String {
        let response = self
            .app
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

    /// A GET of `uri` with no Origin header: a GET needs none.
    async fn get_with(&self, uri: &str, cookie: Option<&str>, headers: &[(&str, &str)]) -> Answer {
        let mut request = Request::get(uri);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        Answer {
            status,
            headers,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            bytes,
        }
    }

    async fn catalog(&self, cookie: &str) -> Answer {
        self.get_with(CATALOG, Some(cookie), &[]).await
    }

    fn write(&self, file: &str, text: &str) {
        std::fs::write(self.project.path().join(file), text).unwrap();
    }

    /// The last sequence of the audit chain.
    async fn head(&self) -> i64 {
        sqlx::query_scalar("SELECT COALESCE(max(sequence), 0) FROM bi_meta.audit_events")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// Every event after `since` that is not a login, oldest first.
    async fn events_since(&self, since: i64) -> Vec<Event> {
        type Row = (
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            String,
        );
        sqlx::query_as::<_, Row>(
            "SELECT payload::jsonb->>'action', payload::jsonb->>'result', \
             payload::jsonb->>'resource_type', payload::jsonb->>'resource_id', \
             payload::jsonb->>'policy_decision_id', \
             (payload::jsonb->'metadata_redacted')::text, \
             payload::jsonb->>'schema_version', payload::jsonb->>'actor_id', payload::text \
             FROM bi_meta.audit_events \
             WHERE sequence > $1 AND payload::jsonb->>'action' NOT IN ('login', 'logout') \
             ORDER BY sequence",
        )
        .bind(since)
        .fetch_all(&self.pool)
        .await
        .unwrap()
        .into_iter()
        .map(
            |(
                action,
                result,
                resource_type,
                resource_id,
                decision,
                metadata,
                schema,
                actor,
                payload,
            )| {
                Event {
                    action,
                    result,
                    resource_type,
                    resource_id,
                    decision,
                    metadata: serde_json::from_str(&metadata).unwrap(),
                    schema,
                    actor,
                    payload,
                }
            },
        )
        .collect()
    }

    /// The single event a request left after `since`.
    async fn only_event_since(&self, since: i64) -> Event {
        let events = self.events_since(since).await;
        assert_eq!(events.len(), 1, "{events:#?}");
        events.into_iter().next().unwrap()
    }
}

fn member(name: &str, value_type: &str, select: &str, field: bool, unit: &Value) -> Value {
    let full = select == "values";
    json!({
        "name": name,
        "type": value_type,
        "unit": unit,
        "lifecycle": { "state": "active" },
        "capabilities": { "select": select, "filter": full, "group": full && field, "order": full },
    })
}

/// The viewer's catalog, written out by hand: `margen` masked only; what
/// is restricted (`nota`, `secreto_centinela`, `reserva_centinela`) and
/// what is not a dimension (`id`, `importe`) is absent.
fn viewer_catalog() -> Value {
    // Its metrics sum `importe`: DECIMAL(38,2) (#261).
    let decimal = "DECIMAL(38,2)";
    json!({
        "status": "ready",
        "models": [{
            "model": "metrics/ventas.yaml",
            "name": "Ventas",
            "metrics": [
                member("filas", "INT64", "values", false, &json!({ "kind": "count" })),
                member("importe_total", decimal, "values", false, &Value::Null),
                member("margen", decimal, "masked", false, &Value::Null),
            ],
            "fields": [
                member("ventas.ciudad", "VARCHAR", "values", true, &Value::Null),
                member("ventas.instante", "TIMESTAMPTZ", "values", true, &Value::Null),
                member("ventas.region", "VARCHAR", "values", true, &Value::Null),
            ],
        }],
    })
}

const EMPTY_READY: &[u8] = br#"{"status":"ready","models":[]}"#;

/// A catalog answer: JSON, never cached, no global fingerprint.
fn assert_listed(answer: &Answer) {
    assert_eq!(answer.status, StatusCode::OK, "{:#}", answer.body);
    assert_eq!(answer.headers["content-type"], "application/json");
    assert_eq!(answer.headers["cache-control"], "no-store");
    for name in ["etag", "last-modified", "vary"] {
        assert!(answer.headers.get(name).is_none(), "{name}");
    }
}

/// A problem document with its correlation, no caching and no catalog,
/// not even a partial one.
fn assert_problem(answer: &Answer, status: StatusCode, code: &str) {
    assert_eq!(answer.status, status, "{:#}", answer.body);
    assert_eq!(answer.body["code"], code, "{:#}", answer.body);
    assert_eq!(answer.headers["content-type"], "application/problem+json");
    assert_eq!(answer.headers["cache-control"], "no-store");
    assert!(answer.headers.get("etag").is_none());
    assert!(
        answer.body["request_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "{:#}",
        answer.body
    );
    // Nothing but the problem's own fields, a fixed detail at most.
    let mut fields: Vec<&str> = answer
        .body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .filter(|field| *field != "detail")
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        ["code", "request_id", "status", "title", "type"],
        "{:#}",
        answer.body
    );
}

fn assert_no_sentinel(what: &str, text: &str, project: &std::path::Path) {
    let path = project.to_string_lossy();
    for sentinel in SENTINELS.iter().copied().chain([path.as_ref()]) {
        assert!(!text.contains(sentinel), "{what}: {sentinel} in {text}");
    }
}

/// The event of a catalog request: the reserved catalog resource, no query
/// shape, schema v1, and nothing of the catalog itself.
fn assert_catalog_event(event: &Event, result: &str, actor: &str) {
    assert_eq!(
        (event.action.as_str(), event.result.as_str()),
        ("query", result),
        "{event:#?}"
    );
    assert_eq!(event.resource_type, "semantic_catalog", "{event:#?}");
    assert_eq!(event.resource_id, "semantic:catalog", "{event:#?}");
    assert_eq!(event.metadata, json!({ "query_shape": null }), "{event:#?}");
    assert_eq!(event.schema, "1", "{event:#?}");
    assert_eq!(event.actor, actor, "{event:#?}");
    for name in [
        "centinela",
        "ventas",
        "importe",
        "filas",
        "margen",
        "region",
        "viewer",
        "metrics/",
    ] {
        assert!(!event.payload.contains(name), "{name} in {}", event.payload);
    }
}

/// An identifier derived from the decisions really taken.
fn assert_derived(event: &Event) -> String {
    let decision = event.decision.clone().unwrap_or_default();
    let digest = decision.strip_prefix(DECISION_PREFIX).unwrap_or_default();
    assert!(
        digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
        "{event:#?}"
    );
    decision
}

// ---- 1. The exact catalog and its event --------------------------------------

#[sqlx::test(migrations = false)]
async fn a_viewer_gets_the_exact_catalog_with_no_store_and_one_success_event_each(pool: PgPool) {
    let h = harness(pool).await;
    // Control: anything that reads a row of the source fails.
    let scanned = h
        .analytics
        .runtime()
        .unwrap()
        .run(|conn| {
            conn.query_row("SELECT count(*) FROM lake.main.ventas", [], |row| {
                row.get::<_, i64>(0)
            })
        })
        .await;
    assert!(scanned.is_err(), "the trap must fire on a scan");
    let ana = h.login("ana").await;
    let since = h.head().await;

    let answer = h.catalog(&ana).await;
    assert_listed(&answer);
    assert_eq!(answer.body, viewer_catalog());
    assert_no_sentinel(
        "catalog",
        &String::from_utf8_lossy(&answer.bytes),
        h.project.path(),
    );
    // The event is committed by the time the response is read.
    let first = h.only_event_since(since).await;
    assert_catalog_event(&first, "success", "ana");
    let decision = assert_derived(&first);

    // Deterministic: the same bytes each time and for every session of the
    // same role; the same decisions each time for the same user, whose
    // identity they include.
    let bea = h.login("bea").await;
    let since = h.head().await;
    let again = h.catalog(&ana).await;
    let other = h.catalog(&bea).await;
    assert_eq!(again.bytes, answer.bytes);
    assert_eq!(other.bytes, answer.bytes);
    let events = h.events_since(since).await;
    assert_eq!(events.len(), 2, "{events:#?}");
    assert_catalog_event(&events[0], "success", "ana");
    assert_catalog_event(&events[1], "success", "bea");
    assert_eq!(events[0].decision.as_deref(), Some(decision.as_str()));
    assert_ne!(assert_derived(&events[1]), decision);
    h.db.close().await;
}

// ---- 2. Usable nothing: an empty catalog, never a denial ------------------------

#[sqlx::test(migrations = false)]
async fn a_session_that_can_use_nothing_gets_an_empty_ready_catalog_audited_as_success(
    pool: PgPool,
) {
    let h = harness(pool).await;
    let nadie = h.login("nadie").await;
    let since = h.head().await;
    let answer = h.catalog(&nadie).await;
    assert_listed(&answer);
    // Nothing tells what or how much is hidden.
    assert_eq!(&answer.bytes[..], EMPTY_READY);
    let event = h.only_event_since(since).await;
    assert_catalog_event(&event, "success", "nadie");
    // Each metric was decided, and refused: that is what it names.
    assert_derived(&event);

    // A project without metrics answers the same bytes, and no decision is
    // invented for it.
    std::fs::remove_file(h.project.path().join("metrics/ventas.yaml")).unwrap();
    let ana = h.login("ana").await;
    let since = h.head().await;
    let answer = h.catalog(&ana).await;
    assert_listed(&answer);
    assert_eq!(&answer.bytes[..], EMPTY_READY);
    let event = h.only_event_since(since).await;
    assert_catalog_event(&event, "success", "ana");
    assert_eq!(event.decision, None, "{event:#?}");
    h.db.close().await;
}

// ---- 3. No project ---------------------------------------------------------------

#[sqlx::test(migrations = false)]
async fn without_a_project_the_catalog_is_not_configured_and_audited_without_a_decision(
    pool: PgPool,
) {
    let db = development(&pool).await;
    let h = build(
        pool,
        Setup {
            db,
            analytics: trapped(RuntimeConfig::in_memory(1)).await,
            timeout: INITIAL.timeout,
            configured: false,
            runs: None,
        },
    );
    let ana = h.login("ana").await;
    let since = h.head().await;
    let answer = h.catalog(&ana).await;
    assert_listed(&answer);
    assert_eq!(
        &answer.bytes[..],
        br#"{"status":"not_configured","models":[]}"#
    );
    let event = h.only_event_since(since).await;
    assert_catalog_event(&event, "success", "ana");
    assert_eq!(event.decision, None, "{event:#?}");
    h.db.close().await;
}

// ---- 4. No session ---------------------------------------------------------------

#[sqlx::test(migrations = false)]
async fn without_a_session_the_catalog_is_refused_and_nothing_is_recorded(pool: PgPool) {
    let h = harness(pool).await;
    let since = h.head().await;
    for cookie in [None, Some("bi_session=forjada"), Some("otra=1")] {
        let answer = h.get_with(CATALOG, cookie, &[]).await;
        assert_problem(&answer, StatusCode::UNAUTHORIZED, "session_required");
    }
    // Client claims are no session either.
    let claimed = h
        .get_with(
            "/api/v1/semantic/catalog?user=ana&skip_security=true",
            None,
            &[("x-user", "ana"), ("x-role", "admin")],
        )
        .await;
    assert_problem(&claimed, StatusCode::UNAUTHORIZED, "session_required");
    assert_eq!(h.events_since(since).await, Vec::new());
    // A logged-out cookie is no session.
    let ana = h.login("ana").await;
    let logout = h
        .app
        .clone()
        .oneshot(
            Request::post("/api/v1/logout")
                .header("origin", ORIGIN)
                .header("cookie", &ana)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let since = h.head().await;
    let answer = h.catalog(&ana).await;
    assert_problem(&answer, StatusCode::UNAUTHORIZED, "session_required");
    assert_eq!(h.events_since(since).await, Vec::new());
    h.db.close().await;
}

// ---- 5. Unusable project, file or engine -------------------------------------------

/// A second file whose compilation fails with a diagnostic that names a
/// sentinel.
const BROKEN: &str = "version: '0.2.0.dev0'\nname: Rota\ndatasets:\n  - name: rota\n    source: lake.main.diagnostico_centinela\n    primary_key: [id]\n    fields:\n      - name: id\n        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: id\nmetrics: []\n";

/// A failure: its stable code, no catalog, no sentinel, and one failure
/// event without a decision.
async fn assert_failed(h: &Harness, cookie: &str, status: StatusCode, code: &str) {
    let since = h.head().await;
    let answer = h.catalog(cookie).await;
    assert_problem(&answer, status, code);
    assert_no_sentinel(
        code,
        &String::from_utf8_lossy(&answer.bytes),
        h.project.path(),
    );
    let event = h.only_event_since(since).await;
    assert_catalog_event(&event, "failure", "ana");
    assert_eq!(event.decision, None, "{event:#?}");
}

#[sqlx::test(migrations = false)]
async fn invalid_policies_or_members_and_a_file_that_fails_to_compile_answer_neutrally(
    pool: PgPool,
) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    let unavailable = StatusCode::SERVICE_UNAVAILABLE;

    h.write(
        "policies/project.cedar",
        "permit (principal secreto_centinela",
    );
    assert_failed(&h, &ana, unavailable, "policies_invalid").await;
    std::fs::remove_file(h.project.path().join("policies/project.cedar")).unwrap();
    h.write("policies/members.yaml", "version: [secreto_centinela\n");
    assert_failed(&h, &ana, unavailable, "policies_invalid").await;
    h.write("policies/members.yaml", MEMBERS);

    // One file that does not compile: no partial catalog of the others.
    h.write("metrics/rota.yaml", BROKEN);
    assert_failed(&h, &ana, unavailable, "catalog_unavailable").await;
    std::fs::remove_file(h.project.path().join("metrics/rota.yaml")).unwrap();
    // An unreadable model and a project without its folder alike.
    h.write("metrics/ventas.yaml", "name: [secreto_centinela");
    assert_failed(&h, &ana, unavailable, "catalog_unavailable").await;
    std::fs::remove_dir_all(h.project.path().join("metrics")).unwrap();
    assert_failed(&h, &ana, unavailable, "catalog_unavailable").await;

    // Fixed, it answers again, exactly.
    std::fs::create_dir(h.project.path().join("metrics")).unwrap();
    h.write("metrics/ventas.yaml", MODEL);
    let since = h.head().await;
    let answer = h.catalog(&ana).await;
    assert_listed(&answer);
    assert_eq!(answer.body, viewer_catalog());
    assert_catalog_event(&h.only_event_since(since).await, "success", "ana");
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn an_engine_that_is_not_ready_answers_catalog_unavailable_with_a_failure_event(
    pool: PgPool,
) {
    let db = development(&pool).await;
    let h = build(
        pool,
        Setup {
            db,
            analytics: Analytics::disabled(),
            timeout: INITIAL.timeout,
            configured: true,
            runs: None,
        },
    );
    let ana = h.login("ana").await;
    assert_failed(
        &h,
        &ana,
        StatusCode::SERVICE_UNAVAILABLE,
        "catalog_unavailable",
    )
    .await;
    h.db.close().await;
}

/// Occupies the worker of `runtime` until the returned sender sends or is
/// dropped.
async fn hold(runtime: &Runtime) -> (std::sync::mpsc::Sender<()>, tokio::task::JoinHandle<()>) {
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let (started, running) = tokio::sync::oneshot::channel();
    let runtime = runtime.clone();
    let held = tokio::spawn(async move {
        let _ = runtime
            .run(move |_| {
                let _ = started.send(());
                let _ = wait.recv();
                Ok(())
            })
            .await;
    });
    running.await.unwrap();
    (release, held)
}

#[sqlx::test(migrations = false)]
async fn an_engine_that_cannot_start_times_out_with_its_failure_event_within_the_timeout(
    pool: PgPool,
) {
    let timeout = Duration::from_millis(2_000);
    let db = development(&pool).await;
    let single = trapped(RuntimeConfig {
        queue_capacity: 4,
        ..RuntimeConfig::in_memory(1)
    })
    .await;
    let h = build(
        pool,
        Setup {
            db,
            analytics: single,
            timeout,
            configured: true,
            runs: None,
        },
    );
    let ana = h.login("ana").await;
    let runtime = h.analytics.runtime().unwrap();
    let (release, held) = hold(&runtime).await;

    let started = Instant::now();
    let since = h.head().await;
    let answer = h.catalog(&ana).await;
    let elapsed = started.elapsed();
    assert_problem(&answer, StatusCode::GATEWAY_TIMEOUT, "semantic_timeout");
    // The projection used execution's share; the event fit in the rest.
    let execution = Profile::new(Budgets { timeout, ..INITIAL })
        .unwrap()
        .execution_timeout();
    assert!(elapsed >= execution, "{elapsed:?} before {execution:?}");
    assert!(
        elapsed < timeout + Duration::from_millis(500),
        "{elapsed:?} for a {timeout:?} timeout"
    );
    let event = h.only_event_since(since).await;
    assert_catalog_event(&event, "failure", "ana");
    assert_eq!(event.decision, None, "{event:#?}");

    release.send(()).unwrap();
    held.await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let load = runtime.load();
            if load.queued == 0 && load.running == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let since = h.head().await;
    let answer = h.catalog(&ana).await;
    assert_listed(&answer);
    assert_eq!(answer.body, viewer_catalog());
    assert_catalog_event(&h.only_event_since(since).await, "success", "ana");
    h.db.close().await;
}

// ---- 6. The audit decides the answer -------------------------------------------

#[sqlx::test(migrations = false)]
async fn a_failing_audit_append_answers_no_catalog_and_the_retry_records_once(pool: PgPool) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    sqlx::query(AUDIT_FAILS).execute(&h.pool).await.unwrap();
    let since = h.head().await;
    // A catalog, an empty one and a failure alike: none leaves without
    // its event.
    let nadie = h.login("nadie").await;
    let answer = h.catalog(&ana).await;
    assert_problem(
        &answer,
        StatusCode::SERVICE_UNAVAILABLE,
        "audit_unavailable",
    );
    let answer = h.catalog(&nadie).await;
    assert_problem(
        &answer,
        StatusCode::SERVICE_UNAVAILABLE,
        "audit_unavailable",
    );
    h.write("policies/project.cedar", "permit (principal");
    let answer = h.catalog(&ana).await;
    assert_problem(
        &answer,
        StatusCode::SERVICE_UNAVAILABLE,
        "audit_unavailable",
    );
    std::fs::remove_file(h.project.path().join("policies/project.cedar")).unwrap();
    assert_eq!(h.events_since(since).await, Vec::new());

    sqlx::query("ALTER TABLE bi_meta.audit_events DROP CONSTRAINT append_fails")
        .execute(&h.pool)
        .await
        .unwrap();
    let since = h.head().await;
    let answer = h.catalog(&ana).await;
    assert_listed(&answer);
    assert_eq!(answer.body, viewer_catalog());
    assert_catalog_event(&h.only_event_since(since).await, "success", "ana");
    h.db.close().await;
}

/// Waits until no other backend of this database waits on a lock.
async fn no_lock_waiters(pool: &PgPool) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
             WHERE datname = current_database() AND wait_event_type = 'Lock'
               AND pid <> pg_backend_pid()",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting == 0 {
            return;
        }
        assert!(Instant::now() < until, "{waiting} backends still wait");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Holds the audit head, sends one catalog request and checks it ended
/// within its timeout with `audit_unavailable` and left no event.
async fn assert_held_head_closes(h: &Harness, cookie: &str, timeout: Duration) {
    let since = h.head().await;
    let mut holder = h.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE bi_meta.audit_head IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *holder)
        .await
        .unwrap();
    let started = Instant::now();
    let answer = h.catalog(cookie).await;
    let elapsed = started.elapsed();
    assert_problem(
        &answer,
        StatusCode::SERVICE_UNAVAILABLE,
        "audit_unavailable",
    );
    // It waited on the lock for what was left of its timeout, no longer.
    assert!(
        elapsed >= timeout / 2 && elapsed < timeout + Duration::from_secs(2),
        "{elapsed:?} for a {timeout:?} timeout"
    );
    // Nothing keeps waiting for the lock once the request has answered.
    no_lock_waiters(&h.pool).await;
    holder.rollback().await.unwrap();
    assert_eq!(h.events_since(since).await, Vec::new());
}

#[sqlx::test(migrations = false)]
async fn a_held_audit_head_ends_the_catalog_within_its_timeout_and_leaves_no_event(pool: PgPool) {
    let timeout = Duration::from_millis(2_000);
    let db = development(&pool).await;
    let h = build(
        pool,
        Setup {
            db,
            analytics: trapped(RuntimeConfig::in_memory(2)).await,
            timeout,
            configured: true,
            runs: None,
        },
    );
    let ana = h.login("ana").await;
    assert_held_head_closes(&h, &ana, timeout).await;

    let since = h.head().await;
    let answer = h.catalog(&ana).await;
    assert_listed(&answer);
    assert_eq!(answer.body, viewer_catalog());
    assert_catalog_event(&h.only_event_since(since).await, "success", "ana");
    h.db.close().await;
}

// ---- 6b. Under the restricted runtime role of #115 ------------------------------

const GRANTS: &str = include_str!("../admin/runtime-grants.sql");
/// A decoy, never a real secret.
const PASSWORD: &str = "decoy-password-7f3a";

/// An owner that migrates and a least-privilege runtime role, as the
/// documented procedure provisions them.
struct Roles {
    superuser: PgPool,
    database: String,
    admin: String,
    runtime: String,
}

impl Roles {
    async fn create(pool: &PgPool) -> Self {
        let database: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(pool)
            .await
            .unwrap();
        assert!(
            database
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        );
        // Unique within PostgreSQL's 63-byte limit for role names.
        let tag: String = database
            .chars()
            .rev()
            .filter(char::is_ascii_alphanumeric)
            .take(16)
            .collect::<String>()
            .to_ascii_lowercase();
        let roles = Self {
            superuser: pool.clone(),
            admin: format!("c{tag}_admin"),
            runtime: format!("c{tag}_runtime"),
            database,
        };
        roles
            .sql(
                "DROP ROLE IF EXISTS {runtime}; DROP ROLE IF EXISTS {admin}; \
                 CREATE ROLE {admin} LOGIN PASSWORD '{password}'; \
                 CREATE ROLE {runtime} LOGIN PASSWORD '{password}'; \
                 ALTER DATABASE {database} OWNER TO {admin};",
            )
            .await;
        roles
    }

    fn expand(&self, sql: &str) -> String {
        sql.replace("{admin}", &self.admin)
            .replace("{runtime}", &self.runtime)
            .replace("{database}", &format!("\"{}\"", self.database))
            .replace("{password}", PASSWORD)
    }

    async fn sql(&self, sql: &str) {
        sqlx::raw_sql(AssertSqlSafe(self.expand(sql)))
            .execute(&self.superuser)
            .await
            .unwrap();
    }

    fn options(&self, role: &str) -> PgConnectOptions {
        self.superuser
            .connect_options()
            .as_ref()
            .clone()
            .username(role)
            .password(PASSWORD)
    }

    /// Migrated by the owner, granted by the documented procedure, and
    /// started as the runtime role in the default restricted mode.
    async fn runtime_database(&self) -> Database {
        db::migrate(&self.options(&self.admin), db::MIGRATION_TIMEOUT)
            .await
            .unwrap();
        let owner = PgPool::connect_lazy_with(self.options(&self.admin));
        sqlx::raw_sql(AssertSqlSafe(GRANTS.replace("bi_runtime", &self.runtime)))
            .execute(&owner)
            .await
            .unwrap();
        owner.close().await;
        let db = Database::start(&self.options(&self.runtime), MetadataMode::Restricted);
        assert!(
            tokio::time::timeout(Duration::from_secs(30), db.wait_until_ready())
                .await
                .unwrap()
        );
        db
    }

    async fn drop(self) {
        self.sql(
            "ALTER DATABASE {database} OWNER TO CURRENT_USER; \
             REASSIGN OWNED BY {admin}, {runtime} TO CURRENT_USER; \
             DROP OWNED BY {admin}, {runtime}; \
             DROP ROLE {runtime}; DROP ROLE {admin};",
        )
        .await;
    }
}

#[sqlx::test(migrations = false)]
async fn under_the_restricted_runtime_role_the_catalog_is_audited_and_a_held_head_closes_it(
    pool: PgPool,
) {
    let roles = Roles::create(&pool).await;
    let db = roles.runtime_database().await;
    let timeout = Duration::from_millis(2_000);
    let h = build(
        pool,
        Setup {
            db,
            analytics: trapped(RuntimeConfig::in_memory(2)).await,
            timeout,
            configured: true,
            runs: None,
        },
    );
    let ana = h.login("ana").await;
    let nadie = h.login("nadie").await;
    let since = h.head().await;
    let answer = h.catalog(&ana).await;
    assert_listed(&answer);
    assert_eq!(answer.body, viewer_catalog());
    let empty = h.catalog(&nadie).await;
    assert_listed(&empty);
    assert_eq!(&empty.bytes[..], EMPTY_READY);
    let events = h.events_since(since).await;
    assert_eq!(events.len(), 2, "{events:#?}");
    assert_catalog_event(&events[0], "success", "ana");
    assert_catalog_event(&events[1], "success", "nadie");
    assert_derived(&events[0]);

    assert_held_head_closes(&h, &ana, timeout).await;
    // The runtime role reads nothing of the chain: only appends.
    let runtime = PgPool::connect_lazy_with(roles.options(&roles.runtime));
    let read: Result<i64, _> = sqlx::query_scalar("SELECT count(*) FROM bi_meta.audit_events")
        .fetch_one(&runtime)
        .await;
    runtime.close().await;
    assert!(read.is_err(), "the runtime role must not read the chain");

    let since = h.head().await;
    assert_listed(&h.catalog(&ana).await);
    assert_catalog_event(&h.only_event_since(since).await, "success", "ana");
    h.db.close().await;
    roles.drop().await;
}

// ---- 7. Revocation -----------------------------------------------------------------

#[sqlx::test(migrations = false)]
async fn removing_a_membership_takes_effect_on_the_next_request_of_the_same_cookie(pool: PgPool) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    let nadie = h.login("nadie").await;
    let before = h.catalog(&ana).await;
    assert_listed(&before);
    assert_eq!(before.body, viewer_catalog());
    let unmapped = h.catalog(&nadie).await;

    h.write(
        "policies/members.yaml",
        "version: 1\nusers:\n  bea: [viewer]\n  analista: [analyst]\n",
    );
    let since = h.head().await;
    let after = h.catalog(&ana).await;
    assert_listed(&after);
    // Exactly what a session without membership gets: nothing left over.
    assert_eq!(&after.bytes[..], EMPTY_READY);
    assert_eq!(after.bytes, unmapped.bytes);
    let event = h.only_event_since(since).await;
    assert_catalog_event(&event, "success", "ana");

    // Granted again, the same cookie sees it again.
    h.write("policies/members.yaml", MEMBERS);
    let restored = h.catalog(&ana).await;
    assert_eq!(restored.bytes, before.bytes);
    h.db.close().await;
}

// ---- 8. Client claims grant nothing -------------------------------------------------

#[sqlx::test(migrations = false)]
async fn security_switches_users_or_roles_sent_by_the_client_grant_nothing(pool: PgPool) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    let analista = h.login("analista").await;
    let plain = h.catalog(&ana).await;
    assert_listed(&plain);
    assert_eq!(plain.body, viewer_catalog());
    // Control: another role's catalog differs, so a granted claim would show.
    let analyst = h.catalog(&analista).await;
    assert_listed(&analyst);
    assert_ne!(analyst.bytes, plain.bytes);

    let since = h.head().await;
    let uris = [
        "/api/v1/semantic/catalog?skip_security=true",
        "/api/v1/semantic/catalog?user=analista&role=analyst&roles=admin&groups=admin",
        "/api/v1/semantic/catalog?skip_security=1&user_id=analista&principal=admin&region=Sur",
    ];
    let headers: [(&str, &str); 8] = [
        ("x-user", "analista"),
        ("x-user-id", "analista"),
        ("x-role", "admin"),
        ("x-roles", "analyst,admin"),
        ("x-groups", "admin"),
        ("x-forwarded-user", "analista"),
        ("remote-user", "analista"),
        ("x-skip-security", "true"),
    ];
    let mut sent = 0;
    for uri in uris {
        for extra in [&[][..], &headers[..]] {
            let claimed = h.get_with(uri, Some(&ana), extra).await;
            assert_listed(&claimed);
            assert_eq!(claimed.bytes, plain.bytes, "{uri} {extra:?}");
            sent += 1;
        }
    }
    let claimed = h.get_with(CATALOG, Some(&ana), &headers).await;
    assert_eq!(claimed.bytes, plain.bytes);
    sent += 1;
    // Each one recorded as the session's own user.
    let events = h.events_since(since).await;
    assert_eq!(events.len(), sent, "{events:#?}");
    for event in &events {
        assert_catalog_event(event, "success", "ana");
    }
    h.db.close().await;
}

// ---- 9. A hidden object changes nothing the viewer receives ------------------------

#[sqlx::test(migrations = false)]
async fn adding_hidden_objects_leaves_the_viewers_payload_byte_identical(pool: PgPool) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    let analista = h.login("analista").await;
    let since = h.head().await;
    let before = h.catalog(&ana).await;
    assert_listed(&before);
    let decision_before = assert_derived(&h.only_event_since(since).await);
    let analyst_before = h.catalog(&analista).await;

    // A restricted metric, a restricted field and a metric derived from the
    // restricted one.
    let hidden_field = "      - name: oculto_centinela\n        dimension: {}\n        custom_extensions:\n          - vendor_name: bi-platform\n            data: '{\"version\": 1, \"sensitivity\": \"restricted\"}'\n        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: ciudad\nmetrics:\n";
    let hidden_metrics = "  - name: nueva_centinela\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{\"version\": 1, \"sensitivity\": \"restricted\"}'\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: MAX(ventas.importe)\n  - name: derivada_centinela\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: nueva_centinela\n";
    let model = format!(
        "{}{hidden_metrics}",
        MODEL.replacen("metrics:\n", hidden_field, 1)
    );
    assert_ne!(model, MODEL);
    h.write("metrics/ventas.yaml", &model);

    let since = h.head().await;
    let after = h.catalog(&ana).await;
    assert_listed(&after);
    assert_eq!(after.bytes, before.bytes);
    // More decisions were taken: the event names them, the payload does not.
    let decision_after = assert_derived(&h.only_event_since(since).await);
    assert_ne!(decision_after, decision_before);
    // Control: the objects are real, the analyst is announced the new
    // restricted metric masked.
    let analyst_after = h.catalog(&analista).await;
    assert_listed(&analyst_after);
    assert_ne!(analyst_after.bytes, analyst_before.bytes);
    let masked = analyst_after.body["models"][0]["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "nueva_centinela")
        .unwrap_or_else(|| panic!("{:#}", analyst_after.body));
    assert_eq!(masked["capabilities"]["select"], "masked");
    h.db.close().await;
}

// ---- Captures never mix ------------------------------------------------------------

#[sqlx::test(migrations = false)]
async fn two_roles_listing_at_once_each_get_their_own_catalog_and_decision(pool: PgPool) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    let analista = h.login("analista").await;
    let since = h.head().await;
    let viewer = h.catalog(&ana).await;
    let analyst = h.catalog(&analista).await;
    let serial = h.events_since(since).await;
    let decisions: BTreeMap<String, Option<String>> = serial
        .iter()
        .map(|e| (e.actor.clone(), e.decision.clone()))
        .collect();
    assert_ne!(decisions["ana"], decisions["analista"]);

    let since = h.head().await;
    for round in 0..4 {
        let (first, second) = if round % 2 == 0 {
            (&ana, &analista)
        } else {
            (&analista, &ana)
        };
        let (a, b) = tokio::join!(h.catalog(first), h.catalog(second));
        let (mine, theirs) = if round % 2 == 0 { (a, b) } else { (b, a) };
        assert_listed(&mine);
        assert_listed(&theirs);
        assert_eq!(mine.bytes, viewer.bytes, "round {round}");
        assert_eq!(theirs.bytes, analyst.bytes, "round {round}");
    }
    let events = h.events_since(since).await;
    assert_eq!(events.len(), 8, "{events:#?}");
    for event in &events {
        assert_catalog_event(event, "success", &event.actor);
        assert_eq!(event.decision, decisions[&event.actor], "{event:#?}");
    }
    h.db.close().await;
}

// ---- Phase metrics with closed labels ----------------------------------------------

#[sqlx::test(migrations = false)]
async fn catalog_metrics_count_each_answer_by_stable_code_and_time_both_phases(pool: PgPool) {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter.clone())
        .build();
    let db = development(&pool).await;
    let h = build(
        pool,
        Setup {
            db,
            analytics: trapped(RuntimeConfig::in_memory(2)).await,
            timeout: INITIAL.timeout,
            configured: true,
            runs: Some(
                Runs::new(Profile::new(INITIAL).unwrap()).with_meter(&provider.meter("test")),
            ),
        },
    );
    let ana = h.login("ana").await;
    assert_listed(&h.catalog(&ana).await);
    assert_listed(&h.catalog(&ana).await);
    h.write("policies/project.cedar", "permit (principal");
    assert_problem(
        &h.catalog(&ana).await,
        StatusCode::SERVICE_UNAVAILABLE,
        "policies_invalid",
    );
    std::fs::remove_file(h.project.path().join("policies/project.cedar")).unwrap();
    sqlx::query(AUDIT_FAILS).execute(&h.pool).await.unwrap();
    assert_problem(
        &h.catalog(&ana).await,
        StatusCode::SERVICE_UNAVAILABLE,
        "audit_unavailable",
    );

    provider.force_flush().unwrap();
    let exports: Vec<ResourceMetrics> = exporter.get_finished_metrics().unwrap();
    let mut codes = BTreeMap::new();
    let mut phases = BTreeMap::new();
    for metric in exports
        .last()
        .into_iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
    {
        match (metric.name(), metric.data()) {
            ("bi.semantic.catalog.requests", AggregatedMetrics::U64(MetricData::Sum(sum))) => {
                for point in sum.data_points() {
                    let labels: Vec<_> = point.attributes().collect();
                    assert_eq!(labels.len(), 1, "{labels:?}");
                    assert_eq!(labels[0].key.as_str(), "code");
                    codes.insert(labels[0].value.to_string(), point.value());
                }
            }
            (
                "bi.semantic.catalog.phase.duration",
                AggregatedMetrics::F64(MetricData::Histogram(histogram)),
            ) => {
                assert_eq!(metric.unit(), "s");
                for point in histogram.data_points() {
                    let labels: Vec<_> = point.attributes().collect();
                    assert_eq!(labels.len(), 1, "{labels:?}");
                    assert_eq!(labels[0].key.as_str(), "phase");
                    phases.insert(labels[0].value.to_string(), point.count());
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        codes,
        BTreeMap::from([
            ("listed".to_owned(), 2),
            ("policies_invalid".to_owned(), 1),
            ("audit_unavailable".to_owned(), 1),
        ])
    );
    assert_eq!(
        phases,
        BTreeMap::from([("audit".to_owned(), 4), ("projection".to_owned(), 4)])
    );
    h.db.close().await;
}

// ---- 10. Latency ----------------------------------------------------------------------

const LATENCY_SAMPLES: usize = 200;
const LATENCY_WARMUP: usize = 20;

/// The Issue's rows in a plain table, as Explorar would read them.
async fn plain_rows() -> Analytics {
    let runtime = tokio::task::spawn_blocking(|| {
        bi_runtime::Bootstrap::open(RuntimeConfig::in_memory(2))
            .unwrap()
            .finish()
            .unwrap()
    })
    .await
    .unwrap();
    runtime
        .run(|conn| {
            conn.execute_batch(&format!(
                "ATTACH ':memory:' AS lake; CREATE TABLE lake.main.ventas (id BIGINT NOT NULL, \
                 region VARCHAR NOT NULL, ciudad VARCHAR NOT NULL, importe DECIMAL(18,2) NOT NULL, \
                 instante TIMESTAMPTZ NOT NULL, nota VARCHAR); INSERT INTO lake.main.ventas VALUES {ROWS};"
            ))
        })
        .await
        .unwrap();
    Analytics::ready(runtime)
}

/// Milliseconds of one GET answered 200, and its body length.
async fn timed(h: &Harness, cookie: &str) -> (f64, usize) {
    let started = Instant::now();
    let answer = h.catalog(cookie).await;
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(answer.status, StatusCode::OK, "{:#}", answer.body);
    (elapsed, answer.bytes.len())
}

fn summary(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    json!({
        "n": values.len(),
        "p50_ms": values[values.len() / 2],
        "p95_ms": values[values.len() * 95 / 100],
        "max_ms": values[values.len() - 1],
    })
}

/// Reproducible latency of the route through the real router: session on
/// PostgreSQL, projection compiling the Issue's model on DuckDB, and the
/// confirmed audit event. `cold`: the first request of a freshly started
/// engine and project, one per sample; `warm`: repeated requests on one.
/// A refusal is never measured as an answer. Run explicitly:
/// `cargo test --release -p bi-server --no-default-features --features dev-auth,postgres-tests --test semantic_catalog -- --ignored --nocapture`
#[sqlx::test(migrations = false)]
#[ignore = "measurement, not a CI latency threshold"]
async fn semantic_catalog_latency(pool: PgPool) {
    let db = development(&pool).await;
    let setup = |analytics| Setup {
        db: db.clone(),
        analytics,
        timeout: Duration::from_secs(10),
        configured: true,
        runs: None,
    };
    let mut report = BTreeMap::new();
    for user in ["ana", "nadie"] {
        let mut cold = Vec::with_capacity(LATENCY_SAMPLES);
        let mut bytes = 0;
        for _ in 0..LATENCY_SAMPLES {
            let h = build(pool.clone(), setup(plain_rows().await));
            let cookie = h.login(user).await;
            let (elapsed, length) = timed(&h, &cookie).await;
            cold.push(elapsed);
            bytes = length;
        }
        let h = build(pool.clone(), setup(plain_rows().await));
        let cookie = h.login(user).await;
        for _ in 0..LATENCY_WARMUP {
            timed(&h, &cookie).await;
        }
        let mut warm = Vec::with_capacity(LATENCY_SAMPLES);
        for _ in 0..LATENCY_SAMPLES {
            warm.push(timed(&h, &cookie).await.0);
        }
        report.insert(
            user,
            json!({ "cold": summary(cold), "warm": summary(warm), "body_bytes": bytes }),
        );
    }
    let system = sysinfo::System::new_all();
    println!(
        "{}",
        json!({
            "task": "140",
            "route": "GET /api/v1/semantic/catalog, router+session+projection+audit",
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "os": sysinfo::System::long_os_version(),
            "cpu": system.cpus()[0].brand(),
            "logical_cpus": system.cpus().len(),
            "ram_bytes": system.total_memory(),
            "fixture": "metrics/ventas.yaml, 4 rows, 7 fields, 4 metrics",
            "warmup": LATENCY_WARMUP,
            "samples": LATENCY_SAMPLES,
            "report": report,
        })
    );
    db.close().await;
}
