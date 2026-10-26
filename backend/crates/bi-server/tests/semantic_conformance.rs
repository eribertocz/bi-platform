//! Conformance of the production API (#144): a client of the server already
//! started in the OIDC environment — compiled without dev-auth, restricted
//! metadata role, the governed project and the DuckLake fixture — signing in
//! at the real provider. Nothing is seeded, intercepted or built in-process.
//!
//! Inputs: `BI_CONFORMANCE_BASE_URL` (the server, also its public origin),
//! `BI_CONFORMANCE_PROJECT_DIR` (the project that server reads),
//! `BI_TEST_OIDC_ISSUER` and an administrative `DATABASE_URL` to read the
//! audit. Every input is required: an absent one fails, it never skips.
//! Optional `BI_CONFORMANCE_CLIENT_ADDR`: the local address this client
//! connects from. The server admits OIDC logins per client address, so a
//! browser suite sharing the server stays a separate client (CI: 127.0.0.2).
//! Run serially (`--test-threads=1`): revocation edits the project.
#![cfg(feature = "postgres-tests")]

#[path = "common/keycloak.rs"]
mod keycloak;

use std::path::PathBuf;
use std::time::Duration;

use reqwest::header::{self, HeaderMap};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required by the conformance target"))
}

/// The server, and a client that follows no redirect.
struct Api {
    base: String,
    http: Client,
}

impl Api {
    /// The configured server, checked before any case: ready, OIDC on, no
    /// development login, the very provider of the fixture.
    async fn ready() -> Self {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let api = Self {
            base: required("BI_CONFORMANCE_BASE_URL"),
            http: Client::builder()
                .local_address(
                    std::env::var("BI_CONFORMANCE_CLIENT_ADDR")
                        .ok()
                        .map(|addr| {
                            addr.parse::<std::net::IpAddr>()
                                .expect("BI_CONFORMANCE_CLIENT_ADDR is an IP address")
                        }),
                )
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap(),
        };
        let issuer = required("BI_TEST_OIDC_ISSUER");
        let ready = api.get("/health/ready", None).await;
        assert_eq!(ready.0, StatusCode::OK, "{}", ready.2);
        let config = api.get("/api/v1/oidc/config", None).await;
        assert_eq!(config.0, StatusCode::OK, "{}", config.2);
        assert_eq!(json(&config.2)["enabled"], true, "{}", config.2);
        // Production: the development login does not exist.
        assert_eq!(
            api.get("/api/v1/dev/users", None).await.0,
            StatusCode::NOT_FOUND
        );
        let discovery = api
            .http
            .get(format!("{issuer}/.well-known/openid-configuration"))
            .send()
            .await
            .unwrap();
        assert_eq!(discovery.status(), StatusCode::OK, "the fixture's provider");
        api
    }

    async fn get(&self, path: &str, session: Option<&str>) -> (StatusCode, HeaderMap, String) {
        let mut request = self.http.get(format!("{}{path}", self.base));
        if let Some(session) = session {
            request = request.header(header::COOKIE, session);
        }
        let response = request.send().await.unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        (status, headers, response.text().await.unwrap())
    }

    /// The same body from any session, with extra headers if any.
    async fn query(
        &self,
        session: &str,
        body: &Value,
        headers: &[(&str, &str)],
    ) -> (StatusCode, String, String) {
        let mut request = self
            .http
            .post(format!("{}/api/v1/semantic/query", self.base))
            .header(header::COOKIE, session)
            .header(header::ORIGIN, &self.base)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_string());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let id = response.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        (status, id, response.text().await.unwrap())
    }

    /// A whole login of `username` at the real provider: its session cookie.
    async fn sign_in(&self, username: &str) -> String {
        let start = self
            .http
            .post(format!("{}/api/v1/oidc/start", self.base))
            .header(header::ORIGIN, &self.base)
            .send()
            .await
            .unwrap();
        assert_eq!(start.status(), StatusCode::OK);
        let attempt = first(&start.headers()[header::SET_COOKIE]);
        let url = json(&start.text().await.unwrap())["authorization_url"]
            .as_str()
            .unwrap()
            .to_owned();
        let callback = keycloak::authorize_as(&url, username).await;
        assert!(callback.starts_with(&self.base), "{callback}");
        let done = self
            .http
            .get(&callback)
            .header(header::COOKIE, attempt)
            .send()
            .await
            .unwrap();
        assert_eq!(done.status(), StatusCode::SEE_OTHER);
        done.headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(first)
            .find(|cookie| cookie.starts_with("bi_session="))
            .expect("a session cookie")
    }

    /// The verified user id behind `session`, as the server resolves it.
    async fn user(&self, session: &str) -> String {
        let (status, _, body) = self.get("/api/v1/session", Some(session)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        json(&body)["user"]["id"].as_str().unwrap().to_owned()
    }

    async fn catalog(&self, session: &str) -> Value {
        let (status, headers, body) = self.get("/api/v1/semantic/catalog", Some(session)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        json(&body)
    }
}

fn first(value: &header::HeaderValue) -> String {
    value
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| panic!("not JSON: {text}"))
}

/// The same body for every identity: the model's total and its row count.
fn totals() -> Value {
    json!({
        "model": "metrics/ventas.yaml",
        "query": { "selections": [
            { "kind": "metric", "id": "t", "name": "importe_total" },
            { "kind": "metric", "id": "f", "name": "filas" }
        ] }
    })
}

/// Oracle of the fixture (`frontend/e2e/semantic/ventas.sql`), computed
/// apart from the server: Norte 100.10 + 0.20, Sur 200.20 - 0.20, 2 rows each.
const NORTE: [&str; 2] = ["100.30", "2"];
const SUR: [&str; 2] = ["200.00", "2"];

/// An answered query: exactly the oracle's row, at the declared exact types.
fn answered(status: StatusCode, text: &str, oracle: [&str; 2]) -> Value {
    assert_eq!(status, StatusCode::OK, "{text}");
    let answer = json(text);
    let block = &answer["result"]["blocks"][0];
    assert_eq!(block["rows"], json!([oracle]), "{text}");
    let types: Vec<_> = block["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["selection_id"].clone(), c["type"].clone()))
        .collect();
    assert_eq!(
        types,
        vec![
            (json!("t"), json!("DECIMAL(38,2)")),
            (json!("f"), json!("INT64"))
        ],
        "{text}"
    );
    answer["receipt"].clone()
}

/// The neutral refusal: no data, no receipt, nothing that names the model.
fn neutral(status: StatusCode, text: &str) {
    assert_eq!(status, StatusCode::NOT_FOUND, "{text}");
    let problem = json(text);
    assert_eq!(problem["code"], "model_not_found", "{text}");
    assert!(
        problem.get("result").is_none() && problem.get("receipt").is_none(),
        "{text}"
    );
    for leaked in ["100.30", "200.00", "ventas", "department", "Norte", "Sur"] {
        assert!(!text.contains(leaked), "{leaked}: {text}");
    }
}

/// The audit event of one request, read with the administrative role:
/// its actor, action and result. The payload holds no value of the rows,
/// the verified context or a credential.
async fn audited(pool: &PgPool, request: &str) -> (String, String, String) {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT payload FROM bi_meta.audit_events
         WHERE (payload::jsonb)->>'request_id' = $1
           AND (payload::jsonb)->>'resource_type' IN ('semantic_model', 'semantic_request')",
    )
    .bind(request)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "one event for {request}: {rows:?}");
    let payload = &rows[0].0;
    for leaked in [
        "100.30",
        "200.00",
        "Norte",
        "Sur",
        "oidc-test-only",
        "bi_session",
    ] {
        assert!(!payload.contains(leaked), "{leaked}: {payload}");
    }
    let event = json(payload);
    let field = |name: &str| event[name].as_str().unwrap().to_owned();
    (field("actor_id"), field("action"), field("result"))
}

async fn audit() -> PgPool {
    PgPool::connect(&required("DATABASE_URL")).await.unwrap()
}

/// `members.yaml` of the project the server reads, restored even when a
/// case fails: a restoration that fails fails the target.
struct Members {
    path: PathBuf,
    original: Vec<u8>,
}

impl Members {
    fn of_project() -> Self {
        let path =
            PathBuf::from(required("BI_CONFORMANCE_PROJECT_DIR")).join("policies/members.yaml");
        let original = std::fs::read(&path).unwrap();
        Self { path, original }
    }

    /// The analysts' group mapped to another role: its sessions lose the model.
    fn revoke(&self) {
        let text = String::from_utf8(self.original.clone()).unwrap();
        let revoked = text.replace("  analysts: [viewer]", "  catalog_authors: [viewer]");
        assert_ne!(revoked, text, "the fixture maps the analysts' group");
        std::fs::write(&self.path, revoked).unwrap();
    }
}

impl Drop for Members {
    fn drop(&mut self) {
        std::fs::write(&self.path, &self.original).expect("members.yaml restored");
    }
}

/// A and B send the same body, alternating A, B, A: each gets its own
/// oracle, receipt and audited event, nothing of the other. The definition
/// and the data they read are the same; their receipts are comparable.
#[tokio::test]
async fn each_identity_gets_its_own_oracle_receipt_and_audit() {
    let api = Api::ready().await;
    let pool = audit().await;
    let a = api.sign_in("oidc-norte").await;
    let b = api.sign_in("oidc-sur").await;
    let (a_id, b_id) = (api.user(&a).await, api.user(&b).await);
    assert_ne!(a_id, b_id);

    let mut receipts = Vec::new();
    for (session, user, oracle) in [(&a, &a_id, NORTE), (&b, &b_id, SUR), (&a, &a_id, NORTE)] {
        let (status, request, text) = api.query(session, &totals(), &[]).await;
        let receipt = answered(status, &text, oracle);
        let other = if oracle == NORTE { SUR[0] } else { NORTE[0] };
        assert!(!text.contains(other), "{text}");
        assert_eq!(
            audited(&pool, &request).await,
            (user.clone(), "query".to_owned(), "success".to_owned())
        );
        receipts.push(receipt);
    }
    // A receipt is of its actor's answer: the same question and data give A
    // the same one, B another. Each request has its own audited event.
    assert_eq!(receipts[0]["id"], receipts[2]["id"]);
    assert_ne!(receipts[0]["id"], receipts[1]["id"]);
    // Comparable across actors: the engine, the canonical query and the
    // sources read. Fingerprints keyed by the context (model, policies,
    // context, plan, decision) only within one actor: never required equal.
    for receipt in &receipts[1..] {
        assert_eq!(receipt["versions"], receipts[0]["versions"]);
        assert_eq!(
            receipt["fingerprints"]["query"],
            receipts[0]["fingerprints"]["query"]
        );
        assert_eq!(receipt["sources"], receipts[0]["sources"]);
    }
    assert_eq!(receipts[0]["fingerprints"], receipts[2]["fingerprints"]);
    // The same definition announced to both.
    let names = |catalog: &Value| -> Vec<Value> {
        catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["model"].clone())
            .collect()
    };
    let (ca, cb) = (api.catalog(&a).await, api.catalog(&b).await);
    assert!(names(&ca).contains(&json!("metrics/ventas.yaml")), "{ca}");
    assert_eq!(names(&ca), names(&cb));
    pool.close().await;
}

/// Revoking the analysts' mapping denies the next query of the same cookie
/// neutrally, empties its catalog and keeps it signed in; restoring it
/// answers the exact oracle again. Nothing granted is kept anywhere.
#[tokio::test]
async fn revocation_and_restoration_act_on_the_next_request_of_the_same_cookie() {
    let api = Api::ready().await;
    let pool = audit().await;
    let a = api.sign_in("oidc-norte").await;
    let a_id = api.user(&a).await;
    let (status, _, text) = api.query(&a, &totals(), &[]).await;
    answered(status, &text, NORTE);

    let members = Members::of_project();
    members.revoke();
    for _ in 0..2 {
        let (status, request, text) = api.query(&a, &totals(), &[]).await;
        neutral(status, &text);
        assert_eq!(
            audited(&pool, &request).await,
            (a_id.clone(), "denial".to_owned(), "denied".to_owned())
        );
    }
    assert_eq!(api.catalog(&a).await["models"], json!([]));
    assert_eq!(api.user(&a).await, a_id, "the session itself stays");

    drop(members);
    let (status, request, text) = api.query(&a, &totals(), &[]).await;
    answered(status, &text, NORTE);
    assert_eq!(
        audited(&pool, &request).await,
        (a_id, "query".to_owned(), "success".to_owned())
    );
    pool.close().await;
}

/// An identity the project does not map gets nothing; what a client claims,
/// in headers or in the body, grants nothing either.
#[tokio::test]
async fn an_unmapped_identity_and_claimed_roles_grant_nothing() {
    let api = Api::ready().await;
    let pool = audit().await;
    let outsider = api.sign_in("oidc-outsider").await;
    let outsider_id = api.user(&outsider).await;
    let (status, request, text) = api.query(&outsider, &totals(), &[]).await;
    neutral(status, &text);
    assert_eq!(
        audited(&pool, &request).await,
        (outsider_id, "denial".to_owned(), "denied".to_owned())
    );
    assert_eq!(api.catalog(&outsider).await["models"], json!([]));

    // Headers claiming another identity, role or department: ignored.
    let a = api.sign_in("oidc-norte").await;
    let claimed = [
        ("x-roles", "admin"),
        ("x-forwarded-groups", "admins"),
        ("x-department", "Sur"),
        ("x-forwarded-user", "oidc-sur"),
    ];
    let (status, _, text) = api.query(&a, &totals(), &claimed).await;
    answered(status, &text, NORTE);
    let (status, _, text) = api.query(&outsider, &totals(), &claimed).await;
    neutral(status, &text);
    // A context in the body: refused before preparing, with no data.
    let mut forged = totals();
    forged["query"]["context"] = json!({ "department": "Sur" });
    let (status, _, text) = api.query(&a, &forged, &[]).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{text}");
    assert_eq!(json(&text)["code"], "invalid_request", "{text}");
    for leaked in ["100.30", "200.00", "receipt"] {
        assert!(!text.contains(leaked), "{leaked}: {text}");
    }
    pool.close().await;
}
