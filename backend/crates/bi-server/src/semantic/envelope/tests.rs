//! The answer of real executions: assembled only when complete and
//! coherent, with a receipt a third party recomputes and that shows nothing
//! the session may not see.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::*;
use crate::semantic::prepare::tests::composition::{
    analytics, dimension, events, metric, model, project, ready, request, session, state,
};
use crate::semantic_execution::Failure;

const MEMBERS: &str = "version: 1\nusers:\n  ana: [viewer]\n";
const KEY: [u8; 32] = [7; 32];

fn scope() -> Scope<'static> {
    Scope {
        project: "p",
        principal: "ana",
    }
}

/// What one execution of a prepared query delivered.
type Execution = (Prepared, Vec<Event>, tempfile::TempDir);

/// One way to tamper with what an execution delivered.
type Change = Box<dyn FnOnce(Vec<Event>) -> Vec<Event>>;

/// A real preparation of `query`, every event of one execution of it, and
/// the project it reads.
async fn executed(query: &Value) -> Execution {
    let p = project(&model("", ""), MEMBERS);
    let analytics = analytics().await;
    let state = state(p.path(), analytics.clone());
    let prepared = ready(&state, session("ana", &[]), &request(query)).await;
    let events = events(&analytics, &prepared).await;
    (prepared, events, p)
}

fn by_region() -> Value {
    json!({ "selections": [
        dimension("r", "ventas.region"), metric("t", "importe_total"), metric("f", "filas")
    ] })
}

/// The events, with the completion's evidence changed by `change`.
fn altered(events: Vec<Event>, change: impl FnOnce(&mut ResultProvenance)) -> Vec<Event> {
    let mut change = Some(change);
    events
        .into_iter()
        .map(|event| match event {
            Event::Complete(mut provenance) => {
                (change.take().unwrap())(&mut provenance);
                Event::Complete(provenance)
            }
            other => other,
        })
        .collect()
}

fn assembled(prepared: &Prepared, events: Vec<Event>) -> Result<SemanticRun, RunError> {
    assemble(prepared, events, &ContextKey::fixed(KEY), &scope())
}

/// JSON with object keys sorted, by another path than `jcs`: what a third
/// party would write to recompute the ID.
fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), sorted(v)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

fn recomputed_id(receipt: &Value) -> String {
    let mut stable = receipt.clone();
    for field in ["id", "read_started_at", "read_finished_at"] {
        stable.as_object_mut().unwrap().remove(field);
    }
    // `serde_json` writes a sorted map in key order: the JCS form here.
    let text = serde_json::to_string(&sorted(&stable)).unwrap();
    format!("run:{}", hex(&Sha256::digest(text.as_bytes())))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_complete_execution_answers_with_a_recomputable_receipt() {
    let (prepared, events, _p) = executed(&by_region()).await;
    let answer = assembled(&prepared, events).unwrap();
    // The rows of #136, against the oracle.
    let mut rows: Vec<Value> =
        serde_json::from_value(serde_json::to_value(&answer.result.blocks[0].rows).unwrap())
            .unwrap();
    rows.sort_by_key(ToString::to_string);
    assert_eq!(
        rows,
        vec![
            json!(["Norte", "100.30", "2"]),
            json!(["Sur", "200.00", "2"])
        ]
    );
    let receipt = serde_json::to_value(&answer.receipt).unwrap();
    // A third party recomputes the ID from what was returned.
    assert_eq!(recomputed_id(&receipt), answer.receipt.id);
    // The fingerprints are the preparation's; the context, the protected one.
    let p = &prepared;
    let f = &answer.receipt.fingerprints;
    assert_eq!(f.query, p.query_fingerprint());
    assert_eq!(f.plan, p.plan_fingerprint());
    assert_eq!(f.policies, p.policies());
    // The decision's plain fingerprint covers the principal: only its
    // protected form is shown.
    let plain = p.decision().fingerprint.to_string();
    assert!(
        f.decision.starts_with("dec-hmac-sha256-v1:"),
        "{}",
        f.decision
    );
    assert!(
        !receipt
            .to_string()
            .contains(plain.split(':').next_back().unwrap())
    );
    assert!(f.context.starts_with("ctx-hmac-sha256-v1:"));
    assert!(answer.receipt.evidence.starts_with("ev-hmac-sha256-v1:"));
    // The canonical intent, with its typed literals: the very bytes.
    let sqir = p.query().canonical_value().encode().unwrap();
    assert_eq!(answer.receipt.query.sqir, hex(&sqir));
    assert_eq!(answer.receipt.version, "bi-run-receipt/1");
    // What it selected, and the dataset it read, by stable identity.
    let ids: Vec<&str> = answer
        .receipt
        .definitions
        .iter()
        .map(|d| d.id.as_str())
        .collect();
    assert_eq!(ids.len(), 3, "{ids:?}");
    assert!(ids.iter().any(|id| id.ends_with("dimension/ventas.region")));
    assert_eq!(answer.receipt.sources.len(), 1);
    let source = &answer.receipt.sources[0];
    assert!(source.dataset.ends_with("dataset/ventas"), "{source:?}");
    // In memory: no persistent version, and no known refresh.
    assert_eq!(
        (&source.data_snapshot, &source.source_refreshed_at),
        (&None, &None)
    );
    assert!(answer.receipt.read_started_at <= answer.receipt.read_finished_at);
    for at in [
        &answer.receipt.read_started_at,
        &answer.receipt.read_finished_at,
    ] {
        assert!(
            chrono::DateTime::parse_from_rfc3339(at).is_ok() && at.ends_with('Z'),
            "{at}"
        );
    }
    // No physical relation, schema hash or SQL in the public form.
    let text = receipt.to_string();
    for absent in ["lake.main", "duckdb-schema-sha256", "SELECT", "\"ana\""] {
        assert!(!text.contains(absent), "{absent} in {text}");
    }
    // The same intent, executed again: the same receipt ID.
    let (again, events, _q) = executed(&by_region()).await;
    assert_eq!(
        assembled(&again, events).unwrap().receipt.id,
        answer.receipt.id
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_id_covers_every_stable_field_and_not_the_read_times() {
    let id = |change: Box<dyn FnOnce(&mut ResultProvenance)>| async move {
        let (prepared, events, _p) = executed(&by_region()).await;
        assembled(&prepared, altered(events, change))
            .unwrap()
            .receipt
    };
    let base = id(Box::new(|_| {})).await;
    // Only the times: the same ID.
    let later = id(Box::new(|p| {
        p.started_at += std::time::Duration::from_secs(60);
        p.finished_at += std::time::Duration::from_secs(61);
    }))
    .await;
    assert_eq!(later.id, base.id);
    assert_ne!(later.read_started_at, base.read_started_at);
    // A snapshot: another ID, and it is shown.
    let snapshot = id(Box::new(|p| p.sources[0].data_snapshot = Some(5))).await;
    assert_ne!(snapshot.id, base.id);
    assert_eq!(snapshot.sources[0].data_snapshot.as_deref(), Some("5"));
    // A version: another ID.
    let engine = id(Box::new(|p| p.engine_version.push('x'))).await;
    assert_ne!(engine.id, base.id);
    // Hidden evidence (a physical schema): another commitment and ID,
    // every public field else the same, and nothing of it shown.
    let schema = id(Box::new(|p| p.sources[0].schema_fingerprint.push('0'))).await;
    assert_ne!(schema.evidence, base.evidence);
    assert_ne!(schema.id, base.id);
    assert_eq!(
        (&schema.fingerprints, &schema.definitions, &schema.sources),
        (&base.fingerprints, &base.definitions, &base.sources)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_but_a_complete_coherent_execution_is_answered() {
    let cases: Vec<(&str, Change, RunError)> = vec![
        (
            "failed after a batch",
            Box::new(|events| {
                let mut events: Vec<Event> = events
                    .into_iter()
                    .filter(|e| matches!(e, Event::Batch(_)))
                    .collect();
                events.push(Event::Failed(Failure::Worker));
                events
            }),
            RunError::Failed(Failure::Worker),
        ),
        (
            "closed without completion",
            Box::new(|events| {
                events
                    .into_iter()
                    .filter(|e| matches!(e, Event::Batch(_)))
                    .collect()
            }),
            RunError::Incomplete,
        ),
        (
            "nothing at all",
            Box::new(|_| Vec::new()),
            RunError::Incomplete,
        ),
        (
            "something after the completion",
            Box::new(|mut events| {
                events.push(Event::Failed(Failure::Deadline));
                events
            }),
            RunError::Evidence("after_complete"),
        ),
        (
            "a lost batch",
            Box::new(|events| {
                let mut events = events;
                let first = events
                    .iter()
                    .position(|e| matches!(e, Event::Batch(_)))
                    .unwrap();
                events.remove(first);
                events
            }),
            RunError::Evidence("rows"),
        ),
        (
            "another certificate",
            Box::new(|events| altered(events, |p| p.certificate.discharges.clear())),
            RunError::Evidence("certificate"),
        ),
        (
            "other definitions",
            Box::new(|events| {
                altered(events, |p| {
                    p.definitions.pop();
                })
            }),
            RunError::Evidence("definitions"),
        ),
        (
            "a source of no dataset of the plan",
            Box::new(|events| altered(events, |p| p.sources[0].relation.push('x'))),
            RunError::Evidence("source"),
        ),
        (
            "a dataset of the plan without its source",
            Box::new(|events| altered(events, |p| p.sources.clear())),
            RunError::Evidence("source"),
        ),
    ];
    for (case, change, expected) in cases {
        let (prepared, events, _p) = executed(&by_region()).await;
        assert_eq!(
            assembled(&prepared, change(events)).map(|_| ()),
            Err(expected),
            "{case}"
        );
    }
    // Without the instance's key nothing is bound, so nothing is answered.
    let (prepared, events, _p) = executed(&by_region()).await;
    let keyless = ContextKey::from_entropy(|_| Err(getrandom::Error::UNSUPPORTED));
    assert_eq!(
        assemble(&prepared, events, &keyless, &scope()).map(|_| ()),
        Err(RunError::Unavailable)
    );
}

/// A definition or dataset the session may not see leaves no name, id or
/// fingerprint in the public form; only the commitment binds it.
#[tokio::test(flavor = "multi_thread")]
async fn what_is_not_shown_leaves_no_trace_in_the_public_form() {
    let (_, events, _p) = executed(&by_region()).await;
    let Some(Event::Complete(provenance)) = events.into_iter().last() else {
        panic!("complete");
    };
    let hidden = provenance
        .definitions
        .iter()
        .find(|d| d.id.as_str().ends_with("metric/filas"))
        .unwrap()
        .clone();
    let shows = |id: &StableId| *id != hidden.id;
    let definitions = shown_definitions(&provenance, shows);
    assert_eq!(definitions.len(), provenance.definitions.len() - 1);
    let text = serde_json::to_string(&definitions).unwrap();
    assert!(!text.contains("filas") && !text.contains(&hidden.content.to_string()));
    let dataset = vec![vec![
        StableId::new("project-file-x/dataset/oculto").unwrap(),
    ]];
    let sources = shown_sources(&provenance, &dataset, |_| false);
    assert_eq!(sources, Vec::new());
    // The commitment still binds the hidden definition: another content,
    // another commitment.
    let key = ContextKey::fixed(KEY);
    let mut other = (*provenance).clone();
    let index = other
        .definitions
        .iter()
        .position(|d| d.id == hidden.id)
        .unwrap();
    other.definitions[index].content =
        other.definitions[(index + 1) % other.definitions.len()].content;
    assert_ne!(
        key.commit(&scope(), private_evidence(&provenance, "d")),
        key.commit(&scope(), private_evidence(&other, "d"))
    );
}

/// Notices are those the preparation captured, as declared: a replacement
/// only when the session may see it, and no trace of a hidden one.
#[tokio::test(flavor = "multi_thread")]
async fn a_deprecated_metric_is_noticed_with_only_a_visible_replacement() {
    let deprecated = |replacement: &str| {
        format!(
            "  - name: vieja
    custom_extensions:
      - vendor_name: bi-platform
        data: '{{\"version\": 1, \"status\": {{\"state\": \"deprecated\", \"sunset\": \"2027-01-01\", \"replacement\": \"{replacement}\"}}}}'
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(ventas.importe)
  - name: oculta
    custom_extensions:
      - vendor_name: bi-platform
        data: '{{\"version\": 1, \"sensitivity\": \"restricted\"}}'
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(ventas.importe)
"
        )
    };
    let analytics = analytics().await;
    let query = json!({ "selections": [metric("v", "vieja")] });
    for (replacement, shown) in [("importe_total", true), ("oculta", false)] {
        let p = project(&model("", &deprecated(replacement)), MEMBERS);
        let state = state(p.path(), analytics.clone());
        let prepared = ready(&state, session("ana", &[]), &request(&query)).await;
        let events = events(&analytics, &prepared).await;
        let answer = assemble(&prepared, events, &ContextKey::fixed(KEY), &scope()).unwrap();
        let [notice] = answer.notices.as_slice() else {
            panic!("one notice: {:?}", answer.notices);
        };
        assert!(notice.metric.ends_with("metric/vieja"));
        assert_eq!(notice.sunset.as_deref(), Some("2027-01-01"));
        assert_eq!(
            notice
                .replacement
                .as_deref()
                .is_some_and(|r| r.ends_with("metric/importe_total")),
            shown,
            "{replacement}"
        );
        if !shown {
            assert_eq!(notice.replacement, None);
            let text = serde_json::to_string(&answer).unwrap();
            assert!(!text.contains("oculta"), "{text}");
        }
    }
}

/// The whole body (rows, receipt and notices) fits 4 MiB to the byte,
/// escapes included; one byte more is refused, with no partial answer.
#[tokio::test(flavor = "multi_thread")]
async fn the_whole_body_holds_its_budget_to_the_byte() {
    use bi_runtime::duckdb::arrow::array::{ArrayRef, Int64Array, StringArray};
    use bi_runtime::duckdb::arrow::datatypes::{Field, Schema};
    use bi_runtime::duckdb::arrow::record_batch::RecordBatch;
    use std::sync::Arc;
    let query = json!({ "selections": [dimension("r", "ventas.region"), metric("f", "filas")] });
    let (prepared, events, _p) = executed(&query).await;
    let mut events = events.into_iter();
    let Some(Event::Batch(block)) = events.next() else {
        panic!("a batch");
    };
    let schema = block.batches[0].schema();
    let Some(Event::Complete(provenance)) = events.last() else {
        panic!("complete");
    };
    // Rows of escaped text: each `"` and control character takes 2 or 6
    // bytes of JSON, the 'ñ' 2 of UTF-8.
    let answer = |texts: &[String]| {
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(texts.to_vec())),
            Arc::new(Int64Array::from(vec![1; texts.len()])),
        ];
        let fields: Vec<Field> = schema
            .fields()
            .iter()
            .zip(&columns)
            .map(|(f, c)| {
                Field::new(f.name(), c.data_type().clone(), true)
                    .with_metadata(f.metadata().clone())
            })
            .collect();
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap();
        let mut provenance = provenance.clone();
        provenance.rows = texts.len();
        let events = vec![
            Event::Batch(BlockResult {
                block_id: block.block_id.clone(),
                batches: vec![batch],
            }),
            Event::Complete(provenance),
        ];
        assemble(&prepared, events, &ContextKey::fixed(KEY), &scope())
    };
    let cell = "\"ñ\u{1}a".repeat(4000);
    let size_of = |texts: &[String]| serde_json::to_vec(&answer(texts).unwrap()).unwrap().len();
    let empty = size_of(&[String::new()]);
    let each = size_of(&[cell.clone(), String::new()]) - empty;
    // Escaped cells, until what is left fits one plain cell.
    let mut texts = vec![cell.clone(); (MAX_RESULT_BYTES - empty - 30_000) / each];
    texts.push(String::new());
    let size = loop {
        let size = serde_json::to_vec(&answer(&texts).unwrap()).unwrap().len();
        if MAX_RESULT_BYTES - size < 60_000 {
            break size;
        }
        texts.insert(0, cell.clone());
    };
    let last = texts.len() - 1;
    texts[last] = "a".repeat(MAX_RESULT_BYTES - size);
    let full = answer(&texts).unwrap();
    assert_eq!(serde_json::to_vec(&full).unwrap().len(), MAX_RESULT_BYTES);
    texts[last].push('a');
    assert_eq!(
        answer(&texts).map(|_| ()),
        Err(RunError::Codec(CodecError::ResultTooLarge))
    );
}

/// On DuckLake, the snapshot a receipt shows holds exactly the rows the
/// answer delivered, checked by time travel to that version as an
/// independent oracle. A write committed while a read is still delivering
/// its blocks is in neither; the next read shows both. (That the snapshot
/// is read in the read's own transaction is the adapter's guarantee; this
/// proves what the receipt publishes of it.)
#[cfg(feature = "postgres-tests")]
#[tokio::test(flavor = "multi_thread")]
async fn on_ducklake_the_snapshot_shown_holds_exactly_the_rows_answered() {
    use crate::Analytics;
    use crate::semantic::prepare::tests::composition::stream;
    let (runtime, _data) = ducklake_with_cities().await;
    let snapshot = || {
        let runtime = runtime.clone();
        async move {
            runtime
                .run(|conn| {
                    conn.query_row("SELECT id FROM lake.current_snapshot()", [], |row| {
                        row.get::<_, i64>(0)
                    })
                })
                .await
                .unwrap()
        }
    };
    let analytics = Analytics::ready(runtime.clone());
    let p = project(&model("", ""), MEMBERS);
    let state = state(p.path(), analytics.clone());
    let query = json!({ "selections": [dimension("c", "ventas.ciudad"), metric("f", "filas")] });
    let prepared = ready(&state, session("ana", &[]), &request(&query)).await;
    let before = snapshot().await;
    let mut reading = stream(&analytics, &prepared, 10_000, 1);
    let first = reading.next().await.unwrap();
    assert!(matches!(first, Event::Batch(_)));
    runtime
        .run(|conn| {
            conn.execute_batch(
                "INSERT INTO lake.main.ventas VALUES (9999, 'Norte', 'nueva', 1.00, \
                 TIMESTAMPTZ '2026-01-01 00:00:00+00', NULL);",
            )
        })
        .await
        .unwrap();
    let after = snapshot().await;
    assert!(after > before);
    let mut events = vec![first];
    while let Some(event) = reading.next().await {
        events.push(event);
    }
    // Blocks were still delivered after the write committed.
    let later = events[1..]
        .iter()
        .filter(|e| matches!(e, Event::Batch(_)))
        .count();
    assert!(later >= 1, "{later}");
    let answer = assemble(&prepared, events, &ContextKey::fixed(KEY), &scope()).unwrap();
    let text = serde_json::to_string(&answer.result).unwrap();
    assert_eq!(answer.result.blocks[0].row_count, 5000);
    assert!(!text.contains("nueva"));
    assert_eq!(
        answer.receipt.sources[0].data_snapshot,
        Some(before.to_string())
    );
    // The next read: the new row, and the snapshot that holds it.
    let again = assemble(
        &prepared,
        events_of(&analytics, &prepared).await,
        &ContextKey::fixed(KEY),
        &scope(),
    )
    .unwrap();
    assert_eq!(again.result.blocks[0].row_count, 5001);
    assert!(
        serde_json::to_string(&again.result)
            .unwrap()
            .contains("nueva")
    );
    assert_eq!(
        again.receipt.sources[0].data_snapshot,
        Some(after.to_string())
    );
    assert_ne!(again.receipt.id, answer.receipt.id);
    // The oracle: each version shown holds exactly the cities answered.
    for (shown, answered) in [(&answer, 5000), (&again, 5001)] {
        let version: i64 = shown.receipt.sources[0]
            .data_snapshot
            .as_deref()
            .unwrap()
            .parse()
            .unwrap();
        let held = runtime
            .run(move |conn| {
                conn.query_row(
                    &format!(
                        "SELECT count(DISTINCT ciudad) FROM lake.main.ventas AT (VERSION => {version})"
                    ),
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(held, answered, "version {version}");
        assert_eq!(
            shown.result.blocks[0].row_count,
            u64::try_from(answered).unwrap()
        );
    }
}

/// A DuckLake of 5000 cities: more than one block, so a read is still open
/// when a write commits. Its data directory lives as long as the result.
#[cfg(feature = "postgres-tests")]
async fn ducklake_with_cities() -> (bi_runtime::Runtime, tempfile::TempDir) {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().to_owned();
    let runtime = tokio::task::spawn_blocking(move || {
        bi_runtime::Bootstrap::open(bi_runtime::RuntimeConfig {
            lake: Some(bi_runtime::LakeConfig {
                catalog: std::env::var("BI_TEST_LAKE_CATALOG")
                    .unwrap_or_else(|_| bi_runtime::config::DEV_LAKE_CATALOG.to_owned()),
                metadata_schema: Some(format!("envelope_test_{}", std::process::id())),
                data_path: path,
            }),
            ..bi_runtime::RuntimeConfig::in_memory(2)
        })
        .unwrap()
        .finish()
        .unwrap()
    })
    .await
    .unwrap();
    runtime
        .run(|conn| {
            conn.execute_batch(
                "CREATE TABLE lake.main.ventas (id BIGINT NOT NULL, region VARCHAR NOT NULL, \
                 ciudad VARCHAR NOT NULL, importe DECIMAL(18,2) NOT NULL, \
                 instante TIMESTAMPTZ NOT NULL, nota VARCHAR); \
                 INSERT INTO lake.main.ventas SELECT i, 'Norte', 'c' || i, 1.00, \
                 TIMESTAMPTZ '2026-01-01 00:00:00+00', NULL FROM range(1, 5001) t(i);",
            )
        })
        .await
        .unwrap();
    (runtime, data)
}

#[cfg(feature = "postgres-tests")]
async fn events_of(analytics: &crate::Analytics, prepared: &Prepared) -> Vec<Event> {
    let mut reading =
        crate::semantic::prepare::tests::composition::stream(analytics, prepared, 10_000, 2);
    let mut events = Vec::new();
    while let Some(event) = reading.next().await {
        events.push(event);
    }
    events
}

/// Every field the ID covers changes it when changed; the read times do
/// not. Checked on the JSON a third party receives.
#[tokio::test(flavor = "multi_thread")]
async fn every_field_the_id_covers_changes_it() {
    fn leaves(value: &Value, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        match value {
            Value::Object(fields) => {
                for (key, field) in fields {
                    path.push(key.clone());
                    leaves(field, path, out);
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    path.push(i.to_string());
                    leaves(item, path, out);
                    path.pop();
                }
            }
            _ => out.push(path.clone()),
        }
    }
    fn at<'a>(value: &'a mut Value, path: &[String]) -> &'a mut Value {
        path.iter().fold(value, |value, step| match value {
            Value::Array(items) => &mut items[step.parse::<usize>().unwrap()],
            other => &mut other[step.as_str()],
        })
    }
    let (prepared, events, _p) = executed(&by_region()).await;
    let receipt = serde_json::to_value(assembled(&prepared, events).unwrap().receipt).unwrap();
    let id = id_of(receipt.clone()).unwrap();
    assert_eq!(receipt["id"], json!(id));
    let mut paths = Vec::new();
    leaves(&receipt, &mut Vec::new(), &mut paths);
    let mut covered = 0;
    for path in paths {
        let mut changed = receipt.clone();
        let leaf = at(&mut changed, &path);
        *leaf = match leaf {
            Value::String(text) => json!(format!("{text}x")),
            Value::Null => json!("x"),
            other => panic!("only text and null are published: {other}"),
        };
        let unchanged = ["id", "read_started_at", "read_finished_at"].contains(&path[0].as_str());
        assert_eq!(id_of(changed).unwrap() == id, unchanged, "{path:?}");
        covered += usize::from(!unchanged);
    }
    assert!(covered >= 20, "{covered}");
    // A definition or source fewer: another ID too.
    for list in ["definitions", "sources"] {
        let mut fewer = receipt.clone();
        fewer[list].as_array_mut().unwrap().pop();
        assert_ne!(id_of(fewer).unwrap(), id, "{list}");
    }
}

/// RFC 8785 section 3.2.3 and 3.2.2.2: keys in UTF-16 code-unit order,
/// strings escaped as the standard writes them; a number is refused, never
/// written in another form.
#[test]
fn jcs_follows_rfc_8785() {
    let sorting = json!({
        "\u{20ac}": "Euro Sign",
        "\r": "Carriage Return",
        "\u{fb33}": "Hebrew Letter Dalet With Dagesh",
        "1": "One",
        "\u{1f600}": "Emoji: Grinning Face",
        "\u{80}": "Control",
        "\u{f6}": "Latin Small Letter O With Diaeresis"
    });
    let mut out = String::new();
    jcs(&sorting, &mut out).unwrap();
    let order = [
        "\\r",
        "1",
        "\u{80}",
        "\u{f6}",
        "\u{20ac}",
        "\u{1f600}",
        "\u{fb33}",
    ];
    let positions: Vec<usize> = order
        .iter()
        .map(|key| out.find(&format!("\"{key}\":")).unwrap())
        .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{out}");
    let mut out = String::new();
    jcs(&json!("\u{20ac}$\u{f}\nA'B\"\\\\\"/"), &mut out).unwrap();
    assert_eq!(out, "\"\u{20ac}$\\u000f\\nA'B\\\"\\\\\\\\\\\"/\"");
    assert_eq!(
        jcs(&json!({ "n": 1 }), &mut String::new()),
        Err(RunError::Unavailable)
    );
}

fn published() -> impl proptest::strategy::Strategy<Value = Value> {
    use proptest::prelude::*;
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        "[ -~\u{f1}\u{20ac}\u{1f600}\u{0}-\u{1f}]{0,8}".prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            proptest::collection::btree_map("[a-z\u{20ac}\u{1f600}]{0,4}", inner, 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

fn reversed(value: &Value) -> Value {
    match value {
        Value::Object(fields) => {
            let mut map = serde_json::Map::new();
            for (k, v) in fields.iter().rev() {
                map.insert(k.clone(), reversed(v));
            }
            Value::Object(map)
        }
        Value::Array(items) => Value::Array(items.iter().map(reversed).collect()),
        other => other.clone(),
    }
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

    /// The canonical form reads back as the same value, and does not depend
    /// on the order the keys were inserted in.
    #[test]
    fn jcs_is_canonical(value in published()) {
        let mut out = String::new();
        jcs(&value, &mut out).unwrap();
        proptest::prop_assert_eq!(&serde_json::from_str::<Value>(&out).unwrap(), &value);
        let mut again = String::new();
        jcs(&reversed(&value), &mut again).unwrap();
        proptest::prop_assert_eq!(out, again);
    }
}

/// Over a real row policy, A in Norte and B in Sur: the public receipt
/// carries neither the value nor any hash a dictionary of `{Norte, Sur}`
/// or of identities would reproduce.
#[tokio::test(flavor = "multi_thread")]
async fn the_public_receipt_reveals_no_context_value_or_identity() {
    use crate::semantic::prepare::tests::composition::ROW_POLICY;
    let p = project(
        &model(ROW_POLICY, ""),
        "version: 1\nusers:\n  ana: [viewer]\n  beto: [viewer]\n",
    );
    let analytics = analytics().await;
    let state = state(p.path(), analytics.clone());
    let query = json!({ "selections": [metric("t", "importe_total")] });
    let mut receipts = Vec::new();
    for (user, region) in [("ana", "Norte"), ("ana", "Sur"), ("beto", "Norte")] {
        let who = session(user, &[("region", json!(region))]);
        let prepared = ready(&state, who, &request(&query)).await;
        let events = events(&analytics, &prepared).await;
        let scope = Scope {
            project: "p",
            principal: user,
        };
        let answer = assemble(&prepared, events, &ContextKey::fixed(KEY), &scope).unwrap();
        let receipt = serde_json::to_value(&answer.receipt).unwrap();
        let text = receipt.to_string();
        let plain = prepared.decision().fingerprint.to_string();
        for absent in [region, user, plain.split(':').next_back().unwrap()] {
            assert!(!text.contains(absent), "{absent} in {text}");
        }
        // Whoever lacks the key can only hash guesses; none is published.
        for guess in ["Norte", "Sur"] {
            let literal = sef_model_ir::types::Literal::Utf8(guess.to_owned());
            let mut old = Sha256::new();
            for part in ["region".to_owned(), format!("{literal:?}")] {
                old.update((part.len() as u64).to_be_bytes());
                old.update(part.as_bytes());
            }
            assert!(!text.contains(&hex(&old.finalize())), "{guess}");
        }
        receipts.push(receipt);
    }
    // Another value or another principal: another context and ID. The
    // decision does not depend on the value (the same for ana in Norte and
    // in Sur), only on the principal.
    let (sur, beto) = (&receipts[1], &receipts[2]);
    for other in [sur, beto] {
        assert_ne!(
            other["fingerprints"]["context"],
            receipts[0]["fingerprints"]["context"]
        );
        assert_ne!(other["id"], receipts[0]["id"]);
    }
    assert_eq!(
        sur["fingerprints"]["decision"],
        receipts[0]["fingerprints"]["decision"]
    );
    assert_ne!(
        beto["fingerprints"]["decision"],
        receipts[0]["fingerprints"]["decision"]
    );
}
