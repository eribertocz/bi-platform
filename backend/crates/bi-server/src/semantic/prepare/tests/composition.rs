//! The preparation composed with the real execution path: a `Prepared`
//! from a real Ossie project lowers onto DuckDB and runs, exact against an
//! independent oracle, with row policies bound only from the verified
//! session. No HTTP route executes yet; this is the composition the later
//! execution route will use.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bi_runtime::duckdb::arrow::array::{Array, Decimal128Array, Int64Array, StringArray};
use bi_runtime::duckdb::arrow::datatypes::DataType;
use sef_adapter_duckdb::execution::{
    BlockResult, ExecutionLimits, PhysicalRelation, SourceCatalog,
};
use serde_json::{Value, json};

use super::super::*;
use crate::analytics::Analytics;
use crate::db::Database;
use crate::metadata::MetadataMode;
use crate::models::Models;
use crate::semantic_execution::{Event, SemanticExecutor};

/// The rows of the Issue: id, region, city, amount, instant, note.
const ROWS: &str = "(1, 'Norte', 'La Paz', 100.10, '2026-01-01T00:00:00Z', NULL),
    (2, 'Norte', 'El Alto', 0.20, '2026-01-02T00:00:00Z', 'n'),
    (3, 'Sur', 'Tarija', 200.20, '2026-01-01T00:00:00Z', 's'),
    (4, 'Sur', 'Sucre', -0.20, '2026-01-02T00:00:00Z', NULL)";

/// The independent oracle: sums written out by hand, in hundredths.
const NORTE: (i128, i64) = (10_030, 2);
const SUR: (i128, i64) = (20_000, 2);
const TOTAL: (i128, i64) = (30_030, 4);

fn field(name: &str, extra: &str) -> String {
    format!(
        "      - name: {name}\n{extra}        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: {name}\n"
    )
}

/// `nota` is restricted: a dimension the viewer may not see.
pub(crate) fn model(extensions: &str, metrics: &str) -> String {
    let restricted = "        custom_extensions:\n          - vendor_name: bi-platform\n            data: '{\"version\": 1, \"sensitivity\": \"restricted\"}'\n";
    format!(
        "version: '0.2.0.dev0'\nname: Ventas\n{extensions}datasets:\n  - name: ventas\n    source: lake.main.ventas\n    primary_key: [id]\n    fields:\n{}{}{}{}{}{}metrics:\n  - name: importe_total\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: SUM(ventas.importe)\n  - name: filas\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: COUNT(ventas.id)\n  - name: margen\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{{\"version\": 1, \"sensitivity\": \"confidential\"}}'\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: SUM(ventas.importe)\n{metrics}",
        field("id", ""),
        field("region", "        dimension: {}\n"),
        field("ciudad", "        dimension: {}\n"),
        field("importe", ""),
        field(
            "instante",
            "        datatype: DateTimeTz\n        dimension: {}\n"
        ),
        field("nota", &format!("        dimension: {{}}\n{restricted}")),
    )
}

/// Rows of each session visible only through the policy on `region`.
pub(crate) const ROW_POLICY: &str = "custom_extensions:\n  - vendor_name: bi-platform\n    data: '{\"version\": 1, \"semantic_version\": \"0.1.0\", \"policies\": [{\"name\": \"zona\", \"dataset\": \"ventas\", \"enforcement\": \"mandatory\", \"predicate\": \"region = :region\", \"context\": [{\"name\": \"region\", \"type\": \"string\", \"sensitivity\": \"sensitive\"}]}]}'\n";

pub(crate) fn project(model: &str, members: &str) -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    std::fs::create_dir(p.path().join("metrics")).unwrap();
    std::fs::create_dir(p.path().join("policies")).unwrap();
    std::fs::write(p.path().join("metrics/ventas.yaml"), model).unwrap();
    std::fs::write(p.path().join("policies/members.yaml"), members).unwrap();
    p
}

pub(crate) async fn analytics() -> Analytics {
    let runtime = tokio::task::spawn_blocking(|| {
        bi_runtime::Bootstrap::open(bi_runtime::RuntimeConfig::in_memory(2))
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

pub(crate) fn state(project: &Path, analytics: Analytics) -> AppState {
    AppState {
        models: Models::new(Some(project.to_owned())),
        analytics,
        ..AppState::new(Database::start(
            &"postgres://bi:bi@127.0.0.1:1/bi".parse().unwrap(),
            MetadataMode::Development,
        ))
    }
}

pub(crate) fn request(query: &Value) -> ExplainRequest {
    serde_json::from_value(json!({ "model": "metrics/ventas.yaml", "query": query })).unwrap()
}

pub(crate) fn selections(selections: &Value) -> ExplainRequest {
    request(&json!({ "selections": selections }))
}

pub(crate) fn metric(id: &str, name: &str) -> Value {
    json!({ "kind": "metric", "id": id, "name": name })
}

pub(crate) fn dimension(id: &str, name: &str) -> Value {
    json!({ "kind": "dimension", "id": id, "name": name })
}

pub(crate) fn session(user: &str, attributes: &[(&str, Value)]) -> Session {
    Session::verified(
        user.to_owned(),
        Vec::new(),
        attributes
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
    )
}

pub(crate) async fn ready(
    state: &AppState,
    session: Session,
    request: &ExplainRequest,
) -> Prepared {
    let Ok(prepared) = prepare(state, session, request).await else {
        panic!("the query must be prepared");
    };
    prepared
}

/// The refusal of `request`, which must not prepare.
async fn refusal(state: &AppState, session: Session, request: &ExplainRequest) -> Outcome {
    let Err(Refusal(outcome)) = prepare(state, session, request).await else {
        panic!("the query must be refused");
    };
    *outcome
}

/// Lowers the preparation onto the sources it captured and runs it: the
/// blocks the executor delivers, as they arrive.
pub(crate) async fn execute(analytics: &Analytics, prepared: &Prepared) -> Vec<BlockResult> {
    let mut blocks = Vec::new();
    let mut complete = 0;
    for event in events(analytics, prepared).await {
        match event {
            Event::Batch(block) => blocks.push(block),
            Event::Complete(_) => complete += 1,
            Event::Failed(failure) => panic!("{failure:?}"),
        }
    }
    assert_eq!(complete, 1);
    blocks
}

/// Every event of one execution of the preparation, in order.
pub(crate) async fn events(analytics: &Analytics, prepared: &Prepared) -> Vec<Event> {
    let mut stream = stream(analytics, prepared, 100, 2);
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    events
}

/// The stream of one execution, with up to `max_rows` rows and `capacity`
/// events in flight.
pub(crate) fn stream(
    analytics: &Analytics,
    prepared: &Prepared,
    max_rows: usize,
    capacity: usize,
) -> crate::semantic_execution::SemanticStream {
    let execution = prepared.execution().expect("nothing masked");
    let catalog: SourceCatalog = execution
        .sources()
        .relations
        .iter()
        .map(|(source, relation)| {
            (
                (relation.source.clone(), relation.relation.clone()),
                PhysicalRelation {
                    name: source.split('.').map(str::to_owned).collect(),
                    columns: relation
                        .columns
                        .iter()
                        .map(|c| (c.handle.clone(), c.name.clone()))
                        .collect(),
                },
            )
        })
        .collect();
    let plan = sef_adapter_duckdb::execution::lower(
        execution.model(),
        execution.validated(),
        execution.plan(),
        execution.certificate(),
        &catalog,
    )
    .unwrap();
    SemanticExecutor::new(analytics.runtime().unwrap())
        .start(
            Arc::new(plan),
            execution.context().clone(),
            "semantic-prepare-test",
            ExecutionLimits {
                max_rows,
                max_bytes: 64 * 1024 * 1024,
                max_intermediate_bytes: 64 * 1024 * 1024,
                deadline: Some(Instant::now() + Duration::from_secs(30)),
                ..ExecutionLimits::default()
            },
            capacity,
        )
        .unwrap()
}

/// The executed rows: `(region or "", importe_total in hundredths, filas)`.
async fn run(analytics: &Analytics, prepared: &Prepared) -> Vec<(String, i128, i64)> {
    let mut rows = Vec::new();
    for block in execute(analytics, prepared).await {
        for batch in block.batches {
            let column = |id: &str| {
                let index = batch
                    .schema()
                    .fields()
                    .iter()
                    .position(|f| f.metadata().values().any(|v| v.ends_with(id)))
                    .unwrap_or_else(|| panic!("{id}: {:?}", batch.schema()));
                batch.column(index).clone()
            };
            let amounts = column("importe_total");
            let amounts = amounts.as_any().downcast_ref::<Decimal128Array>().unwrap();
            let counts = column("filas");
            let counts = counts.as_any().downcast_ref::<Int64Array>().unwrap();
            let regions = batch
                .schema()
                .fields()
                .iter()
                .position(|f| f.data_type() == &DataType::Utf8)
                .map(|i| batch.column(i).clone());
            for i in 0..batch.num_rows() {
                let region = regions.as_ref().map_or_else(String::new, |r| {
                    r.as_any()
                        .downcast_ref::<StringArray>()
                        .unwrap()
                        .value(i)
                        .to_owned()
                });
                assert_eq!(amounts.scale(), 2);
                rows.push((region, amounts.value(i), counts.value(i)));
            }
        }
    }
    rows.sort();
    rows
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prepared_query_runs_exactly_on_the_sources_it_captured() {
    let p = project(&model("", ""), "version: 1\nusers:\n  ana: [viewer]\n");
    let analytics = analytics().await;
    let state = state(p.path(), analytics.clone());
    let totals = selections(&json!([metric("t", "importe_total"), metric("f", "filas")]));
    let prepared = ready(&state, session("ana", &[]), &totals).await;
    assert_eq!(
        run(&analytics, &prepared).await,
        vec![(String::new(), TOTAL.0, TOTAL.1)]
    );
    let by_region = selections(&json!([
        dimension("r", "ventas.region"),
        metric("t", "importe_total"),
        metric("f", "filas")
    ]));
    let prepared = ready(&state, session("ana", &[]), &by_region).await;
    assert_eq!(
        run(&analytics, &prepared).await,
        vec![
            ("Norte".to_owned(), NORTE.0, NORTE.1),
            ("Sur".to_owned(), SUR.0, SUR.1),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn row_policies_bind_only_the_verified_session() {
    let p = project(
        &model(ROW_POLICY, ""),
        "version: 1\nusers:\n  a: [viewer]\n  b: [viewer]\n  jefa: [admin]\n",
    );
    let analytics = analytics().await;
    let state = state(p.path(), analytics.clone());
    let totals = selections(&json!([metric("t", "importe_total"), metric("f", "filas")]));
    // The same definition and snapshot, two verified attributes.
    let a = ready(&state, session("a", &[("region", json!("Norte"))]), &totals).await;
    let b = ready(&state, session("b", &[("region", json!("Sur"))]), &totals).await;
    assert_eq!(a.model().content_hash(), b.model().content_hash());
    assert_eq!(a.policies(), b.policies());
    assert_eq!(
        run(&analytics, &a).await,
        vec![(String::new(), NORTE.0, NORTE.1)]
    );
    assert_eq!(
        run(&analytics, &b).await,
        vec![(String::new(), SUR.0, SUR.1)]
    );
    // An absent or mistyped attribute never prepares, an administrator's
    // neither: the policy is mandatory for everyone.
    for (user, attributes) in [
        ("a", vec![]),
        ("a", vec![("region", json!(7))]),
        ("a", vec![("region", json!(null))]),
        ("jefa", vec![]),
    ] {
        let refused = refusal(&state, session(user, &attributes), &totals).await;
        let Outcome::Invalid { code, .. } = refused else {
            panic!("{user} {attributes:?}: a missing context is a correctness refusal");
        };
        assert_eq!(code, "query_incorrect", "{user} {attributes:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn refused_requests_are_never_prepared_and_say_why() {
    let restricted = "  - name: reserva_centinela\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{\"version\": 1, \"sensitivity\": \"restricted\"}'\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: SUM(ventas.importe)\n  - name: derivada\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: reserva_centinela\n";
    let members =
        "version: 1\nusers:\n  lector: [viewer]\n  jefa: [admin]\n  inspector: [catalog_reader]\n";
    let p = project(&model("", restricted), members);
    let state = state(p.path(), analytics().await);
    let lector = || session("lector", &[]);
    let total = selections(&json!([metric("t", "importe_total")]));
    // Without membership: denied and not acknowledged, the denial recorded.
    let Outcome::Undisclosed { audit: Some(audit) } =
        refusal(&state, session("nadie", &[]), &total).await
    else {
        panic!("no membership");
    };
    assert!(!audit.allowed);
    // A restricted metric, and one derived from it.
    let hidden = selections(&json!([metric("r", "reserva_centinela")]));
    assert!(matches!(
        refusal(&state, lector(), &hidden).await,
        Outcome::Undisclosed { audit: Some(_) }
    ));
    let derived = selections(&json!([metric("d", "derivada")]));
    let Outcome::Denied { denial, .. } = refusal(&state, lector(), &derived).await else {
        panic!("derived from a restricted metric");
    };
    assert!(
        denial
            .1
            .is_some_and(|d| d.usage == "lineage" && d.name.is_none())
    );
    // Column-level denial of a restricted dimension, grouped or filtered.
    let grouped = selections(&json!([
        dimension("n", "ventas.nota"),
        metric("t", "importe_total")
    ]));
    let mut filtered = selections(&json!([metric("t", "importe_total")]));
    filtered.query = serde_json::from_value(json!({
        "selections": [metric("t", "importe_total")],
        "filter": { "op": "is_null", "target": { "kind": "dimension", "name": "ventas.nota" } }
    }))
    .unwrap();
    for request in [&grouped, &filtered] {
        assert!(matches!(
            refusal(&state, lector(), request).await,
            Outcome::Undisclosed { audit: Some(_) }
        ));
    }
    // A masked metric is prepared and explained, but never run unmasked.
    let masked = selections(&json!([metric("m", "margen")]));
    let prepared = ready(&state, lector(), &masked).await;
    assert!(matches!(prepared.execution(), Err(Masked)));
    assert!(ready(&state, lector(), &total).await.execution().is_ok());
    // Inspecting the catalog grants no data: refused, with its reason.
    let inspection = "permit (principal in Role::\"catalog_reader\", action == Action::\"view\", resource == Model::\"project:catalog\");";
    std::fs::write(
        p.path().join("policies/project.cedar"),
        format!("{}\n{inspection}", sef_authz::BASE_POLICIES),
    )
    .unwrap();
    let Outcome::Denied { denial, .. } = refusal(&state, session("inspector", &[]), &total).await
    else {
        panic!("an inspector without data");
    };
    assert_eq!(denial.0, "no_permit");
    // An explicit forbid binds the administrator too.
    std::fs::write(
        p.path().join("policies/project.cedar"),
        format!(
            "{}\nforbid (principal, action, resource is Metric);",
            sef_authz::BASE_POLICIES
        ),
    )
    .unwrap();
    let Outcome::Denied { denial, .. } = refusal(&state, session("jefa", &[]), &total).await else {
        panic!("a forbid on the administrator");
    };
    assert_eq!(denial.0, "forbidden");
    // Corrupt policies close everything.
    std::fs::write(p.path().join("policies/project.cedar"), "permit (principal").unwrap();
    let Outcome::Failed(status, code, _) = refusal(&state, session("jefa", &[]), &total).await
    else {
        panic!("corrupt policies");
    };
    assert_eq!(
        (status, code),
        (StatusCode::SERVICE_UNAVAILABLE, "policies_invalid")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deprecated_metric_names_only_a_visible_replacement() {
    let deprecated = |replacement: &str| {
        format!(
            "  - name: vieja\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{{\"version\": 1, \"status\": {{\"state\": \"deprecated\", \"sunset\": \"2027-01-01\", \"replacement\": \"{replacement}\"}}}}'\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: SUM(ventas.importe)\n  - name: oculta\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{{\"version\": 1, \"sensitivity\": \"restricted\"}}'\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: SUM(ventas.importe)\n"
        )
    };
    let analytics = analytics().await;
    // Selected twice: one notice.
    let query = selections(&json!([metric("v", "vieja"), metric("w", "vieja")]));
    for (replacement, shown) in [("importe_total", true), ("oculta", false)] {
        let p = project(
            &model("", &deprecated(replacement)),
            "version: 1\nusers:\n  lector: [viewer]\n",
        );
        let state = state(p.path(), analytics.clone());
        let prepared = ready(&state, session("lector", &[]), &query).await;
        let notices: &[LifecycleNotice] = prepared.notices();
        assert_eq!(notices.len(), 1, "{replacement}");
        assert!(notices[0].metric.as_str().ends_with("vieja"));
        assert_eq!(notices[0].sunset.as_deref(), Some("2027-01-01"));
        assert_eq!(
            notices[0]
                .replacement
                .as_ref()
                .is_some_and(|r| r.as_str().ends_with(replacement)),
            shown,
            "{replacement}"
        );
        if !shown {
            assert_eq!(notices[0].replacement, None);
        }
    }
}
