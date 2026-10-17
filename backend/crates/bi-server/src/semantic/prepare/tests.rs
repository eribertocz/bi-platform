//! The two phases of a preparation, apart: what the first one read is all
//! the second one uses.

use std::path::Path;

use serde_json::json;

use super::*;
use crate::analytics::Analytics;
use crate::db::Database;
use crate::metadata::MetadataMode;
use crate::models::Models;

const MODEL: &str = "version: '0.2.0.dev0'
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
      - name: importe
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: importe
metrics:
  - name: importe_total
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(ventas.importe)
";

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
                "ATTACH ':memory:' AS lake; CREATE TABLE lake.main.ventas \
                 (id BIGINT NOT NULL, importe DECIMAL(18,2) NOT NULL);",
            )
        })
        .await
        .unwrap();
    Analytics::ready(runtime)
}

fn state(project: &Path, analytics: Analytics) -> AppState {
    AppState {
        models: Models::new(Some(project.to_owned())),
        analytics,
        ..AppState::new(Database::start(
            &"postgres://bi:bi@127.0.0.1:1/bi".parse().unwrap(),
            MetadataMode::Development,
        ))
    }
}

fn request() -> ExplainRequest {
    serde_json::from_value(json!({
        "model": "metrics/ventas.yaml",
        "query": { "selections": [{ "kind": "metric", "id": "t", "name": "importe_total" }] }
    }))
    .unwrap()
}

fn lector() -> Session {
    Session::verified("lector".to_owned(), Vec::new(), BTreeMap::new())
}

#[tokio::test]
async fn a_change_between_the_phases_never_mixes_versions() {
    let p = tempfile::tempdir().unwrap();
    std::fs::create_dir(p.path().join("metrics")).unwrap();
    std::fs::create_dir(p.path().join("policies")).unwrap();
    std::fs::write(p.path().join("metrics/ventas.yaml"), MODEL).unwrap();
    std::fs::write(
        p.path().join("policies/members.yaml"),
        "version: 1\nusers:\n  lector: [viewer]\n",
    )
    .unwrap();
    let state = state(p.path(), analytics().await);
    let Ok(before) = prepare(&state, lector(), &request()).await else {
        panic!("prepared before the change");
    };
    // The first phase reads; then the model, the policies and the members
    // all change; then the second phase runs on what was read.
    let Ok(loaded) = load_request(&state, lector(), &request()).await else {
        panic!("loaded");
    };
    std::fs::write(
        p.path().join("metrics/ventas.yaml"),
        MODEL.replace("SUM(ventas.importe)", "COUNT(ventas.id)"),
    )
    .unwrap();
    std::fs::write(
        p.path().join("policies/project.cedar"),
        format!(
            "{}\nforbid (principal, action, resource is Metric);",
            sef_authz::BASE_POLICIES
        ),
    )
    .unwrap();
    std::fs::write(p.path().join("policies/members.yaml"), "version: 1\n").unwrap();
    let Ok(during) = prepare_loaded(&state, loaded).await else {
        panic!("the request in progress keeps its read");
    };
    assert_eq!(during.model().content_hash(), before.model().content_hash());
    assert_eq!(during.policies(), before.policies());
    assert_eq!(during.roles(), before.roles());
    assert_eq!(during.decision().fingerprint, before.decision().fingerprint);
    assert_eq!(during.plan_fingerprint(), before.plan_fingerprint());
    // The next request sees the change: no membership left.
    let Err(Refusal(next)) = prepare(&state, lector(), &request()).await else {
        panic!("the next request sees the change");
    };
    assert!(matches!(*next, Outcome::Undisclosed { audit: Some(_) }));
    // With access restored, it prepares the changed model.
    std::fs::remove_file(p.path().join("policies/project.cedar")).unwrap();
    std::fs::write(
        p.path().join("policies/members.yaml"),
        "version: 1
users:
  lector: [viewer]
",
    )
    .unwrap();
    let Ok(after) = prepare(&state, lector(), &request()).await else {
        panic!("prepared after the change");
    };
    assert_ne!(after.model().content_hash(), before.model().content_hash());
}

#[tokio::test]
async fn a_query_is_authorized_again_on_everything_its_plan_reads() {
    // The metric is visible; the dataset it scans is restricted, and only
    // the plan says it reads it.
    let restricted = MODEL.replace(
        "  - name: ventas
",
        "  - name: ventas
    custom_extensions:
      - vendor_name: bi-platform
        data: '{\"version\": 1, \"sensitivity\": \"restricted\"}'
",
    );
    assert_ne!(restricted, MODEL);
    let p = tempfile::tempdir().unwrap();
    std::fs::create_dir(p.path().join("metrics")).unwrap();
    std::fs::create_dir(p.path().join("policies")).unwrap();
    std::fs::write(p.path().join("metrics/ventas.yaml"), restricted).unwrap();
    std::fs::write(
        p.path().join("policies/members.yaml"),
        "version: 1
users:
  lector: [viewer]
",
    )
    .unwrap();
    let state = state(p.path(), analytics().await);
    // Control: the same metric over an open dataset prepares, so what
    // refuses below is the dataset the plan reads, not the request.
    let open = tempfile::tempdir().unwrap();
    std::fs::create_dir(open.path().join("metrics")).unwrap();
    std::fs::create_dir(open.path().join("policies")).unwrap();
    std::fs::write(open.path().join("metrics/ventas.yaml"), MODEL).unwrap();
    std::fs::write(
        open.path().join("policies/members.yaml"),
        "version: 1
users:
  lector: [viewer]
",
    )
    .unwrap();
    let control = AppState {
        models: Models::new(Some(open.path().to_owned())),
        ..state.clone()
    };
    assert!(prepare(&control, lector(), &request()).await.is_ok());
    // Refused by the final decision, recorded as a denial, never prepared.
    let Err(Refusal(outcome)) = prepare(&state, lector(), &request()).await else {
        panic!("what the plan reads must be authorized too");
    };
    let Outcome::Denied { audit, denial } = *outcome else {
        panic!("decided after planning");
    };
    assert!(!audit.allowed);
    // An implicit read is never named.
    assert!(denial.1.is_some_and(|denied| denied.name.is_none()));
}

mod composition;
