//! Exactness of types and aggregation contracts (review of #63): casts keep
//! their target, `MIN`/`MAX` and combinations of metrics are never summed
//! across groups unless that is exact, and declared types are checked on
//! computed values too.
// Fixtures are trusted test inputs.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::OnceLock;

use proptest::prelude::*;
use sef_model_ir::expressions::ScalarExprKind;
use sef_model_ir::identity::SourceHandle;
use sef_model_ir::model::{AggregationClass, ModelDefinition};
use sef_model_ir::types::{DecimalType, Literal, ValueType};
use sef_ossie::catalog::{Catalog, Column, Relation};
use sef_ossie::csmir::{Options, to_csmir};
use sef_ossie::document::{
    CustomExtension, DataType, Dialect, DialectExpression, Expression, Metric, SemanticModel,
};
use sef_ossie::{Code, Diagnostic};

fn read(file: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(file);
    std::fs::read_to_string(path).expect("fixture")
}

fn integer() -> ValueType {
    ValueType::Integer {
        signed: true,
        bits: Some(64),
    }
}

fn amount() -> ValueType {
    ValueType::Decimal(DecimalType::new(18, 2).expect("type"))
}

fn column(name: &str, value_type: ValueType, nullable: bool) -> Column {
    Column {
        name: name.to_owned(),
        handle: SourceHandle::new(name).expect("handle"),
        value_type,
        nullable,
    }
}

fn relation(name: &str, columns: Vec<Column>) -> (String, Relation) {
    (
        name.to_owned(),
        Relation {
            source: SourceHandle::new("corpus").expect("handle"),
            relation: SourceHandle::new(name).expect("handle"),
            columns,
        },
    )
}

/// CASE-001: `sales(sale_id, region, amount)`.
fn sales() -> (SemanticModel, Catalog) {
    let model = sef_ossie::load(&read("CASE-001.yaml"))
        .expect("corpus document")
        .model;
    let catalog = Catalog {
        relations: BTreeMap::from([relation(
            "sales",
            vec![
                column("sale_id", integer(), false),
                column("region", ValueType::Utf8, false),
                column("amount", amount(), true),
            ],
        )]),
    };
    (model, catalog)
}

fn options() -> Options {
    Options {
        namespace: "corpus".to_owned(),
        model: "case-001".to_owned(),
    }
}

fn convert(model: &SemanticModel, catalog: &Catalog) -> Result<ModelDefinition, Vec<Diagnostic>> {
    to_csmir(model, catalog, &options())
}

fn portable(text: &str) -> Expression {
    Expression {
        dialects: vec![DialectExpression {
            dialect: Dialect::OssieSql2026,
            expression: text.to_owned(),
        }],
    }
}

fn ours(data: &str) -> Vec<CustomExtension> {
    vec![CustomExtension {
        vendor_name: "bi-platform".to_owned(),
        data: data.to_owned(),
    }]
}

fn metric(name: &str, expression: &str) -> Metric {
    Metric {
        name: name.to_owned(),
        expression: portable(expression),
        description: None,
        datatype: None,
        ai_context: None,
        custom_extensions: Vec::new(),
    }
}

fn rejects(result: Result<ModelDefinition, Vec<Diagnostic>>, code: Code, path: &str) {
    let errors = result.expect_err("must be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.code == code && error.path == path),
        "expected {} at {path}, got:\n{}",
        code.as_str(),
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn class(model: &ModelDefinition, name: &str) -> AggregationClass {
    model
        .metrics
        .iter()
        .find(|metric| metric.metadata.id().as_str() == format!("corpus/metric/{name}"))
        .expect("metric")
        .aggregation_contract
        .classification
}

// ---- Casts keep their target ----------------------------------------------

#[test]
fn casting_a_number_to_text_yields_text() {
    let (mut model, catalog) = sales();
    model.datasets[0].fields[1].expression = portable("CAST(123 AS VARCHAR)");
    let converted = convert(&model, &catalog).expect("converts");
    let binding = &converted.dimensions[0].bindings[0].expression;
    assert_eq!(binding.value_type, ValueType::Utf8);
    let ScalarExprKind::ExplicitCast { operand, .. } = &binding.kind else {
        panic!("a cast of the literal, not a retyped literal: {binding:?}");
    };
    assert_eq!(
        operand.kind,
        ScalarExprKind::Literal(Literal::SignedInteger(123))
    );
}

#[test]
fn a_number_compared_with_text_is_a_type_mismatch() {
    let (mut model, catalog) = sales();
    model.metrics[0].expression = portable("SUM(CASE WHEN sales.region = 5 THEN sales.amount END)");
    rejects(
        convert(&model, &catalog),
        Code::TypeMismatch,
        "/metrics/0/expression/dialects/0/expression",
    );
}

#[test]
fn float_precision_selects_the_width_or_is_rejected() {
    for (target, expected) in [
        ("FLOAT(24)", Some(32)),
        ("FLOAT(25)", Some(64)),
        ("FLOAT(53)", Some(64)),
        ("REAL", Some(32)),
        ("DOUBLE", Some(64)),
        ("DOUBLE PRECISION", Some(64)),
        ("FLOAT(54)", None),
        ("FLOAT", None),
    ] {
        let (mut model, catalog) = sales();
        model.metrics[0].expression = portable(&format!("SUM(CAST(sales.amount AS {target}))"));
        assert!(
            sef_ossie::validate::validate(&model)
                .iter()
                .all(|d| !d.is_error()),
            "{target}"
        );
        match expected {
            Some(bits) => {
                let converted = convert(&model, &catalog).expect(target);
                assert_eq!(
                    converted.measures[0].value_type,
                    ValueType::Float { bits },
                    "{target}"
                );
            }
            None => rejects(
                convert(&model, &catalog),
                Code::TypeMismatch,
                "/metrics/0/expression/dialects/0/expression",
            ),
        }
    }
}

#[test]
fn a_bounded_varchar_cast_is_not_representable() {
    let (mut model, catalog) = sales();
    model.datasets[0].fields[1].expression = portable("CAST(region AS VARCHAR(2))");
    rejects(
        convert(&model, &catalog),
        Code::NotRepresentable,
        "/datasets/0/fields/1/expression/dialects/0/expression",
    );
    let (mut model, catalog) = sales();
    model.datasets[0].fields[1].expression = portable("CAST(region AS VARCHAR)");
    convert(&model, &catalog).expect("an unbounded cast is exact");
}

// ---- Aggregation contracts --------------------------------------------------

#[test]
fn min_and_max_are_recomputed_never_summed() {
    let (mut model, catalog) = sales();
    model.metrics.push(metric("lo", "MIN(sales.amount)"));
    model.metrics.push(metric("hi", "MAX(sales.amount)"));
    model.metrics.push(metric("combined", "lo + hi"));
    let converted = convert(&model, &catalog).expect("converts");
    assert_eq!(class(&converted, "lo"), AggregationClass::NonAdditive);
    assert_eq!(class(&converted, "hi"), AggregationClass::NonAdditive);
    assert_eq!(
        class(&converted, "combined"),
        AggregationClass::Recomputable
    );

    let (mut model, catalog) = sales();
    let mut lo = metric("lo", "MIN(sales.amount)");
    lo.custom_extensions = ours(r#"{"version": 1, "aggregation": {"class": "fully_additive"}}"#);
    model.metrics.push(lo);
    rejects(
        convert(&model, &catalog),
        Code::InvalidExtension,
        "/metrics/1/custom_extensions",
    );
}

#[test]
fn a_combination_of_sums_is_recomputed_unless_declared_additive() {
    let (mut model, mut catalog) = sales();
    catalog
        .relations
        .get_mut("sales")
        .expect("sales")
        .columns
        .push(column("other", amount(), true));
    let mut other = model.datasets[0].fields[2].clone();
    other.name = "other".to_owned();
    other.expression = portable("other");
    model.datasets[0].fields.push(other);
    model.metrics.push(metric("b", "SUM(sales.other)"));
    model.metrics.push(metric("total", "revenue + b"));
    let converted = convert(&model, &catalog).expect("converts");
    let total = converted
        .metrics
        .iter()
        .find(|metric| metric.metadata.id().as_str() == "corpus/metric/total")
        .expect("total");
    assert_eq!(
        total.aggregation_contract.classification,
        AggregationClass::Recomputable
    );
    assert!(total.aggregation_contract.decomposition.is_some());

    // The author may claim exactness for a combination of additive metrics…
    model.metrics[2].custom_extensions =
        ours(r#"{"version": 1, "aggregation": {"class": "fully_additive"}}"#);
    let converted = convert(&model, &catalog).expect("declared additive");
    assert_eq!(class(&converted, "total"), AggregationClass::FullyAdditive);
    // …but not for anything else.
    model.metrics[2].expression = portable("CAST(revenue / b AS DECIMAL(18, 2))");
    rejects(
        convert(&model, &catalog),
        Code::InvalidExtension,
        "/metrics/2/custom_extensions",
    );
}

#[test]
fn a_single_aggregate_cannot_declare_its_own_grain() {
    let (mut model, catalog) = sales();
    model.metrics[0].custom_extensions =
        ours(r#"{"version": 1, "grain": {"dimensions": ["missing"]}}"#);
    rejects(
        convert(&model, &catalog),
        Code::InvalidExtension,
        "/metrics/0/custom_extensions",
    );
}

#[test]
fn a_declared_grain_must_exist() {
    let (mut model, catalog) = sales();
    let mut doubled = metric("doubled", "revenue + revenue");
    doubled.custom_extensions = ours(r#"{"version": 1, "grain": {"dimensions": ["missing"]}}"#);
    model.metrics.push(doubled);
    rejects(
        convert(&model, &catalog),
        Code::UnknownReference,
        "/metrics/1/custom_extensions",
    );
}

// ---- Declared types are checked on computed values --------------------------

#[test]
fn a_computed_field_must_match_its_datatype() {
    let (mut model, catalog) = sales();
    model.datasets[0].fields[2].name = "doubled".to_owned();
    model.datasets[0].fields[2].expression = portable("amount * 2");
    model.datasets[0].fields[2].datatype = Some(DataType::Integer);
    model.metrics[0].expression = portable("SUM(sales.doubled)");
    rejects(
        convert(&model, &catalog),
        Code::DatatypeMismatch,
        "/datasets/0/fields/2/datatype",
    );
}

#[test]
fn a_computed_dimension_must_match_its_datatype() {
    let (mut model, catalog) = sales();
    model.datasets[0].fields[1].expression =
        portable("CASE WHEN region = 'north' THEN 'n' ELSE 'other' END");
    model.datasets[0].fields[1].datatype = Some(DataType::Integer);
    rejects(
        convert(&model, &catalog),
        Code::DatatypeMismatch,
        "/datasets/0/fields/1/datatype",
    );
}

// ---- SEF's additive partition property -----------------------------------------

/// `t(id, a, b)`, with the metrics whose classification is checked.
const PARTITION_MODEL: &str = r#"
version: "0.2.0.dev0"
name: partition
datasets:
  - name: t
    source: t
    primary_key: [id]
    fields:
      - {name: id, expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: id}]}}
      - {name: a, expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: a}]}}
      - {name: b, expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: b}]}}
metrics:
  - {name: sum_a, expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: "SUM(t.a)"}]}}
  - {name: sum_b, expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: "SUM(t.b)"}]}}
  - {name: count_a, expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: "COUNT(t.a)"}]}}
  - {name: rows, expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: "COUNT(*)"}]},
     custom_extensions: [{vendor_name: bi-platform, data: '{"version": 1, "dataset": "t"}'}]}
  - {name: min_a, expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: "MIN(t.a)"}]}}
  - {name: max_a, expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: "MAX(t.a)"}]}}
  - {name: distinct_a, expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: "COUNT(DISTINCT t.a)"}]}}
  - {name: a_plus_b, expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: "sum_a + sum_b"}]}}
  - {name: a_minus_b, expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: "sum_a - sum_b"}]}}
  - {name: range_a, expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: "max_a - min_a"}]}}
"#;

type Row = (Option<i64>, Option<i64>);

/// SQL semantics of each metric over a set of rows.
fn evaluate(name: &str, rows: &[Row]) -> Option<i128> {
    let sum = |values: Vec<i64>| {
        (!values.is_empty()).then(|| values.iter().map(|&v| i128::from(v)).sum::<i128>())
    };
    let a: Vec<i64> = rows.iter().filter_map(|row| row.0).collect();
    let b: Vec<i64> = rows.iter().filter_map(|row| row.1).collect();
    let count = |n: usize| Some(i128::try_from(n).expect("count"));
    match name {
        "sum_a" => sum(a),
        "sum_b" => sum(b),
        "count_a" => count(a.len()),
        "rows" => count(rows.len()),
        "min_a" => a.iter().min().map(|&v| i128::from(v)),
        "max_a" => a.iter().max().map(|&v| i128::from(v)),
        "distinct_a" => count(a.iter().collect::<BTreeSet<_>>().len()),
        "a_plus_b" => Some(evaluate("sum_a", rows)? + evaluate("sum_b", rows)?),
        "a_minus_b" => Some(evaluate("sum_a", rows)? - evaluate("sum_b", rows)?),
        "range_a" => Some(evaluate("max_a", rows)? - evaluate("min_a", rows)?),
        other => panic!("unknown metric {other}"),
    }
}

/// The metrics the converter classifies as fully additive.
fn fully_additive() -> &'static BTreeSet<String> {
    static CLASSIFIED: OnceLock<BTreeSet<String>> = OnceLock::new();
    CLASSIFIED.get_or_init(|| {
        let model = sef_ossie::load(PARTITION_MODEL).expect("model").model;
        let catalog = Catalog {
            relations: BTreeMap::from([relation(
                "t",
                vec![
                    column("id", integer(), false),
                    column("a", integer(), true),
                    column("b", integer(), true),
                ],
            )]),
        };
        let options = Options {
            namespace: "test".to_owned(),
            model: "partition".to_owned(),
        };
        let converted = to_csmir(&model, &catalog, &options).expect("partition model");
        converted
            .metrics
            .iter()
            .filter(|metric| {
                metric.aggregation_contract.classification == AggregationClass::FullyAdditive
            })
            .map(|metric| {
                metric
                    .metadata
                    .id()
                    .as_str()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    })
}

#[test]
fn only_sums_and_counts_are_classified_fully_additive() {
    let expected: BTreeSet<String> = ["sum_a", "sum_b", "count_a", "rows"]
        .map(str::to_owned)
        .into();
    assert_eq!(fully_additive(), &expected);
}

proptest! {
    /// For a fully additive metric over a complete disjoint partition,
    /// m(all) == Σ m(partition_i), with SQL's NULL rules for the Σ
    /// (`docs/semantica/04_CORRECTNESS_AND_PLANNING.md`, §23).
    #[test]
    fn fully_additive_metrics_satisfy_the_partition_property(
        rows in prop::collection::vec(
            ((prop::option::of(-1_000_i64..1_000), prop::option::of(-1_000_i64..1_000)), 0_usize..4),
            0..40,
        ),
    ) {
        let all: Vec<Row> = rows.iter().map(|(row, _)| *row).collect();
        for name in fully_additive() {
            let parts: Vec<i128> = (0..4)
                .filter_map(|part| {
                    let rows: Vec<Row> = rows
                        .iter()
                        .filter(|(_, p)| *p == part)
                        .map(|(row, _)| *row)
                        .collect();
                    (!rows.is_empty()).then(|| evaluate(name, &rows)).flatten()
                })
                .collect();
            let summed = (!parts.is_empty()).then(|| parts.iter().sum::<i128>());
            let summed = match (name.as_str(), summed) {
                // A count of nothing is zero, not NULL.
                ("count_a" | "rows", None) => Some(0),
                (_, summed) => summed,
            };
            prop_assert_eq!(evaluate(name, &all), summed, "{}", name);
        }
    }
}

#[test]
fn the_partition_property_rejects_summing_min_or_a_combination() {
    // The counterexamples from the review, evaluated directly.
    let rows = [(Some(1), None), (Some(2), None)];
    let by_row: Option<i128> = rows.iter().map(|row| evaluate("min_a", &[*row])).sum();
    assert_ne!(evaluate("min_a", &rows), by_row);
    let rows = [(Some(1), None), (None, Some(2))];
    let parts: Vec<Option<i128>> = rows
        .iter()
        .map(|row| evaluate("a_plus_b", &[*row]))
        .collect();
    assert_eq!(parts, [None, None]);
    assert_eq!(evaluate("a_plus_b", &rows), Some(3));
    // The classification keeps both out of the summable set.
    assert!(!fully_additive().contains("min_a"));
    assert!(!fully_additive().contains("a_plus_b"));
}

#[test]
fn a_column_named_like_a_field_of_another_column_is_ambiguous() {
    let (mut model, mut catalog) = sales();
    let columns = &mut catalog.relations.get_mut("sales").expect("sales").columns;
    columns.push(column("zone", ValueType::Utf8, false));
    // `region` now reads the column `zone`; a computed field still reads the
    // column `region`, whose implicit field would take the same ID.
    model.datasets[0].fields[1].expression = portable("zone");
    let mut label = model.datasets[0].fields[1].clone();
    label.name = "label".to_owned();
    label.expression = portable("CASE WHEN region = 'north' THEN 'n' ELSE 'other' END");
    label.custom_extensions = ours(r#"{"version": 1, "dimension": "label"}"#);
    model.datasets[0].fields.push(label);
    rejects(
        convert(&model, &catalog),
        Code::AmbiguousReference,
        "/datasets/0/fields/3/expression/dialects/0/expression",
    );
}
