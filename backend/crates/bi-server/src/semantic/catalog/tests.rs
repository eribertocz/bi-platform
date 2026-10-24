//! The session's catalog on real Ossie projects, real Cedar policies and a
//! real engine: what it announces is what the session's queries accept,
//! and nothing it hides leaves, nor changes what it shows.

use std::time::{Duration, Instant};

use proptest::prelude::*;
use serde_json::{Value, json};

use super::*;
use crate::analytics::Analytics;
use crate::semantic::prepare::tests::composition::{
    ROW_POLICY, ROWS, analytics, project, ready, selections, session, state,
};

const SENTINELS: [&str; 6] = [
    "descripcion_centinela",
    "contexto_ia_centinela",
    "reserva_centinela",
    "secreto_centinela",
    "ratio_centinela",
    "diagnostico_centinela",
];

const MEMBERS: &str = "version: 1\nusers:\n  lector: [viewer]\n  analista: [analyst]\n  jefa: [admin]\n  inspector: [catalog_reader]\n";

fn extension(indent: &str, data: &str) -> String {
    format!(
        "{indent}custom_extensions:\n{indent}  - vendor_name: bi-platform\n{indent}    data: '{data}'\n"
    )
}

fn field(name: &str, extra: &str) -> String {
    format!(
        "      - name: {name}\n{extra}        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: {name}\n"
    )
}

/// One metric of `expression`, with `extra` lines (extensions, description).
fn metric(name: &str, extra: &str, expression: &str) -> String {
    format!(
        "  - name: {name}\n{extra}    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: {expression}\n"
    )
}

fn sensitivity(level: &str) -> String {
    extension(
        "    ",
        &format!("{{\"version\": 1, \"sensitivity\": \"{level}\"}}"),
    )
}

fn deprecated(replacement: &str) -> String {
    extension(
        "    ",
        &format!(
            "{{\"version\": 1, \"status\": {{\"state\": \"deprecated\", \"sunset\": \"2027-01-01\", \"replacement\": \"{replacement}\"}}}}"
        ),
    )
}

/// The project of the Issue: `ventas` over the four rows, with a
/// restricted field, a confidential one (masked-only for a viewer), a
/// masked metric, a restricted one and one derived from it, deprecated
/// metrics with a visible and a hidden replacement, and free text that must
/// never leave. `hidden` adds more fields and `metrics` more metrics.
fn ventas(extensions: &str, hidden_fields: &str, metrics: &str) -> String {
    let dimension = "        dimension: {}\n";
    let restricted = extension("        ", r#"{"version": 1, "sensitivity": "restricted"}"#);
    let confidential = extension(
        "        ",
        r#"{"version": 1, "sensitivity": "confidential"}"#,
    );
    let bob = extension("        ", r#"{"version": 1, "unit": {"currency": "BOB"}}"#);
    let fields = [
        field("id", ""),
        field("region", dimension),
        field("ciudad", dimension),
        field("importe", &bob),
        field(
            "instante",
            "        datatype: DateTimeTz\n        dimension: {}\n",
        ),
        field("nota", &format!("{dimension}{restricted}")),
        // Over `nota`: a confidential dimension a viewer may only mask.
        format!(
            "      - name: secreto_centinela\n{dimension}{confidential}        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: nota\n"
        ),
    ]
    .concat();
    let free_text =
        "    description: descripcion_centinela\n    ai_context: contexto_ia_centinela\n";
    let all_metrics = [
        metric("importe_total", free_text, "SUM(ventas.importe)"),
        metric("filas", "", "COUNT(ventas.id)"),
        metric(
            "margen",
            &sensitivity("confidential"),
            "SUM(ventas.importe)",
        ),
        metric(
            "reserva_centinela",
            &sensitivity("restricted"),
            "SUM(ventas.importe)",
        ),
        metric("derivada", "", "reserva_centinela"),
        metric(
            "ratio_centinela",
            &sensitivity("restricted"),
            "SUM(ventas.importe) - MIN(ventas.importe)",
        ),
        metric("vieja", &deprecated("importe_total"), "SUM(ventas.importe)"),
        metric(
            "vieja_oculta",
            &deprecated("reserva_centinela"),
            "SUM(ventas.importe)",
        ),
        metrics.to_owned(),
    ]
    .concat();
    format!(
        "version: '0.2.0.dev0'\nname: Ventas\n{extensions}datasets:\n  - name: ventas\n    source: lake.main.ventas\n    primary_key: [id]\n    fields:\n{fields}{hidden_fields}metrics:\n{all_metrics}"
    )
}

fn sales() -> String {
    ventas("", "", "")
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

fn bob() -> Value {
    json!({ "kind": "currency", "code": "BOB" })
}

async fn catalog_of(state: &AppState, user: &str) -> Value {
    let catalog = for_session(state, session(user, &[]))
        .await
        .unwrap_or_else(|error| panic!("{user}: {error:?}"));
    serde_json::to_value(catalog).unwrap()
}

fn names(catalog: &Value, list: &str) -> Vec<String> {
    catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|model| model[list].as_array().unwrap())
        .map(|member| member["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_viewer_is_announced_exactly_what_its_queries_accept() {
    let p = project(&sales(), MEMBERS);
    let state = state(p.path(), analytics().await);
    let catalog = catalog_of(&state, "lector").await;
    // The oracle, written out by hand: the masked metric is selectable
    // masked only; what is restricted, derived from it, split out of a
    // metric by the compiler or masked-only as a field is absent.
    let decimal = "DECIMAL(18,2)";
    let mut vieja = member("vieja", decimal, "values", false, &bob());
    vieja["lifecycle"] =
        json!({ "state": "deprecated", "sunset": "2027-01-01", "replacement": "importe_total" });
    let mut vieja_oculta = member("vieja_oculta", decimal, "values", false, &bob());
    vieja_oculta["lifecycle"] =
        json!({ "state": "deprecated", "sunset": "2027-01-01", "replacement": null });
    assert_eq!(
        catalog,
        json!({ "models": [{
            "model": "metrics/ventas.yaml",
            "name": "Ventas",
            "metrics": [
                member("filas", "INT64", "values", false, &json!({ "kind": "count" })),
                member("importe_total", decimal, "values", false, &bob()),
                member("margen", decimal, "masked", false, &bob()),
                vieja,
                vieja_oculta,
            ],
            "fields": [
                member("ventas.ciudad", "VARCHAR", "values", true, &Value::Null),
                member("ventas.instante", "TIMESTAMPTZ", "values", true, &Value::Null),
                member("ventas.region", "VARCHAR", "values", true, &Value::Null),
            ],
        }]})
    );
    let text = catalog.to_string();
    for forbidden in SENTINELS.iter().copied().chain([
        "SUM(",
        "lake.main",
        "project-file",
        "/metric/",
        "restricted",
        "viewer",
        "fingerprint",
        &p.path().to_string_lossy(),
    ]) {
        assert!(!text.contains(forbidden), "{forbidden} leaked: {text}");
    }
    // Deterministic, and the same for every verified session of the role.
    assert_eq!(catalog_of(&state, "lector").await, catalog);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_announcement_is_backed_by_the_query_path() {
    let p = project(&sales(), MEMBERS);
    let state = state(p.path(), analytics().await);
    let catalog = catalog_of(&state, "lector").await;
    // Each announced metric prepares alone, masked exactly when announced
    // masked; each field prepares next to an announced metric.
    for metric in catalog["models"][0]["metrics"].as_array().unwrap() {
        let name = metric["name"].as_str().unwrap();
        let prepared = ready(
            &state,
            session("lector", &[]),
            &selections(&json!([{ "kind": "metric", "id": "m", "name": name }])),
        )
        .await;
        assert_eq!(
            prepared.decision().constraints.is_empty(),
            metric["capabilities"]["select"] == "values",
            "{name}"
        );
    }
    for field in catalog["models"][0]["fields"].as_array().unwrap() {
        let name = field["name"].as_str().unwrap();
        ready(
            &state,
            session("lector", &[]),
            &selections(&json!([
                { "kind": "dimension", "id": "d", "name": name },
                { "kind": "metric", "id": "m", "name": "filas" }
            ])),
        )
        .await;
    }
    // What is left out is refused by the same path.
    for name in ["reserva_centinela", "derivada", "ratio_centinela"] {
        let request = selections(&json!([{ "kind": "metric", "id": "m", "name": name }]));
        assert!(
            prepare::prepare(&state, session("lector", &[]), &request)
                .await
                .is_err(),
            "{name}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_role_gets_its_own_projection_and_masking_grants_no_other_use() {
    let p = project(&sales(), MEMBERS);
    let state = state(p.path(), analytics().await);
    // The analyst masks what is restricted: a masked metric is selectable,
    // never filtered nor ordered; a masked-only field cannot be grouped,
    // so it is not announced at all, and neither is a metric derived from
    // a masked one.
    let analyst = catalog_of(&state, "analista").await;
    let metrics = &analyst["models"][0]["metrics"];
    let reserva = metrics
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "reserva_centinela")
        .expect("masked for the analyst");
    assert_eq!(
        reserva["capabilities"],
        json!({ "select": "masked", "filter": false, "group": false, "order": false })
    );
    assert!(!names(&analyst, "metrics").contains(&"derivada".to_owned()));
    assert_eq!(
        names(&analyst, "fields"),
        [
            "ventas.ciudad",
            "ventas.instante",
            "ventas.region",
            "ventas.secreto_centinela"
        ]
    );
    // The replacement is named once it is announced.
    let vieja_oculta = metrics
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "vieja_oculta")
        .unwrap();
    assert_eq!(
        vieja_oculta["lifecycle"]["replacement"],
        "reserva_centinela"
    );
    // The administrator sees everything, but never what the compiler split
    // out of a metric.
    let admin = catalog_of(&state, "jefa").await;
    let metrics = names(&admin, "metrics");
    assert!(metrics.contains(&"derivada".to_owned()));
    assert!(metrics.contains(&"ratio_centinela".to_owned()));
    assert!(!metrics.iter().any(|m| m.starts_with("ratio_centinela.")));
    assert!(names(&admin, "fields").contains(&"ventas.nota".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn inspection_membership_or_a_forbid_never_widen_the_catalog() {
    let p = project(&sales(), MEMBERS);
    let state = state(p.path(), analytics().await);
    // Inspecting the whole catalog is no permission on its data.
    let inspection = "permit (principal in Role::\"catalog_reader\", action == Action::\"view\", resource == Model::\"project:catalog\");";
    std::fs::write(
        p.path().join("policies/project.cedar"),
        format!("{}\n{inspection}", sef_authz::BASE_POLICIES),
    )
    .unwrap();
    let empty = json!({ "models": [] });
    assert_eq!(catalog_of(&state, "inspector").await, empty);
    // No membership, no catalog.
    assert_eq!(catalog_of(&state, "nadie").await, empty);
    // An explicit forbid binds the administrator.
    std::fs::write(
        p.path().join("policies/project.cedar"),
        format!(
            "{}\nforbid (principal, action, resource is Metric);",
            sef_authz::BASE_POLICIES
        ),
    )
    .unwrap();
    assert_eq!(catalog_of(&state, "jefa").await, empty);
    // A forbid on one field leaves the rest of the model.
    std::fs::write(
        p.path().join("policies/project.cedar"),
        format!(
            "{}\nforbid (principal, action, resource is Dimension);",
            sef_authz::BASE_POLICIES
        ),
    )
    .unwrap();
    let admin = catalog_of(&state, "jefa").await;
    assert_eq!(names(&admin, "fields"), Vec::<String>::new());
    assert_eq!(names(&admin, "metrics").len(), 8);
    // Corrupt policies close it for everyone.
    std::fs::write(p.path().join("policies/project.cedar"), "permit (principal").unwrap();
    assert_eq!(
        for_session(&state, session("jefa", &[])).await,
        Err(CatalogError::PoliciesInvalid)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn row_policies_decide_with_the_verified_context_only() {
    let p = project(&ventas(ROW_POLICY, "", ""), MEMBERS);
    let state = state(p.path(), analytics().await);
    let a = for_session(&state, session("lector", &[("region", json!("Norte"))]))
        .await
        .unwrap();
    let b = for_session(&state, session("lector", &[("region", json!("Sur"))]))
        .await
        .unwrap();
    // The same definition and snapshot: the same catalog, whatever rows
    // each one's policy keeps.
    assert_eq!(a, b);
    assert_eq!(a.models.len(), 1);
    // Without the verified attribute no query has a correct result: not
    // usable, which is not a denial, and nothing is announced.
    for attributes in [vec![], vec![("region", json!(7))]] {
        let none = for_session(&state, session("lector", &attributes))
            .await
            .unwrap();
        assert_eq!(none, SessionCatalog { models: Vec::new() });
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_that_does_not_compile_leaves_no_partial_catalog() {
    let p = project(&sales(), MEMBERS);
    let state = state(p.path(), analytics().await);
    let before = catalog_of(&state, "lector").await;
    // A second file with an error whose diagnostic carries a sentinel.
    let broken = "version: '0.2.0.dev0'\nname: Rota\ndatasets:\n  - name: rota\n    source: lake.main.diagnostico_centinela\n    primary_key: [id]\n    fields:\n      - name: id\n        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: id\nmetrics: []\n";
    std::fs::write(p.path().join("metrics/rota.yaml"), broken).unwrap();
    for user in ["lector", "jefa", "inspector"] {
        let failed = for_session(&state, session(user, &[])).await;
        assert_eq!(failed, Err(CatalogError::Unavailable), "{user}");
        assert!(!format!("{failed:?}").contains("centinela"));
    }
    // Fixed, it answers again; broken again, the old catalog never returns.
    std::fs::remove_file(p.path().join("metrics/rota.yaml")).unwrap();
    assert_eq!(catalog_of(&state, "lector").await, before);
    std::fs::write(p.path().join("metrics/ventas.yaml"), "name: [").unwrap();
    assert_eq!(
        for_session(&state, session("lector", &[])).await,
        Err(CatalogError::Unavailable)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn global_conditions_fail_closed_and_bounded() {
    let analytics = analytics().await;
    // No project, an empty one, no metrics folder.
    let none = AppState {
        models: crate::models::Models::new(None),
        ..state(std::path::Path::new("."), analytics.clone())
    };
    assert_eq!(
        for_session(&none, session("lector", &[])).await,
        Err(CatalogError::NotConfigured)
    );
    let empty = tempfile::tempdir().unwrap();
    std::fs::create_dir(empty.path().join("metrics")).unwrap();
    let empty_state = state(empty.path(), analytics.clone());
    assert_eq!(
        for_session(&empty_state, session("lector", &[])).await,
        Ok(SessionCatalog { models: Vec::new() })
    );
    let missing = tempfile::tempdir().unwrap();
    assert_eq!(
        for_session(
            &state(missing.path(), analytics.clone()),
            session("lector", &[])
        )
        .await,
        Err(CatalogError::Unavailable)
    );
    // Compilations already admitted: refused before reading anything.
    let p = project(&sales(), MEMBERS);
    let busy = state(p.path(), analytics.clone());
    let held = busy
        .models
        .admission
        .clone()
        .try_acquire_many_owned(2)
        .unwrap();
    assert_eq!(
        for_session(&busy, session("lector", &[])).await,
        Err(CatalogError::Busy)
    );
    drop(held);
    // No time left: nothing is answered.
    let late = AppState {
        query_timeout: Duration::ZERO,
        ..busy.clone()
    };
    assert_eq!(
        for_session(&late, session("lector", &[])).await,
        Err(CatalogError::Timeout)
    );
    // An engine that is not ready.
    let down = AppState {
        analytics: Analytics::disabled(),
        ..busy.clone()
    };
    assert_eq!(
        for_session(&down, session("lector", &[])).await,
        Err(CatalogError::Unavailable)
    );
    // Every permit comes back: the work abandoned at its deadline releases
    // its own when the native read it travels with ends, promptly.
    let back = tokio::time::timeout(
        Duration::from_secs(5),
        busy.models.admission.acquire_many(2),
    )
    .await
    .expect("every permit is released within the bound");
    drop(back.unwrap());
}

/// The rows of the Issue behind a view that fails when anything reads a
/// row: describing it works, scanning it does not.
async fn trapped() -> Analytics {
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

#[tokio::test(flavor = "multi_thread")]
async fn the_projection_reads_no_rows_and_its_work_is_bounded() {
    let analytics = trapped().await;
    // Control: anything that reads a row of the source fails.
    let scanned = analytics
        .runtime()
        .unwrap()
        .run(|conn| {
            conn.query_row("SELECT count(*) FROM lake.main.ventas", [], |row| {
                row.get::<_, i64>(0)
            })
        })
        .await;
    assert!(scanned.is_err(), "the trap must fire on a scan");
    let p = project(&sales(), MEMBERS);
    let state = state(p.path(), analytics);
    for user in ["lector", "analista", "jefa", "nadie"] {
        let (catalog, work) = project_with_work(&state, session(user, &[]))
            .await
            .unwrap_or_else(|error| panic!("{user}: {error:?}"));
        // One file read and compiled; one minimal query per declared
        // metric, and per field at most one per announced metric (here the
        // first one always pairs).
        assert_eq!((work.files, work.compiled), (1, 1), "{user}");
        let metrics = catalog.models.first().map_or(0, |m| m.metrics.len());
        let fields = catalog.models.first().map_or(0, |m| m.fields.len());
        let declared = 8;
        assert!(work.governed <= declared + 7 * metrics.min(MAX_PAIRINGS_PER_FIELD));
        if metrics > 0 {
            assert_eq!(work.governed, declared + fields, "{user}");
        } else {
            // Nothing announced: no field is ever paired.
            assert_eq!(work.governed, declared, "{user}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_field_no_announced_metric_reaches_is_left_out_within_its_bound() {
    // A second dataset no relationship joins: its field pairs with no
    // metric of `ventas`, and is tried at most MAX_PAIRINGS_PER_FIELD times.
    let mut text = sales();
    text = text.replacen(
        "metrics:\n",
        "  - name: otros\n    source: lake.main.ventas\n    primary_key: [id]\n    fields:\n      - name: id\n        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: id\n      - name: canal\n        dimension: {}\n        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: ciudad\nmetrics:\n",
        1,
    );
    let p = project(&text, MEMBERS);
    let state = state(p.path(), analytics().await);
    let (catalog, work) = project_with_work(&state, session("lector", &[]))
        .await
        .unwrap();
    let catalog = serde_json::to_value(catalog).unwrap();
    assert!(!names(&catalog, "fields").contains(&"otros.canal".to_owned()));
    let metrics = names(&catalog, "metrics").len();
    let fields = names(&catalog, "fields").len();
    assert_eq!(
        work.governed,
        8 + fields + metrics.min(MAX_PAIRINGS_PER_FIELD)
    );
}

/// Hidden objects added to the project of the Issue: restricted fields and
/// metrics, metrics derived from them, and a file with nothing visible.
#[derive(Debug, Clone)]
struct Hidden {
    fields: Vec<String>,
    metrics: Vec<(String, bool)>,
    file: Option<String>,
}

fn hidden() -> impl Strategy<Value = Hidden> {
    let name = "[a-z]{3,10}".prop_map(|s| format!("h_{s}"));
    (
        prop::collection::btree_set(name.clone(), 0..3),
        prop::collection::btree_map(name.clone(), any::<bool>(), 0..4),
        prop::option::of(name),
    )
        .prop_map(|(fields, metrics, file)| Hidden {
            fields: fields.into_iter().collect(),
            metrics: metrics.into_iter().collect(),
            file,
        })
}

fn with_hidden(hidden: &Hidden) -> (String, Option<String>) {
    let restricted = extension("        ", r#"{"version": 1, "sensitivity": "restricted"}"#);
    let fields = hidden
        .fields
        .iter()
        .map(|name| {
            format!(
                "      - name: {name}\n        dimension: {{}}\n{restricted}        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: ciudad\n"
            )
        })
        .collect::<Vec<_>>()
        .concat();
    let metrics: String = hidden
        .metrics
        .iter()
        .map(|(name, derived)| {
            if *derived {
                // Visible itself, derived from a restricted metric.
                metric(name, "", "reserva_centinela + reserva_centinela")
            } else {
                metric(name, &sensitivity("restricted"), "COUNT(ventas.id)")
            }
        })
        .collect();
    let file = hidden.file.as_ref().map(|name| {
        format!(
            "version: '0.2.0.dev0'\nname: {name}\ndatasets:\n  - name: {name}\n    source: lake.main.ventas\n    primary_key: [id]\n    fields:\n      - name: id\n        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: id\nmetrics:\n{}",
            metric(name, &sensitivity("restricted"), &format!("COUNT({name}.id)"))
        )
    });
    (ventas("", &fields, &metrics), file)
}

#[test]
fn hidden_objects_never_change_what_is_announced() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let _context = rt.enter();
    let analytics = rt.block_on(analytics());
    let baseline = |user: &str| {
        let p = project(&sales(), MEMBERS);
        let state = state(p.path(), analytics.clone());
        rt.block_on(for_session(&state, session(user, &[])))
            .unwrap()
    };
    // Restricted objects are hidden from a viewer (an analyst masks them);
    // a metric derived from a restricted one is hidden from both.
    let expected = [("lector", baseline("lector")), ("nadie", baseline("nadie"))];
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig::with_cases(24));
    runner
        .run(&hidden(), |hidden| {
            let (text, file) = with_hidden(&hidden);
            let p = project(&text, MEMBERS);
            if let Some(file) = &file {
                // Before and after `ventas.yaml` in file order.
                std::fs::write(p.path().join("metrics/a_hidden.yaml"), file).unwrap();
                std::fs::write(p.path().join("metrics/z_hidden.yaml"), file).unwrap();
            }
            let state = state(p.path(), analytics.clone());
            for (user, expected) in &expected {
                let projected = rt.block_on(for_session(&state, session(user, &[])));
                prop_assert_eq!(projected.as_ref(), Ok(expected), "{}", user);
            }
            Ok(())
        })
        .unwrap();
}

/// Cold and warm latency of the projection on the Issue's project, from
/// `cargo test ... catalog_latency -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "measurement"]
async fn catalog_latency() {
    let p = project(&sales(), MEMBERS);
    let state = state(p.path(), analytics().await);
    let started = Instant::now();
    for_session(&state, session("lector", &[])).await.unwrap();
    let cold = started.elapsed();
    let mut samples = Vec::new();
    for _ in 0..200 {
        let started = Instant::now();
        for_session(&state, session("lector", &[])).await.unwrap();
        samples.push(started.elapsed());
    }
    samples.sort();
    let at = |q: usize| samples[samples.len() * q / 100];
    println!(
        "catalog: cold {cold:?}, warm p50 {:?}, p95 {:?} (n={})",
        at(50),
        at(95),
        samples.len()
    );
}
