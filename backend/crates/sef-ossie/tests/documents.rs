//! Loading and validation against the official examples and the cases of
//! Ossie's own validator (`validation/tests/test_validate.py`).

// Tests fail loudly on purpose: a broken fixture or contract must stop the run.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sef_ossie::document::DataType;
use sef_ossie::{Code, Diagnostic, Severity, load, parse, to_json, to_yaml};

const TPCDS: &str = include_str!("fixtures/ossie/tpcds_semantic_model.yaml");
const FLIGHTS: &str = include_str!("fixtures/ossie/flights.semantic_model.yaml");

/// A small valid model; tests change one thing at a time.
const BASE: &str = r"
version: 0.2.0.dev0
name: sales
datasets:
  - name: orders
    source: shop.public.orders
    primary_key: [order_id]
    fields:
      - name: order_id
        expression: { dialects: [ { dialect: OSSIE_SQL_2026, expression: order_id } ] }
      - name: customer_id
        expression: { dialects: [ { dialect: OSSIE_SQL_2026, expression: customer_id } ] }
  - name: customers
    source: shop.public.customers
    primary_key: [id]
    unique_keys: [[email], [first_name, last_name]]
relationships:
  - name: orders_to_customers
    from: orders
    to: customers
    from_columns: [customer_id]
    to_columns: [id]
metrics:
  - name: revenue
    expression: { dialects: [ { dialect: OSSIE_SQL_2026, expression: SUM(orders.amount) } ] }
";

fn errors(text: &str) -> Vec<Diagnostic> {
    load(text).expect_err("the document must be rejected")
}

fn codes(diagnostics: &[Diagnostic]) -> Vec<(Code, &str)> {
    diagnostics
        .iter()
        .map(|d| (d.code, d.path.as_str()))
        .collect()
}

#[test]
fn the_official_examples_load_without_diagnostics() {
    for (name, text) in [("tpcds", TPCDS), ("flights", FLIGHTS)] {
        let loaded = load(text).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(loaded.warnings.is_empty(), "{name}: {:?}", loaded.warnings);
        assert!(!loaded.model.datasets.is_empty(), "{name}");
    }
}

#[test]
fn documents_survive_a_yaml_and_a_json_round_trip() {
    for text in [TPCDS, FLIGHTS, BASE] {
        let model = parse(text).unwrap();
        assert_eq!(parse(&to_yaml(&model).unwrap()).unwrap(), model);
        assert_eq!(parse(&to_json(&model).unwrap()).unwrap(), model);
    }
}

#[test]
fn json_documents_are_read_by_the_same_loader() {
    let json = r#"{"version": "0.2.0.dev0", "name": "m",
                   "datasets": [{"name": "d", "source": "s.t"}]}"#;
    assert_eq!(load(json).unwrap().model.datasets[0].name, "d");
}

#[test]
fn a_duplicate_key_is_a_located_syntax_error_not_a_silent_override() {
    let text = BASE.replace("name: revenue", "name: revenue\n    name: margin");
    let found = errors(&text);
    assert_eq!(found[0].code, Code::Syntax);
    assert!(found[0].location.is_some(), "{found:?}");
}

#[test]
fn yaml_1_1_booleans_and_merge_keys_are_rejected() {
    let yes = BASE.replace(
        "expression: order_id } ] }",
        "expression: order_id } ] }\n        dimension: { is_time: yes }",
    );
    assert_eq!(errors(&yes)[0].code, Code::Syntax);
    let merge = BASE.replace(
        "  - name: customers\n",
        "  - <<: { description: shared }\n    name: customers\n",
    );
    assert_eq!(errors(&merge)[0].code, Code::Syntax);
}

#[test]
fn unknown_properties_and_wrong_types_are_syntax_errors() {
    for text in [
        BASE.replace("name: sales", "name: sales\nowner: me"),
        BASE.replace("name: revenue", "name: revenue\n    owner: me"),
        BASE.replace("primary_key: [order_id]", "primary_key: order_id"),
        BASE.replace(
            "dialect: OSSIE_SQL_2026, expression: SUM",
            "dialect: POSTGRES, expression: SUM",
        ),
    ] {
        assert_eq!(errors(&text)[0].code, Code::Syntax, "{text}");
    }
}

#[test]
fn the_legacy_wrapper_gets_its_migration_message() {
    let legacy = "semantic_model:\n  - name: sales\n    datasets: []\n";
    let found = errors(legacy);
    assert_eq!(codes(&found), [(Code::LegacyWrapper, "/semantic_model")]);
    assert!(found[0].message.contains("un modelo por archivo"));
}

#[test]
fn oversized_documents_are_refused_before_parsing() {
    let huge = format!(
        "{BASE}\n#{}",
        "x".repeat(sef_ossie::load::MAX_DOCUMENT_BYTES)
    );
    assert_eq!(errors(&huge)[0].code, Code::TooLarge);
}

#[test]
fn schema_constraints_serde_cannot_express_are_checked() {
    let cases = [
        (
            BASE.replace("0.2.0.dev0", "0.1.1"),
            Code::UnsupportedVersion,
            "/version",
        ),
        (
            BASE.replace("name: sales", "name: ''"),
            Code::EmptyValue,
            "/name",
        ),
        (
            BASE.replace("source: shop.public.orders", "source: ''"),
            Code::EmptyValue,
            "/datasets/0/source",
        ),
        (
            BASE.replace(
                "{ dialects: [ { dialect: OSSIE_SQL_2026, expression: SUM(orders.amount) } ] }",
                "{ dialects: [] }",
            ),
            Code::EmptyList,
            "/metrics/0/expression/dialects",
        ),
        (
            BASE.replace("to: customers", "to: ''"),
            Code::EmptyValue,
            "/relationships/0/to",
        ),
    ];
    for (text, code, path) in cases {
        assert!(
            codes(&errors(&text)).contains(&(code, path)),
            "{code:?} {path}"
        );
    }
    let no_datasets = "version: 0.2.0.dev0\nname: m\ndatasets: []\n";
    assert_eq!(
        codes(&errors(no_datasets)),
        [(Code::EmptyList, "/datasets")]
    );
}

#[test]
fn a_name_repeated_three_times_is_one_error() {
    let text = BASE.replace(
        "metrics:\n",
        "metrics:\n  - name: revenue\n    expression: { dialects: [ { dialect: ANSI_SQL, expression: '1' } ] }\n  \
         - name: revenue\n    expression: { dialects: [ { dialect: ANSI_SQL, expression: '1' } ] }\n",
    );
    assert_eq!(
        codes(&errors(&text)),
        [(Code::DuplicateName, "/metrics/1/name")]
    );
}

#[test]
fn names_collide_as_normalized_identifiers() {
    // Unquoted identifiers are case-insensitive in Ossie.
    let text = BASE.replace(
        "  - name: customers\n",
        "  - name: ORDERS\n    source: x.y\n  - name: customers\n",
    );
    assert_eq!(
        codes(&errors(&text)),
        [(Code::DuplicateName, "/datasets/1/name")]
    );
    // A quoted identifier keeps its case and is distinct from the unquoted one.
    let quoted = BASE.replace(
        "  - name: customers\n",
        "  - name: '\"Orders\"'\n    source: x.y\n  - name: customers\n",
    );
    assert!(load(&quoted).is_ok());
}

#[test]
fn field_names_are_unique_within_their_dataset_only() {
    let same_dataset = BASE.replace("name: customer_id", "name: ORDER_ID");
    assert_eq!(
        codes(&errors(&same_dataset)),
        [(Code::DuplicateName, "/datasets/0/fields/1/name")]
    );
}

#[test]
fn relationships_must_name_declared_datasets_and_pair_their_columns() {
    let unknown = BASE.replace("to: customers", "to: clients");
    assert_eq!(
        codes(&errors(&unknown)),
        [(Code::UnknownDataset, "/relationships/0/to")]
    );
    let arity = BASE.replace("to_columns: [id]", "to_columns: [id, email]");
    assert_eq!(
        codes(&errors(&arity)),
        [(Code::ColumnArity, "/relationships/0")]
    );
}

#[test]
fn key_coverage_is_a_warning_and_accepts_keys_supersets_and_any_order() {
    let warned = load(&BASE.replace(
        "from_columns: [customer_id]\n    to_columns: [id]",
        "from_columns: [customer_id]\n    to_columns: [first_name]",
    ))
    .unwrap();
    assert_eq!(warned.warnings.len(), 1);
    assert_eq!(warned.warnings[0].severity, Severity::Warning);
    assert_eq!(warned.warnings[0].code, Code::KeyNotCovered);

    for (from, to) in [
        ("[customer_id]", "[email]"),          // a unique key
        ("[customer_id, e]", "[id, email]"),   // a superset of a key
        ("[a, b]", "[last_name, first_name]"), // composite, other order
    ] {
        let text = BASE.replace(
            "from_columns: [customer_id]\n    to_columns: [id]",
            &format!("from_columns: {from}\n    to_columns: {to}"),
        );
        assert!(load(&text).unwrap().warnings.is_empty(), "{to}");
    }
    let keyless = BASE
        .replace(
            "    primary_key: [id]\n    unique_keys: [[email], [first_name, last_name]]\n",
            "",
        )
        .replace("to_columns: [id]", "to_columns: [anything]");
    assert_eq!(load(&keyless).unwrap().warnings, []);
}

#[test]
fn every_error_is_reported_at_once_and_errors_precede_warnings() {
    let text = BASE
        .replace("name: sales", "name: ''")
        .replace("to: customers", "to: clients")
        .replace("name: customer_id", "name: order_id");
    let found = errors(&text);
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(found.iter().all(Diagnostic::is_error));
}

#[test]
fn time_role_follows_the_datatype_unless_set_explicitly() {
    let model = parse(FLIGHTS).unwrap();
    for field in model.datasets.iter().flat_map(|d| &d.fields) {
        let explicit = field.dimension.as_ref().and_then(|d| d.is_time);
        let expected =
            explicit.unwrap_or_else(|| field.datatype.is_some_and(DataType::is_temporal));
        assert_eq!(field.is_time(), expected, "{}", field.name);
    }
}
