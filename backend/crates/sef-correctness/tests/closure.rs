//! The semantic closure of a request (review of #83): every metric it
//! depends on, every intrinsic filter, every dataset it reads and every
//! value of time it buckets is proven, not only what the caller selected.
#![allow(clippy::expect_used, clippy::panic)]

mod common;

use common::{dimension, metric, model, query};
use sef_correctness::{Obligation, SemanticContext, ValidatedQuery, validate};
use sef_model_ir::diagnostic::{Diagnostic, ErrorCode as Code};
use sef_model_ir::expressions::{MetricExpr, MetricExprKind, ScalarExpr, ScalarExprKind};
use sef_model_ir::identity::{ObjectIdentity, StableId};
use sef_model_ir::model::{
    AggregationContract, CalendarDefinition, CompiledSemanticModel, ModelDefinition, ObjectMetadata,
};
use sef_model_ir::policy::{ContextSlot, Enforcement, Policy, PolicyScope, Sensitivity};
use sef_model_ir::time::{CalendarProfile, TimeGrain};
use sef_model_ir::types::{Literal, ValueType};
use sef_query_ir::unbound::{UnboundDimension, UnboundItem, UnboundQuerySet, UnboundSelection};

fn id(text: &str) -> StableId {
    StableId::new(text).expect("stable id")
}

fn changed(case: u8, change: impl FnOnce(&mut ModelDefinition)) -> CompiledSemanticModel {
    let mut definition = model(case).definition().clone();
    change(&mut definition);
    definition
        .compile()
        .unwrap_or_else(|errors| panic!("changed CASE-{case:03} must compile: {errors:#?}"))
}

fn run(
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
    code: Code,
) -> Vec<Diagnostic> {
    let errors = run(model, request, &SemanticContext::default()).expect_err("must be rejected");
    assert!(
        errors.iter().any(|error| error.code == code),
        "expected {}, got {errors:#?}",
        code.as_str()
    );
    errors
}

fn obligations(validated: &ValidatedQuery) -> String {
    validated.to_string()
}

/// A metric that only references `original`: fully additive outside, with
/// none of the original's contracts repeated.
fn alias(definition: &mut ModelDefinition, original: &str) {
    let source = definition
        .metrics
        .iter()
        .find(|metric| metric.metadata.id().as_str() == original)
        .expect("original metric")
        .clone();
    let mut alias = source.clone();
    alias.metadata.identity.stable_id = id("corpus/metric/alias");
    alias.metadata.label = Some("alias".to_owned());
    alias.expression = MetricExpr {
        kind: MetricExprKind::MetricRef(source.metadata.id().clone()),
        value_type: source.value_type.clone(),
        unit: source.unit.clone(),
    };
    alias.dependencies = vec![source.metadata.id().clone()];
    alias.aggregation_contract = AggregationContract::fully_additive();
    alias.time_behavior = None;
    alias.window_behavior = None;
    alias.policy_refs = Vec::new();
    alias.allocation_bridge = None;
    alias.intrinsic_filter = None;
    definition.metrics.push(alias);
}

fn tenant_slot() -> ContextSlot {
    ContextSlot {
        name: "tenant_id".to_owned(),
        value_type: ValueType::Utf8,
        required: true,
        sensitivity: Sensitivity::Sensitive,
    }
}

fn typed(kind: ScalarExprKind, value_type: ValueType) -> ScalarExpr {
    ScalarExpr {
        kind,
        value_type,
        unit: None,
    }
}

/// A mandatory policy on `dataset` that needs `:tenant_id` (its predicate
/// only checks the value is bound, enough to prove attachment).
fn protect(definition: &mut ModelDefinition, dataset: &str, policy: &str) {
    let version = definition.metadata.identity.semantic_version.clone();
    definition
        .datasets
        .iter_mut()
        .find(|candidate| candidate.metadata.id().as_str() == dataset)
        .expect("dataset")
        .policy_refs
        .push(id(policy));
    definition.policies.push(Policy {
        metadata: ObjectMetadata::new(ObjectIdentity {
            stable_id: id(policy),
            semantic_version: version,
        }),
        scope: PolicyScope::Dataset(id(dataset)),
        enforcement: Enforcement::Mandatory,
        predicate: typed(
            ScalarExprKind::NullTest {
                operand: Box::new(typed(
                    ScalarExprKind::ContextSlot("tenant_id".to_owned()),
                    ValueType::Utf8,
                )),
                negated: true,
            },
            ValueType::Boolean,
        ),
        context_slots: vec![tenant_slot()],
    });
}

fn tenant() -> SemanticContext {
    let mut context = SemanticContext::default();
    context
        .slots
        .insert("tenant_id".to_owned(), Literal::Utf8("acme".to_owned()));
    context
}

// ---- 1. Policy lineage ---------------------------------------------------------

#[test]
fn an_intrinsic_filter_reads_and_protects_its_datasets() {
    // CASE-005: `revenue` keeps only sales whose region is known, through
    // sales → store → region; a policy on `region` must then apply even
    // when the caller selects `revenue` alone.
    let model = changed(5, |definition| {
        protect(
            definition,
            "corpus/dataset/region",
            "corpus/policy/region_tenant",
        );
        let revenue = definition
            .metrics
            .iter_mut()
            .find(|metric| metric.metadata.id().as_str() == "corpus/metric/revenue")
            .expect("revenue");
        revenue.intrinsic_filter = Some(typed(
            ScalarExprKind::NullTest {
                operand: Box::new(typed(
                    ScalarExprKind::DimensionRef(id("corpus/dimension/region")),
                    ValueType::Utf8,
                )),
                negated: true,
            },
            ValueType::Boolean,
        ));
    });
    let request = query(vec![metric("revenue")]);
    rejects(&model, &request, Code::PolicyContextMissing);
    let validated = run(&model, &request, &tenant()).expect("tenant bound");
    assert!(
        obligations(&validated)
            .contains("ApplyMandatoryPolicy corpus/policy/region_tenant on corpus/dataset/region")
    );
}

#[test]
fn a_policy_on_an_intermediate_dataset_of_the_path_applies() {
    // Positive control: `store` is crossed to reach the region.
    let model = changed(5, |definition| {
        protect(
            definition,
            "corpus/dataset/store",
            "corpus/policy/store_tenant",
        );
    });
    let request = query(vec![metric("revenue"), dimension("region")]);
    rejects(&model, &request, Code::PolicyContextMissing);
    let validated = run(&model, &request, &tenant()).expect("tenant bound");
    assert!(
        obligations(&validated)
            .contains("ApplyMandatoryPolicy corpus/policy/store_tenant on corpus/dataset/store")
    );
}

#[test]
fn a_metric_policy_protects_every_metric_built_on_it() {
    // CASE-015 with the tenant policy scoped to `revenue`, queried through
    // an alias that only references it.
    let model = changed(15, |definition| {
        for dataset in &mut definition.datasets {
            dataset.policy_refs.clear();
        }
        for policy in &mut definition.policies {
            policy.scope = PolicyScope::Metric(id("corpus/metric/revenue"));
        }
        let policies: Vec<StableId> = definition
            .policies
            .iter()
            .map(|policy| policy.metadata.id().clone())
            .collect();
        definition
            .metrics
            .iter_mut()
            .find(|metric| metric.metadata.id().as_str() == "corpus/metric/revenue")
            .expect("revenue")
            .policy_refs = policies;
        alias(definition, "corpus/metric/revenue");
    });
    rejects(
        &model,
        &query(vec![metric("alias")]),
        Code::PolicyContextMissing,
    );
}

// ---- 2. Contracts of dependencies ---------------------------------------------

#[test]
fn an_alias_cannot_sum_balances_across_months() {
    let model = changed(20, |definition| alias(definition, "corpus/metric/balance"));
    rejects(
        &model,
        &query(vec![metric("alias")]),
        Code::IllegalTemporalReaggregation,
    );
}

#[test]
fn an_alias_keeps_the_period_end_selection_of_its_source() {
    let model = changed(7, |definition| alias(definition, "corpus/metric/balance"));
    let validated = run(
        &model,
        &query(vec![metric("alias")]),
        &SemanticContext::default(),
    )
    .expect("valid over the whole range");
    assert!(obligations(&validated).contains("ApplyPeriodEndSelection corpus/metric/balance"));
}

#[test]
fn an_alias_of_a_cumulative_metric_needs_its_order() {
    let model = changed(21, |definition| {
        alias(definition, "corpus/metric/cumulative_revenue");
    });
    rejects(
        &model,
        &query(vec![metric("alias")]),
        Code::InvalidWindowSemantics,
    );
    let validated = run(
        &model,
        &query(vec![metric("alias"), dimension("date")]),
        &SemanticContext::default(),
    )
    .expect("ordered by date");
    assert!(
        obligations(&validated).contains("ApplyOrderedCumulative corpus/metric/cumulative_revenue")
    );
}

#[test]
fn an_allocation_declared_by_a_dependency_covers_its_users() {
    // CASE-011: the alias does not repeat the bridge; its dependency does.
    let model = changed(11, |definition| {
        alias(definition, "corpus/metric/allocated_revenue");
    });
    let validated = run(
        &model,
        &query(vec![metric("alias"), dimension("campaign")]),
        &SemanticContext::default(),
    )
    .expect("allocated through the dependency");
    assert!(obligations(&validated).contains(
        "ApplyBridgeAllocation corpus/metric/allocated_revenue through corpus/bridge/order_campaign"
    ));
}

// ---- 3–4, 7. Time domains -------------------------------------------------------

fn bucket(
    name: &str,
    grain: TimeGrain,
    zone: Option<&str>,
    calendar: Option<&str>,
) -> UnboundSelection {
    UnboundSelection {
        selection_id: format!("{name}_bucket"),
        item: UnboundItem::TimeDimension {
            dimension: UnboundDimension {
                name: name.to_owned(),
                roles: Vec::new(),
            },
            grain,
            timezone: zone.map(str::to_owned),
            calendar: calendar.map(str::to_owned),
        },
        output_label: None,
    }
}

#[test]
fn monthly_targets_cannot_be_split_into_days() {
    // CASE-010: `month` is a monthly domain (invoices derive it by month),
    // so targets observed per month have no daily values.
    let errors = rejects(
        &model(10),
        &query(vec![
            metric("target_revenue"),
            bucket("month", TimeGrain::Day, None, None),
            dimension("region"),
        ]),
        Code::GrainRefinementUnsupported,
    );
    assert!(errors[0].query_refs[0].starts_with("/blocks/0/selections/1"));
}

#[test]
fn a_gregorian_month_is_not_a_fiscal_month() {
    let model = changed(12, |definition| {
        let version = definition.metadata.identity.semantic_version.clone();
        definition.calendars.push(CalendarDefinition {
            metadata: ObjectMetadata::new(ObjectIdentity {
                stable_id: id("corpus/calendar/fiscal"),
                semantic_version: version,
            }),
            profile: CalendarProfile::Fiscal {
                id: id("corpus/calendar/fiscal"),
            },
        });
    });
    let errors = rejects(
        &model,
        &query(vec![
            metric("revenue"),
            bucket(
                "month",
                TimeGrain::Month,
                Some("UTC"),
                Some("corpus/calendar/fiscal"),
            ),
        ]),
        Code::InvalidGrainTransition,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.query_refs[0] == "/blocks/0/selections/1/calendar")
    );
}

#[test]
fn a_date_bucketed_by_day_is_the_date_itself() {
    // CASE-021: grouping `date` by day groups exactly as `date` does.
    let validated = run(
        &model(21),
        &query(vec![
            metric("cumulative_revenue"),
            bucket("date", TimeGrain::Day, None, None),
        ]),
        &SemanticContext::default(),
    )
    .expect("same grain as the date");
    assert!(obligations(&validated).contains("ApplyOrderedCumulative"));
}

// ---- 6. Context values -----------------------------------------------------------

#[test]
fn an_optional_context_value_must_still_have_its_type() {
    let model = changed(15, |definition| {
        for policy in &mut definition.policies {
            for slot in &mut policy.context_slots {
                slot.required = false;
            }
        }
    });
    let mut context = SemanticContext::default();
    context
        .slots
        .insert("tenant_id".to_owned(), Literal::SignedInteger(7));
    let errors = run(&model, &common::request(15), &context).expect_err("wrong type");
    assert!(
        errors
            .iter()
            .any(|error| error.code == Code::IncompatibleTypes)
    );
}

#[test]
fn an_integer_context_value_must_fit_its_width() {
    // A slot declared as an 8-bit integer accepts 100 and refuses 1000.
    let model = changed(15, |definition| {
        for policy in &mut definition.policies {
            let slot_type = ValueType::Integer {
                signed: true,
                bits: Some(8),
            };
            policy.context_slots = vec![ContextSlot {
                value_type: slot_type.clone(),
                ..tenant_slot()
            }];
            policy.predicate = typed(
                ScalarExprKind::NullTest {
                    operand: Box::new(typed(
                        ScalarExprKind::ContextSlot("tenant_id".to_owned()),
                        slot_type,
                    )),
                    negated: true,
                },
                ValueType::Boolean,
            );
        }
    });
    let with = |value: i128| {
        let mut context = SemanticContext::default();
        context
            .slots
            .insert("tenant_id".to_owned(), Literal::SignedInteger(value));
        run(&model, &common::request(15), &context)
    };
    with(100).expect("fits in 8 bits");
    let errors = with(1000).expect_err("does not fit");
    assert!(
        errors
            .iter()
            .any(|error| error.code == Code::IncompatibleTypes)
    );
    assert!(with(-129).is_err() && with(-128).is_ok());
}

#[test]
fn exactly_the_obligations_of_the_original_requests_stay() {
    // Sanity: the closure adds the components' contracts, never drops one.
    let validated =
        run(&model(6), &common::request(6), &SemanticContext::default()).expect("valid");
    let text = obligations(&validated);
    assert!(text.contains("RecomputeAtTargetGrain corpus/metric/average_order_value"));
    assert!(
        validated.blocks[0]
            .obligations
            .iter()
            .any(|obligation| matches!(
                obligation,
                Obligation::PreserveExactDecimalSemantics { .. }
            ))
    );
}
