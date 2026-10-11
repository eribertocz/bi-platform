//! What binding rejects (§16), how literals are typed, and properties of
//! the canonical form.
#![allow(clippy::expect_used, clippy::panic)]

mod common;

use proptest::prelude::*;
use sef_model_ir::expressions::ComparisonOp;
use sef_model_ir::identity::StableId;
use sef_model_ir::model::ModelDefinition;
use sef_model_ir::time::TimeGrain;
use sef_model_ir::types::Literal;
use sef_query_ir::query::{FilterExpr, SelectionItem, SemanticQuerySet};
use sef_query_ir::unbound::{
    UnboundBlock, UnboundDimension, UnboundFilter, UnboundItem, UnboundOrder, UnboundQuerySet,
    UnboundSelection, UnboundTarget, Value,
};
use sef_query_ir::{QueryErrorCode as Code, bind};

fn metric(name: &str) -> UnboundSelection {
    UnboundSelection {
        selection_id: name.to_owned(),
        item: UnboundItem::Metric(name.to_owned()),
        output_label: None,
    }
}

fn dimension(name: &str) -> UnboundDimension {
    UnboundDimension {
        name: name.to_owned(),
        roles: Vec::new(),
    }
}

fn dimension_selection(name: &str) -> UnboundSelection {
    UnboundSelection {
        selection_id: name.to_owned(),
        item: UnboundItem::Dimension(dimension(name)),
        output_label: None,
    }
}

/// CASE-001: `revenue` (DECIMAL(18,2)) by `region` (VARCHAR).
fn revenue_by_region() -> UnboundBlock {
    UnboundBlock {
        block_id: "result".to_owned(),
        selections: vec![metric("revenue"), dimension_selection("region")],
        ..UnboundBlock::default()
    }
}

fn one(block: UnboundBlock) -> UnboundQuerySet {
    UnboundQuerySet {
        blocks: vec![block],
    }
}

fn region(value: &str) -> UnboundFilter {
    UnboundFilter::Compare {
        target: UnboundTarget::Dimension(dimension("region")),
        op: ComparisonOp::Equal,
        value: Value::Text(value.to_owned()),
    }
}

fn rejects(query: &UnboundQuerySet, model: &ModelDefinition, code: Code, path: &str) {
    let errors = bind(query, model).expect_err("must be rejected");
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

fn bound(query: &UnboundQuerySet, model: &ModelDefinition) -> SemanticQuerySet {
    bind(query, model).unwrap_or_else(|errors| panic!("{errors:#?}"))
}

// ---- What binding rejects ------------------------------------------------------

#[test]
fn the_shape_of_a_request_is_checked() {
    let model = common::model(1);
    rejects(
        &UnboundQuerySet::default(),
        &model,
        Code::EmptyQuery,
        "/blocks",
    );

    let duplicated = UnboundQuerySet {
        blocks: vec![revenue_by_region(), revenue_by_region()],
    };
    rejects(
        &duplicated,
        &model,
        Code::DuplicateBlock,
        "/blocks/1/block_id",
    );

    let mut dimensions_only = revenue_by_region();
    dimensions_only.selections.remove(0);
    rejects(
        &one(dimensions_only),
        &model,
        Code::NoMetric,
        "/blocks/0/selections",
    );

    let mut repeated = revenue_by_region();
    repeated.selections[1].selection_id = "revenue".to_owned();
    rejects(
        &one(repeated),
        &model,
        Code::DuplicateSelection,
        "/blocks/0/selections/1/selection_id",
    );
}

#[test]
fn unknown_and_ambiguous_names_are_rejected() {
    let model = common::model(1);
    let mut unknown = revenue_by_region();
    unknown.selections[0].item = UnboundItem::Metric("profit".to_owned());
    rejects(
        &one(unknown),
        &model,
        Code::UnknownMetric,
        "/blocks/0/selections/0",
    );

    let mut unknown = revenue_by_region();
    unknown.selections[1].item = UnboundItem::Dimension(dimension("country"));
    rejects(
        &one(unknown),
        &model,
        Code::UnknownDimension,
        "/blocks/0/selections/1",
    );

    // Two metrics whose labels differ only in case.
    let mut model = common::model(1);
    let mut twin = model.metrics[0].clone();
    twin.metadata.identity.stable_id = StableId::new("corpus/metric/revenue-2").expect("id");
    twin.metadata.label = Some("Revenue".to_owned());
    model.metrics.push(twin);
    rejects(
        &one(revenue_by_region()),
        &model,
        Code::AmbiguousName,
        "/blocks/0/selections/0",
    );
}

#[test]
fn roles_must_be_declared_by_a_relationship() {
    let model = common::model(16);
    let mut query = one(revenue_by_region());
    query.blocks[0].selections[1].item = UnboundItem::Dimension(UnboundDimension {
        name: "country".to_owned(),
        roles: vec!["delivery_customer".to_owned()],
    });
    rejects(
        &query,
        &model,
        Code::UnknownRole,
        "/blocks/0/selections/1/roles/0",
    );

    query.blocks[0].selections[1].item = UnboundItem::Dimension(UnboundDimension {
        name: "country".to_owned(),
        roles: vec!["Billing_Customer".to_owned()],
    });
    let bound = bound(&query, &model);
    let SelectionItem::Dimension(country) = &bound.blocks[0].selections[1].item else {
        panic!("dimension");
    };
    assert_eq!(country.roles, ["billing_customer"]);
}

#[test]
fn literals_must_fit_their_target_without_loss() {
    let model = common::model(1);
    let having = |value: &str| {
        let mut block = revenue_by_region();
        block.post_aggregate_filter = Some(UnboundFilter::Compare {
            target: UnboundTarget::Metric("revenue".to_owned()),
            op: ComparisonOp::Greater,
            value: Value::Number(value.to_owned()),
        });
        one(block)
    };
    let path = "/blocks/0/post_aggregate_filter/value";
    rejects(&having("1.234"), &model, Code::InvalidLiteral, path);
    rejects(&having("1e3"), &model, Code::InvalidLiteral, path);
    rejects(
        &having("10000000000000000000"),
        &model,
        Code::InvalidLiteral,
        path,
    );

    // 5 is stated exactly at the metric's type: DECIMAL(18,2) 5.00.
    let bound = bound(&having("5"), &model);
    let Some(FilterExpr::Compare { value, .. }) = &bound.blocks[0].post_aggregate_filter else {
        panic!("compare");
    };
    let Literal::Decimal(value) = value else {
        panic!("decimal");
    };
    assert_eq!(value.coefficient(), "500");
    assert_eq!(value.decimal_type().scale(), 2);

    let mut block = revenue_by_region();
    block.filter = Some(UnboundFilter::Compare {
        target: UnboundTarget::Dimension(dimension("region")),
        op: ComparisonOp::Equal,
        value: Value::Number("5".to_owned()),
    });
    rejects(
        &one(block),
        &model,
        Code::InvalidLiteral,
        "/blocks/0/filter/value",
    );
}

#[test]
fn null_intent_is_explicit() {
    let model = common::model(1);
    let mut block = revenue_by_region();
    block.filter = Some(UnboundFilter::Compare {
        target: UnboundTarget::Dimension(dimension("region")),
        op: ComparisonOp::Equal,
        value: Value::Null,
    });
    rejects(
        &one(block),
        &model,
        Code::NullComparison,
        "/blocks/0/filter/value",
    );

    let mut block = revenue_by_region();
    block.filter = Some(UnboundFilter::In {
        target: UnboundTarget::Dimension(dimension("region")),
        values: Vec::new(),
    });
    rejects(
        &one(block),
        &model,
        Code::EmptyList,
        "/blocks/0/filter/values",
    );

    let mut block = revenue_by_region();
    block.filter = Some(UnboundFilter::IsNull(UnboundTarget::Dimension(dimension(
        "region",
    ))));
    bound(&one(block), &model);
}

#[test]
fn filters_target_the_right_phase() {
    let model = common::model(1);
    let mut block = revenue_by_region();
    block.filter = Some(UnboundFilter::IsNull(UnboundTarget::Metric(
        "revenue".to_owned(),
    )));
    rejects(
        &one(block),
        &model,
        Code::InvalidFilterTarget,
        "/blocks/0/filter/target",
    );

    let mut block = revenue_by_region();
    block.post_aggregate_filter = Some(region("North"));
    rejects(
        &one(block),
        &model,
        Code::InvalidFilterTarget,
        "/blocks/0/post_aggregate_filter/target",
    );

    let mut block = revenue_by_region();
    block.post_aggregate_filter = Some(UnboundFilter::IsNull(UnboundTarget::Metric(
        "region".to_owned(),
    )));
    rejects(
        &one(block),
        &model,
        Code::InvalidFilterTarget,
        "/blocks/0/post_aggregate_filter/target",
    );
}

#[test]
fn time_buckets_need_a_temporal_dimension_and_valid_overrides() {
    let model = common::model(7);
    let bucket = |name: &str, timezone: Option<&str>, calendar: Option<&str>| {
        let mut block = revenue_by_region();
        block.selections = vec![
            metric("balance"),
            UnboundSelection {
                selection_id: "period".to_owned(),
                item: UnboundItem::TimeDimension {
                    dimension: dimension(name),
                    grain: TimeGrain::Month,
                    timezone: timezone.map(str::to_owned),
                    calendar: calendar.map(str::to_owned),
                },
                output_label: None,
            },
        ];
        one(block)
    };
    rejects(
        &bucket("region", None, None),
        &model,
        Code::NotTemporal,
        "/blocks/0/selections/1",
    );
    rejects(
        &bucket("snapshot_date", Some("Mars/Olympus Mons"), None),
        &model,
        Code::InvalidLiteral,
        "/blocks/0/selections/1/timezone",
    );
    rejects(
        &bucket("snapshot_date", None, Some("fiscal")),
        &model,
        Code::InvalidLiteral,
        "/blocks/0/selections/1/calendar",
    );
    bound(
        &bucket("snapshot_date", Some("America/La_Paz"), None),
        &model,
    );
}

#[test]
fn ordering_and_pagination_never_depend_on_engine_row_order() {
    let model = common::model(1);
    let mut block = revenue_by_region();
    block.order_by = vec![UnboundOrder {
        selection_id: "profit".to_owned(),
        descending: true,
        nulls_first: false,
    }];
    rejects(
        &one(block),
        &model,
        Code::UnknownSelection,
        "/blocks/0/order_by/0/selection_id",
    );

    let mut block = revenue_by_region();
    block.limit = Some(10);
    rejects(
        &one(block),
        &model,
        Code::InvalidPagination,
        "/blocks/0/limit",
    );

    let mut block = revenue_by_region();
    block.offset = Some(10);
    block.order_by = vec![UnboundOrder {
        selection_id: "revenue".to_owned(),
        descending: true,
        nulls_first: false,
    }];
    rejects(
        &one(block.clone()),
        &model,
        Code::InvalidPagination,
        "/blocks/0/offset",
    );

    block.limit = Some(10);
    bound(&one(block), &model);
}

// ---- Canonical form --------------------------------------------------------------

#[test]
fn output_labels_do_not_change_meaning_but_selection_order_does() {
    let model = common::model(1);
    let plain = bound(&one(revenue_by_region()), &model);
    let mut labelled = revenue_by_region();
    labelled.selections[0].output_label = Some("Ingresos".to_owned());
    let labelled = bound(&one(labelled), &model);
    assert_eq!(plain.fingerprint().ok(), labelled.fingerprint().ok());
    let mut swapped = revenue_by_region();
    swapped.selections.reverse();
    let swapped = bound(&one(swapped), &model);
    assert_ne!(plain.fingerprint().ok(), swapped.fingerprint().ok());
}

#[test]
fn double_negation_and_single_operands_normalize_away() {
    let model = common::model(1);
    let with = |filter: UnboundFilter| {
        let mut block = revenue_by_region();
        block.filter = Some(filter);
        bound(&one(block), &model).fingerprint().expect("encoding")
    };
    let plain = with(region("North"));
    assert_eq!(
        with(UnboundFilter::Not(Box::new(UnboundFilter::Not(Box::new(
            region("North")
        ))))),
        plain
    );
    assert_eq!(with(UnboundFilter::And(vec![region("North")])), plain);
    assert_eq!(
        with(UnboundFilter::Or(vec![region("North"), region("North")])),
        plain
    );
}

/// A random boolean tree over `region` comparisons.
fn filters() -> impl Strategy<Value = UnboundFilter> {
    let leaf = prop::sample::select(vec!["North", "South", "East", "West"]).prop_map(region);
    leaf.prop_recursive(4, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 1..4).prop_map(UnboundFilter::And),
            prop::collection::vec(inner.clone(), 1..4).prop_map(UnboundFilter::Or),
            inner.prop_map(|filter| UnboundFilter::Not(Box::new(filter))),
        ]
    })
}

/// The same tree with every operand list reversed.
fn mirrored(filter: &UnboundFilter) -> UnboundFilter {
    match filter {
        UnboundFilter::And(operands) => {
            UnboundFilter::And(operands.iter().rev().map(mirrored).collect())
        }
        UnboundFilter::Or(operands) => {
            UnboundFilter::Or(operands.iter().rev().map(mirrored).collect())
        }
        UnboundFilter::Not(operand) => UnboundFilter::Not(Box::new(mirrored(operand))),
        UnboundFilter::In { target, values } => UnboundFilter::In {
            target: target.clone(),
            values: values.iter().rev().cloned().collect(),
        },
        leaf => leaf.clone(),
    }
}

proptest! {
    #[test]
    fn normalization_is_idempotent(filter in filters()) {
        let model = common::model(1);
        let mut block = revenue_by_region();
        block.filter = Some(filter);
        let bound = bound(&one(block), &model);
        let normal = bound.blocks[0].filter.clone().expect("filter");
        prop_assert_eq!(normal.clone().normalized(), normal);
    }

    #[test]
    fn operand_order_does_not_change_the_fingerprint(filter in filters()) {
        let model = common::model(1);
        let fingerprint = |filter: UnboundFilter| {
            let mut block = revenue_by_region();
            block.filter = Some(filter);
            bound(&one(block), &model).fingerprint().expect("encoding")
        };
        prop_assert_eq!(fingerprint(filter.clone()), fingerprint(mirrored(&filter)));
    }

    #[test]
    fn in_list_order_and_repetition_do_not_change_the_fingerprint(
        values in prop::collection::vec(prop::sample::select(vec!["North", "South", "East"]), 1..6),
    ) {
        let model = common::model(1);
        let fingerprint = |values: Vec<&str>| {
            let mut block = revenue_by_region();
            block.filter = Some(UnboundFilter::In {
                target: UnboundTarget::Dimension(dimension("region")),
                values: values.into_iter().map(|value| Value::Text(value.to_owned())).collect(),
            });
            bound(&one(block), &model).fingerprint().expect("encoding")
        };
        let mut sorted = values.clone();
        sorted.sort_unstable();
        sorted.dedup();
        prop_assert_eq!(fingerprint(values), fingerprint(sorted));
    }
}
