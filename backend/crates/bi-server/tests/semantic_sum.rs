//! The exact result type of `SUM` and its overflow (#261), through the real
//! router: sessions and audit on PostgreSQL, a project on disk and DuckDB.
//! Every value is checked against a hand-written oracle; an overflow is a
//! typed 422 with its single failure event, never a 503 nor a rounded sum.
#![cfg(all(feature = "postgres-tests", feature = "dev-auth"))]

use std::time::Duration;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use bi_runtime::RuntimeConfig;
use bi_server::metadata::MetadataMode;
use bi_server::{AppState, analytics::Analytics, db::Database, models::Models};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:8080";

/// The largest DECIMAL(38,2): 36 integer digits and two decimals.
const M: &str = "999999999999999999999999999999999999.99";
/// The largest DECIMAL(38,0).
const N: &str = "99999999999999999999999999999999999999";
/// The largest DECIMAL(37,0): `a * b` is DECIMAL(76,0) (38 + 37 + 1 digits).
const N37: &str = "9999999999999999999999999999999999999";

/// One table per input type; `caso` selects a case's rows.
const TABLES: &str = "ATTACH ':memory:' AS lake;
CREATE TABLE lake.main.n18 (id BIGINT NOT NULL, caso VARCHAR NOT NULL, v DECIMAL(18,2));
CREATE TABLE lake.main.n38 (id BIGINT NOT NULL, caso VARCHAR NOT NULL, v DECIMAL(38,2));
CREATE TABLE lake.main.ni (id BIGINT NOT NULL, caso VARCHAR NOT NULL, v BIGINT);
CREATE TABLE lake.main.nw (id BIGINT NOT NULL, caso VARCHAR NOT NULL, a DECIMAL(38,0), b DECIMAL(37,0));";

fn rows() -> String {
    format!(
        "INSERT INTO lake.main.n18 VALUES
            (1, 'propietario', 9999999999999999.99), (2, 'propietario', 48735710.81),
            (3, 'nulos', NULL), (4, 'cero', 0.00), (5, 'cero', 0.00);
        INSERT INTO lake.main.n38 VALUES
            (1, 'max', {M}), (2, 'max_mas', {M}), (3, 'max_mas', 0.01),
            (4, 'menos', -{M}), (5, 'menos', -0.01),
            (6, 'cancela', {M}), (7, 'cancela', -{M}), (8, 'cancela', 0.01);
        INSERT INTO lake.main.ni VALUES
            (1, 'max_mas', 9223372036854775807), (2, 'max_mas', 1),
            (3, 'min_menos', -9223372036854775808), (4, 'min_menos', -1),
            (5, 'js', 9007199254740993), (6, 'js', 1);
        INSERT INTO lake.main.nw SELECT 1, 'uno', {N}, {N37}
            UNION ALL SELECT 10 + i, 'diez', {N}, {N37} FROM range(10) t(i)
            UNION ALL SELECT 30 + i, 'once', {N}, {N37} FROM range(11) t(i);"
    )
}

/// A model over `table` with `fields` and `metrics`, as an author writes it.
fn model(table: &str, fields: &[&str], metrics: &[(&str, &str)]) -> String {
    let field = |name: &&str| {
        let dimension = if *name == "caso" {
            "        dimension: {}\n"
        } else {
            ""
        };
        format!(
            "      - name: {name}\n{dimension}        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: {name}\n"
        )
    };
    let metric = |(name, expression): &(&str, &str)| {
        format!(
            "  - name: {name}\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: {expression}\n"
        )
    };
    format!(
        "version: '0.2.0.dev0'\nname: {table}\ndatasets:\n  - name: {table}\n    source: lake.main.{table}\n    primary_key: [id]\n    fields:\n{}metrics:\n{}",
        ["id", "caso"]
            .iter()
            .chain(fields)
            .map(field)
            .collect::<String>(),
        metrics.iter().map(metric).collect::<String>()
    )
}

fn project() -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    std::fs::create_dir(p.path().join("metrics")).unwrap();
    std::fs::create_dir(p.path().join("policies")).unwrap();
    let write = |file: &str, text: String| {
        std::fs::write(p.path().join("metrics").join(file), text).unwrap();
    };
    write(
        "n18.yaml",
        model(
            "n18",
            &["v"],
            &[("suma", "SUM(n18.v)"), ("doble", "suma + suma")],
        ),
    );
    write(
        "n38.yaml",
        model(
            "n38",
            &["v"],
            &[("suma", "SUM(n38.v)"), ("doble", "suma + suma")],
        ),
    );
    write(
        "ni.yaml",
        model(
            "ni",
            &["v"],
            &[("suma", "SUM(ni.v)"), ("filas", "COUNT(ni.v)")],
        ),
    );
    write(
        "nw.yaml",
        model("nw", &["a", "b"], &[("producto", "SUM(nw.a * nw.b)")]),
    );
    std::fs::write(
        p.path().join("policies/members.yaml"),
        "version: 1\nusers:\n  ana: [viewer]\n",
    )
    .unwrap();
    p
}

struct Harness {
    app: Router,
    pool: sqlx::PgPool,
    _project: tempfile::TempDir,
    db: Database,
}

async fn harness(pool: sqlx::PgPool) -> Harness {
    let runtime = tokio::task::spawn_blocking(|| {
        bi_runtime::Bootstrap::open(RuntimeConfig::in_memory(2))
            .unwrap()
            .finish()
            .unwrap()
    })
    .await
    .unwrap();
    let data = format!("{TABLES}\n{}", rows());
    runtime
        .run(move |conn| conn.execute_batch(&data))
        .await
        .unwrap();
    let project = project();
    let db = Database::start(&pool.connect_options(), MetadataMode::Development);
    assert!(
        tokio::time::timeout(Duration::from_secs(30), db.wait_until_ready())
            .await
            .unwrap()
    );
    let auth = bi_server::auth::AuthConfig::from_lookup(&|key| match key {
        "BI_PUBLIC_ORIGIN" => Some(ORIGIN.into()),
        "BI_DEV_USERS" => Some(r#"[{"id":"ana","display_name":"Ana"}]"#.into()),
        _ => None,
    })
    .unwrap();
    let app = bi_server::router(AppState {
        auth,
        analytics: Analytics::ready(runtime),
        models: Models::new(Some(project.path().into())),
        ..AppState::new(db.clone())
    });
    Harness {
        app,
        pool,
        _project: project,
        db,
    }
}

impl Harness {
    async fn login(&self) -> String {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::post("/api/v1/dev/login")
                    .header("origin", ORIGIN)
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "user_id": "ana" }).to_string()))
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

    /// The metrics `selected` of `table`, over the rows of `caso` only.
    async fn sum(
        &self,
        cookie: &str,
        table: &str,
        selected: &[&str],
        caso: &str,
    ) -> (StatusCode, Value) {
        let selections: Vec<Value> = selected
            .iter()
            .enumerate()
            .map(|(i, name)| json!({ "id": format!("m{i}"), "kind": "metric", "name": name }))
            .collect();
        let body = json!({
            "model": format!("metrics/{table}.yaml"),
            "query": {
                "selections": selections,
                "filter": {
                    "op": "compare",
                    "target": { "kind": "dimension", "name": format!("{table}.caso") },
                    "comparison": "=",
                    "value": { "type": "text", "value": caso },
                },
            },
        });
        let response = self
            .app
            .clone()
            .oneshot(
                Request::post("/api/v1/semantic/query")
                    .header("content-type", "application/json")
                    .header("origin", ORIGIN)
                    .header("cookie", cookie)
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// The (result, code) of every query event after `since`.
    async fn events_since(&self, since: i64) -> Vec<(String, Option<String>)> {
        sqlx::query_as(
            "SELECT payload::jsonb->>'result', payload::jsonb->>'policy_decision_id' \
             FROM bi_meta.audit_events WHERE sequence > $1 \
             AND payload::jsonb->>'action' NOT IN ('login', 'logout') ORDER BY sequence",
        )
        .bind(since)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }

    async fn head(&self) -> i64 {
        sqlx::query_scalar("SELECT COALESCE(max(sequence), 0) FROM bi_meta.audit_events")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

/// Column types and the single row of an answer.
fn answered(body: &Value) -> (Vec<String>, Vec<Value>) {
    let block = &body["result"]["blocks"][0];
    let types = block["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["type"].as_str().unwrap().to_owned())
        .collect();
    (types, block["rows"][0].as_array().unwrap().clone())
}

#[sqlx::test(migrations = false)]
async fn the_owners_sum_is_exact_in_its_widened_type(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let ana = h.login().await;
    // The owner's stack: a DECIMAL(18,2) sum past 18 digits (#260).
    let (status, body) = h.sum(&ana, "n18", &["suma", "doble"], "propietario").await;
    assert_eq!(status, StatusCode::OK, "{body:#}");
    assert_eq!(
        answered(&body),
        (
            vec!["DECIMAL(38,2)".to_owned(), "DECIMAL(39,2)".to_owned()],
            vec![json!("10000000048735710.80"), json!("20000000097471421.60")],
        )
    );
    // Empty or all NULL: NULL; zeros: zero.
    let (_, nulls) = h.sum(&ana, "n18", &["suma"], "nulos").await;
    assert_eq!(answered(&nulls).1, vec![Value::Null]);
    let (_, zeros) = h.sum(&ana, "n18", &["suma"], "cero").await;
    assert_eq!(answered(&zeros).1, vec![json!("0.00")]);
    let (_, none) = h.sum(&ana, "n18", &["suma"], "ninguno").await;
    assert_eq!(answered(&none).1, vec![Value::Null]);
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn int64_sums_are_exact_decimals_past_64_bits(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let ana = h.login().await;
    for (caso, sum) in [
        ("max_mas", "9223372036854775808"),
        ("min_menos", "-9223372036854775809"),
        ("js", "9007199254740994"),
    ] {
        let (status, body) = h.sum(&ana, "ni", &["suma", "filas"], caso).await;
        assert_eq!(status, StatusCode::OK, "{caso}: {body:#}");
        assert_eq!(
            answered(&body),
            (
                vec!["DECIMAL(38,0)".to_owned(), "INT64".to_owned()],
                vec![json!(sum), json!("2")],
            ),
            "{caso}"
        );
    }
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_sum_past_38_digits_is_a_typed_422_and_the_next_query_recovers(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let ana = h.login().await;
    // At the edge, or cancelling back inside it, the sum is exact.
    for (caso, sum) in [("max", M.to_owned()), ("cancela", "0.01".to_owned())] {
        let (status, body) = h.sum(&ana, "n38", &["suma"], caso).await;
        assert_eq!(status, StatusCode::OK, "{caso}: {body:#}");
        assert_eq!(answered(&body).1, vec![json!(sum)], "{caso}");
    }
    // Twice the largest DECIMAL(38,2) needs 39 digits: a derived metric
    // of two sums is computed exactly, never limited to 38.
    let (status, body) = h.sum(&ana, "n38", &["doble"], "max").await;
    assert_eq!(status, StatusCode::OK, "{body:#}");
    assert_eq!(
        answered(&body),
        (
            vec!["DECIMAL(39,2)".to_owned()],
            vec![json!("1999999999999999999999999999999999999.98")],
        )
    );
    // One cent past it either way: refused whole, typed, no figure.
    for caso in ["max_mas", "menos"] {
        let since = h.head().await;
        let (status, body) = h.sum(&ana, "n38", &["suma"], caso).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{caso}: {body:#}");
        assert_eq!(body["code"], "result_out_of_range", "{caso}");
        assert_eq!(
            body["detail"],
            "El resultado numérico está fuera del rango admitido por la consulta."
        );
        assert!(body.get("result").is_none() && body.get("receipt").is_none());
        let text = body.to_string();
        assert!(!text.contains("99999") && !text.contains("SUM") && !text.contains("Conversion"));
        let events = h.events_since(since).await;
        assert_eq!(events.len(), 1, "{caso}: one terminal event");
        assert_eq!(events[0].0, "failure", "{caso}");
        assert!(events[0].1.is_some(), "{caso}: the decision taken names it");
    }
    // The next valid query is answered.
    let (status, _) = h.sum(&ana, "n38", &["suma"], "max").await;
    assert_eq!(status, StatusCode::OK);
    h.db.close().await;
}

#[sqlx::test(migrations = false)]
async fn an_emulated_sum_past_76_digits_is_the_same_typed_422(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let ana = h.login().await;
    // A row contributes at most N * N37 to a DECIMAL(76,0) sum: ten such
    // rows still fit, eleven do not.
    for (caso, sum) in [
        (
            "uno",
            "999999999999999999999999999999999999890000000000000000000000000000000000001",
        ),
        (
            "diez",
            "9999999999999999999999999999999999998900000000000000000000000000000000000010",
        ),
    ] {
        let (status, body) = h.sum(&ana, "nw", &["producto"], caso).await;
        assert_eq!(status, StatusCode::OK, "{caso}: {body:#}");
        assert_eq!(
            answered(&body),
            (vec!["DECIMAL(76,0)".to_owned()], vec![json!(sum)]),
            "{caso}"
        );
    }
    let (status, body) = h.sum(&ana, "nw", &["producto"], "once").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
    assert_eq!(body["code"], "result_out_of_range");
    let (status, _) = h.sum(&ana, "nw", &["producto"], "uno").await;
    assert_eq!(status, StatusCode::OK, "recovers");
    h.db.close().await;
}
