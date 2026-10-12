//! The planner and its independent certificate checker beyond the corpus
//! (review of #86): plan identity covers the meaning of every model object,
//! branches keep each metric's own semantics, derived metrics wait for what
//! they read, reads carry every field they are used for, and the checker
//! rejects plans that break an invariant even when the builder is bypassed.
#![allow(clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeMap;

use common::{dimension, metric, model, query};
use sef_correctness::{Obligation, SemanticContext, ValidatedQuery, validate};
use sef_exec_ir::{NodeId, ObjectRef, Operator, PlanBlock, SeirGraph, SemanticPlan};
use sef_model_ir::diagnostic::{Diagnostic, ErrorCode};
use sef_model_ir::expressions::{MetricExpr, MetricExprKind, ScalarExpr, ScalarExprKind, UnaryOp};
use sef_model_ir::identity::StableId;
use sef_model_ir::model::{
    AggregationClass, AggregationContract, CompiledSemanticModel, Decomposition, ModelDefinition,
};
use sef_model_ir::time::IanaZoneId;
use sef_model_ir::types::Literal;
use sef_planner::{PlannedQuery, certify, plan};
use sef_query_ir::unbound::UnboundQuerySet;

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

fn tenant(value: &str) -> SemanticContext {
    let mut context = SemanticContext::default();
    context
        .slots
        .insert("tenant_id".to_owned(), Literal::Utf8(value.to_owned()));
    context
}

fn validated(
    model: &CompiledSemanticModel,
    request: &UnboundQuerySet,
    context: &SemanticContext,
) -> ValidatedQuery {
    let query = sef_query_ir::bind(request, model.definition())
        .unwrap_or_else(|errors| panic!("must bind: {errors:#?}"));
    validate(model, &query, context).unwrap_or_else(|errors| panic!("must validate: {errors:#?}"))
}

fn planned(model: &CompiledSemanticModel, validated: &ValidatedQuery) -> PlannedQuery {
    plan(model, validated).unwrap_or_else(|errors| panic!("must plan: {errors:#?}"))
}

fn case(
    case: u8,
    context: &SemanticContext,
) -> (CompiledSemanticModel, ValidatedQuery, PlannedQuery) {
    let model = model(case);
    let validated = validated(&model, &common::request(case), context);
    let planned = planned(&model, &validated);
    (model, validated, planned)
}

fn object(model: &CompiledSemanticModel, text: &str) -> ObjectRef {
    let id = id(text);
    let content = *model.object_hash(&id).expect("model object");
    ObjectRef { id, content }
}

/// The plan's nodes, inputs first.
fn nodes(plan: &SemanticPlan) -> Vec<(NodeId, Operator)> {
    let roots: Vec<NodeId> = plan.blocks.iter().map(|block| block.root).collect();
    plan.graph
        .topological(&roots)
        .into_iter()
        .map(|node| (node.id, node.operator))
        .collect()
}

/// The row pipeline under a node, nearest first, along main inputs.
fn chain(plan: &SemanticPlan, id: NodeId) -> Vec<Operator> {
    let mut found = Vec::new();
    let mut next = plan
        .graph
        .operator(&id)
        .and_then(|op| op.inputs().first().copied());
    while let Some(current) = next {
        let operator = plan.graph.operator(&current).expect("node").clone();
        next = operator.inputs().first().copied();
        found.push(operator);
    }
    found
}

fn serves(operator: &Operator) -> Vec<&str> {
    match operator {
        Operator::AggregateAtGrain { serves, .. } | Operator::DistinctAggregate { serves, .. } => {
            serves.iter().map(|served| served.id.as_str()).collect()
        }
        _ => Vec::new(),
    }
}

fn reads(plan: &SemanticPlan, dataset: &str) -> Vec<Vec<String>> {
    nodes(plan)
        .into_iter()
        .filter_map(|(_, operator)| match operator {
            Operator::ReadSemanticDataset {
                dataset: read,
                fields,
            } if read.id.as_str() == dataset => {
                Some(fields.iter().map(ToString::to_string).collect())
            }
            _ => None,
        })
        .collect()
}

fn rejected(
    model: &CompiledSemanticModel,
    validated: &ValidatedQuery,
    plan: &SemanticPlan,
    code: ErrorCode,
    needle: &str,
) -> Vec<Diagnostic> {
    let errors = certify(model, validated, plan).expect_err("the checker must reject this plan");
    assert!(
        errors
            .iter()
            .any(|error| error.code == code && error.message.contains(needle)),
        "expected {} mentioning {needle:?}, got {errors:#?}",
        code.as_str()
    );
    errors
}

// ---- Rewriting a finished plan -----------------------------------------------------

enum Edit {
    Keep,
    Replace(Operator),
    /// Readers read this node's main input instead.
    Bypass,
}

fn remap(operator: &mut Operator, map: &BTreeMap<NodeId, NodeId>) {
    let to = |id: &mut NodeId| *id = map[id];
    match operator {
        Operator::ReadSemanticDataset { .. } => {}
        Operator::AlignSemanticBranches { inputs, .. } => inputs.iter_mut().for_each(to),
        Operator::AttachDimension { input, lookups, .. } => {
            to(input);
            lookups.iter_mut().for_each(to);
        }
        Operator::PolicyGuard { input, .. }
        | Operator::ContractGuard { input, .. }
        | Operator::ApplyRowFilter { input, .. }
        | Operator::ApplyIntrinsicFilter { input, .. }
        | Operator::TemporalAsOf { input, .. }
        | Operator::BridgeAllocate { input, .. }
        | Operator::NormalizeTime { input, .. }
        | Operator::TimeBucket { input, .. }
        | Operator::SelectPeriodEnd { input, .. }
        | Operator::AggregateAtGrain { input, .. }
        | Operator::DistinctAggregate { input, .. }
        | Operator::RecomposeMetric { input, .. }
        | Operator::ComputeWindowMetric { input, .. }
        | Operator::ApplyPostAggregateFilter { input, .. }
        | Operator::ProjectSemanticOutput { input, .. }
        | Operator::OrderSemanticOutput { input, .. }
        | Operator::LimitSemanticOutput { input, .. } => to(input),
    }
}

/// The same plan with some operators edited, rebuilt bottom-up.
fn rewrite(plan: &SemanticPlan, edit: impl Fn(&Operator) -> Edit) -> SemanticPlan {
    let mut graph = SeirGraph::new();
    let mut map: BTreeMap<NodeId, NodeId> = BTreeMap::new();
    for (old, original) in nodes(plan) {
        let mut operator = match edit(&original) {
            Edit::Keep => original,
            Edit::Replace(replacement) => replacement,
            Edit::Bypass => {
                let input = original.inputs()[0];
                map.insert(old, map[&input]);
                continue;
            }
        };
        remap(&mut operator, &map);
        map.insert(old, graph.add(operator).expect("rebuilt node"));
    }
    SemanticPlan {
        graph,
        blocks: plan
            .blocks
            .iter()
            .map(|block| PlanBlock {
                block_id: block.block_id.clone(),
                root: map[&block.root],
            })
            .collect(),
    }
}

// ---- 1. Identity covers the meaning of model objects ---------------------------------

#[test]
fn a_changed_definition_changes_the_plan_and_a_description_does_not() {
    let hash = |model: &CompiledSemanticModel, context: &SemanticContext, case: u8| {
        let validated = validated(model, &common::request(case), context);
        planned(model, &validated).certificate.seir_hash
    };
    let none = SemanticContext::default();
    let base = hash(&model(1), &none, 1);
    // SUM(sales.amount) → SUM(-sales.amount): same IDs, opposite values.
    let negated = changed(1, |definition| {
        let measure = definition
            .measures
            .iter_mut()
            .find(|m| m.metadata.id().as_str() == "corpus/measure/revenue")
            .expect("revenue measure");
        let operand = measure.scalar_expression.clone().expect("scalar");
        measure.scalar_expression = Some(ScalarExpr {
            value_type: operand.value_type.clone(),
            unit: operand.unit.clone(),
            kind: ScalarExprKind::Unary {
                op: UnaryOp::Negate,
                operand: Box::new(operand),
            },
        });
    });
    assert_ne!(base, hash(&negated, &none, 1), "an expression is meaning");
    let described = changed(1, |definition| {
        for metric in &mut definition.metrics {
            metric.metadata.label = Some("Ingresos".to_owned());
            metric.metadata.description = Some("Texto para personas".to_owned());
        }
    });
    assert_eq!(base, hash(&described, &none, 1), "a description is not");

    // A policy predicate is meaning; the caller's context value is not.
    let protected = hash(&model(15), &tenant("secret-a"), 15);
    assert_eq!(protected, hash(&model(15), &tenant("secret-b"), 15));
    let stricter = changed(15, |definition| {
        let policy = &mut definition.policies[0];
        policy.predicate = ScalarExpr {
            value_type: policy.predicate.value_type.clone(),
            unit: None,
            kind: ScalarExprKind::Unary {
                op: UnaryOp::Not,
                operand: Box::new(policy.predicate.clone()),
            },
        };
    });
    assert_ne!(protected, hash(&stricter, &tenant("secret-a"), 15));

    // A data contract is meaning: the allocation tolerance of CASE-011.
    let allocated = hash(&model(11), &none, 11);
    let tolerant = changed(11, |definition| {
        let allocation = definition.bridges[0]
            .allocation
            .as_mut()
            .expect("allocation");
        allocation.tolerance = allocation.required_total.clone();
    });
    assert_ne!(allocated, hash(&tolerant, &none, 11));
}

#[test]
fn context_values_never_reach_the_plan_or_its_explanation() {
    let (_, _, planned) = case(15, &tenant("secret-a"));
    let text = format!("{}{}", planned.plan.explain(), planned.certificate);
    assert!(!text.contains("secret-a"), "{text}");
    assert!(
        text.contains("PolicyGuard corpus/policy/tenant with :tenant_id"),
        "{text}"
    );
}

#[test]
fn a_plan_for_another_definition_is_not_certified() {
    let (_, _, original) = case(1, &SemanticContext::default());
    let negated = changed(1, |definition| {
        let measure = &mut definition.measures[0];
        let operand = measure.scalar_expression.clone().expect("scalar");
        measure.scalar_expression = Some(ScalarExpr {
            value_type: operand.value_type.clone(),
            unit: operand.unit.clone(),
            kind: ScalarExprKind::Unary {
                op: UnaryOp::Negate,
                operand: Box::new(operand),
            },
        });
    });
    let validated = validated(&negated, &common::request(1), &SemanticContext::default());
    rejected(
        &negated,
        &validated,
        &original.plan,
        ErrorCode::PlannerInternalInvariantViolation,
        "no coincide con su definición",
    );
}

// ---- 2. Branches keep each metric's semantics -------------------------------------

/// CASE-007 with `all_balances`: the same measure, summed over every
/// snapshot instead of the last one of each month.
fn mixed_balances() -> (CompiledSemanticModel, ValidatedQuery) {
    let model = changed(7, |definition| {
        let mut all = definition
            .metrics
            .iter()
            .find(|m| m.metadata.id().as_str() == "corpus/metric/balance")
            .expect("balance")
            .clone();
        all.metadata.identity.stable_id = id("corpus/metric/all_balances");
        all.metadata.label = Some("all_balances".to_owned());
        all.aggregation_contract = AggregationContract::fully_additive();
        all.time_behavior = None;
        definition.metrics.push(all);
    });
    let mut request = common::request(7);
    request.blocks[0].selections.push(metric("all_balances"));
    let validated = validated(&model, &request, &SemanticContext::default());
    (model, validated)
}

#[test]
fn a_period_end_metric_and_a_plain_sum_of_the_same_measure_do_not_share_rows() {
    let (model, validated) = mixed_balances();
    let planned = planned(&model, &validated);
    let aggregates: Vec<(NodeId, Operator)> = nodes(&planned.plan)
        .into_iter()
        .filter(|(_, op)| matches!(op, Operator::AggregateAtGrain { .. }))
        .collect();
    assert_eq!(aggregates.len(), 2, "{}", planned.plan.explain());
    for (node, operator) in &aggregates {
        let selected = chain(&planned.plan, *node)
            .iter()
            .any(|op| matches!(op, Operator::SelectPeriodEnd { .. }));
        match serves(operator).as_slice() {
            ["corpus/metric/balance"] => assert!(selected, "balance keeps the last snapshot"),
            ["corpus/metric/all_balances"] => assert!(!selected, "all_balances sums every one"),
            other => panic!("unexpected branch {other:?}"),
        }
    }
    assert!(planned.plan.explain().contains("AlignSemanticBranches"));
}

#[test]
fn a_period_end_selection_feeding_another_metric_is_rejected() {
    let (model, validated) = mixed_balances();
    let planned = planned(&model, &validated);
    let all = object(&model, "corpus/metric/all_balances");
    let shared = rewrite(&planned.plan, |operator| match operator {
        Operator::AggregateAtGrain {
            input,
            grain,
            measures,
            serves,
        } if serves
            .iter()
            .any(|s| s.id.as_str() == "corpus/metric/balance") =>
        {
            let mut serves = serves.clone();
            serves.push(all.clone());
            Edit::Replace(Operator::AggregateAtGrain {
                input: *input,
                grain: grain.clone(),
                measures: measures.clone(),
                serves,
            })
        }
        _ => Edit::Keep,
    });
    rejected(
        &model,
        &validated,
        &shared,
        ErrorCode::PlannerInternalInvariantViolation,
        "fin de periodo",
    );
}

#[test]
fn an_alias_of_a_balance_reads_its_selected_snapshots() {
    let model = changed(7, |definition| {
        let balance = definition
            .metrics
            .iter()
            .find(|m| m.metadata.id().as_str() == "corpus/metric/balance")
            .expect("balance")
            .clone();
        let mut alias = balance.clone();
        alias.metadata.identity.stable_id = id("corpus/metric/alias");
        alias.metadata.label = Some("alias".to_owned());
        alias.expression = MetricExpr {
            kind: MetricExprKind::MetricRef(balance.metadata.id().clone()),
            value_type: balance.value_type.clone(),
            unit: balance.unit.clone(),
        };
        alias.dependencies = vec![balance.metadata.id().clone()];
        alias.aggregation_contract = AggregationContract::fully_additive();
        alias.time_behavior = None;
        definition.metrics.push(alias);
    });
    let validated = validated(
        &model,
        &query(vec![metric("alias")]),
        &SemanticContext::default(),
    );
    let planned = planned(&model, &validated);
    let text = planned.plan.explain();
    let selection = text
        .find("SelectPeriodEnd corpus/metric/balance")
        .expect(&text);
    let alias = text
        .find("RecomposeMetric corpus/metric/alias")
        .expect(&text);
    assert!(selection < alias, "{text}");
}

// ---- 3. Derived metrics wait for what they read --------------------------------------

/// CASE-021 with `wrapper`, recomposed from the cumulative metric.
fn wrapped_window() -> (CompiledSemanticModel, ValidatedQuery) {
    let model = changed(21, |definition| {
        let cumulative = definition
            .metrics
            .iter()
            .find(|m| m.metadata.id().as_str() == "corpus/metric/cumulative_revenue")
            .expect("cumulative")
            .clone();
        let reference = MetricExpr {
            kind: MetricExprKind::MetricRef(cumulative.metadata.id().clone()),
            value_type: cumulative.value_type.clone(),
            unit: cumulative.unit.clone(),
        };
        let mut wrapper = cumulative.clone();
        wrapper.metadata.identity.stable_id = id("corpus/metric/wrapper");
        wrapper.metadata.label = Some("wrapper".to_owned());
        wrapper.expression = reference.clone();
        wrapper.dependencies = vec![cumulative.metadata.id().clone()];
        wrapper.aggregation_contract = AggregationContract {
            classification: AggregationClass::Recomputable,
            axis_rules: Vec::new(),
            decomposition: Some(Decomposition {
                components: vec![cumulative.metadata.id().clone()],
                recomposition: reference,
            }),
        };
        wrapper.window_behavior = None;
        definition.metrics.push(wrapper);
    });
    let mut request = common::request(21);
    request.blocks[0].selections.push(metric("wrapper"));
    let validated = validated(&model, &request, &SemanticContext::default());
    (model, validated)
}

#[test]
fn a_recomposition_over_a_window_runs_after_the_window() {
    let (model, validated) = wrapped_window();
    let planned = planned(&model, &validated);
    let text = planned.plan.explain();
    let window = text
        .find("ComputeWindowMetric corpus/metric/cumulative_revenue")
        .expect(&text);
    let wrapper = text
        .find("RecomposeMetric corpus/metric/wrapper")
        .expect(&text);
    assert!(window < wrapper, "{text}");
}

#[test]
fn a_recomposition_before_its_window_is_rejected() {
    let (model, validated) = wrapped_window();
    let planned = planned(&model, &validated);
    let mut plan = planned.plan.clone();
    let (aggregate, _) = nodes(&plan)
        .into_iter()
        .find(|(_, op)| matches!(op, Operator::AggregateAtGrain { .. }))
        .expect("aggregate");
    let (window, project) =
        nodes(&plan)
            .into_iter()
            .fold((None, None), |found, (_, op)| match op {
                Operator::ComputeWindowMetric { .. } => (Some(op), found.1),
                Operator::ProjectSemanticOutput { .. } => (found.0, Some(op)),
                _ => found,
            });
    let Some(Operator::ComputeWindowMetric {
        metric,
        base,
        order_by,
        partition_by,
        ..
    }) = window
    else {
        panic!("window")
    };
    let Some(Operator::ProjectSemanticOutput {
        selections,
        definitions,
        ..
    }) = project
    else {
        panic!("projection")
    };
    let early = plan
        .graph
        .add(Operator::RecomposeMetric {
            input: aggregate,
            metric: object(&model, "corpus/metric/wrapper"),
        })
        .expect("node");
    let late = plan
        .graph
        .add(Operator::ComputeWindowMetric {
            input: early,
            metric,
            base,
            order_by,
            partition_by,
        })
        .expect("node");
    plan.blocks[0].root = plan
        .graph
        .add(Operator::ProjectSemanticOutput {
            input: late,
            selections,
            definitions,
        })
        .expect("node");
    rejected(
        &model,
        &validated,
        &plan,
        ErrorCode::PlannerInternalInvariantViolation,
        "se calcula antes de corpus/metric/cumulative_revenue",
    );
}

// ---- 4. The checker verifies, it does not trust ---------------------------------------

#[test]
fn a_distinct_count_aggregated_again_through_a_projection_is_rejected() {
    // CASE-008 overall: Read → DistinctAggregate → Project → AggregateAtGrain.
    let (model, validated, planned) = case(8, &SemanticContext::default());
    let mut plan = planned.plan.clone();
    let index = plan
        .blocks
        .iter()
        .position(|block| block.block_id == "overall")
        .expect("overall block");
    let projection = plan.blocks[index].root;
    let Some(Operator::ProjectSemanticOutput {
        selections,
        definitions,
        ..
    }) = plan.graph.operator(&projection).cloned()
    else {
        panic!("projection")
    };
    let again = plan
        .graph
        .add(Operator::AggregateAtGrain {
            input: projection,
            grain: Vec::new(),
            measures: vec![object(&model, "corpus/measure/active_customers")],
            serves: vec![object(&model, "corpus/metric/active_customers")],
        })
        .expect("node");
    plan.blocks[index].root = plan
        .graph
        .add(Operator::ProjectSemanticOutput {
            input: again,
            selections,
            definitions,
        })
        .expect("node");
    rejected(
        &model,
        &validated,
        &plan,
        ErrorCode::PlannerInternalInvariantViolation,
        "AggregateAtGrain lee ProjectSemanticOutput",
    );
}

#[test]
fn a_bucket_before_its_normalization_is_rejected() {
    // CASE-012: TimeBucket → NormalizeTime instead of the reverse.
    let (model, validated, planned) = case(12, &SemanticContext::default());
    let mut reversed = planned.plan.clone();
    let (attach, _) = nodes(&reversed)
        .into_iter()
        .find(|(_, op)| matches!(op, Operator::AttachDimension { .. }))
        .expect("attachment");
    let mut ops = nodes(&planned.plan).into_iter().map(|(_, op)| op);
    let normalize = ops
        .find(|op| matches!(op, Operator::NormalizeTime { .. }))
        .expect("normalize");
    let bucket = ops
        .find(|op| matches!(op, Operator::TimeBucket { .. }))
        .expect("bucket");
    let aggregate = ops
        .find(|op| matches!(op, Operator::AggregateAtGrain { .. }))
        .expect("aggregate");
    let project = ops
        .find(|op| matches!(op, Operator::ProjectSemanticOutput { .. }))
        .expect("project");
    let mut input = attach;
    for mut operator in [bucket, normalize, aggregate, project] {
        let map = BTreeMap::from([(operator.inputs()[0], input)]);
        remap(&mut operator, &map);
        input = reversed.graph.add(operator).expect("node");
    }
    reversed.blocks[0].root = input;
    rejected(
        &model,
        &validated,
        &reversed,
        ErrorCode::PlannerInternalInvariantViolation,
        "sin haberse convertido antes a su zona",
    );
}

#[test]
fn a_normalization_to_another_zone_does_not_discharge_the_conversion() {
    let (model, validated, planned) = case(12, &SemanticContext::default());
    let elsewhere = rewrite(&planned.plan, |operator| match operator {
        Operator::NormalizeTime {
            input,
            dimension,
            calendar,
            ..
        } => Edit::Replace(Operator::NormalizeTime {
            input: *input,
            dimension: dimension.clone(),
            timezone: IanaZoneId::new("America/La_Paz").expect("zone"),
            calendar: calendar.clone(),
        }),
        _ => Edit::Keep,
    });
    rejected(
        &model,
        &validated,
        &elsewhere,
        ErrorCode::PlannerObligationUnsatisfied,
        "ConvertTimezoneBeforeBucketing",
    );
}

#[test]
fn a_read_that_escapes_its_policy_is_rejected() {
    let (model, validated, planned) = case(15, &tenant("acme"));
    let unguarded = rewrite(&planned.plan, |operator| match operator {
        Operator::PolicyGuard { .. } => Edit::Bypass,
        _ => Edit::Keep,
    });
    rejected(
        &model,
        &validated,
        &unguarded,
        ErrorCode::PlannerObligationUnsatisfied,
        "corpus/policy/tenant",
    );
}

#[test]
fn a_mandatory_policy_without_its_obligation_is_rejected() {
    // Whatever the validation says, a read of a protected dataset must be
    // covered by its policy's obligation.
    let (model, mut validated, planned) = case(15, &tenant("acme"));
    validated.blocks[0]
        .obligations
        .retain(|obligation| !matches!(obligation, Obligation::ApplyMandatoryPolicy { .. }));
    rejected(
        &model,
        &validated,
        &planned.plan,
        ErrorCode::PlannerInternalInvariantViolation,
        "sin exigir su política obligatoria",
    );
}

#[test]
fn a_path_other_than_the_proved_one_is_rejected() {
    // CASE-005: sales → store → region, attached as if it were one hop.
    let (model, validated, planned) = case(5, &SemanticContext::default());
    let shortcut = rewrite(&planned.plan, |operator| match operator {
        Operator::AttachDimension {
            input,
            lookups,
            dimension,
            definition,
            path,
        } => Edit::Replace(Operator::AttachDimension {
            input: *input,
            lookups: lookups[1..].to_vec(),
            dimension: dimension.clone(),
            definition: definition.clone(),
            path: path[1..].to_vec(),
        }),
        _ => Edit::Keep,
    });
    rejected(
        &model,
        &validated,
        &shortcut,
        ErrorCode::PlannerInternalInvariantViolation,
        "camino distinto del probado",
    );
}

#[test]
fn an_obligation_without_evidence_is_never_certified() {
    let (model, mut validated, planned) = case(1, &SemanticContext::default());
    validated.blocks[0]
        .obligations
        .insert(Obligation::UseTemporalRelationship {
            relationship: id("corpus/relationship/none"),
        });
    rejected(
        &model,
        &validated,
        &planned.plan,
        ErrorCode::PlannerObligationUnsatisfied,
        "UseTemporalRelationship",
    );
}

// ---- 5. Reads carry every field they are used for ---------------------------------------

#[test]
fn every_read_provides_what_its_rows_are_used_for() {
    let none = SemanticContext::default();
    let has =
        |reads: &[Vec<String>], field: &str| reads.iter().any(|r| r.iter().any(|f| f == field));
    // CASE-015: the policy reads the tenant; the result groups by region.
    let (_, _, planned) = case(15, &tenant("acme"));
    let sales = reads(&planned.plan, "corpus/dataset/sales");
    assert!(has(&sales, "corpus/field/sales.tenant_id"), "{sales:?}");
    assert!(has(&sales, "corpus/field/sales.region"), "{sales:?}");
    // CASE-007: snapshots are selected per account and ordered by date.
    let (_, _, planned) = case(7, &none);
    let balances = reads(&planned.plan, "corpus/dataset/balances");
    assert!(
        has(&balances, "corpus/field/balances.account_id"),
        "{balances:?}"
    );
    assert!(
        has(&balances, "corpus/field/balances.snapshot_date"),
        "{balances:?}"
    );
    // CASE-013: the as-of join needs the event time and the validity.
    let (_, _, planned) = case(13, &none);
    assert!(has(
        &reads(&planned.plan, "corpus/dataset/sales"),
        "corpus/field/sales.sale_date"
    ));
    let history = reads(&planned.plan, "corpus/dataset/customer_history");
    assert!(
        has(&history, "corpus/field/customer_history.valid_from"),
        "{history:?}"
    );
    assert!(
        has(&history, "corpus/field/customer_history.valid_to"),
        "{history:?}"
    );
    // CASE-011: allocation weights come from the bridge.
    let (_, _, planned) = case(11, &none);
    let bridge = reads(&planned.plan, "corpus/dataset/order_campaign");
    assert!(
        has(&bridge, "corpus/field/order_campaign.allocation_weight"),
        "{bridge:?}"
    );
}

#[test]
fn a_read_missing_a_needed_field_is_rejected() {
    for (case_id, dataset, field, context) in [
        (
            15,
            "corpus/dataset/sales",
            "corpus/field/sales.tenant_id",
            tenant("acme"),
        ),
        (
            13,
            "corpus/dataset/sales",
            "corpus/field/sales.sale_date",
            SemanticContext::default(),
        ),
        (
            11,
            "corpus/dataset/order_campaign",
            "corpus/field/order_campaign.allocation_weight",
            SemanticContext::default(),
        ),
    ] {
        let (model, validated, planned) = case(case_id, &context);
        let narrowed = rewrite(&planned.plan, |operator| match operator {
            Operator::ReadSemanticDataset {
                dataset: read,
                fields,
            } if read.id.as_str() == dataset => Edit::Replace(Operator::ReadSemanticDataset {
                dataset: read.clone(),
                fields: fields
                    .iter()
                    .filter(|f| f.as_str() != field)
                    .cloned()
                    .collect(),
            }),
            _ => Edit::Keep,
        });
        rejected(
            &model,
            &validated,
            &narrowed,
            ErrorCode::PlannerInternalInvariantViolation,
            &format!("omite {field}"),
        );
    }
}

// ---- 6. The explanation shows what each step means -----------------------------------

#[test]
fn the_explanation_shows_paths_contracts_measures_and_alignment() {
    let none = SemanticContext::default();
    let explain = |case_id: u8| case(case_id, &none).2.plan.explain();
    let five = explain(5);
    assert!(
        five.contains("through [corpus/relationship/store, corpus/relationship/region]"),
        "{five}"
    );
    assert!(explain(3).contains("unique target of corpus/relationship/customer"));
    let aligned = explain(9);
    assert!(
        aligned.contains(
            "AlignSemanticBranches on [corpus/dimension/month × corpus/dimension/region]"
        ),
        "{aligned}"
    );
    assert!(
        aligned.contains("[corpus/measure/refunds] at [corpus/dimension/month × corpus/dimension/region] for [corpus/metric/refunds]"),
        "{aligned}"
    );
    assert!(explain(21).contains("ordered by [corpus/dimension/date]"));
    assert!(explain(13).contains("TemporalAsOf corpus/relationship/historical_customer"));
}

// ---- Policies on paths and on metrics -------------------------------------------------

#[test]
fn a_policy_on_an_intermediate_dataset_guards_its_lookup() {
    let model = changed(5, |definition| {
        definition
            .datasets
            .iter_mut()
            .find(|d| d.metadata.id().as_str() == "corpus/dataset/store")
            .expect("store")
            .policy_refs
            .push(id("corpus/policy/tenant"));
        let mut policy = model(15).definition().policies[0].clone();
        policy.scope = sef_model_ir::policy::PolicyScope::Dataset(id("corpus/dataset/store"));
        policy.predicate = ScalarExpr {
            value_type: policy.predicate.value_type.clone(),
            unit: None,
            kind: ScalarExprKind::NullTest {
                operand: Box::new(ScalarExpr {
                    kind: ScalarExprKind::ContextSlot("tenant_id".to_owned()),
                    value_type: sef_model_ir::types::ValueType::Utf8,
                    unit: None,
                }),
                negated: true,
            },
        };
        definition.policies.push(policy);
    });
    let validated = validated(
        &model,
        &query(vec![metric("revenue"), dimension("region")]),
        &tenant("acme"),
    );
    let planned = planned(&model, &validated);
    let guarded = nodes(&planned.plan).into_iter().any(|(_, op)| {
        matches!(&op, Operator::PolicyGuard { input, .. }
            if matches!(planned.plan.graph.operator(input),
                Some(Operator::ReadSemanticDataset { dataset, .. }) if dataset.id.as_str() == "corpus/dataset/store"))
    });
    assert!(guarded, "{}", planned.plan.explain());
}

#[test]
fn a_metric_policy_guards_the_rows_of_every_metric_built_on_it() {
    // CASE-015 with the tenant policy scoped to `revenue`, selected through
    // an alias that only references it.
    let model = changed(15, |definition| {
        for dataset in &mut definition.datasets {
            dataset.policy_refs.clear();
        }
        for policy in &mut definition.policies {
            policy.scope = sef_model_ir::policy::PolicyScope::Metric(id("corpus/metric/revenue"));
        }
        let revenue = definition
            .metrics
            .iter()
            .find(|m| m.metadata.id().as_str() == "corpus/metric/revenue")
            .expect("revenue")
            .clone();
        let mut alias = revenue.clone();
        alias.metadata.identity.stable_id = id("corpus/metric/alias");
        alias.metadata.label = Some("alias".to_owned());
        alias.expression = MetricExpr {
            kind: MetricExprKind::MetricRef(revenue.metadata.id().clone()),
            value_type: revenue.value_type.clone(),
            unit: revenue.unit.clone(),
        };
        alias.dependencies = vec![revenue.metadata.id().clone()];
        alias.aggregation_contract = AggregationContract::fully_additive();
        alias.policy_refs = Vec::new();
        definition.metrics.push(alias);
    });
    let validated = validated(
        &model,
        &query(vec![metric("alias"), dimension("region")]),
        &tenant("acme"),
    );
    let planned = planned(&model, &validated);
    let (aggregate, _) = nodes(&planned.plan)
        .into_iter()
        .find(|(_, op)| serves(op) == ["corpus/metric/revenue"])
        .expect("revenue branch");
    let guarded = chain(&planned.plan, aggregate)
        .iter()
        .any(|op| matches!(op, Operator::PolicyGuard { policy, .. } if policy.id.as_str() == "corpus/policy/tenant"));
    assert!(guarded, "{}", planned.plan.explain());
}
