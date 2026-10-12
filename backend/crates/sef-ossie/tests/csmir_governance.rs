//! Contracts beyond Ossie 0.2 (part 3b of #48): period selection, distinct
//! axes, bridges with allocation, as-of relationships, row policies,
//! rollup measures and cumulative windows. Each must reject what it cannot
//! state exactly, with a stable code and the path of the declaration.
// Fixtures are trusted test inputs.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use sef_model_ir::expressions::ScalarExprKind;
use sef_model_ir::identity::SourceHandle;
use sef_model_ir::model::ModelDefinition;
use sef_model_ir::time::TimestampSemantics;
use sef_model_ir::types::{DecimalType, ValueType};
use sef_ossie::catalog::{Catalog, Column, Relation};
use sef_ossie::csmir::{Options, to_csmir};
use sef_ossie::document::{
    CustomExtension, Dialect, DialectExpression, Expression, Metric, SemanticModel,
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

fn value_type(spec: &str) -> ValueType {
    match spec {
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
    }
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
                        .map(|(column, spec)| Column {
                            handle: SourceHandle::new(column.as_str()).expect("handle"),
                            name: column,
                            value_type: value_type(&spec),
                            nullable: false,
                        })
                        .collect(),
                };
                (name, relation)
            })
            .collect(),
    }
}

fn convert(case: u8, model: &SemanticModel) -> Result<ModelDefinition, Vec<Diagnostic>> {
    let options = Options {
        namespace: "corpus".to_owned(),
        model: format!("case-{case:03}"),
    };
    to_csmir(model, &catalog(case), &options)
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

// ---- Period selection (CASE-007) -------------------------------------------

const BALANCE: &str = r#""aggregation": {"class": "semi_additive", "axes": [
    {"dimension": "snapshot_date", "behavior": "period_end"},
    {"entity": "account", "behavior": "additive"}]}"#;

#[test]
fn period_selection_must_name_existing_axes_keys_and_fields() {
    for (behavior, what) in [
        (
            r#""time_behavior": {"axis": {"dimension": "day"}, "rule": "period_end", "selection_scope": {"entities": ["account"]}, "ties": "unspecified"}"#,
            "unknown axis",
        ),
        (
            r#""time_behavior": {"axis": {"dimension": "snapshot_date"}, "rule": "period_end", "order_by": [{"field": "balances.missing"}], "selection_scope": {"entities": ["account"]}, "ties": "unspecified"}"#,
            "unknown order field",
        ),
        (
            r#""time_behavior": {"axis": {"dimension": "snapshot_date"}, "rule": "period_end", "selection_scope": {"entities": ["account"]}, "ties": {"unique_by_key": "missing"}}"#,
            "unknown key",
        ),
    ] {
        let mut model = document(7);
        model.metrics[0].custom_extensions =
            ours(&format!(r#"{{"version": 1, {BALANCE}, {behavior}}}"#));
        let errors = convert(7, &model).expect_err(what);
        assert!(
            errors
                .iter()
                .any(|error| error.code == Code::UnknownReference
                    && error.path == "/metrics/0/custom_extensions"),
            "{what}: {errors:#?}"
        );
    }
}

#[test]
fn an_axis_names_exactly_one_dimension_or_entity() {
    let mut model = document(7);
    model.metrics[0].custom_extensions = ours(
        r#"{"version": 1, "aggregation": {"class": "semi_additive", "axes": [
            {"dimension": "snapshot_date", "entity": "account", "behavior": "period_end"}]}}"#,
    );
    rejects(
        convert(7, &model),
        Code::InvalidExtension,
        "/metrics/0/custom_extensions",
    );
}

// ---- Bridges and allocation (CASE-011) ---------------------------------------

fn bridge(allocation: &str, left_relationship: &str) -> Vec<CustomExtension> {
    ours(&format!(
        r#"{{"version": 1, "semantic_version": "0.1.0", "bridges": [{{"name": "order_campaign",
            "left": "orders", "bridge": "order_campaign", "right": "campaign",
            "left_relationship": "{left_relationship}", "right_relationship": "campaign",
            "allocation": {allocation}}}]}}"#
    ))
}

const ALLOCATION: &str = r#"{"weight": "allocation_weight", "normalization_scope": {"entities": ["order"]}, "required_total": "1.00", "tolerance": "0.00"}"#;

#[test]
fn a_bridge_uses_relationships_that_start_at_the_bridge() {
    let mut model = document(11);
    model.custom_extensions = bridge(ALLOCATION, "campaign");
    rejects(
        convert(11, &model),
        Code::UnknownReference,
        "/custom_extensions/bridges/0",
    );
}

#[test]
fn an_allocation_weight_is_an_exact_decimal_at_its_own_scale() {
    for allocation in [
        r#"{"weight": "order_id", "normalization_scope": {"entities": ["order"]}, "required_total": "1", "tolerance": "0"}"#,
        r#"{"weight": "allocation_weight", "normalization_scope": {"entities": ["order"]}, "required_total": "1.000", "tolerance": "0.00"}"#,
    ] {
        let mut model = document(11);
        model.custom_extensions = bridge(allocation, "order");
        rejects(
            convert(11, &model),
            Code::TypeMismatch,
            "/custom_extensions/bridges/0",
        );
    }
}

#[test]
fn a_metric_names_an_existing_bridge() {
    let mut model = document(11);
    model.metrics[0].custom_extensions =
        ours(r#"{"version": 1, "allocation_bridge": "order_channel"}"#);
    rejects(
        convert(11, &model),
        Code::UnknownReference,
        "/metrics/0/custom_extensions",
    );
}

// ---- As-of relationships (CASE-013) --------------------------------------------

#[test]
fn an_as_of_match_names_column_fields() {
    let mut model = document(13);
    model.relationships[0].custom_extensions = ours(
        r#"{"version": 1, "optionality": "required", "temporal": {"event_time": "sales.sold_at",
            "valid_from": "customer_history.valid_from", "boundary": "closed_open",
            "match": "exactly_one"}}"#,
    );
    rejects(
        convert(13, &model),
        Code::UnknownReference,
        "/relationships/0/custom_extensions",
    );
}

// ---- Row policies (CASE-015) -----------------------------------------------------

fn policy(predicate: &str, scope: &str, context: &str) -> Vec<CustomExtension> {
    ours(&format!(
        r#"{{"version": 1, "semantic_version": "0.1.0", "policies": [{{"name": "tenant", {scope},
            "enforcement": "mandatory", "predicate": "{predicate}", "context": {context}}}]}}"#
    ))
}

const TENANT: &str = r#"[{"name": "tenant_id", "type": "string", "sensitivity": "sensitive"}]"#;

#[test]
fn a_policy_binds_its_context_values_as_parameters() {
    let model = document(15);
    let converted = convert(15, &model).expect("converts");
    let ScalarExprKind::Comparison { right, .. } = &converted.policies[0].predicate.kind else {
        panic!("comparison");
    };
    assert_eq!(
        right.kind,
        ScalarExprKind::ContextSlot("tenant_id".to_owned()),
        "a parameter, never a value spliced into the SQL"
    );
    assert_eq!(converted.datasets[0].policy_refs.len(), 1);
}

#[test]
fn malformed_policies_are_rejected() {
    let dataset = r#""dataset": "sales""#;
    for (predicate, scope, context, code) in [
        (
            "tenant_id = :tenant",
            dataset,
            TENANT,
            Code::UnknownReference,
        ),
        ("tenant_id = 'a'", dataset, TENANT, Code::InvalidExtension),
        ("tenant_id = ?", dataset, "[]", Code::NotRepresentable),
        ("tenant_id", dataset, "[]", Code::TypeMismatch),
        ("amount = :tenant_id", dataset, TENANT, Code::TypeMismatch),
        (
            "tenant_id = :tenant_id",
            r#""metric": "revenue""#,
            TENANT,
            Code::NotRepresentable,
        ),
        (
            "tenant_id = :tenant_id",
            r#""dataset": "sales", "metric": "revenue""#,
            TENANT,
            Code::InvalidExtension,
        ),
    ] {
        let mut model = document(15);
        model.custom_extensions = policy(predicate, scope, context);
        let errors = convert(15, &model).expect_err(predicate);
        assert!(
            errors
                .iter()
                .any(|error| error.code == code && error.path == "/custom_extensions/policies/0"),
            "{predicate} / {scope}: {errors:#?}"
        );
    }
}

#[test]
fn context_values_belong_to_policies_only() {
    let mut model = document(15);
    // Bypass load(): its validator already rejects the parameter.
    model.metrics[0].expression =
        portable("SUM(CASE WHEN sales.tenant_id = :tenant_id THEN sales.amount END)");
    rejects(
        convert(15, &model),
        Code::NotRepresentable,
        "/metrics/0/expression/dialects/0/expression",
    );
}

// ---- Rollup measures (CASE-018) ---------------------------------------------------

#[test]
fn a_dataset_measure_is_one_aggregate_of_its_own_dataset() {
    for (measure, code) in [
        (
            r#"{"name": "x", "expression": "SUM(sales.amount)"}"#,
            Code::UnknownReference,
        ),
        (
            r#"{"name": "x", "expression": "revenue + revenue"}"#,
            Code::NotRepresentable,
        ),
    ] {
        let mut model = document(18);
        model.datasets[1].custom_extensions = ours(&format!(
            r#"{{"version": 1, "entity": "month_category", "grain": {{"dimensions": ["month", "category"]}},
                "measures": [{measure}]}}"#
        ));
        let errors = convert(18, &model).expect_err(measure);
        assert!(
            errors
                .iter()
                .any(|error| error.code == code && error.path == "/datasets/1/custom_extensions"),
            "{measure}: {errors:#?}"
        );
    }
}

// ---- Cumulative windows (CASE-021) -------------------------------------------------

#[test]
fn only_cumulative_sums_over_dimensions_are_windows() {
    for window in [
        r#"AVG(daily_revenue) OVER (ORDER BY "date")"#,
        "SUM(daily_revenue) OVER ()",
        r#"SUM(daily_revenue) OVER (ORDER BY "date" ROWS BETWEEN 1 PRECEDING AND CURRENT ROW)"#,
        r#"SUM(daily_revenue) OVER (ORDER BY "date" ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING)"#,
        "SUM(daily_revenue) OVER (ORDER BY amount)",
    ] {
        let mut model = document(21);
        model.metrics[1] = Metric {
            expression: portable(window),
            ..model.metrics[1].clone()
        };
        let errors = convert(21, &model).expect_err(window);
        assert!(
            errors.iter().any(|error| matches!(
                error.code,
                Code::NotRepresentable | Code::UnknownReference
            ) && error.path
                == "/metrics/1/expression/dialects/0/expression"),
            "{window}: {errors:#?}"
        );
    }
}
