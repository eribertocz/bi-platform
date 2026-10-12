//! Each rejection of the engine (§21.2), proven with the corpus models or a
//! minimal change to them, next to the request that makes it valid.
#![allow(clippy::expect_used, clippy::panic)]

mod common;

use common::{bound, dimension, metric, model, query, zoned};
use sef_correctness::{SemanticContext, ValidatedQuery, validate};
use sef_model_ir::diagnostic::{Diagnostic, ErrorCode as Code};
use sef_model_ir::model::{CompiledSemanticModel, ModelDefinition};
use sef_model_ir::time::TimeGrain;
use sef_model_ir::types::Literal;
use sef_query_ir::unbound::{UnboundDimension, UnboundItem, UnboundQuerySet, UnboundSelection};

fn changed(case: u8, change: impl FnOnce(&mut ModelDefinition)) -> CompiledSemanticModel {
    let mut definition = model(case).definition().clone();
    change(&mut definition);
    definition
        .compile()
        .unwrap_or_else(|errors| panic!("changed CASE-{case:03} must compile: {errors:#?}"))
}

fn check(
    model: &CompiledSemanticModel,
    request: &UnboundQuerySet,
    context: &SemanticContext,
) -> Result<ValidatedQuery, Vec<Diagnostic>> {
    let query = sef_query_ir::bind(request, model.definition())
        .unwrap_or_else(|errors| panic!("must bind: {errors:#?}"));
    validate(model, &query, context)
}

fn rejects(
    model: &CompiledSemanticModel,
    request: &UnboundQuerySet,
    context: &SemanticContext,
    code: Code,
    path: &str,
) {
    let errors = check(model, request, context).expect_err("must be rejected");
    assert!(
        errors.iter().any(|error| error.code == code
            && error.query_refs.first().map(String::as_str) == Some(path)),
        "expected {} at {path}, got:\n{}",
        code.as_str(),
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn none() -> SemanticContext {
    SemanticContext::default()
}

fn by_role(name: &str, role: &str) -> UnboundSelection {
    UnboundSelection {
        selection_id: name.to_owned(),
        item: UnboundItem::Dimension(UnboundDimension {
            name: name.to_owned(),
            roles: vec![role.to_owned()],
        }),
        output_label: None,
    }
}

// ---- Relationships and fanout ----------------------------------------------------

#[test]
fn a_bridge_without_its_declared_allocation_double_counts() {
    // CASE-011 without the metric's allocation: an order in two campaigns
    // would count its revenue twice.
    let model = changed(11, |definition| {
        for metric in &mut definition.metrics {
            metric.allocation_bridge = None;
        }
    });
    rejects(
        &model,
        &common::request(11),
        &none(),
        Code::UnresolvedFanout,
        "/blocks/0/selections/1",
    );
}

#[test]
fn an_unproven_cardinality_never_reaches_the_engine() {
    // CASE-003 with the customer key no longer unique: CSMIR compilation
    // rejects the many-to-one contract, so no query can rely on it.
    let mut definition = model(3).definition().clone();
    for dataset in &mut definition.datasets {
        for key in &mut dataset.key_bindings {
            key.unique = false;
        }
    }
    let errors = definition.compile().expect_err("unproven contract");
    assert!(
        errors
            .iter()
            .any(|error| error.code.as_str() == "INVALID_RELATIONSHIP_CONTRACT")
    );
}

#[test]
fn a_role_the_path_does_not_use_is_rejected() {
    // CASE-002: region is reached through `customer`, never through `product`.
    rejects(
        &model(2),
        &query(vec![metric("revenue"), by_role("region", "product")]),
        &none(),
        Code::InvalidRelationshipRole,
        "/blocks/0/selections/1",
    );
}

#[test]
fn an_unreachable_dimension_is_not_a_grain_of_the_metric() {
    let model = changed(2, |definition| {
        definition
            .relationships
            .retain(|relationship| relationship.role != "customer");
    });
    rejects(
        &model,
        &query(vec![metric("revenue"), dimension("region")]),
        &none(),
        Code::InvalidGrainTransition,
        "/blocks/0/selections/1",
    );
}

#[test]
fn an_unrelated_relationship_changes_nothing() {
    // Fanout invariance (§23): dropping the product relationship, which the
    // region path never uses, leaves facts and obligations identical.
    let request = query(vec![metric("revenue"), dimension("region")]);
    let with = check(&model(2), &request, &none()).expect("valid");
    let without = check(
        &changed(2, |definition| {
            definition
                .relationships
                .retain(|relationship| relationship.role != "product");
        }),
        &request,
        &none(),
    )
    .expect("valid");
    assert_eq!(with.blocks[0].facts, without.blocks[0].facts);
    assert_eq!(with.blocks[0].obligations, without.blocks[0].obligations);
}

#[test]
fn one_fact_needs_no_alignment() {
    let text = check(
        &model(4),
        &query(vec![metric("order_revenue"), dimension("region")]),
        &none(),
    )
    .expect("valid")
    .to_string();
    assert!(!text.contains("AggregateBeforeAlignment"), "{text}");
}

// ---- Time --------------------------------------------------------------------------

#[test]
fn a_monthly_dimension_cannot_be_split_into_days() {
    rejects(
        &model(12),
        &query(vec![
            metric("revenue"),
            zoned("day", "month", TimeGrain::Day, None),
        ]),
        &none(),
        Code::GrainRefinementUnsupported,
        "/blocks/0/selections/1",
    );
}

#[test]
fn a_business_day_keeps_the_zone_it_was_derived_in() {
    rejects(
        &model(19),
        &query(vec![
            metric("revenue"),
            zoned("day", "business_day", TimeGrain::Day, Some("UTC")),
        ]),
        &none(),
        Code::InvalidGrainTransition,
        "/blocks/0/selections/1/timezone",
    );
}

#[test]
fn a_calendar_date_has_no_timezone() {
    rejects(
        &model(7),
        &query(vec![
            metric("balance"),
            zoned("month", "snapshot_date", TimeGrain::Month, Some("UTC")),
        ]),
        &none(),
        Code::InvalidGrainTransition,
        "/blocks/0/selections/1/timezone",
    );
}

#[test]
fn a_period_end_balance_over_the_whole_range_keeps_the_last_snapshot() {
    let text = check(&model(7), &query(vec![metric("balance")]), &none())
        .expect("valid")
        .to_string();
    assert!(text.contains("period-end on corpus/dimension/snapshot_date per whole range"));
}

#[test]
fn balances_grouped_by_each_month_end_are_legal() {
    // CASE-020's metric is fine when the request keeps its time axis.
    check(
        &model(20),
        &query(vec![metric("balance"), dimension("month_end")]),
        &none(),
    )
    .expect("one balance per month end");
}

#[test]
fn a_cumulative_metric_needs_its_order() {
    rejects(
        &model(21),
        &query(vec![metric("cumulative_revenue")]),
        &none(),
        Code::InvalidWindowSemantics,
        "/blocks/0/selections/0",
    );
}

// ---- Policies ----------------------------------------------------------------------

#[test]
fn a_policy_value_of_the_wrong_type_is_rejected() {
    let mut context = SemanticContext::default();
    context
        .slots
        .insert("tenant_id".to_owned(), Literal::SignedInteger(7));
    let errors = validate(&model(15), &bound(15), &context).expect_err("wrong type");
    assert!(
        errors
            .iter()
            .any(|error| error.code == Code::IncompatibleTypes
                && error.query_refs.first().map(String::as_str)
                    == Some("/context/slots/tenant_id"))
    );
}
