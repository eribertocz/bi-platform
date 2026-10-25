//! The frontend's semantic fixtures (#141, #147) are real answers of
//! `POST /api/v1/semantic/query` (#138) and `GET /api/v1/semantic/catalog`
//! (#140): the real router, sessions and audit on PostgreSQL, DuckDB with
//! the Issue's rows and an Ossie project on disk.
//!
//! `BI_UPDATE_FIXTURES=1` rewrites them, formatted as the frontend's
//! Prettier formats JSON. Otherwise each committed file must equal a fresh
//! answer once the values that change on every run (the receipt's ID, read
//! times, protected fingerprints and evidence, keyed per instance) are
//! replaced, on both sides, by a placeholder of their format: any drift of
//! the contract or of a value fails. The catalog holds no such value and is
//! compared whole.
#![cfg(all(feature = "postgres-tests", feature = "dev-auth"))]

use std::path::PathBuf;
use std::time::Duration;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use bi_runtime::RuntimeConfig;
use bi_server::metadata::MetadataMode;
use bi_server::semantic::run::{INITIAL, Profile, Runs};
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

/// Extremes: INT64 beyond 2^53 and at both signed bounds, DECIMAL(38,10)
/// at its largest and smallest steps, negatives, a group whose metrics are
/// NULL (`C`) and a NULL group.
const EXTREME_ROWS: &str = "(1, 'A', 9007199254740993, 9999999999999999999999999999.9999999999),
    (2, 'A', -9223372036854775808, -0.0000000001),
    (3, 'B', 9223372036854775807, -12345.6789012345),
    (4, 'C', NULL, NULL),
    (5, NULL, 42, 0.0000000001)";

/// The Issue's model, as `semantic_query.rs` declares it.
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

const EXTREMES: &str = r"version: '0.2.0.dev0'
name: Extremos
datasets:
  - name: extremos
    source: lake.main.extremos
    primary_key: [id]
    fields:
      - name: id
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: id
      - name: grupo
        dimension: {}
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: grupo
      - name: entero
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: entero
      - name: preciso
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: preciso
metrics:
  - name: entero_max
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: MAX(extremos.entero)
  - name: entero_min
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: MIN(extremos.entero)
  - name: preciso_total
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(extremos.preciso)
";

/// Two deprecated metrics: `vieja` replaced by `importe_total`, which the
/// viewer sees; `antigua` by `reserva_centinela`, which she does not.
const DEPRECATED: &str = r#"  - name: vieja
    custom_extensions:
      - vendor_name: bi-platform
        data: '{"version": 1, "status": {"state": "deprecated", "sunset": "2027-01-01", "replacement": "importe_total"}}'
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(ventas.importe)
  - name: antigua
    custom_extensions:
      - vendor_name: bi-platform
        data: '{"version": 1, "status": {"state": "deprecated", "sunset": "2027-06-30", "replacement": "reserva_centinela"}}'
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: COUNT(ventas.id)
"#;

/// A metric beyond DECIMAL(38): `importe` cubed is DECIMAL(56,6), summed
/// by the adapter's exact emulation (as in `semantic::run::tests`).
const CUBED: &str = "  - name: cubo
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(ventas.importe * ventas.importe * ventas.importe)
";

/// Distinct counts beside the Issue's metrics (#258).
const DISTINCT: &str = r"  - name: ciudades
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: COUNT(DISTINCT ventas.ciudad)
  - name: importes_distintos
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: COUNT(DISTINCT ventas.importe)
";

const MEMBERS: &str = "
version: 1
users:
  ana: [viewer]
";

/// The Issue's model with `instante` on the event timeline, its month in
/// La Paz (UTC-4) as a governed bucket, and the latest instant as a metric.
fn instants_model() -> String {
    let instant = "        datatype: DateTimeTz\n        dimension: {}\n";
    assert_eq!(MODEL.matches(instant).count(), 1);
    let month = "      - name: mes
        dimension: {}
        custom_extensions:
          - vendor_name: bi-platform
            data: '{\"version\": 1, \"time\": {\"timezone\": \"America/La_Paz\"}}'
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: DATE_TRUNC('MONTH', instante)
";
    let latest = "  - name: ultimo_instante
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: MAX(ventas.instante)
";
    assert_eq!(MODEL.matches("metrics:\n").count(), 1);
    format!(
        "{}{latest}",
        MODEL
            .replacen(
                instant,
                &format!(
                    "{instant}        custom_extensions:\n          - vendor_name: bi-platform\n            data: '{{\"version\": 1, \"time_role\": \"event\"}}'\n"
                ),
                1,
            )
            .replacen("metrics:\n", &format!("{month}metrics:\n"), 1)
    )
}

fn project() -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    std::fs::create_dir(p.path().join("metrics")).unwrap();
    std::fs::create_dir(p.path().join("policies")).unwrap();
    for (file, text) in [
        ("metrics/ventas.yaml", MODEL.to_owned()),
        ("metrics/extremos.yaml", EXTREMES.to_owned()),
        ("metrics/instantes.yaml", instants_model()),
        ("metrics/obsoletas.yaml", format!("{MODEL}{DEPRECATED}")),
        ("metrics/anchos.yaml", format!("{MODEL}{CUBED}")),
        ("policies/members.yaml", MEMBERS.to_owned()),
    ] {
        std::fs::write(p.path().join(file), text).unwrap();
    }
    p
}

/// A DuckDB runtime holding the Issue's rows and the extremes.
async fn analytics() -> Analytics {
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
                 instante TIMESTAMPTZ NOT NULL, nota VARCHAR); INSERT INTO lake.main.ventas VALUES {ROWS}; \
                 CREATE TABLE lake.main.extremos (id BIGINT NOT NULL, grupo VARCHAR, entero BIGINT, \
                 preciso DECIMAL(38,10)); INSERT INTO lake.main.extremos VALUES {EXTREME_ROWS};"
            ))
        })
        .await
        .unwrap();
    Analytics::ready(runtime)
}

struct Harness {
    app: Router,
    db: Database,
    project: tempfile::TempDir,
}

async fn harness(pool: sqlx::PgPool) -> Harness {
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
        analytics: analytics().await,
        models: Models::new(Some(project.path().into())),
        semantic_runs: Runs::new(Profile::new(INITIAL).unwrap()),
        ..AppState::new(db.clone())
    });
    Harness { app, db, project }
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

    /// The body of a successful answer to `body`, as the client reads it.
    async fn query(&self, cookie: &str, body: &Value) -> Value {
        let (status, answer) = self.answer(cookie, body).await;
        assert_eq!(status, StatusCode::OK, "{body}: {answer:#}");
        answer
    }

    /// The catalog the session may use, as the client reads it.
    async fn catalog(&self, cookie: &str) -> Value {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::get("/api/v1/semantic/catalog")
                    .header("cookie", cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let catalog: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::OK, "{catalog:#}");
        catalog
    }

    async fn answer(&self, cookie: &str, body: &Value) -> (StatusCode, Value) {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::post("/api/v1/semantic/query")
                    .header("origin", ORIGIN)
                    .header("cookie", cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
}

fn metric(id: &str, name: &str) -> Value {
    json!({ "kind": "metric", "id": id, "name": name })
}

fn dimension(id: &str, name: &str) -> Value {
    json!({ "kind": "dimension", "id": id, "name": name })
}

fn request(model: &str, query: Value) -> Value {
    let mut body = json!({ "model": model });
    body["query"] = query;
    body
}

fn rows(answer: &Value) -> &Value {
    &answer["result"]["blocks"][0]["rows"]
}

// ---- Fixtures ----------------------------------------------------------------

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../frontend/src/lib/features/semantic/fixtures")
}

/// `prefix:` and 64 lowercase hexadecimal digits, with its digits zeroed.
fn hex_placeholder(value: &Value, at: &str) -> Value {
    let text = value.as_str().unwrap_or_else(|| panic!("{at}: {value}"));
    let (prefix, digits) = text
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("{at}: {text}"));
    assert!(
        digits.len() == 64
            && digits
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{at}: {text}"
    );
    json!(format!("{prefix}:{}", "0".repeat(64)))
}

/// An RFC 3339 instant in UTC, as one fixed instant.
fn instant_placeholder(value: &Value, at: &str) -> Value {
    let text = value.as_str().unwrap_or_else(|| panic!("{at}: {value}"));
    assert!(
        text.ends_with('Z') && chrono::DateTime::parse_from_rfc3339(text).is_ok(),
        "{at}: {text}"
    );
    json!("1970-01-01T00:00:00Z")
}

/// `answer` with every value that changes on each run replaced by a
/// placeholder of its format; everything else as it is.
fn normalized(answer: &Value) -> Value {
    let mut answer = answer.clone();
    let receipt = answer
        .get_mut("receipt")
        .and_then(Value::as_object_mut)
        .expect("receipt");
    for field in ["id", "evidence"] {
        let placeholder = hex_placeholder(&receipt[field], field);
        receipt.insert(field.into(), placeholder);
    }
    for field in ["read_started_at", "read_finished_at"] {
        let placeholder = instant_placeholder(&receipt[field], field);
        receipt.insert(field.into(), placeholder);
    }
    let fingerprints = receipt
        .get_mut("fingerprints")
        .and_then(Value::as_object_mut)
        .expect("fingerprints");
    for field in ["context", "decision", "model", "plan", "policies"] {
        let placeholder = hex_placeholder(&fingerprints[field], field);
        fingerprints.insert(field.into(), placeholder);
    }
    answer
}

/// Writes `answer` as `<name>.json`, or checks it against the committed one
/// once `normalize` has replaced its volatile values on both sides.
fn fixture(name: &str, answer: &Value) {
    compared(name, answer, normalized);
}

fn compared(name: &str, answer: &Value, normalize: fn(&Value) -> Value) {
    let path = fixtures_dir().join(format!("{name}.json"));
    if std::env::var_os("BI_UPDATE_FIXTURES").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(fixtures_dir()).unwrap();
        std::fs::write(&path, prettier(answer)).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{}: {error}; regenerate with BI_UPDATE_FIXTURES=1",
            path.display()
        )
    });
    let committed: Value = serde_json::from_str(&committed).unwrap();
    assert_eq!(
        normalize(&committed),
        normalize(answer),
        "{name}.json differs from the real answer; regenerate with BI_UPDATE_FIXTURES=1 \
         if the change is intended"
    );
}

// ---- JSON as Prettier writes it -------------------------------------------------

const PRINT_WIDTH: usize = 100;

/// `value` as Prettier 3 formats JSON with the frontend's settings: every
/// non-empty object expanded (it preserves the expansion), an array on one
/// line when it fits and nothing forces it to break, a final newline.
fn prettier(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0, 0);
    out.push('\n');
    out
}

fn flat(value: &Value) -> String {
    match value {
        Value::Array(items) if !items.is_empty() => format!(
            "[{}]",
            items.iter().map(flat).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) if !map.is_empty() => unreachable!("objects never print flat"),
        _ => serde_json::to_string(value).unwrap(),
    }
}

/// Whether a non-empty object lies within `value`: its expansion breaks
/// every array around it.
fn holds_object(value: &Value) -> bool {
    match value {
        Value::Object(map) => !map.is_empty(),
        Value::Array(items) => items.iter().any(holds_object),
        _ => false,
    }
}

/// Prettier breaks an array of two or more arrays (or objects) that each
/// hold more than one element.
fn breaks(items: &[Value]) -> bool {
    let many = |item: &Value| match item {
        Value::Array(inner) => inner.len() > 1,
        Value::Object(map) => map.len() > 1,
        _ => false,
    };
    let same = |a: &Value, b: &Value| {
        matches!(
            (a, b),
            (Value::Array(_), Value::Array(_)) | (Value::Object(_), Value::Object(_))
        )
    };
    items.len() > 1 && items.iter().all(many) && items.windows(2).all(|w| same(&w[0], &w[1]))
}

/// Writes `value` at `column`, nested `depth` levels; `trailing` characters
/// follow it on its line.
fn write_value(out: &mut String, value: &Value, depth: usize, trailing: usize) {
    let indent = |level: usize| "  ".repeat(level);
    match value {
        Value::Object(map) if !map.is_empty() => {
            out.push_str("{\n");
            let last = map.len() - 1;
            for (i, (key, item)) in map.iter().enumerate() {
                let key = serde_json::to_string(key).unwrap();
                out.push_str(&indent(depth + 1));
                out.push_str(&key);
                out.push_str(": ");
                let comma = usize::from(i != last);
                write_value(out, item, depth + 1, comma);
                if comma == 1 {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&indent(depth));
            out.push('}');
        }
        Value::Array(items) if !items.is_empty() => {
            let line_start = out.rfind('\n').map_or(0, |at| at + 1);
            let column = out[line_start..].chars().count();
            let one_line = flat_if_flat(value);
            let fits = one_line
                .as_ref()
                .is_some_and(|line| column + line.chars().count() + trailing <= PRINT_WIDTH);
            if fits && !breaks(items) {
                out.push_str(one_line.as_deref().unwrap());
                return;
            }
            out.push_str("[\n");
            let last = items.len() - 1;
            for (i, item) in items.iter().enumerate() {
                out.push_str(&indent(depth + 1));
                let comma = usize::from(i != last);
                write_value(out, item, depth + 1, comma);
                if comma == 1 {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&indent(depth));
            out.push(']');
        }
        _ => out.push_str(&flat(value)),
    }
}

/// The one-line form of an array, unless something in it must break.
fn flat_if_flat(value: &Value) -> Option<String> {
    (!holds_object(value)).then(|| flat(value))
}

// ---- The answers -------------------------------------------------------------

#[sqlx::test(migrations = false)]
async fn real_semantic_answers_match_the_frontend_fixtures(pool: sqlx::PgPool) {
    let h = harness(pool).await;
    let ana = h.login("ana").await;
    let ventas = "metrics/ventas.yaml";

    // What the viewer may use of the project, in the order of its files:
    // nothing volatile, compared whole. Restricted and confidential members
    // never appear, and neither does a sentinel.
    let catalog = h.catalog(&ana).await;
    assert_eq!(catalog["status"], "ready", "{catalog:#}");
    let models: Vec<_> = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["model"].as_str().unwrap())
        .collect();
    assert!(models.contains(&ventas), "{models:?}");
    let text = catalog.to_string();
    assert!(
        !text.contains("centinela") && !text.contains("nota"),
        "{text}"
    );
    compared("catalog", &catalog, Value::clone);

    // The Issue's oracle by region, ordered by region.
    let region = h
        .query(
            &ana,
            &request(
                ventas,
                json!({
                    "selections": [
                        dimension("r", "ventas.region"),
                        metric("t", "importe_total"),
                        metric("f", "filas")
                    ],
                    "order_by": [{ "selection": "r" }]
                }),
            ),
        )
        .await;
    assert_eq!(
        rows(&region),
        &json!([["Norte", "100.30", "2"], ["Sur", "200.00", "2"]])
    );
    fixture("region", &region);

    // The same metrics as a KPI: no dimension.
    let total = h
        .query(
            &ana,
            &request(
                ventas,
                json!({ "selections": [metric("t", "importe_total"), metric("f", "filas")] }),
            ),
        )
        .await;
    assert_eq!(rows(&total), &json!([["300.30", "4"]]));
    fixture("total", &total);

    // Exactness at the edges.
    let extremes = h
        .query(
            &ana,
            &request(
                "metrics/extremos.yaml",
                json!({
                    "selections": [
                        dimension("g", "extremos.grupo"),
                        metric("max", "entero_max"),
                        metric("min", "entero_min"),
                        metric("p", "preciso_total")
                    ],
                    "order_by": [{ "selection": "g" }]
                }),
            ),
        )
        .await;
    assert_eq!(
        rows(&extremes),
        &json!([
            [
                "A",
                "9007199254740993",
                "-9223372036854775808",
                "9999999999999999999999999999.9999999998"
            ],
            [
                "B",
                "9223372036854775807",
                "9223372036854775807",
                "-12345.6789012345"
            ],
            ["C", null, null, null],
            [null, "42", "42", "0.0000000001"]
        ])
    );
    fixture("extremes", &extremes);

    // An instant as a metric, next to a governed month in La Paz.
    let instant = h
        .query(
            &ana,
            &request(
                "metrics/instantes.yaml",
                json!({
                    "selections": [
                        { "kind": "time_dimension", "id": "m", "name": "ventas.mes", "grain": "month" },
                        metric("u", "ultimo_instante"),
                        metric("f", "filas")
                    ],
                    "order_by": [{ "selection": "m" }]
                }),
            ),
        )
        .await;
    assert_eq!(
        rows(&instant),
        &json!([
            ["2025-12-01", "2026-01-01T00:00:00Z", "2"],
            ["2026-01-01", "2026-01-02T00:00:00Z", "2"]
        ])
    );
    assert_eq!(
        instant["result"]["blocks"][0]["columns"][0]["timezone"],
        "America/La_Paz"
    );
    fixture("instant", &instant);

    // Deprecated metrics: one replacement visible, one hidden.
    let deprecated = h
        .query(
            &ana,
            &request(
                "metrics/obsoletas.yaml",
                json!({ "selections": [metric("v", "vieja"), metric("a", "antigua")] }),
            ),
        )
        .await;
    let notices: Vec<_> = deprecated["notices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            let name = n["metric"].as_str().unwrap().rsplit('/').next().unwrap();
            let replacement = n["replacement"]
                .as_str()
                .map(|r| r.rsplit('/').next().unwrap());
            (name, n["sunset"].as_str().unwrap(), replacement)
        })
        .collect();
    assert_eq!(
        notices,
        [
            ("antigua", "2027-06-30", None),
            ("vieja", "2027-01-01", Some("importe_total"))
        ]
    );
    fixture("deprecated", &deprecated);

    // More groups than the limit.
    let limited = h
        .query(
            &ana,
            &request(
                ventas,
                json!({
                    "selections": [
                        dimension("r", "ventas.region"),
                        metric("t", "importe_total")
                    ],
                    "order_by": [{ "selection": "r" }],
                    "limit": 1
                }),
            ),
        )
        .await;
    assert_eq!(rows(&limited), &json!([["Norte", "100.30"]]));
    let block = &limited["result"]["blocks"][0];
    assert_eq!(
        (&block["row_count"], &block["limit"]),
        (&json!(1), &json!(1))
    );
    fixture("limited", &limited);

    // Beyond DECIMAL(38): the cube of `importe`, aggregated exactly by the
    // emulation. Written out by hand: 100.10³ + 0.20³ + 200.20³ + (-0.20)³.
    let wide = h
        .query(
            &ana,
            &request(
                "metrics/anchos.yaml",
                json!({ "selections": [metric("c", "cubo")] }),
            ),
        )
        .await;
    assert_eq!(
        wide["result"]["blocks"][0]["columns"][0]["type"],
        "DECIMAL(56,6)"
    );
    assert_eq!(rows(&wide), &json!([["9027027.009000"]]));
    fixture("wide", &wide);

    // The same cube by region, in the API's canonical order and limit
    // (#256): Norte = 100.10³ + 0.20³, Sur = 200.20³ + (-0.20)³.
    let cube_by_region = |extra: Value| {
        let mut query = json!({
            "selections": [dimension("r", "ventas.region"), metric("c", "cubo")]
        });
        for (key, value) in extra.as_object().unwrap() {
            query[key] = value.clone();
        }
        request("metrics/anchos.yaml", query)
    };
    let grouped = h.query(&ana, &cube_by_region(json!({}))).await;
    let block = &grouped["result"]["blocks"][0];
    assert_eq!(block["columns"][1]["type"], "DECIMAL(56,6)");
    assert_eq!(
        rows(&grouped),
        &json!([["Norte", "1003003.009000"], ["Sur", "8024024.000000"]])
    );
    assert_eq!(
        (&block["row_count"], &block["limit"]),
        (&json!(2), &json!(1000))
    );
    fixture("wide_grouped", &grouped);
    // Ordered by the cube descending, then cut: limit 1 is Sur, offset 1
    // Norte, an offset past both no rows.
    let order = json!([{ "selection": "c", "descending": true }, { "selection": "r" }]);
    for (window, expected) in [
        (json!({ "limit": 1 }), json!([["Sur", "8024024.000000"]])),
        (
            json!({ "limit": 1, "offset": 1 }),
            json!([["Norte", "1003003.009000"]]),
        ),
        (json!({ "limit": 1, "offset": 2 }), json!([])),
    ] {
        let mut extra = window.clone();
        extra["order_by"] = order.clone();
        let cut = h.query(&ana, &cube_by_region(extra)).await;
        assert_eq!(rows(&cut), &expected, "{window}");
        assert_eq!(
            cut["result"]["blocks"][0]["columns"][1]["type"],
            "DECIMAL(56,6)"
        );
    }

    // A distinct count beside other totals, without a grain (#258): one
    // row, in the order asked, each value as it is alone.
    // Written only now: the catalog fixture above is the project as it was.
    let distintos = "metrics/distintos.yaml";
    std::fs::write(
        h.project.path().join(distintos),
        format!("{MODEL}{DISTINCT}"),
    )
    .unwrap();
    let nothing = json!({
        "op": "compare",
        "target": { "kind": "dimension", "name": "ventas.region" },
        "comparison": "=",
        "value": { "type": "text", "value": "Nada" }
    });
    for (selected, filter, expected) in [
        (
            vec![("c", "ciudades"), ("f", "filas")],
            None,
            json!([["4", "4"]]),
        ),
        (
            vec![("f", "filas"), ("c", "ciudades")],
            None,
            json!([["4", "4"]]),
        ),
        (
            vec![("d", "importes_distintos"), ("t", "importe_total")],
            None,
            json!([["4", "300.30"]]),
        ),
        // Nothing passes: COUNT is 0, SUM is NULL, still one row.
        (
            vec![("c", "ciudades"), ("f", "filas")],
            Some(&nothing),
            json!([["0", "0"]]),
        ),
        (
            vec![("c", "ciudades"), ("t", "importe_total")],
            Some(&nothing),
            json!([["0", null]]),
        ),
    ] {
        let mut query = json!({
            "selections": selected.iter().map(|(id, name)| metric(id, name)).collect::<Vec<_>>()
        });
        if let Some(filter) = filter {
            query["filter"] = filter.clone();
        }
        let (status, answer) = h.answer(&ana, &request(distintos, query)).await;
        assert_eq!(status, StatusCode::OK, "{selected:?}: {answer:#}");
        assert_eq!(rows(&answer), &expected, "{selected:?}");
        let columns: Vec<(&str, &str)> = answer["result"]["blocks"][0]["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["selection_id"].as_str().unwrap(),
                    c["type"].as_str().unwrap(),
                )
            })
            .collect();
        let ids: Vec<&str> = selected.iter().map(|(id, _)| *id).collect();
        assert_eq!(columns.iter().map(|c| c.0).collect::<Vec<_>>(), ids);
        assert!(
            columns
                .iter()
                .all(|c| c.1 == "INT64" || c.1 == "DECIMAL(38,2)")
        );
    }
    // The grouped control keeps its groups, counts and canonical order.
    let grouped = h
        .query(
            &ana,
            &request(
                distintos,
                json!({ "selections": [
                    dimension("r", "ventas.region"),
                    metric("c", "ciudades"),
                    metric("f", "filas")
                ] }),
            ),
        )
        .await;
    assert_eq!(
        rows(&grouped),
        &json!([["Norte", "2", "2"], ["Sur", "2", "2"]])
    );

    // A masked column is still refused whole: there is no `masked.json`
    // until the server can answer one.
    let (status, refused) = h
        .answer(
            &ana,
            &request(ventas, json!({ "selections": [metric("m", "margen")] })),
        )
        .await;
    assert_eq!(
        (status, &refused["code"]),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            &json!("masked_unsupported")
        ),
        "{refused:#}"
    );

    h.db.close().await;
}
