//! `POST /api/v1/semantic/query` (#138) through the real router: sessions
//! on PostgreSQL, a project on disk, DuckDB with the Issue's rows, and the
//! audit chain. Every answer is checked against the hand-written oracle and
//! against the single terminal audit event it must leave, or not leave.
#![cfg(all(feature = "postgres-tests", feature = "dev-auth"))]

use std::time::{Duration, Instant};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use bi_runtime::{Runtime, RuntimeConfig};
use bi_server::metadata::MetadataMode;
use bi_server::semantic::run::{Budgets, INITIAL, Profile, Runs};
use bi_server::{AppState, analytics::Analytics, db::Database, models::Models};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:8080";

/// The rows of the Issue: id, region, city, amount, instant, note.
const ROWS: &str = "(1, 'Norte', 'La Paz', 100.10, '2026-01-01T00:00:00Z', NULL),
    (2, 'Norte', 'El Alto', 0.20, '2026-01-02T00:00:00Z', 'n'),
    (3, 'Sur', 'Tarija', 200.20, '2026-01-01T00:00:00Z', 's'),
    (4, 'Sur', 'Sucre', -0.20, '2026-01-02T00:00:00Z', NULL)";

/// The Issue's model: `nota` restricted, `margen` confidential, and a
/// restricted metric whose name is a sentinel.
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
metrics:
  - name: importe_total
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

/// The only detail a session without inspection gets for an execution
/// that depends on the catalog.
const UNDISCLOSED_RUN: &str = "No se puede ejecutar la consulta solicitada.";

fn project() -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    std::fs::create_dir(p.path().join("metrics")).unwrap();
    std::fs::create_dir(p.path().join("policies")).unwrap();
    std::fs::write(p.path().join("metrics/ventas.yaml"), MODEL).unwrap();
    std::fs::write(p.path().join("policies/members.yaml"), MEMBERS).unwrap();
    p
}

/// A DuckDB runtime of `config` holding the Issue's rows.
async fn analytics(config: RuntimeConfig) -> Analytics {
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
                "ATTACH ':memory:' AS lake; CREATE TABLE lake.main.ventas (id BIGINT NOT NULL, \
                 region VARCHAR NOT NULL, ciudad VARCHAR NOT NULL, importe DECIMAL(18,2) NOT NULL, \
                 instante TIMESTAMPTZ NOT NULL, nota VARCHAR); INSERT INTO lake.main.ventas VALUES {ROWS};"
            ))
        })
        .await
        .unwrap();
    Analytics::ready(runtime)
}

struct Harness {
    app: Router,
    pool: sqlx::PgPool,
    project: tempfile::TempDir,
    db: Database,
    analytics: Analytics,
}

/// One answer as the client reads it.
struct Answer {
    status: StatusCode,
    headers: HeaderMap,
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
    shape: Option<Value>,
    actor: String,
}

async fn harness(pool: sqlx::PgPool) -> Harness {
    harness_with(
        pool,
        analytics(RuntimeConfig::in_memory(2)).await,
        INITIAL.timeout,
    )
    .await
}

/// A harness on `analytics` whose execution profile times out at `timeout`.
async fn harness_with(pool: sqlx::PgPool, analytics: Analytics, timeout: Duration) -> Harness {
    let project = project();
    let db = Database::start(&pool.connect_options(), MetadataMode::Development);
    assert!(
        tokio::time::timeout(Duration::from_secs(30), db.wait_until_ready())
            .await
            .unwrap()
    );
    let auth = bi_server::auth::AuthConfig::from_lookup(&|key| match key {
        "BI_PUBLIC_ORIGIN" => Some(ORIGIN.into()),
        "BI_DEV_USERS" => Some(
            r#"[{"id":"ana","display_name":"Ana"},{"id":"bea","display_name":"Bea"},{"id":"analista","display_name":"Analista"},{"id":"inspector","display_name":"Inspector"}]"#
                .into(),
        ),
        _ => None,
    })
    .unwrap();
    let app = bi_server::router(AppState {
        auth,
        analytics: analytics.clone(),
        models: Models::new(Some(project.path().into())),
        semantic_runs: Runs::new(Profile::new(Budgets { timeout, ..INITIAL }).unwrap()),
        ..AppState::new(db.clone())
    });
    Harness {
        app,
        pool,
        project,
        db,
        analytics,
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
        assert_eq!(response.status(), StatusCode::OK);
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }

    async fn send(
        &self,
        path: &str,
        cookie: Option<&str>,
        origin: Option<&str>,
        body: String,
    ) -> Answer {
        let mut request = Request::post(path).header("content-type", "application/json");
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        Answer {
            status,
            headers,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    async fn query(&self, cookie: &str, body: &Value) -> Answer {
        self.send(
            "/api/v1/semantic/query",
            Some(cookie),
            Some(ORIGIN),
            body.to_string(),
        )
        .await
    }

    async fn explain(&self, cookie: &str, body: &Value) -> Answer {
        self.send(
            "/api/v1/semantic/explain",
            Some(cookie),
            Some(ORIGIN),
            body.to_string(),
        )
        .await
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
            Option<String>,
            String,
        );
        sqlx::query_as::<_, Row>(
            "SELECT payload::jsonb->>'action', payload::jsonb->>'result', \
             payload::jsonb->>'resource_type', payload::jsonb->>'resource_id', \
             payload::jsonb->>'policy_decision_id', \
             (payload::jsonb->'metadata_redacted'->'query_shape')::text, \
             payload::jsonb->>'actor_id' \
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
            |(action, result, resource_type, resource_id, decision, shape, actor)| Event {
                action,
                result,
                resource_type,
                resource_id,
                decision,
                shape: shape.map(|text| serde_json::from_str(&text).unwrap()),
                actor,
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

fn metric(id: &str, name: &str) -> Value {
    json!({ "kind": "metric", "id": id, "name": name })
}

fn dimension(id: &str, name: &str) -> Value {
    json!({ "kind": "dimension", "id": id, "name": name })
}

fn request(query: &Value) -> Value {
    json!({ "model": "metrics/ventas.yaml", "query": query })
}

fn selections(selections: &Value) -> Value {
    request(&json!({ "selections": selections }))
}

fn total() -> Value {
    selections(&json!([metric("t", "importe_total"), metric("f", "filas")]))
}

/// With no order and no limit: the default 1000 rows, ordered by region.
fn by_region() -> Value {
    selections(&json!([
        dimension("r", "ventas.region"),
        metric("t", "importe_total"),
        metric("f", "filas")
    ]))
}

/// `name` asked about `model`.
fn asked(model: &str, name: &str) -> Value {
    let mut body = selections(&json!([metric("m", name)]));
    body["model"] = json!(model);
    body
}

fn rows(answer: &Answer) -> &Value {
    &answer.body["result"]["blocks"][0]["rows"]
}

fn oracle_total() -> Value {
    json!([["300.30", "4"]])
}

fn oracle_regions() -> Value {
    json!([["Norte", "100.30", "2"], ["Sur", "200.00", "2"]])
}

/// A complete answer with the oracle's rows for `total()`.
fn assert_total(answer: &Answer) {
    assert_eq!(answer.status, StatusCode::OK, "{:#}", answer.body);
    assert_eq!(rows(answer), &oracle_total(), "{:#}", answer.body);
}

fn assert_regions(answer: &Answer) {
    assert_eq!(answer.status, StatusCode::OK, "{:#}", answer.body);
    assert_eq!(rows(answer), &oracle_regions(), "{:#}", answer.body);
}

/// A problem document with its correlation and no caching.
fn assert_problem(answer: &Answer, status: StatusCode, code: &str) {
    assert_eq!(answer.status, status, "{:#}", answer.body);
    assert_eq!(answer.body["code"], code, "{:#}", answer.body);
    assert_eq!(answer.headers["content-type"], "application/problem+json");
    assert_eq!(answer.headers["cache-control"], "no-store");
    assert!(
        answer.body["request_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "{:#}",
        answer.body
    );
    for part in ["result", "receipt"] {
        assert!(answer.body.get(part).is_none(), "{part}: {:#}", answer.body);
    }
    // Every 422 is a semantic rejection, as the contract declares it.
    if status == StatusCode::UNPROCESSABLE_ENTITY {
        assert!(answer.body["diagnostics"].is_array(), "{:#}", answer.body);
    }
}

/// The neutral answer: nothing but its code, its fixed detail and the
/// correlation.
fn assert_undisclosed(answer: &Answer) {
    assert_problem(answer, StatusCode::NOT_FOUND, "model_not_found");
    let mut fields: Vec<&str> = answer
        .body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        ["code", "detail", "request_id", "status", "title", "type"],
        "{:#}",
        answer.body
    );
    assert_eq!(answer.body["detail"], UNDISCLOSED_RUN);
}

fn without_correlation(answer: &Answer) -> Value {
    let mut body = answer.body.clone();
    body.as_object_mut().unwrap().remove("request_id");
    body
}

/// A query event of a decided model: its decision and shape recorded.
fn assert_decided(event: &Event, action: &str, result: &str) {
    assert_eq!(
        (event.action.as_str(), event.result.as_str()),
        (action, result),
        "{event:#?}"
    );
    assert_eq!(event.resource_type, "semantic_model", "{event:#?}");
    assert!(
        !event.resource_id.is_empty() && event.resource_id != "semantic:request",
        "{event:#?}"
    );
    assert!(
        event.decision.as_deref().is_some_and(|d| !d.is_empty()),
        "{event:#?}"
    );
    assert!(event.shape.is_some(), "{event:#?}");
}

/// A failure before any decision: the reserved request resource.
fn assert_undecided(event: &Event) {
    assert_eq!(
        (event.action.as_str(), event.result.as_str()),
        ("query", "failure"),
        "{event:#?}"
    );
    assert_eq!(event.resource_type, "semantic_request", "{event:#?}");
    assert_eq!(event.resource_id, "semantic:request", "{event:#?}");
    assert_eq!(event.decision, None, "{event:#?}");
    assert!(event.shape.is_some(), "{event:#?}");
}

const AUDIT_FAILS: &str =
    "ALTER TABLE bi_meta.audit_events ADD CONSTRAINT append_fails CHECK (false) NOT VALID";

// ---- 1. The exact answer and its event ------------------------------------

#[sqlx::test(migrations = false)]
async fn a_viewer_gets_the_oracle_rows_with_no_store_and_one_success_event_each(
    pool: sqlx::PgPool,
) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    let since = h.head().await;

    let answer = h.query(&ana, &total()).await;
    assert_total(&answer);
    assert_eq!(answer.headers["cache-control"], "no-store");
    assert_eq!(answer.headers["content-type"], "application/json");
    let block = &answer.body["result"]["blocks"][0];
    assert_eq!(block["row_count"], 1);
    // One row by construction: no subset, no limit.
    assert_eq!(block["limit"], Value::Null);
    assert!(block.as_object().unwrap().contains_key("limit"));
    assert!(
        answer.body["receipt"]["id"]
            .as_str()
            .unwrap()
            .starts_with("run:"),
        "{:#}",
        answer.body
    );
    assert_eq!(answer.body["notices"], json!([]));

    let answer = h.query(&ana, &by_region()).await;
    assert_regions(&answer);
    assert_eq!(answer.headers["cache-control"], "no-store");
    let block = &answer.body["result"]["blocks"][0];
    assert_eq!(
        (block["row_count"].clone(), block["limit"].clone()),
        (json!(2), json!(1000))
    );

    let events = h.events_since(since).await;
    assert_eq!(events.len(), 2, "{events:#?}");
    for event in &events {
        assert_decided(event, "query", "success");
        assert_eq!(event.actor, "ana");
    }
    assert_eq!(events[0].resource_id, events[1].resource_id, "one model");
    assert_eq!(
        events[0].shape,
        Some(json!({ "dimensions": 0, "measures": 2, "filters": 0, "limit": 0 }))
    );
    assert_eq!(
        events[1].shape,
        Some(json!({ "dimensions": 1, "measures": 2, "filters": 0, "limit": 1000 }))
    );
    // The resource is the model's content hash, the one explain receipts.
    let explained = h.explain(&ana, &total()).await;
    assert_eq!(explained.status, StatusCode::OK, "{:#}", explained.body);
    assert_eq!(explained.body["receipt"]["model"], events[0].resource_id);
    h.db.close().await;
}

// ---- 2. Transport -----------------------------------------------------------

#[sqlx::test(migrations = false)]
async fn anonymous_foreign_origin_and_oversized_requests_are_refused_without_an_event(
    pool: sqlx::PgPool,
) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    let since = h.head().await;
    let path = "/api/v1/semantic/query";

    let anonymous = h.send(path, None, Some(ORIGIN), total().to_string()).await;
    assert_problem(&anonymous, StatusCode::UNAUTHORIZED, "session_required");
    for origin in [None, Some("https://evil.example")] {
        let refused = h.send(path, Some(&ana), origin, total().to_string()).await;
        assert_problem(&refused, StatusCode::FORBIDDEN, "origin_forbidden");
    }
    // A valid query padded past the 64 KiB body limit: whitespace only.
    let oversized = format!("{}{}", " ".repeat(65 * 1024), total());
    let refused = h.send(path, Some(&ana), Some(ORIGIN), oversized).await;
    assert_problem(
        &refused,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_request",
    );
    assert_eq!(h.events_since(since).await, Vec::new());

    // The same query padded within the limit runs: the size was the cause.
    let within = format!("{}{}", " ".repeat(32 * 1024), total());
    let answer = h.send(path, Some(&ana), Some(ORIGIN), within).await;
    assert_total(&answer);
    assert_decided(&h.only_event_since(since).await, "query", "success");
    h.db.close().await;
}

// ---- 3. Request bounds ------------------------------------------------------

#[sqlx::test(migrations = false)]
async fn malformed_bodies_and_out_of_profile_limits_are_refused_without_an_event(
    pool: sqlx::PgPool,
) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    let since = h.head().await;
    let path = "/api/v1/semantic/query";

    for body in [
        "{not json".to_owned(),
        json!({ "model": "metrics/ventas.yaml" }).to_string(),
    ] {
        let refused = h.send(path, Some(&ana), Some(ORIGIN), body).await;
        assert_problem(
            &refused,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
        );
    }
    let paged = |limit: Option<u64>, offset: Option<u64>| {
        let mut body = request(&json!({
            "selections": [dimension("r", "ventas.region"), metric("f", "filas")],
            "order_by": [{ "selection": "r" }]
        }));
        if let Some(limit) = limit {
            body["query"]["limit"] = json!(limit);
        }
        if let Some(offset) = offset {
            body["query"]["offset"] = json!(offset);
        }
        body
    };
    for (limit, offset) in [
        (Some(0), None),
        (Some(10_001), None),
        (Some(10), Some(10_001)),
        (None, Some(10_001)),
    ] {
        let refused = h.query(&ana, &paged(limit, offset)).await;
        assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "limit_invalid");
    }
    assert_eq!(h.events_since(since).await, Vec::new());

    // The bounds themselves are accepted.
    let answer = h.query(&ana, &paged(Some(10_000), None)).await;
    assert_eq!(answer.status, StatusCode::OK, "{:#}", answer.body);
    assert_eq!(answer.body["result"]["blocks"][0]["limit"], 10_000);
    assert_eq!(answer.body["result"]["blocks"][0]["row_count"], 2);
    let answer = h.query(&ana, &paged(Some(10), Some(10_000))).await;
    assert_eq!(answer.status, StatusCode::OK, "{:#}", answer.body);
    assert_eq!(rows(&answer), &json!([]));
    let events = h.events_since(since).await;
    assert_eq!(events.len(), 2, "{events:#?}");
    for event in &events {
        assert_decided(event, "query", "success");
    }
    h.db.close().await;
}

// ---- 4. Non-disclosure (#156) -----------------------------------------------

/// The variant of the model whose dataset the viewer may not see, with a
/// relationship role on it.
fn hidden_role_model() -> String {
    let model = MODEL
        .replacen(
            "  - name: ventas\n",
            "  - name: ventas\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{\"version\": 1, \"sensitivity\": \"restricted\"}'\n",
            1,
        )
        .replacen(
            "      - name: region\n        dimension: {}\n",
            "      - name: region\n        dimension: {}\n        custom_extensions:\n          - vendor_name: bi-platform\n            data: '{\"version\": 1, \"dimension\": \"region\"}'\n",
            1,
        );
    assert_ne!(model, MODEL);
    format!(
        "{model}relationships:\n  - name: self_role\n    from: ventas\n    to: ventas\n    from_columns: [id]\n    to_columns: [id]\n"
    )
}

fn with_role(role: &str) -> Value {
    selections(&json!([
        metric("m", "importe_total"),
        { "kind": "dimension", "id": "r", "name": "region", "roles": [role] }
    ]))
}

#[sqlx::test(migrations = false)]
async fn without_inspection_catalog_failures_answer_alike_and_none_executes(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    h.write(
        "metrics/oculto.yaml",
        "version: [\ndescription: secreto_centinela\n",
    );
    // (case, request, whether a decision on a model was taken)
    let cases = [
        (
            "absent file",
            asked("metrics/no_existe.yaml", "importe_total"),
            false,
        ),
        (
            "invalid file",
            asked("metrics/oculto.yaml", "importe_total"),
            false,
        ),
        (
            "unknown member",
            asked("metrics/ventas.yaml", "centinela_inexistente"),
            false,
        ),
        (
            "member it may not see",
            asked("metrics/ventas.yaml", "reserva_centinela"),
            true,
        ),
        (
            "visible then hidden",
            selections(&json!([
                metric("a", "importe_total"),
                metric("b", "reserva_centinela")
            ])),
            true,
        ),
    ];
    let sentinels = [
        "centinela",
        "secreto",
        "oculto",
        "no_existe",
        "ventas",
        "reserva",
    ];
    let mut answers = Vec::new();
    for (case, body, decided) in &cases {
        let since = h.head().await;
        let answer = h.query(&ana, body).await;
        assert_undisclosed(&answer);
        let text = answer.body.to_string();
        for sentinel in sentinels {
            assert!(!text.contains(sentinel), "{case}: {sentinel} in {text}");
        }
        // Nothing ran: one terminal event, never a success.
        let event = h.only_event_since(since).await;
        if *decided {
            assert_decided(&event, "denial", "denied");
        } else {
            assert_undecided(&event);
        }
        answers.push((*case, without_correlation(&answer), answer.headers));
    }
    let (first, reference, headers) = &answers[0];
    for (case, answer, other) in &answers[1..] {
        assert_eq!(answer, reference, "{case} differs from {first}");
        for name in ["content-type", "cache-control"] {
            assert_eq!(other[name], headers[name], "{case}: {name}");
        }
    }

    // A relationship role towards a dataset the viewer may not see answers
    // like one the model does not declare.
    h.write("metrics/ventas.yaml", &hidden_role_model());
    for (role, decided) in [("self_role", true), ("rol_inexistente", false)] {
        let since = h.head().await;
        let answer = h.query(&ana, &with_role(role)).await;
        assert_undisclosed(&answer);
        assert_eq!(&without_correlation(&answer), reference, "{role}");
        let event = h.only_event_since(since).await;
        if decided {
            assert_decided(&event, "denial", "denied");
        } else {
            assert_undecided(&event);
        }
    }

    // Still allowed: a query the viewer may run.
    h.write("metrics/ventas.yaml", MODEL);
    assert_total(&h.query(&ana, &total()).await);
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn an_inspector_keeps_diagnostics_for_unknown_names_and_is_denied_data(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let inspect = "permit (principal in Role::\"catalog_reader\", action == Action::\"view\", \
                   resource == Model::\"project:catalog\");";
    h.write(
        "policies/project.cedar",
        &format!("{}\n{inspect}", sef_authz::BASE_POLICIES),
    );
    h.write(
        "policies/members.yaml",
        &format!("{MEMBERS}  inspector: [catalog_reader]\n"),
    );
    let inspector = h.login("inspector").await;

    let since = h.head().await;
    let answer = h
        .query(&inspector, &asked("metrics/ventas.yaml", "inexistente"))
        .await;
    assert_problem(&answer, StatusCode::UNPROCESSABLE_ENTITY, "query_invalid");
    assert_eq!(answer.body["diagnostics"][0]["phase"], "binding");
    assert_undecided(&h.only_event_since(since).await);

    let since = h.head().await;
    let answer = h
        .query(
            &inspector,
            &asked("metrics/no_existe.yaml", "importe_total"),
        )
        .await;
    assert_problem(&answer, StatusCode::NOT_FOUND, "model_not_found");
    assert_ne!(answer.body["detail"], UNDISCLOSED_RUN);
    assert_undecided(&h.only_event_since(since).await);

    let since = h.head().await;
    let answer = h.query(&inspector, &total()).await;
    assert_problem(&answer, StatusCode::FORBIDDEN, "semantic_access_denied");
    assert_decided(&h.only_event_since(since).await, "denial", "denied");
    h.db.close().await;
}

// ---- 5–6. Refused before running --------------------------------------------

#[sqlx::test(migrations = false)]
async fn a_masked_metric_is_refused_with_a_decided_failure_event_then_queries_recover(
    pool: sqlx::PgPool,
) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    let since = h.head().await;
    let answer = h
        .query(&ana, &selections(&json!([metric("m", "margen")])))
        .await;
    assert_problem(
        &answer,
        StatusCode::UNPROCESSABLE_ENTITY,
        "masked_unsupported",
    );
    let event = h.only_event_since(since).await;
    assert_decided(&event, "query", "failure");
    assert_eq!(
        event.shape,
        Some(json!({ "dimensions": 0, "measures": 1, "filters": 0, "limit": 0 }))
    );
    let since = h.head().await;
    assert_total(&h.query(&ana, &total()).await);
    assert_decided(&h.only_event_since(since).await, "query", "success");
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn an_ungoverned_day_bucket_is_refused_with_a_decided_failure_event_then_queries_recover(
    pool: sqlx::PgPool,
) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    let since = h.head().await;
    let day = request(&json!({
        "selections": [
            { "kind": "time_dimension", "id": "d", "name": "ventas.instante", "grain": "day",
              "timezone": "UTC" },
            metric("f", "filas")
        ],
        "order_by": [{ "selection": "d" }]
    }));
    let answer = h.query(&ana, &day).await;
    assert_problem(
        &answer,
        StatusCode::UNPROCESSABLE_ENTITY,
        "temporal_unsupported",
    );
    assert_decided(&h.only_event_since(since).await, "query", "failure");
    let since = h.head().await;
    assert_regions(&h.query(&ana, &by_region()).await);
    assert_decided(&h.only_event_since(since).await, "query", "success");
    h.db.close().await;
}

// ---- 7–8. The audit decides the answer --------------------------------------

#[sqlx::test(migrations = false)]
async fn a_failing_audit_append_answers_nothing_and_the_retry_records_once(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    sqlx::query(AUDIT_FAILS).execute(&h.pool).await.unwrap();
    let since = h.head().await;
    // An answer and a denial alike: neither leaves without its event.
    for body in [total(), asked("metrics/ventas.yaml", "reserva_centinela")] {
        let answer = h.query(&ana, &body).await;
        assert_problem(
            &answer,
            StatusCode::SERVICE_UNAVAILABLE,
            "audit_unavailable",
        );
    }
    assert_eq!(h.events_since(since).await, Vec::new());
    sqlx::query("ALTER TABLE bi_meta.audit_events DROP CONSTRAINT append_fails")
        .execute(&h.pool)
        .await
        .unwrap();
    assert_total(&h.query(&ana, &total()).await);
    assert_decided(&h.only_event_since(since).await, "query", "success");
    h.db.close().await;
}

/// Waits until no other backend of this database waits on a lock.
async fn no_lock_waiters(pool: &sqlx::PgPool) {
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

#[sqlx::test(migrations = false)]
async fn a_held_audit_head_ends_the_query_within_its_timeout_and_leaves_no_event(
    pool: sqlx::PgPool,
) {
    let timeout = Duration::from_millis(2_000);
    let h = harness_with(pool, analytics(RuntimeConfig::in_memory(2)).await, timeout).await;
    let ana = h.login("ana").await;
    let since = h.head().await;
    let mut holder = h.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE bi_meta.audit_head IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *holder)
        .await
        .unwrap();

    let started = Instant::now();
    let answer = h.query(&ana, &total()).await;
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

    assert_total(&h.query(&ana, &total()).await);
    assert_decided(&h.only_event_since(since).await, "query", "success");
    h.db.close().await;
}

// ---- 9. Deadline ------------------------------------------------------------

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
async fn a_query_that_cannot_start_times_out_with_its_event_committed_within_the_timeout(
    pool: sqlx::PgPool,
) {
    let timeout = Duration::from_millis(2_000);
    let single = analytics(RuntimeConfig {
        queue_capacity: 4,
        ..RuntimeConfig::in_memory(1)
    })
    .await;
    let h = harness_with(pool, single, timeout).await;
    let ana = h.login("ana").await;
    let runtime = h.analytics.runtime().unwrap();
    let since = h.head().await;
    let (release, held) = hold(&runtime).await;

    let started = Instant::now();
    let answer = h.query(&ana, &by_region()).await;
    let elapsed = started.elapsed();
    assert_problem(&answer, StatusCode::GATEWAY_TIMEOUT, "semantic_timeout");
    // Execution used its share; the confirmed event fit in the rest.
    let execution = Profile::new(Budgets { timeout, ..INITIAL })
        .unwrap()
        .execution_timeout();
    assert!(elapsed >= execution, "{elapsed:?} before {execution:?}");
    assert!(
        elapsed < timeout + Duration::from_millis(500),
        "{elapsed:?} for a {timeout:?} timeout"
    );
    // The preparation never reached the worker: nothing was decided. The
    // shape is the query as bounded, with the limit it would have applied.
    let event = h.only_event_since(since).await;
    assert_undecided(&event);
    assert_eq!(
        event.shape,
        Some(json!({ "dimensions": 1, "measures": 2, "filters": 0, "limit": 1000 }))
    );

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
    assert_total(&h.query(&ana, &total()).await);
    assert_decided(&h.only_event_since(since).await, "query", "success");
    h.db.close().await;
}

// ---- 10. Concurrency --------------------------------------------------------

#[sqlx::test(migrations = false)]
async fn two_viewers_querying_at_once_each_get_their_own_exact_answer(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    let bea = h.login("bea").await;
    let since = h.head().await;
    let (total, by_region) = (total(), by_region());
    for round in 0..4 {
        // Alternate who asks what, so neither answer can follow its user.
        let (first, second) = if round % 2 == 0 {
            (&ana, &bea)
        } else {
            (&bea, &ana)
        };
        let (totals, regions) = tokio::join!(h.query(first, &total), h.query(second, &by_region));
        assert_total(&totals);
        assert_regions(&regions);
    }
    let events = h.events_since(since).await;
    assert_eq!(events.len(), 8, "{events:#?}");
    for event in &events {
        assert_decided(event, "query", "success");
    }
    for user in ["ana", "bea"] {
        let mine: Vec<_> = events.iter().filter(|e| e.actor == user).collect();
        assert_eq!(mine.len(), 4, "{user}: {events:#?}");
        let regions = mine
            .iter()
            .filter(|e| e.shape.as_ref().is_some_and(|s| s["dimensions"] == 1))
            .count();
        assert_eq!(regions, 2, "{user}: {events:#?}");
    }
    h.db.close().await;
}

// ---- 11. Explain is unchanged -----------------------------------------------

#[sqlx::test(migrations = false)]
async fn explain_still_explains_the_same_body_without_executing(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    let since = h.head().await;
    let answer = h.explain(&ana, &by_region()).await;
    assert_eq!(answer.status, StatusCode::OK, "{:#}", answer.body);
    assert_eq!(answer.headers["cache-control"], "no-store");
    assert_eq!(answer.body["decision"]["reason"], "permitted");
    assert_eq!(answer.body["decision"]["roles"], json!(["viewer"]));
    assert!(
        answer.body["receipt"]["id"]
            .as_str()
            .unwrap()
            .starts_with("receipt:")
    );
    assert!(answer.body.get("result").is_none(), "{:#}", answer.body);
    let event = h.only_event_since(since).await;
    assert_decided(&event, "query", "success");
    // Explain records the request as written: no limit was added.
    assert_eq!(
        event.shape,
        Some(json!({ "dimensions": 1, "measures": 2, "filters": 0, "limit": 0 }))
    );
    h.db.close().await;
}
