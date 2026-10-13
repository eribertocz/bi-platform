//! The single semantic path (#98) end to end: real sessions on PostgreSQL,
//! real DuckDB catalog, project policies and members from files, and the
//! audit chain.
#![cfg(all(feature = "postgres-tests", feature = "dev-auth"))]

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
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
}

async fn harness(pool: sqlx::PgPool) -> Harness {
    let project = project();
    let db = Database::start(pool.clone());
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(30), db.wait_until_migrated())
            .await
            .unwrap()
    );
    let auth = bi_server::auth::AuthConfig::from_lookup(&|key| match key {
        "BI_PUBLIC_ORIGIN" => Some(ORIGIN.into()),
        "BI_DEV_USERS" => Some(
            r#"[{"id":"lector","display_name":"Lector"},{"id":"analista","display_name":"Analista"},{"id":"sinrol","display_name":"Sin rol"}]"#
                .into(),
        ),
        _ => None,
    })
    .unwrap();
    let app = bi_server::router(AppState {
        auth,
        analytics: analytics().await,
        models: Models::new(Some(project.path().into())),
        ..AppState::new(db)
    });
    Harness { app, pool, project }
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
    let (status, denied) = h
        .explain(Some(&lector), &query(&json!([metric("x", "coste")])))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied:#}");
    assert_eq!(denied["code"], "semantic_access_denied");
    assert_eq!(denied["reason"], "no_permit");
    assert_eq!(denied["denied"]["kind"], "metric");
    assert_eq!(denied["denied"]["name"], "coste");
    assert_eq!(denied["denied"]["usage"], "project");
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
    // A name the model does not have.
    let (status, body) = h
        .explain(
            Some(&analista),
            &query(&json!([metric("m", "inexistente")])),
        )
        .await;
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
    let (status, body) = h.explain(Some(&analista), &incorrect).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
    assert!(
        !body["diagnostics"].as_array().unwrap().is_empty(),
        "{body:#}"
    );
    // A model that is not in the project.
    let mut missing = query(&json!([metric("m", "importe")]));
    missing["model"] = json!("metrics/otro.yaml");
    let (status, body) = h.explain(Some(&analista), &missing).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:#}");
    assert_eq!(body["code"], "model_not_found");
}

#[sqlx::test(migrations = false)]
async fn identity_comes_only_from_the_session(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let body = query(&json!([metric("m", "importe")]));
    let (status, _) = h.explain(None, &body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A user the project grants no role is denied by default…
    let sinrol = h.login("sinrol").await;
    let (status, denied) = h.explain(Some(&sinrol), &body).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied:#}");
    assert_eq!(denied["reason"], "no_permit");
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
}
