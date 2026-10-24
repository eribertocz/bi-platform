//! The single semantic path (#98) end to end: real sessions on PostgreSQL,
//! real DuckDB catalog, project policies and members from files, and the
//! audit chain.
#![cfg(all(feature = "postgres-tests", feature = "dev-auth"))]

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use bi_server::metadata::MetadataMode;
use bi_server::{AppState, analytics::Analytics, db::Database, models::Models};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:8080";

const MODEL: &str = r#"
version: '0.2.0.dev0'
name: Ventas
datasets:
  - name: sales
    source: lake.main.sales
    primary_key: [order_id]
    fields:
      - name: order_id
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: order_id
      - name: region
        label: Región
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: region
        dimension: {}
      - name: amount
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: amount
      - name: quantity
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: quantity
metrics:
  - name: importe
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(sales.amount)
  - name: margen
    custom_extensions:
      - vendor_name: bi-platform
        data: '{"version": 1, "sensitivity": "confidential"}'
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(sales.amount)
  - name: coste
    custom_extensions:
      - vendor_name: bi-platform
        data: '{"version": 1, "sensitivity": "restricted"}'
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(sales.quantity)
"#;

const MEMBERS: &str = "
version: 1
users:
  lector: [viewer]
  analista: [analyst]
  modelador: [modeler]
";

fn project() -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    std::fs::create_dir(p.path().join("metrics")).unwrap();
    std::fs::create_dir(p.path().join("policies")).unwrap();
    std::fs::write(p.path().join("metrics/ventas.yaml"), MODEL).unwrap();
    std::fs::write(p.path().join("policies/members.yaml"), MEMBERS).unwrap();
    p
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
                 NULL, region VARCHAR, quantity INTEGER, amount DECIMAL(18,2));",
            )
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
    auth: bi_server::auth::AuthConfig,
}

async fn harness(pool: sqlx::PgPool) -> Harness {
    let project = project();
    let db = Database::start(&pool.connect_options(), MetadataMode::Development);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(30), db.wait_until_ready())
            .await
            .unwrap()
    );
    let auth = bi_server::auth::AuthConfig::from_lookup(&|key| match key {
        "BI_PUBLIC_ORIGIN" => Some(ORIGIN.into()),
        "BI_DEV_USERS" => Some(
            r#"[{"id":"lector","display_name":"Lector"},{"id":"analista","display_name":"Analista"},{"id":"sinrol","display_name":"Sin rol"},{"id":"modelador","display_name":"Modelador"},{"id":"inspector","display_name":"Inspector"}]"#
                .into(),
        ),
        _ => None,
    })
    .unwrap();
    let app = bi_server::router(AppState {
        auth: auth.clone(),
        analytics: analytics().await,
        models: Models::new(Some(project.path().into())),
        ..AppState::new(db.clone())
    });
    Harness {
        app,
        pool,
        project,
        db,
        auth,
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

    async fn explain(&self, cookie: Option<&str>, body: &Value) -> (StatusCode, Value) {
        let mut request = Request::post("/api/v1/semantic/explain")
            .header("origin", ORIGIN)
            .header("content-type", "application/json");
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// The public headers of an explanation, with a correlation id.
    async fn explain_headers(&self, cookie: &str, body: &Value) -> axum::http::HeaderMap {
        let request = Request::post("/api/v1/semantic/explain")
            .header("origin", ORIGIN)
            .header("content-type", "application/json")
            .header("cookie", cookie)
            .header("x-request-id", "req-semantic-test");
        self.app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
            .headers()
            .clone()
    }

    async fn receipt(&self, cookie: &str, body: Value) -> Value {
        let (status, response) = self.explain(Some(cookie), &body).await;
        assert_eq!(status, StatusCode::OK, "{response:#}");
        response["receipt"].clone()
    }

    fn write(&self, file: &str, text: &str) {
        std::fs::write(self.project.path().join(file), text).unwrap();
    }

    async fn audit(&self) -> Vec<(String, String)> {
        sqlx::query_as::<_, (String, String)>(
            "SELECT payload::jsonb->>'action', payload::jsonb->>'result' FROM \
             bi_meta.audit_events WHERE payload::jsonb->>'resource_type' = 'semantic_model' \
             ORDER BY sequence",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }
}

/// The only detail a session without inspection gets for a failure that
/// depends on the catalog (#156).
const UNDISCLOSED: &str = "No se puede explicar la consulta solicitada.";

/// The neutral answer: nothing but its code, its fixed detail and the
/// correlation.
fn assert_undisclosed(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:#}");
    let mut fields: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        ["code", "detail", "request_id", "status", "title", "type"],
        "{body:#}"
    );
    assert_eq!(body["code"], "model_not_found");
    assert_eq!(body["detail"], UNDISCLOSED);
}

fn metric(id: &str, name: &str) -> Value {
    json!({ "kind": "metric", "id": id, "name": name })
}

fn query(selections: &Value) -> Value {
    json!({ "model": "metrics/ventas.yaml", "query": { "selections": selections } })
}

fn region() -> Value {
    json!({ "kind": "dimension", "id": "r", "name": "sales.region" })
}

#[sqlx::test(migrations = false)]
async fn a_viewer_sees_open_metrics_masked_confidential_and_never_restricted(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let lector = h.login("lector").await;
    let (status, open) = h
        .explain(
            Some(&lector),
            &query(&json!([region(), metric("m", "importe")])),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{open:#}");
    assert_eq!(open["decision"]["reason"], "permitted");
    assert_eq!(open["decision"]["masked"], json!([]));
    assert_eq!(open["decision"]["roles"], json!(["viewer"]));
    assert!(
        open["plan"]["explanation"]
            .as_str()
            .unwrap()
            .contains("contract")
    );
    assert!(open["plan"]["obligations"].as_u64().is_some());
    for part in ["query", "model", "plan", "decision", "policies", "context"] {
        assert!(
            open["receipt"][part]
                .as_str()
                .is_some_and(|v| !v.is_empty()),
            "{part}"
        );
    }
    let (status, masked) = h
        .explain(
            Some(&lector),
            &query(&json!([region(), metric("c", "margen")])),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{masked:#}");
    assert_eq!(masked["decision"]["masked"], json!(["c"]));
    // A metric the viewer may not even see is not acknowledged (#156): the
    // same answer as a name the model does not have.
    let (status, denied) = h
        .explain(Some(&lector), &query(&json!([metric("x", "coste")])))
        .await;
    assert_undisclosed(status, &denied);
    // The analyst sees the confidential metric unmasked.
    let analista = h.login("analista").await;
    let (status, analyst) = h
        .explain(
            Some(&analista),
            &query(&json!([region(), metric("c", "margen")])),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{analyst:#}");
    assert_eq!(analyst["decision"]["masked"], json!([]));
    // One audit event per explanation, denials included.
    assert_eq!(
        h.audit().await,
        vec![
            ("query".to_owned(), "success".to_owned()),
            ("query".to_owned(), "success".to_owned()),
            ("denial".to_owned(), "denied".to_owned()),
            ("query".to_owned(), "success".to_owned()),
        ]
    );
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_masked_metric_can_be_shown_but_not_filtered_or_ordered(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let lector = h.login("lector").await;
    let mut ordered = query(&json!([region(), metric("c", "margen")]));
    ordered["query"]["order_by"] = json!([{ "selection": "c", "descending": true }]);
    let (status, body) = h.explain(Some(&lector), &ordered).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:#}");
    assert_eq!(body["denied"]["usage"], "order");
    let mut filtered = query(&json!([region(), metric("c", "margen")]));
    filtered["query"]["having"] = json!({
        "op": "compare",
        "target": { "kind": "metric", "selection": "c" },
        "comparison": ">",
        "value": { "type": "number", "value": "10" }
    });
    let (status, body) = h.explain(Some(&lector), &filtered).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:#}");
    assert_eq!(body["denied"]["usage"], "filter");
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn malformed_unknown_and_incorrect_queries_are_rejected_with_reasons(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let analista = h.login("analista").await;
    // Not the request format.
    let (status, body) = h
        .explain(Some(&analista), &json!({ "model": "metrics/ventas.yaml" }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
    assert_eq!(body["code"], "invalid_request");
    // A name the model does not have: neutral without inspection of the
    // catalog (#156), with its reasons for whoever inspects it.
    let unknown = query(&json!([metric("m", "inexistente")]));
    let (status, body) = h.explain(Some(&analista), &unknown).await;
    assert_undisclosed(status, &body);
    let modelador = h.login("modelador").await;
    let (status, body) = h.explain(Some(&modelador), &unknown).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
    assert_eq!(body["code"], "query_invalid");
    assert_eq!(body["diagnostics"][0]["phase"], "binding");
    assert!(
        body["diagnostics"][0]["path"]
            .as_str()
            .unwrap()
            .starts_with("/query")
    );
    // A metric in the pre-aggregate filter: not a row predicate.
    let mut incorrect = query(&json!([region(), metric("m", "importe")]));
    incorrect["query"]["filter"] = json!({
        "op": "compare",
        "target": { "kind": "metric", "selection": "m" },
        "comparison": ">",
        "value": { "type": "number", "value": "1" }
    });
    // Binding resolves the model's names, so without inspection even a
    // misplaced metric answers neutrally: telling it apart would tell
    // which names exist (#156).
    let (status, body) = h.explain(Some(&analista), &incorrect).await;
    assert_undisclosed(status, &body);
    let (status, body) = h.explain(Some(&modelador), &incorrect).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
    assert!(
        !body["diagnostics"].as_array().unwrap().is_empty(),
        "{body:#}"
    );
    // A model that is not in the project.
    let mut missing = query(&json!([metric("m", "importe")]));
    missing["model"] = json!("metrics/otro.yaml");
    let (status, body) = h.explain(Some(&analista), &missing).await;
    assert_undisclosed(status, &body);
    let (status, body) = h.explain(Some(&modelador), &missing).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:#}");
    assert_eq!(body["code"], "model_not_found");
    assert_ne!(body["detail"], UNDISCLOSED);
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn identity_comes_only_from_the_session(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let body = query(&json!([metric("m", "importe")]));
    let (status, _) = h.explain(None, &body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A user the project grants no role is denied by default, without
    // acknowledging what the model holds (#156)…
    let sinrol = h.login("sinrol").await;
    let (status, denied) = h.explain(Some(&sinrol), &body).await;
    assert_undisclosed(status, &denied);
    assert_eq!(h.audit().await.last().unwrap().0, "denial");
    // …and cannot claim one: a principal or roles in the body are refused,
    // never read.
    for claim in [
        json!({ "roles": ["admin"] }),
        json!({ "principal": "analista" }),
    ] {
        let mut forged = body.clone();
        for (key, value) in claim.as_object().unwrap() {
            forged[key] = value.clone();
        }
        let (status, refused) = h.explain(Some(&sinrol), &forged).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused:#}");
        assert_eq!(refused["code"], "invalid_request");
    }
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn invalid_policies_or_members_close_the_path(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let analista = h.login("analista").await;
    let body = query(&json!([metric("m", "importe")]));
    h.write("policies/rota.cedar", "permit (principal, action, resource");
    let (status, closed) = h.explain(Some(&analista), &body).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{closed:#}");
    assert_eq!(closed["code"], "policies_invalid");
    std::fs::remove_file(h.project.path().join("policies/rota.cedar")).unwrap();
    h.write(
        "policies/members.yaml",
        "version: 1\nusers:\n  analista: [Admin]\n",
    );
    let (status, closed) = h.explain(Some(&analista), &body).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{closed:#}");
    h.write("policies/members.yaml", MEMBERS);
    let (status, open) = h.explain(Some(&analista), &body).await;
    assert_eq!(status, StatusCode::OK, "{open:#}");
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn the_receipt_follows_query_model_and_policies_but_not_labels(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let analista = h.login("analista").await;
    let receipt = |body| h.receipt(&analista, body);
    let base = receipt(query(&json!([region(), metric("m", "importe")]))).await;
    let again = receipt(query(&json!([region(), metric("m", "importe")]))).await;
    assert_eq!(base, again, "deterministic");
    // An output label is presentation only.
    let labelled = receipt(query(&json!([
        region(),
        { "kind": "metric", "id": "m", "name": "importe", "label": "Importe €" }
    ])))
    .await;
    assert_eq!(base["id"], labelled["id"]);
    // Another query.
    let other = receipt(query(&json!([region(), metric("m", "margen")]))).await;
    assert_ne!(base["id"], other["id"]);
    assert_ne!(base["query"], other["query"]);
    // Another policy set, even one with the same rules.
    h.write(
        "policies/base.cedar",
        &format!("{}\n// copia del proyecto\n", sef_authz::BASE_POLICIES),
    );
    let policies = receipt(query(&json!([region(), metric("m", "importe")]))).await;
    assert_ne!(base["policies"], policies["policies"]);
    assert_ne!(base["id"], policies["id"]);
    // Another model.
    h.write(
        "metrics/ventas.yaml",
        &MODEL.replace("SUM(sales.quantity)", "MAX(sales.quantity)"),
    );
    let model = receipt(query(&json!([region(), metric("m", "importe")]))).await;
    assert_ne!(policies["model"], model["model"]);
    assert_ne!(policies["id"], model["id"]);
    h.db.close().await;
}

/// Baseline for the semantic engine's performance work (#100): the whole
/// path per request, from the session to the audit, with nothing cached.
#[sqlx::test(migrations = false)]
async fn explain_latency(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let analista = h.login("analista").await;
    let body = query(&json!([region(), metric("m", "importe")]));
    let mut times = Vec::new();
    for _ in 0..30 {
        let start = std::time::Instant::now();
        let (status, _) = h.explain(Some(&analista), &body).await;
        assert_eq!(status, StatusCode::OK);
        times.push(start.elapsed());
    }
    times.sort();
    println!(
        "explain end to end: p50 {:.1} ms, p95 {:.1} ms (30 runs)",
        times[15].as_secs_f64() * 1e3,
        times[28].as_secs_f64() * 1e3
    );
    h.db.close().await;
}

// ---- From the review of #110: lineage and names never leak ------------------

fn with_derived(extra: &str) -> String {
    format!("{MODEL}{extra}")
}

const DOUBLE: &str = "  - name: doble\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: margen + margen\n";
const QUADRUPLE: &str = "  - name: cuadruple\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: doble + doble\n";

#[sqlx::test(migrations = false)]
async fn showing_a_masked_metric_never_waives_deriving_from_it(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    h.write(
        "metrics/ventas.yaml",
        &with_derived(&format!("{DOUBLE}{QUADRUPLE}")),
    );
    let lector = h.login("lector").await;
    // Deriving from the confidential metric: denied, without naming it.
    let (status, denied) = h
        .explain(Some(&lector), &query(&json!([metric("d", "doble")])))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied:#}");
    assert_eq!(denied["denied"]["usage"], "lineage", "{denied:#}");
    assert!(denied["denied"].get("name").is_none(), "{denied:#}");
    // Showing it masked next to the derived one changes nothing…
    let (status, body) = h
        .explain(
            Some(&lector),
            &query(&json!([metric("c", "margen"), metric("d", "doble")])),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "lineage bypass: {body:#}");
    // …also through a chain of derivations.
    let (status, body) = h
        .explain(
            Some(&lector),
            &query(&json!([metric("c", "margen"), metric("q", "cuadruple")])),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "transitive bypass: {body:#}");
    // The analyst may derive from it.
    let analista = h.login("analista").await;
    let (status, body) = h
        .explain(
            Some(&analista),
            &query(&json!([metric("c", "margen"), metric("q", "cuadruple")])),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body:#}");
    h.db.close().await;
}

/// Two ratios, one restricted and one confidential: each splits into the
/// components `<ratio>.m1` (the sum) and `<ratio>.m2` (the count).
const RATIOS: &str = "  - name: coste_ratio\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{\"version\": 1, \"sensitivity\": \"restricted\"}'\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: CAST(SUM(sales.amount) / COUNT(sales.order_id) AS DECIMAL(18, 4))\n  - name: margen_ratio\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{\"version\": 1, \"sensitivity\": \"confidential\"}'\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: CAST(SUM(sales.amount) / COUNT(sales.order_id) AS DECIMAL(18, 4))\n";

#[sqlx::test(migrations = false)]
async fn a_component_is_never_more_visible_than_its_metric(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    h.write("metrics/ventas.yaml", &with_derived(RATIOS));
    let lector = h.login("lector").await;
    let analista = h.login("analista").await;
    let modelador = h.login("modelador").await;
    for name in ["coste_ratio", "coste_ratio.m1", "coste_ratio.m2"] {
        let asked = query(&json!([metric("x", name)]));
        // The viewer never sees the restricted ratio nor any component.
        let (status, body) = h.explain(Some(&lector), &asked).await;
        assert_undisclosed(status, &body);
        // The modeler sees all of them, unmasked.
        let (status, body) = h.explain(Some(&modelador), &asked).await;
        assert_eq!(status, StatusCode::OK, "{name}: {body:#}");
        assert_eq!(body["decision"]["masked"], json!([]), "{name}");
    }
    for name in ["margen_ratio", "margen_ratio.m1", "margen_ratio.m2"] {
        // The analyst sees the confidential ratio and its components unmasked.
        let (status, body) = h
            .explain(Some(&analista), &query(&json!([metric("x", name)])))
            .await;
        assert_eq!(status, StatusCode::OK, "{name}: {body:#}");
        assert_eq!(body["decision"]["masked"], json!([]), "{name}");
    }
    // Whoever sees a ratio only masked sees each of its components masked
    // too, never their values.
    for (cookie, name) in [
        (&analista, "coste_ratio.m1"),
        (&analista, "coste_ratio.m2"),
        (&lector, "margen_ratio.m1"),
        (&lector, "margen_ratio.m2"),
    ] {
        let (status, body) = h
            .explain(Some(cookie), &query(&json!([metric("x", name)])))
            .await;
        assert_eq!(status, StatusCode::OK, "{name}: {body:#}");
        assert_eq!(body["decision"]["masked"], json!(["x"]), "{name}");
    }
    // The masked ratio derives from components it may not derive from:
    // denied for that lineage, never named (it fails closed).
    for (cookie, name) in [(&analista, "coste_ratio"), (&lector, "margen_ratio")] {
        let (status, body) = h
            .explain(Some(cookie), &query(&json!([metric("x", name)])))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{name}: {body:#}");
        assert_eq!(body["denied"]["usage"], "lineage", "{name}: {body:#}");
        assert!(body["denied"].get("name").is_none(), "{name}: {body:#}");
    }
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn an_audit_failure_closes_both_answers_and_denials(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let lector = h.login("lector").await;
    // An isolated database per test: nothing else is touched. Only the write
    // fails; schema and role still validate, so whichever connection the
    // pool hands out, the failure is the append itself.
    sqlx::query(
        "ALTER TABLE bi_meta.audit_events ADD CONSTRAINT append_fails CHECK (false) NOT VALID",
    )
    .execute(&h.pool)
    .await
    .unwrap();
    for name in ["importe", "coste"] {
        let (status, body) = h
            .explain(Some(&lector), &query(&json!([metric("m", name)])))
            .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body:#}");
        assert_eq!(body["code"], "audit_unavailable", "{body:#}");
    }
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn requests_over_the_bounds_are_refused(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let lector = h.login("lector").await;
    let selections: Vec<_> = (0..65)
        .map(|n| metric(&format!("m{n}"), "importe"))
        .collect();
    let mut nodes = query(&json!([metric("m", "importe")]));
    nodes["query"]["filter"] = json!({ "op": "and", "filters": (0..256)
        .map(|_| json!({ "op": "is_null", "target": { "kind": "dimension", "name": "sales.region" } }))
        .collect::<Vec<_>>() });
    for body in [
        query(&json!(selections)),
        query(&json!([metric("m", &"x".repeat(257))])),
        nodes,
    ] {
        let (status, body) = h.explain(Some(&lector), &body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
        assert_eq!(body["code"], "invalid_request", "{body:#}");
    }
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn authorization_comes_before_correctness(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let lector = h.login("lector").await;
    let having = |name: &str| {
        let mut body = query(&json!([metric("m", name)]));
        body["query"]["having"] = json!({ "op": "compare",
            "target": { "kind": "metric", "selection": "m" }, "comparison": ">",
            "value": { "type": "number", "value": "1" } });
        body
    };
    // Visible masked: its refused use is named before any correctness check.
    let (status, body) = h.explain(Some(&lector), &having("margen")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:#}");
    assert_eq!(body["denied"]["name"], "margen");
    // Not visible at all: still refused first, without acknowledging it.
    let (status, body) = h.explain(Some(&lector), &having("coste")).await;
    assert_undisclosed(status, &body);
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn model_errors_never_disclose_objects_the_request_did_not_name(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    h.write(
        "metrics/ventas.yaml",
        &MODEL.replace("SUM(sales.quantity)", "SUM(sales.secret_internal)"),
    );
    for user in ["sinrol", "lector"] {
        let cookie = h.login(user).await;
        let (status, body) = h
            .explain(Some(&cookie), &query(&json!([metric("m", "importe")])))
            .await;
        assert_undisclosed(status, &body);
    }
    {
        let user = "modelador";
        let cookie = h.login(user).await;
        let (status, body) = h
            .explain(Some(&cookie), &query(&json!([metric("m", "importe")])))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
        assert_eq!(body["code"], "model_invalid", "{body:#}");
        assert!(
            !body["diagnostics"].as_array().unwrap().is_empty(),
            "{body:#}"
        );
        assert!(
            !body.to_string().contains("secret_internal"),
            "{user}: unrequested name disclosed: {body:#}"
        );
    }
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_role_to_a_hidden_dataset_is_never_acknowledged(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let model = MODEL
        .replace(
            "  - name: sales\n",
            "  - name: sales\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{\"version\": 1, \"sensitivity\": \"restricted\"}'\n",
        )
        .replace(
            "        dimension: {}",
            "        dimension: {}\n        custom_extensions:\n          - vendor_name: bi-platform\n            data: '{\"version\": 1, \"dimension\": \"region\"}'",
        );
    h.write(
        "metrics/ventas.yaml",
        &format!(
            "{model}\nrelationships:\n  - name: self_role\n    from: sales\n    to: sales\n    from_columns: [order_id]\n    to_columns: [order_id]\n"
        ),
    );
    let with_role = |role: &str| {
        query(&json!([
            metric("m", "importe"),
            { "kind": "dimension", "id": "r", "name": "region", "roles": [role] }
        ]))
    };
    // A role the request writes is a reference of the request: one that
    // leads to a dataset the viewer may not see answers like one the model
    // does not declare (#156).
    let lector = h.login("lector").await;
    let (status, hidden) = h.explain(Some(&lector), &with_role("self_role")).await;
    assert_undisclosed(status, &hidden);
    let (status, unknown) = h
        .explain(Some(&lector), &with_role("rol_inexistente"))
        .await;
    assert_undisclosed(status, &unknown);
    assert_eq!(without_correlation(hidden), without_correlation(unknown));
    // Whoever inspects the catalog learns the use is refused, still without
    // the dataset's name.
    let modelador = h.login("modelador").await;
    h.write(
        "policies/project.cedar",
        &format!(
            "{}\nforbid (principal, action, resource is Dataset);",
            sef_authz::BASE_POLICIES
        ),
    );
    let (status, body) = h.explain(Some(&modelador), &with_role("self_role")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:#}");
    assert_eq!(body["denied"]["kind"], "dataset", "{body:#}");
    assert!(
        body["denied"].get("name").is_none(),
        "unrequested dataset disclosed: {body:#}"
    );
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_correctness_message_never_quotes_an_unrequested_object(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    // Summing `importe` across regions is forbidden: the message would
    // quote the `region` axis, which this request does not name.
    let model = MODEL
        .replace(
            "  - name: importe\n",
            "  - name: importe\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{\"version\":1,\"aggregation\":{\"class\":\"semi_additive\",\"axes\":[{\"dimension\":\"region\",\"behavior\":\"forbidden\"}]}}'\n",
        )
        .replace(
            "        dimension: {}",
            "        dimension: {}\n        custom_extensions:\n          - vendor_name: bi-platform\n            data: '{\"version\":1,\"dimension\":\"region\"}'",
        );
    h.write("metrics/ventas.yaml", &model);
    let lector = h.login("lector").await;
    let (status, body) = h
        .explain(Some(&lector), &query(&json!([metric("m", "importe")])))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
    assert_eq!(body["code"], "query_incorrect", "{body:#}");
    assert_eq!(
        body["diagnostics"][0]["code"], "ILLEGAL_REAGGREGATION",
        "{body:#}"
    );
    assert!(
        !body.to_string().contains("region"),
        "unrequested axis leaked: {body:#}"
    );
    h.db.close().await;
}

/// The same request and session, whatever the project holds: the bodies
/// differ only in their correlation.
fn without_correlation(mut body: Value) -> Value {
    body.as_object_mut().unwrap().remove("request_id");
    body
}

/// The context fingerprint is an HMAC under this instance's key and its
/// project (#204), and the receipt ID covers it: both differ on every run.
/// Their form is fixed here; their values, in `semantic::context`.
fn per_instance(mut body: Value) -> Value {
    let hex = |text: &str, prefix: &str| {
        text.strip_prefix(prefix)
            .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
    };
    if let Some(receipt) = body.get_mut("receipt") {
        for (field, prefix) in [("context", "ctx-hmac-sha256-v1:"), ("id", "receipt:")] {
            let value = receipt[field].as_str().unwrap();
            assert!(hex(value, prefix), "{field}: {value}");
            receipt[field] = json!(format!("{prefix}<per instance>"));
        }
    }
    body
}

/// An explanation of `name` asked about `model`.
fn asked(model: &str, name: &str) -> Value {
    let mut body = query(&json!([metric("m", name)]));
    body["model"] = json!(model);
    body
}

#[sqlx::test(migrations = false)]
async fn without_inspection_catalog_failures_are_indistinguishable(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let lector = h.login("lector").await;
    h.write(
        "metrics/oculto.yaml",
        "version: [\ndescription: descripcion_centinela\n",
    );
    let cases = [
        ("absent file", asked("metrics/no_existe.yaml", "importe")),
        ("invalid file", asked("metrics/oculto.yaml", "importe")),
        (
            "unknown member",
            asked("metrics/ventas.yaml", "centinela_inexistente"),
        ),
        (
            "member it may not see",
            asked("metrics/ventas.yaml", "coste"),
        ),
        (
            "visible then hidden",
            query(&json!([metric("a", "importe"), metric("b", "coste")])),
        ),
        (
            "hidden then visible",
            query(&json!([metric("b", "coste"), metric("a", "importe")])),
        ),
    ];
    let mut answers = Vec::new();
    for (case, body) in &cases {
        let (status, answer) = h.explain(Some(&lector), body).await;
        assert_undisclosed(status, &answer);
        let text = answer.to_string();
        for sentinel in ["centinela", "oculto", "coste", "no_existe", "ventas"] {
            assert!(!text.contains(sentinel), "{case}: {sentinel} in {text}");
        }
        answers.push((*case, without_correlation(answer)));
    }
    for (case, answer) in &answers[1..] {
        assert_eq!(
            answer, &answers[0].1,
            "{case} differs from {}",
            answers[0].0
        );
    }
    // The same public headers as well.
    // The same public headers as well.
    for (case, body) in &cases {
        let headers = h.explain_headers(&lector, body).await;
        assert_eq!(
            headers["content-type"], "application/problem+json",
            "{case}"
        );
        assert_eq!(headers["cache-control"], "no-store", "{case}");
        assert!(!headers["x-request-id"].is_empty(), "{case}");
    }
    // The same requests on a project without that file: identical.
    std::fs::remove_file(h.project.path().join("metrics/oculto.yaml")).unwrap();
    for (case, body) in &cases {
        let (status, answer) = h.explain(Some(&lector), body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{case}");
        assert_eq!(without_correlation(answer), answers[0].1, "{case}");
    }
    // Still allowed: a query the viewer may run, with its receipt.
    let (status, body) = h
        .explain(Some(&lector), &query(&json!([metric("m", "importe")])))
        .await;
    assert_eq!(status, StatusCode::OK, "{body:#}");
    assert!(body["receipt"]["decision"].as_str().is_some());
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn generated_names_never_tell_hidden_from_absent(pool: sqlx::PgPool) {
    use proptest::prelude::*;
    use proptest::strategy::ValueTree as _;
    use proptest::test_runner::TestRunner;

    let h = harness(pool).await;
    let lector = h.login("lector").await;
    let (_, reference) = h
        .explain(Some(&lector), &asked("metrics/ventas.yaml", "coste"))
        .await;
    let reference = without_correlation(reference);
    let name = prop_oneof![
        Just("coste".to_owned()),
        Just("COSTE".to_owned()),
        Just("sales.order_id".to_owned()),
        "[a-z_]{1,24}",
    ];
    let file = prop_oneof![
        Just("metrics/ventas.yaml".to_owned()),
        "metrics/[a-z]{1,12}[.](yaml|yml|json)",
    ];
    let visible = ["importe", "margen", "region", "sales.region"];
    let mut runner = TestRunner::deterministic();
    let mut sent = 0;
    while sent < 32 {
        let (name, file) = (name.clone(), file.clone())
            .new_tree(&mut runner)
            .unwrap()
            .current();
        if file == "metrics/ventas.yaml" && visible.contains(&name.as_str()) {
            continue;
        }
        let (status, answer) = h.explain(Some(&lector), &asked(&file, &name)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{file} {name}: {answer:#}");
        assert_eq!(without_correlation(answer), reference, "{file} {name}");
        sent += 1;
    }
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_forbid_or_an_inspection_grant_proves_nothing_about_data(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    // The analyst may see the restricted metric masked, until a forbid.
    let analista = h.login("analista").await;
    let restricted = asked("metrics/ventas.yaml", "coste");
    let (status, masked) = h.explain(Some(&analista), &restricted).await;
    assert_eq!(status, StatusCode::OK, "{masked:#}");
    let forbid = "forbid (principal, action, resource is Metric) \
                  when { resource.classification == \"restricted\" };";
    h.write(
        "policies/project.cedar",
        &format!("{}\n{forbid}", sef_authz::BASE_POLICIES),
    );
    let (status, body) = h.explain(Some(&analista), &restricted).await;
    assert_undisclosed(status, &body);
    // Inspecting the catalog only: full diagnostics, never data.
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
    let (status, body) = h
        .explain(Some(&inspector), &asked("metrics/ventas.yaml", "importe"))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:#}");
    assert_eq!(body["code"], "semantic_access_denied");
    assert_eq!(body["denied"]["name"], "importe");
    let (status, body) = h
        .explain(
            Some(&inspector),
            &asked("metrics/ventas.yaml", "inexistente"),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
    assert_eq!(body["code"], "query_invalid");
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn operational_failures_never_tell_whether_a_file_exists(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let lector = h.login("lector").await;
    let existing = asked("metrics/ventas.yaml", "importe");
    let absent = asked("metrics/no_existe.yaml", "importe");
    // Invalid policies: the same answer for an existing and an absent file.
    h.write("policies/rota.cedar", "permit (principal, action, resource");
    let (status, with_file) = h.explain(Some(&lector), &existing).await;
    let (other, without_file) = h.explain(Some(&lector), &absent).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{with_file:#}");
    assert_eq!(other, status);
    assert_eq!(with_file["code"], "policies_invalid");
    assert_eq!(
        without_correlation(with_file),
        without_correlation(without_file)
    );
    std::fs::remove_file(h.project.path().join("policies/rota.cedar")).unwrap();
    // The engine down: the same, whether the file exists or not.
    let down = bi_server::router(AppState {
        auth: h.auth.clone(),
        analytics: Analytics::disabled(),
        models: Models::new(Some(h.project.path().into())),
        ..AppState::new(h.db.clone())
    });
    let mut answers = Vec::new();
    for body in [&existing, &absent] {
        let response = down
            .clone()
            .oneshot(
                Request::post("/api/v1/semantic/explain")
                    .header("origin", ORIGIN)
                    .header("cookie", &lector)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let answer: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(answer["code"], "analytics_unavailable");
        answers.push(without_correlation(answer));
    }
    assert_eq!(answers[0], answers[1]);
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn an_evaluation_error_never_makes_a_member_disclosable(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    // `view` fails to evaluate on the confidential metric, which the viewer
    // could otherwise see masked: like `sef_authz`, never retried as masked.
    let failing = "forbid (principal, action == Action::\"view\", resource is Metric) \
                   when { resource.classification == \"confidential\" && 9223372036854775807 + 1 > 0 };";
    h.write(
        "policies/project.cedar",
        &format!("{}\n{failing}", sef_authz::BASE_POLICIES),
    );
    let lector = h.login("lector").await;
    let mut filtered = query(&json!([metric("c", "margen")]));
    filtered["query"]["having"] = json!({
        "op": "compare",
        "target": { "kind": "metric", "selection": "c" },
        "comparison": ">",
        "value": { "type": "number", "value": "10" }
    });
    let (status, body) = h.explain(Some(&lector), &filtered).await;
    assert_undisclosed(status, &body);
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_visible_refused_use_never_answers_for_a_hidden_one(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    // `region` restricted: hidden from the viewer, who sees `margen` masked.
    h.write(
        "metrics/ventas.yaml",
        &MODEL.replace(
            "        dimension: {}",
            "        dimension: {}\n        custom_extensions:\n          - vendor_name: bi-platform\n            data: '{\"version\": 1, \"sensitivity\": \"restricted\"}'",
        ),
    );
    let lector = h.login("lector").await;
    // A refused filter on the visible `margen`, and a filter on the hidden
    // region: whichever use is refused first, nothing is acknowledged.
    let mut mixed = query(&json!([metric("c", "margen")]));
    mixed["query"]["having"] = json!({
        "op": "compare",
        "target": { "kind": "metric", "selection": "c" },
        "comparison": ">",
        "value": { "type": "number", "value": "10" }
    });
    mixed["query"]["filter"] = json!({
        "op": "is_null",
        "target": { "kind": "dimension", "name": "sales.region" }
    });
    let (status, body) = h.explain(Some(&lector), &mixed).await;
    assert_undisclosed(status, &body);
    // Without the hidden filter, the visible refusal is named.
    mixed["query"].as_object_mut().unwrap().remove("filter");
    let (status, body) = h.explain(Some(&lector), &mixed).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:#}");
    assert_eq!(body["denied"]["name"], "margen");
    h.db.close().await;
}

/// Catalog events of this route, oldest first.
async fn catalog_events(pool: &sqlx::PgPool) -> Vec<(String, String)> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT payload::jsonb->>'action', payload::jsonb->>'result' FROM \
         bi_meta.audit_events WHERE payload::jsonb->>'resource_type' = 'project_catalog' \
         ORDER BY sequence",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = false)]
async fn diagnostics_released_under_inspection_are_recorded_first(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let modelador = h.login("modelador").await;
    let unknown = asked("metrics/ventas.yaml", "inexistente");
    let (status, body) = h.explain(Some(&modelador), &unknown).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
    assert_eq!(
        catalog_events(&h.pool).await,
        vec![("query".to_owned(), "success".to_owned())]
    );
    // Nothing of the catalog leaves without that event.
    sqlx::query(
        "ALTER TABLE bi_meta.audit_events ADD CONSTRAINT append_fails CHECK (false) NOT VALID",
    )
    .execute(&h.pool)
    .await
    .unwrap();
    for body in [unknown, asked("metrics/no_existe.yaml", "importe")] {
        let (status, answer) = h.explain(Some(&modelador), &body).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{answer:#}");
        assert_eq!(answer["code"], "audit_unavailable");
    }
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_hidden_homonym_never_changes_a_visible_query(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let lector = h.login("lector").await;
    // Quoted identifiers are exact: `"importe"` and `"IMPORTE"` are two names.
    let visible = MODEL.replace("  - name: importe", "  - name: '\"importe\"'");
    h.write("metrics/ventas.yaml", &visible);
    let request = asked("metrics/ventas.yaml", "\"importe\"");
    let (before, answer) = h.explain(Some(&lector), &request).await;
    assert_eq!(before, StatusCode::OK, "{answer:#}");
    let hidden = "  - name: '\"IMPORTE\"'\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{\"version\": 1, \"sensitivity\": \"restricted\"}'\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: SUM(sales.quantity)\n";
    h.write("metrics/ventas.yaml", &format!("{visible}{hidden}"));
    let (after, answer) = h.explain(Some(&lector), &request).await;
    assert_eq!(after, before, "{answer:#}");
    // Whoever inspects the catalog sees both, and the ambiguity.
    let modelador = h.login("modelador").await;
    let (status, answer) = h.explain(Some(&modelador), &request).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer:#}");
    assert_eq!(
        answer["diagnostics"][0]["code"], "ambiguous_name",
        "{answer:#}"
    );
    h.db.close().await;
}

/// A golden case that first rewrites one project file: name, user, the
/// file and its new contents, and the request.
type Staged = (
    &'static str,
    &'static str,
    Option<(&'static str, String)>,
    Value,
);

/// The explain contract for a fixed set of requests, recorded on the base
/// commit before the shared preparation (#135): answers and fingerprints
/// must stay byte for byte, the correlation aside. Regenerate only on
/// purpose, with `BI_GOLDEN_UPDATE=1`.
#[sqlx::test(migrations = false)]
async fn the_explain_contract_is_preserved(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let having = |name: &str| {
        let mut body = query(&json!([metric("m", name)]));
        body["query"]["having"] = json!({ "op": "compare",
            "target": { "kind": "metric", "selection": "m" }, "comparison": ">",
            "value": { "type": "number", "value": "1" } });
        body
    };
    let mut row_filter = query(&json!([region(), metric("m", "importe")]));
    row_filter["query"]["filter"] = json!({
        "op": "compare",
        "target": { "kind": "metric", "selection": "m" },
        "comparison": ">",
        "value": { "type": "number", "value": "1" }
    });
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "open",
            "lector",
            query(&json!([region(), metric("m", "importe")])),
        ),
        (
            "masked",
            "lector",
            query(&json!([region(), metric("c", "margen")])),
        ),
        ("hidden", "lector", asked("metrics/ventas.yaml", "coste")),
        ("refused masked use", "lector", having("margen")),
        (
            "restricted masked",
            "analista",
            asked("metrics/ventas.yaml", "coste"),
        ),
        (
            "unknown, no inspection",
            "analista",
            asked("metrics/ventas.yaml", "inexistente"),
        ),
        (
            "unknown, inspector",
            "modelador",
            asked("metrics/ventas.yaml", "inexistente"),
        ),
        (
            "absent, inspector",
            "modelador",
            asked("metrics/no_existe.yaml", "importe"),
        ),
        ("binding, inspector", "modelador", row_filter),
        (
            "all metrics",
            "modelador",
            query(&json!([
                region(),
                metric("a", "importe"),
                metric("b", "margen"),
                metric("c", "coste")
            ])),
        ),
        ("no role", "sinrol", asked("metrics/ventas.yaml", "importe")),
    ];
    let mut recorded = Vec::new();
    for (case, user, body) in cases {
        let cookie = h.login(user).await;
        let (status, answer) = h.explain(Some(&cookie), &body).await;
        recorded.push(json!({
            "case": case,
            "user": user,
            "status": status.as_u16(),
            "answer": per_instance(without_correlation(answer)),
        }));
    }
    // Cases that need another project state, applied in this order.
    let semi_additive = MODEL
        .replace(
            "  - name: importe\n",
            "  - name: importe\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{\"version\":1,\"aggregation\":{\"class\":\"semi_additive\",\"axes\":[{\"dimension\":\"region\",\"behavior\":\"forbidden\"}]}}'\n",
        )
        .replace(
            "        dimension: {}",
            "        dimension: {}\n        custom_extensions:\n          - vendor_name: bi-platform\n            data: '{\"version\":1,\"dimension\":\"region\"}'",
        );
    let restricted_dataset = MODEL.replace(
        "  - name: sales\n",
        "  - name: sales\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{\"version\": 1, \"sensitivity\": \"restricted\"}'\n",
    );
    let over_bounds = query(&json!(
        (0..65)
            .map(|n| metric(&format!("m{n}"), "importe"))
            .collect::<Vec<_>>()
    ));
    let staged: Vec<Staged> = vec![
        ("over the bounds", "lector", None, over_bounds),
        (
            "invalid model, inspector",
            "modelador",
            Some(("metrics/oculto.yaml", "version: [\n".to_owned())),
            asked("metrics/oculto.yaml", "importe"),
        ),
        (
            "incorrect query",
            "lector",
            Some(("metrics/ventas.yaml", semi_additive)),
            query(&json!([metric("m", "importe")])),
        ),
        (
            "plan reads a restricted dataset",
            "lector",
            Some(("metrics/ventas.yaml", restricted_dataset)),
            query(&json!([metric("m", "importe")])),
        ),
        (
            "invalid policies",
            "lector",
            Some((
                "policies/rota.cedar",
                "permit (principal, action, resource".to_owned(),
            )),
            asked("metrics/ventas.yaml", "importe"),
        ),
    ];
    for (case, user, change, body) in staged {
        if let Some((file, contents)) = change {
            h.write(file, &contents);
        }
        let cookie = h.login(user).await;
        let (status, answer) = h.explain(Some(&cookie), &body).await;
        recorded.push(json!({
            "case": case,
            "user": user,
            "status": status.as_u16(),
            "answer": per_instance(without_correlation(answer)),
        }));
    }
    let recorded = serde_json::to_string_pretty(&Value::from(recorded)).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/explain.json");
    if std::env::var("BI_GOLDEN_UPDATE").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &recorded).unwrap();
    }
    let golden = std::fs::read_to_string(&path).expect("tests/golden/explain.json");
    assert_eq!(recorded, golden.replace("\r\n", "\n"));
    h.db.close().await;
}
