//! Ossie → CSMIR rules: what is rejected and with which code, what is
//! translated exactly, and properties of the conversion.
// Fixtures are trusted test inputs.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use proptest::prelude::*;
use sef_model_ir::expressions::{BooleanOp, ComparisonOp, ScalarExprKind};
use sef_model_ir::identity::SourceHandle;
use sef_model_ir::model::ModelDefinition;
use sef_model_ir::relationships::Optionality;
use sef_model_ir::time::TimestampSemantics;
use sef_model_ir::types::{DecimalType, ValueType};
use sef_ossie::catalog::{Catalog, Column, Relation};
use sef_ossie::csmir::{Options, compile, to_csmir};
use sef_ossie::document::{
    CustomExtension, Dialect, DialectExpression, Expression, Field, Metric, SemanticModel,
};
use sef_ossie::{Code, Diagnostic};

fn read(file: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(file);
    std::fs::read_to_string(path).expect("fixture")
}

fn document(case: u8) -> SemanticModel {
    sef_ossie::load(&read(&format!("CASE-{case:03}.yaml")))
        .expect("corpus document")
        .model
}

fn value_type(spec: &str) -> (ValueType, bool) {
    let (spec, nullable) = spec
        .strip_suffix(" null")
        .map_or((spec, false), |spec| (spec, true));
    let value_type = match spec {
        "int64" => ValueType::Integer {
            signed: true,
            bits: Some(64),
        },
        "utf8" => ValueType::Utf8,
        "date" => ValueType::Date,
        "timestamptz" => ValueType::Timestamp {
            timezone_semantics: TimestampSemantics::Instant,
        },
        decimal => {
            let (p, s) = decimal
                .strip_prefix("decimal(")
                .and_then(|rest| rest.strip_suffix(')'))
                .and_then(|rest| rest.split_once(','))
                .expect("decimal spec");
            ValueType::Decimal(
                DecimalType::new(p.parse().expect("p"), s.parse().expect("s")).expect("type"),
            )
        }
    };
    (value_type, nullable)
}

fn catalog(case: u8) -> Catalog {
    let relations: BTreeMap<String, BTreeMap<String, String>> =
        serde_saphyr::from_str(&read(&format!("CASE-{case:03}.catalog.yaml"))).expect("catalog");
    Catalog {
        relations: relations
            .into_iter()
            .map(|(name, columns)| {
                let relation = Relation {
                    source: SourceHandle::new("corpus").expect("handle"),
                    relation: SourceHandle::new(name.as_str()).expect("handle"),
                    columns: columns
                        .into_iter()
                        .map(|(column, spec)| {
                            let (value_type, nullable) = value_type(&spec);
                            Column {
                                handle: SourceHandle::new(column.as_str()).expect("handle"),
                                name: column,
                                value_type,
                                nullable,
                            }
                        })
                        .collect(),
                };
                (name, relation)
            })
            .collect(),
    }
}

fn options(case: u8) -> Options {
    Options {
        namespace: "corpus".to_owned(),
        model: format!("case-{case:03}"),
    }
}

fn convert(case: u8, model: &SemanticModel) -> Result<ModelDefinition, Vec<Diagnostic>> {
    to_csmir(model, &catalog(case), &options(case))
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

/// The model must be rejected with `code` at `path`.
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

// ---- What must be rejected -------------------------------------------------

#[test]
fn a_source_the_catalog_does_not_know_is_rejected() {
    let mut model = document(1);
    model.datasets[0].source = "warehouse.sales".to_owned();
    rejects(
        convert(1, &model),
        Code::UnknownSource,
        "/datasets/0/source",
    );
}

#[test]
fn a_column_the_source_lacks_is_rejected() {
    let mut model = document(1);
    model.datasets[0].fields[2].expression = portable("importe");
    rejects(
        convert(1, &model),
        Code::UnknownColumn,
        "/datasets/0/fields/2/expression/dialects/0/expression",
    );
}

#[test]
fn a_datatype_that_contradicts_the_source_is_rejected() {
    let mut model = document(1);
    model.datasets[0].fields[2].datatype = Some(sef_ossie::document::DataType::Float);
    rejects(
        convert(1, &model),
        Code::DatatypeMismatch,
        "/datasets/0/fields/2/datatype",
    );
}

#[test]
fn a_division_without_its_result_type_is_rejected() {
    let mut model = document(6);
    model.metrics[2].expression = portable("revenue / order_count");
    rejects(
        convert(6, &model),
        Code::TypeMismatch,
        "/metrics/2/expression/dialects/0/expression",
    );
}

#[test]
fn aggregates_the_engine_cannot_compute_exactly_are_rejected_not_guessed() {
    for aggregate in [
        "STDDEV(sales.amount)",
        "MEDIAN(sales.amount)",
        "APPROX_COUNT_DISTINCT(sales.region)",
        "SUM(DISTINCT sales.amount)",
        "SUM(sales.amount) OVER (ORDER BY sales.sale_id)",
    ] {
        let mut model = document(1);
        model.metrics[0].expression = portable(aggregate);
        rejects(
            convert(1, &model),
            Code::NotRepresentable,
            "/metrics/0/expression/dialects/0/expression",
        );
    }
}

#[test]
fn an_aggregate_over_two_datasets_is_rejected() {
    let mut model = document(2);
    model.metrics[0].expression =
        portable("SUM(sales.unit_price * CAST(customers.customer_id AS DECIMAL(20, 0)))");
    rejects(
        convert(2, &model),
        Code::NotRepresentable,
        "/metrics/0/expression/dialects/0/expression",
    );
}

#[test]
fn count_star_must_say_which_rows_it_counts() {
    let mut model = document(14);
    model.metrics[2].custom_extensions.clear();
    rejects(
        convert(14, &model),
        Code::UnknownReference,
        "/metrics/2/expression/dialects/0/expression",
    );
}

#[test]
fn combining_metrics_of_different_grains_requires_a_declared_grain() {
    let mut model = document(10);
    model.metrics[2].custom_extensions = ours(r#"{"version": 1, "unit": "ratio"}"#);
    rejects(
        convert(10, &model),
        Code::GrainRequired,
        "/metrics/2/expression/dialects/0/expression",
    );
}

#[test]
fn metrics_that_depend_on_themselves_are_rejected() {
    let mut model = document(9);
    model.metrics[0].expression = portable("net_revenue + refunds");
    rejects(
        convert(9, &model),
        Code::ReferenceCycle,
        "/metrics/2/expression/dialects/0/expression",
    );
}

#[test]
fn date_trunc_without_a_timezone_is_rejected() {
    let mut model = document(12);
    model.datasets[0].fields[3].custom_extensions = ours(r#"{"version": 1, "dimension": "month"}"#);
    rejects(
        convert(12, &model),
        Code::InvalidExtension,
        "/datasets/0/fields/3/expression/dialects/0/expression",
    );
}

#[test]
fn malformed_extensions_are_rejected_never_ignored() {
    for (data, why) in [
        (r#"{"version": 1, "entity": "sale", "entyty": "x"}"#, "typo"),
        (r#"{"version": 2, "entity": "sale"}"#, "future version"),
        (r#"{"entity": "sale"}"#, "missing version"),
        ("not json", "not JSON"),
    ] {
        let mut model = document(1);
        model.datasets[0].custom_extensions = ours(data);
        let errors = convert(1, &model).expect_err(why);
        assert!(
            errors
                .iter()
                .any(|error| error.code == Code::InvalidExtension
                    && error.path == "/datasets/0/custom_extensions/0/data"),
            "{why}: {errors:#?}"
        );
    }
    let mut model = document(1);
    model.datasets[0]
        .custom_extensions
        .extend(ours(r#"{"version": 1}"#));
    rejects(
        convert(1, &model),
        Code::InvalidExtension,
        "/datasets/0/custom_extensions/1",
    );
}

#[test]
fn an_unqualified_field_in_several_datasets_is_ambiguous() {
    let mut model = document(9);
    model.metrics[0].expression = portable("SUM(amount)");
    rejects(
        convert(9, &model),
        Code::AmbiguousReference,
        "/metrics/0/expression/dialects/0/expression",
    );
}

#[test]
fn implicit_numeric_promotion_is_rejected() {
    let mut model = document(2);
    model.metrics[0].expression = portable("SUM(sales.quantity * sales.unit_price)");
    rejects(
        convert(2, &model),
        Code::TypeMismatch,
        "/metrics/0/expression/dialects/0/expression",
    );
}

#[test]
fn a_conformed_dimension_needs_one_type() {
    let mut model = document(9);
    // `amount` (DECIMAL) claims to be the `region` (VARCHAR) dimension.
    model.datasets[1].fields[2].dimension = Some(sef_ossie::document::Dimension { is_time: None });
    model.datasets[1].fields[2].custom_extensions =
        ours(r#"{"version": 1, "dimension": "region"}"#);
    rejects(
        convert(9, &model),
        Code::TypeMismatch,
        "/datasets/1/fields/2",
    );
}

#[test]
fn only_the_portable_dialect_has_meaning() {
    let mut model = document(1);
    model.metrics[0].expression.dialects[0].dialect = Dialect::AnsiSql;
    rejects(
        convert(1, &model),
        Code::NotRepresentable,
        "/metrics/0/expression/dialects",
    );
}

#[test]
fn a_key_must_name_a_column_field() {
    let mut model = document(12);
    model.datasets[0].primary_key = Some(vec!["month".to_owned()]);
    rejects(
        convert(12, &model),
        Code::UnknownColumn,
        "/datasets/0/primary_key/0",
    );
}

#[test]
fn adding_amounts_of_different_currencies_is_rejected_by_csmir() {
    let mut model = document(17);
    model
        .metrics
        .push(metric("combined", "revenue_usd + revenue_bob"));
    let errors = compile(&model, &catalog(17), &options(17)).expect_err("units differ");
    assert!(
        errors
            .iter()
            .any(|error| error.code == Code::SemanticContract
                && error.path == "/metrics/2"
                && error.message.contains("INCOMPATIBLE_UNITS")),
        "{errors:#?}"
    );
}

#[test]
fn invalid_options_are_rejected() {
    let model = document(1);
    let mut options = options(1);
    options.namespace = "Corpus".to_owned();
    let errors = to_csmir(&model, &catalog(1), &options).expect_err("uppercase namespace");
    assert_eq!(errors[0].code, Code::InvalidIdentifier);
}

// ---- What is translated exactly --------------------------------------------

fn dimension_expression(model: &ModelDefinition, name: &str) -> ScalarExprKind {
    model
        .dimensions
        .iter()
        .find(|dimension| dimension.metadata.id().as_str().ends_with(name))
        .expect("dimension")
        .bindings[0]
        .expression
        .kind
        .clone()
}

#[test]
fn between_and_in_become_exact_comparisons() {
    let mut model = document(1);
    model.datasets[0].fields.push(Field {
        name: "band".to_owned(),
        expression: portable(
            "CASE WHEN amount BETWEEN 0 AND 99.99 THEN 'small' \
             WHEN region IN ('north', 'south') THEN 'regional' END",
        ),
        dimension: Some(sef_ossie::document::Dimension { is_time: None }),
        label: None,
        description: None,
        datatype: None,
        ai_context: None,
        custom_extensions: Vec::new(),
    });
    let converted = convert(1, &model).expect("converts");
    let ScalarExprKind::Case {
        branches,
        otherwise,
    } = dimension_expression(&converted, "sales.band")
    else {
        panic!("CASE");
    };
    let ScalarExprKind::BooleanLogic {
        op: BooleanOp::And,
        operands,
    } = &branches[0].0.kind
    else {
        panic!("BETWEEN is AND");
    };
    assert!(matches!(
        operands[0].kind,
        ScalarExprKind::Comparison {
            op: ComparisonOp::GreaterOrEqual,
            ..
        }
    ));
    // The decimal literal takes the column's exact type: 99.99 at (18, 2).
    let ScalarExprKind::Comparison { right, .. } = &operands[1].kind else {
        panic!("comparison");
    };
    assert_eq!(right.value_type, value_type("decimal(18,2)").0);
    assert!(matches!(
        branches[1].0.kind,
        ScalarExprKind::BooleanLogic {
            op: BooleanOp::Or,
            ..
        }
    ));
    // No ELSE: NULL of the branches' type.
    assert_eq!(otherwise.value_type, ValueType::Utf8);
}

#[test]
fn coalesce_and_nullif_become_case_expressions() {
    let mut model = document(14);
    model.metrics[0].expression = portable("SUM(COALESCE(measurements.amount, 0))");
    let converted = convert(14, &model).expect("converts");
    let measure = &converted.measures[0];
    let scalar = measure.scalar_expression.as_ref().expect("scalar");
    assert!(matches!(scalar.kind, ScalarExprKind::Case { .. }));
    assert_eq!(scalar.value_type, value_type("decimal(18,2)").0);
}

#[test]
fn relationships_are_optional_unless_declared_required() {
    let mut model = document(3);
    model.relationships[0].custom_extensions.clear();
    let converted = convert(3, &model).expect("converts");
    assert_eq!(
        converted.relationships[0].optionality,
        Optionality::Optional
    );
}

#[test]
fn average_is_an_exact_ratio_of_two_synthesized_metrics() {
    let mut model = document(1);
    model.metrics.push(metric(
        "average_sale",
        "CAST(AVG(sales.amount) AS DECIMAL(18, 2))",
    ));
    let compiled = compile(&model, &catalog(1), &options(1)).expect("compiles");
    let names: Vec<&str> = compiled
        .definition()
        .metrics
        .iter()
        .map(|metric| metric.metadata.id().as_str())
        .collect();
    assert!(
        names.contains(&"corpus/metric/average_sale.m1"),
        "{names:?}"
    );
    assert!(
        names.contains(&"corpus/metric/average_sale.m2"),
        "{names:?}"
    );
}

#[test]
fn non_ascii_names_get_distinct_valid_ids() {
    let mut model = document(1);
    model.datasets[0].fields[1].custom_extensions =
        ours(r#"{"version": 1, "dimension": "región"}"#);
    let converted = convert(1, &model).expect("converts");
    // `región` is upper-cased like any unquoted name; `Ó` is U+00D3.
    assert_eq!(
        converted.dimensions[0].metadata.id().as_str(),
        "corpus/dimension/regi-d3-n"
    );
    assert_ne!(
        sef_ossie::ids::segment("región"),
        sef_ossie::ids::segment("region")
    );
    assert_ne!(
        sef_ossie::ids::segment("\"region\""),
        sef_ossie::ids::segment("region")
    );
}

// ---- Properties -------------------------------------------------------------

/// Inverse of [`sef_ossie::ids::encode`]; exists only to prove injectivity.
fn decode(segment: &str) -> Option<String> {
    let body = match segment.strip_prefix("x-") {
        Some(rest) if rest.starts_with(['-', '_']) => rest,
        _ => segment,
    };
    let mut decoded = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            'a'..='z' => decoded.push(c.to_ascii_uppercase()),
            '0'..='9' | '_' => decoded.push(c),
            '-' => {
                let hex: String = chars.by_ref().take_while(|&c| c != '-').collect();
                decoded.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            _ => return None,
        }
    }
    Some(decoded)
}

proptest! {
    #[test]
    fn ids_are_valid_segments_and_injective(normalized in "\\PC{1,40}") {
        let segment = sef_ossie::ids::encode(&normalized);
        prop_assert!(sef_model_ir::identity::valid_segment(&segment), "{segment}");
        prop_assert_eq!(decode(&segment), Some(normalized));
    }

    /// Document order is not meaning: any permutation of datasets, fields,
    /// relationships and metrics compiles to the same fingerprint.
    #[test]
    fn document_order_does_not_change_the_fingerprint(
        case in prop::sample::select(vec![2_u8, 4, 5, 9, 10, 16]),
        seed in any::<u64>(),
    ) {
        let original = document(case);
        let mut shuffled = original.clone();
        let mut state = seed;
        let mut next = move |n: usize| {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            usize::try_from(state >> 33).unwrap_or_default() % n
        };
        let mut shuffle = |len: usize| -> Vec<usize> {
            let mut order: Vec<usize> = (0..len).collect();
            for i in (1..len).rev() {
                order.swap(i, next(i + 1));
            }
            order
        };
        let order = shuffle(shuffled.datasets.len());
        shuffled.datasets = order.iter().map(|&i| original.datasets[i].clone()).collect();
        for dataset in &mut shuffled.datasets {
            let order = shuffle(dataset.fields.len());
            dataset.fields = order.iter().map(|&i| dataset.fields[i].clone()).collect();
        }
        let order = shuffle(shuffled.relationships.len());
        shuffled.relationships = order.iter().map(|&i| original.relationships[i].clone()).collect();
        let order = shuffle(shuffled.metrics.len());
        shuffled.metrics = order.iter().map(|&i| original.metrics[i].clone()).collect();
        let a = compile(&original, &catalog(case), &options(case)).expect("original");
        let b = compile(&shuffled, &catalog(case), &options(case)).expect("shuffled");
        prop_assert_eq!(a.content_hash(), b.content_hash());
    }
}
