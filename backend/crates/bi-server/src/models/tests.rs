//! The listing an allowed session gets (#124 decides before this point,
//! see `tests/models.rs`), on a real DuckDB, plus the project reader's
//! bounds. No fixture catalog or mock compiler is used.

use std::path::Path;

use proptest::prelude::*;
use serde_json::Value;

use super::*;
use crate::analytics::Analytics;
use crate::db::Database;
use crate::metadata::MetadataMode;

const VALID: &str = include_str!("../../../../../examples/models-project/metrics/ventas.yaml");
const INVALID: &str =
    include_str!("../../../../../examples/models-project/metrics/origen-inexistente.yaml");

#[test]
fn public_types_never_show_internal_rust_metadata() {
    assert_eq!(type_label(&ValueType::Time), "TIME");
    let opaque = ValueType::OpaqueExtension {
        namespace: "private_namespace".into(),
        type_name: "internal_type".into(),
    };
    assert_eq!(
        type_label(&opaque),
        "Tipo sin representación pública definida"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn nofollow_rejects_links_for_arbitrary_filenames(name in "[a-zA-Z0-9_-]{1,40}") {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "never expose this").unwrap();
        std::fs::create_dir(root.path().join("metrics")).unwrap();
        let filename = format!("{name}.yaml");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("secret"), root.path().join("metrics").join(&filename)).unwrap();
        #[cfg(windows)]
        {
            let output = std::process::Command::new("cmd").args(["/c", "mklink", "/J"])
                .arg(root.path().join("metrics").join(&filename)).arg(outside.path()).output().unwrap();
            prop_assert!(output.status.success());
        }
        let loaded = read_project(root.path()).unwrap();
        prop_assert_eq!(loaded.len(), 1);
        prop_assert_eq!(loaded[0].text.as_ref().unwrap_err().code.as_str(), "unsafe_file");
    }
}

#[test]
fn directory_entries_budget_includes_ignored_extensions() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("metrics")).unwrap();
    for i in 0..=MAX_DIRECTORY_ENTRIES {
        std::fs::write(root.path().join(format!("metrics/{i}.txt")), "").unwrap();
    }
    assert_eq!(
        read_project(root.path()).err().unwrap().code,
        "too_many_entries"
    );
}

#[tokio::test]
async fn annotations_keep_opaque_context_defaults_and_explicit_time_overrides() {
    let p = project();
    write(p.path(), "ventas.yaml", VALID);
    let state = app(p.path(), analytics().await);
    let (_, initial) = response(&state).await;
    let model = &initial["models"][0];
    assert_eq!(model["status"], "valid", "{initial:#}");
    assert_eq!(model["description"], "Métricas de ventas del proyecto.");
    assert_eq!(model["datasets"][0]["description"], "Una fila por pedido.");
    let region = &model["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "sales.region")
        .unwrap()["annotations"][0];
    assert_eq!(region["label"], "Región comercial");
    assert_eq!(
        region["ai_context"]["vendor_hint"],
        serde_json::json!({"category": "geography"})
    );
    assert_eq!(
        region["ai_context"]["examples"],
        serde_json::json!(["Norte"])
    );
    let time = &model["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "sales.ordered_at")
        .unwrap()["annotations"][0];
    assert_eq!(time["is_time"], true);
    assert_eq!(time["declared_is_time"], Value::Null);
    assert_eq!(
        model["metrics"][0]["annotations"][0]["ai_context"]["synonyms"],
        serde_json::json!(["facturación"])
    );
    let fingerprint = model["fingerprint"].clone();
    // Override a temporal datatype and explicitly mark a text field as time.
    let changed = VALID
        .replace(
            "datatype: DateTimeTz\n        dimension: {}",
            "datatype: DateTimeTz\n        dimension: {is_time: false}",
        )
        .replacen(
            "dimension: {}",
            "datatype: String\n        dimension: {is_time: true}",
            1,
        );
    write(p.path(), "ventas.yaml", &changed);
    let (_, body) = response(&state).await;
    let model = &body["models"][0];
    assert_eq!(model["status"], "valid", "{body:#}");
    assert_eq!(model["fingerprint"], fingerprint);
    assert_eq!(model["diagnostics"][0]["code"], "non_temporal_time_role");
    assert_eq!(model["diagnostics"][0]["severity"], "warning");
    assert_eq!(
        model["diagnostics"][0]["path"],
        "/datasets/0/fields/1/dimension/is_time"
    );
    let time = &model["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "sales.ordered_at")
        .unwrap()["annotations"][0];
    assert_eq!(time["is_time"], false);
    assert_eq!(time["declared_is_time"], false);
}

#[tokio::test]
async fn declared_computed_fields_keep_metadata_even_when_the_engine_inlines_them() {
    let mut document = sef_ossie::load(VALID).unwrap().model;
    let mut field = document.datasets[0].fields[2].clone();
    field.name = "double_amount".into();
    field.expression.dialects[0].expression = "amount + amount".into();
    field.dimension = None;
    field.datatype = None;
    field.description = Some("Importe duplicado sin agregación.".into());
    field.ai_context = Some(sef_ossie::document::AiContext::Text(
        "Contexto libre".into(),
    ));
    document.datasets[0].fields.push(field);
    let p = project();
    write(
        p.path(),
        "ventas.yaml",
        &sef_ossie::to_yaml(&document).unwrap(),
    );
    let (_, body) = response(&app(p.path(), analytics().await)).await;
    assert_eq!(body["models"][0]["status"], "valid", "{body:#}");
    let field = body["models"][0]["datasets"][0]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "double_amount")
        .unwrap();
    assert_eq!(field["is_dimension"], false);
    assert_eq!(
        field["annotation"]["description"],
        "Importe duplicado sin agregación."
    );
    assert_eq!(field["annotation"]["ai_context"], "Contexto libre");
}

fn project() -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join("metrics")).unwrap();
    project
}

fn write(project: &Path, file: &str, text: &str) {
    std::fs::write(project.join("metrics").join(file), text).unwrap();
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
    runtime.run(|conn| conn.execute_batch("ATTACH ':memory:' AS lake; CREATE TABLE lake.main.sales (order_id BIGINT NOT NULL, ordered_at TIMESTAMPTZ, region VARCHAR, quantity INTEGER, amount DECIMAL(18,2));")).await.unwrap();
    Analytics::ready(runtime)
}

fn app(project: &Path, analytics: Analytics) -> AppState {
    AppState {
        models: Models::new(Some(project.to_owned())),
        analytics,
        ..AppState::new(Database::start(
            &"postgres://bi:bi@127.0.0.1:1/bi".parse().unwrap(),
            MetadataMode::Development,
        ))
    }
}

/// What the client reads once the catalog decision allowed the request.
async fn response(state: &AppState) -> (StatusCode, Value) {
    let directory = state.models.directory.clone().unwrap();
    let permit = state.models.admission.clone().try_acquire_owned().unwrap();
    match answer(state, directory, permit).await {
        Ok(body) => (StatusCode::OK, serde_json::to_value(body).unwrap()),
        Err((status, code, _)) => (status, serde_json::json!({ "code": code })),
    }
}

#[tokio::test]
async fn missing_folder_and_empty_folder_are_distinct() {
    // Without a configured project there is nothing to decide on: see
    // `an_unconfigured_instance_answers_not_configured_without_a_decision`.
    let p = project();
    let (_, body) = response(&app(&p.path().join("missing"), Analytics::disabled())).await;
    assert_eq!(body["project_status"], "unavailable");
    assert_eq!(body["diagnostics"][0]["code"], "project_unavailable");
    let (_, body) = response(&app(p.path(), Analytics::disabled())).await;
    assert_eq!(body["project_status"], "ready");
    assert_eq!(body["models"], serde_json::json!([]));
}

#[tokio::test]
async fn configured_models_require_a_ready_analytical_runtime() {
    let p = project();
    write(p.path(), "ventas.yaml", VALID);
    let (status, body) = response(&app(p.path(), Analytics::disabled())).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "analytics_unavailable");
}

#[tokio::test]
async fn compiles_each_file_with_exact_metadata_and_reloads_without_stale_values() {
    let p = project();
    write(p.path(), "ventas.yaml", VALID);
    write(p.path(), "bad.yml", INVALID);
    let loaded = sef_ossie::load::load(VALID).unwrap();
    write(
        p.path(),
        "json.json",
        &sef_ossie::load::to_json(&loaded.model).unwrap(),
    );
    write(p.path(), "ignore.txt", "invalid");
    std::fs::create_dir(p.path().join("metrics/nested")).unwrap();
    std::fs::write(p.path().join("metrics/nested/hidden.yaml"), VALID).unwrap();
    let app = app(p.path(), analytics().await);
    let (status, first) = response(&app).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["models"].as_array().unwrap().len(), 3);
    let valid = &first["models"][2];
    assert_eq!(valid["status"], "valid", "{first:#}");
    assert_eq!(valid["metrics"][0]["name"], "importe_total");
    assert_eq!(valid["metrics"][0]["data_type"], "DECIMAL(18,2)");
    let region = valid["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "sales.region")
        .unwrap();
    assert_eq!(region["data_type"], "VARCHAR");
    assert_eq!(region["nullable"], true);
    assert!(
        valid["fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("sef-sha256-v1:")
    );
    assert_eq!(
        first["models"][0]["diagnostics"][0]["code"],
        "unknown_source"
    );
    assert_eq!(
        first["models"][0]["diagnostics"][0]["path"],
        "/datasets/0/source"
    );
    // Where `source:` names the missing table.
    assert_eq!(first["models"][0]["diagnostics"][0]["line"], 5);
    assert_eq!(first["models"][0]["diagnostics"][0]["column"], 13);
    // A conversion error: located, and quoting names as written.
    write(
        p.path(),
        "ventas.yaml",
        &VALID.replace("SUM(sales.amount)", "SUM(sales.importe)"),
    );
    let (_, broken) = response(&app).await;
    let reference = &broken["models"][2]["diagnostics"][0];
    assert_eq!(reference["code"], "unknown_reference", "{broken:#}");
    assert_eq!(
        reference["message"],
        r#"El dataset "sales" no tiene el campo "importe"."#
    );
    assert_eq!(reference["line"], 56);
    assert_eq!(reference["column"], 23);
    write(p.path(), "ventas.yaml", "version: [\n");
    let (_, changed) = response(&app).await;
    let bad = &changed["models"][2];
    assert_eq!(bad["status"], "invalid");
    assert!(bad.get("metrics").is_none());
    assert!(bad.get("fingerprint").is_none());
    assert_eq!(bad["diagnostics"][0]["code"], "syntax");
    assert!(bad["diagnostics"][0]["line"].as_u64().is_some());
    assert!(bad["diagnostics"][0]["column"].as_u64().is_some());
    assert!(
        bad["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .starts_with("El archivo")
    );
    write(
        p.path(),
        "ventas.yaml",
        &VALID.replace("importe_total", "nueva_metrica"),
    );
    let (_, changed) = response(&app).await;
    assert_eq!(changed["models"][2]["metrics"][0]["name"], "nueva_metrica");
    assert_ne!(changed["models"][2]["fingerprint"], valid["fingerprint"]);
    std::fs::remove_file(p.path().join("metrics/ventas.yaml")).unwrap();
    assert_eq!(
        response(&app).await.1["models"].as_array().unwrap().len(),
        2
    );
}

#[tokio::test]
async fn too_large_duplicate_keys_and_unsupported_source_types_are_file_errors() {
    let p = project();
    write(
        p.path(),
        "large.yaml",
        &" ".repeat(sef_ossie::load::MAX_DOCUMENT_BYTES + 1),
    );
    write(
        p.path(),
        "duplicate.yaml",
        &format!("{VALID}\nname: otra\n"),
    );
    write(
        p.path(),
        "type.yaml",
        &VALID.replace("lake.main.sales", "bad"),
    );
    let analytics = analytics().await;
    analytics
        .runtime()
        .unwrap()
        .run(|conn| conn.execute_batch("CREATE TABLE bad (v HUGEINT)"))
        .await
        .unwrap();
    let (_, body) = response(&app(p.path(), analytics)).await;
    for (i, code) in ["syntax", "too_large", "unsupported_source_type"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(body["models"][i]["status"], "invalid");
        assert_eq!(body["models"][i]["diagnostics"][0]["code"], code);
    }
}

#[tokio::test]
async fn file_count_and_total_bytes_fail_without_partial_results() {
    let p = project();
    for i in 0..=MAX_FILES {
        write(p.path(), &format!("m{i}.yaml"), VALID);
    }
    let (_, body) = response(&app(p.path(), Analytics::disabled())).await;
    assert_eq!(body["diagnostics"][0]["code"], "too_many_files");
    assert_eq!(body["models"], serde_json::json!([]));
    let p = project();
    for i in 0..5 {
        write(
            p.path(),
            &format!("m{i}.yaml"),
            &" ".repeat(sef_ossie::load::MAX_DOCUMENT_BYTES),
        );
    }
    let (_, body) = response(&app(p.path(), Analytics::disabled())).await;
    assert_eq!(body["diagnostics"][0]["code"], "project_too_large");
    assert_eq!(body["models"], serde_json::json!([]));
}

#[cfg(unix)]
fn symlink_file(from: &Path, to: &Path) {
    std::os::unix::fs::symlink(from, to).unwrap();
}
#[cfg(unix)]
fn symlink_dir(from: &Path, to: &Path) {
    std::os::unix::fs::symlink(from, to).unwrap();
}
#[cfg(windows)]
fn symlink_dir(from: &Path, to: &Path) {
    // Junctions are reparse points available without Windows symlink privilege.
    let output = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(to.to_string_lossy().replace('/', "\\"))
        .arg(from.to_string_lossy().replace('/', "\\"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn symlink_file_metrics_directory_and_project_root_never_read_external_content() {
    let external = project();
    let secret = external.path().join("secret.yaml");
    std::fs::write(&secret, "SECRETO_NO_LEER").unwrap();
    let p = project();
    #[cfg(unix)]
    symlink_file(&secret, &p.path().join("metrics/link.yaml"));
    #[cfg(windows)]
    symlink_dir(external.path(), &p.path().join("metrics/link.yaml"));
    let (_, body) = response(&app(p.path(), analytics().await)).await;
    assert_eq!(body["models"][0]["diagnostics"][0]["code"], "unsafe_file");
    assert!(!body.to_string().contains("SECRETO_NO_LEER"));
    let root = tempfile::tempdir().unwrap();
    symlink_dir(
        &external.path().join("metrics"),
        &root.path().join("metrics"),
    );
    let (_, body) = response(&app(root.path(), Analytics::disabled())).await;
    assert_eq!(body["diagnostics"][0]["code"], "project_unavailable");
    let link = root.path().join("linked-root");
    symlink_dir(external.path(), &link);
    let (_, body) = response(&app(&link, Analytics::disabled())).await;
    assert_eq!(body["diagnostics"][0]["code"], "project_unavailable");
}

#[cfg(feature = "postgres-tests")]
#[tokio::test]
async fn compiles_the_synthetic_fixture_on_real_ducklake() {
    let data = tempfile::tempdir().unwrap();
    let data_path = data.path().to_owned();
    let runtime = tokio::task::spawn_blocking(move || {
        let config = bi_runtime::RuntimeConfig {
            lake: Some(bi_runtime::LakeConfig {
                catalog: std::env::var("BI_TEST_LAKE_CATALOG")
                    .unwrap_or_else(|_| bi_runtime::config::DEV_LAKE_CATALOG.into()),
                metadata_schema: Some(format!("models_test_{}", std::process::id())),
                data_path,
            }),
            ..bi_runtime::RuntimeConfig::in_memory(1)
        };
        let bootstrap = bi_runtime::Bootstrap::open(config).unwrap();
        bi_runtime::fixtures::load_sales(
            &bootstrap,
            bi_fixtures::sales::FixtureSpec {
                seed: 42,
                rows: 1000,
            },
        )
        .unwrap();
        bootstrap.finish().unwrap()
    })
    .await
    .unwrap();
    let p = project();
    write(p.path(), "ventas.yaml", VALID);
    let (_, body) = response(&app(p.path(), Analytics::ready(runtime))).await;
    assert_eq!(body["models"][0]["status"], "valid", "{body}");
    assert_eq!(
        body["models"][0]["metrics"][0]["data_type"],
        "DECIMAL(18,2)"
    );
}
