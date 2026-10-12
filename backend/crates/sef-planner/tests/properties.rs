//! Planner properties over arbitrary requests (§23, §33).
#![allow(clippy::expect_used, clippy::panic)]

mod common;

use proptest::prelude::*;
use sef_correctness::{SemanticContext, validate};
use sef_exec_ir::Operator;
use sef_model_ir::expressions::ComparisonOp;
use sef_model_ir::types::Literal;
use sef_planner::plan;
use sef_query_ir::unbound::{UnboundDimension, UnboundFilter, UnboundTarget, Value};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever the caller filters by, the protected rows are read only
    /// through the tenant guard, and every obligation stays discharged.
    #[test]
    fn the_policy_guard_always_dominates_the_protected_source(filter in filters()) {
        let model = common::model(15);
        let mut request = common::request(15);
        request.blocks[0].filter = Some(filter);
        let query = sef_query_ir::bind(&request, model.definition()).expect("bound");
        let mut context = SemanticContext::default();
        context.slots.insert("tenant_id".to_owned(), Literal::Utf8("acme".to_owned()));
        let validated = validate(&model, &query, &context).expect("valid");
        let planned = plan(&model, &validated).expect("planned");
        let nodes = planned.plan.graph.topological(&[planned.plan.blocks[0].root]);
        let reads: Vec<_> = nodes
            .iter()
            .filter(|node| matches!(node.operator, Operator::ReadSemanticDataset { .. }))
            .map(|node| node.id)
            .collect();
        for node in &nodes {
            let reads_source = node.operator.inputs().iter().any(|input| reads.contains(input));
            prop_assert!(
                !reads_source || matches!(node.operator, Operator::PolicyGuard { .. }),
                "{} reads the protected source directly", node.operator.name()
            );
        }
        let obligations: usize = validated.blocks.iter().map(|block| block.obligations.len()).sum();
        prop_assert_eq!(planned.certificate.discharges.len(), obligations);
    }
}

fn region() -> UnboundTarget {
    UnboundTarget::Dimension(UnboundDimension {
        name: "region".to_owned(),
        roles: Vec::new(),
    })
}

fn filters() -> impl Strategy<Value = UnboundFilter> {
    let text = "[a-zA-Z ]{0,12}".prop_map(Value::Text);
    let leaf = prop_oneof![
        text.clone().prop_map(|value| UnboundFilter::Compare {
            target: region(),
            op: ComparisonOp::Equal,
            value,
        }),
        prop::collection::vec(text, 1..4).prop_map(|values| UnboundFilter::In {
            target: region(),
            values,
        }),
        Just(UnboundFilter::IsNull(region())),
    ];
    leaf.prop_recursive(3, 16, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 1..4).prop_map(UnboundFilter::And),
            prop::collection::vec(inner.clone(), 1..4).prop_map(UnboundFilter::Or),
            inner.prop_map(|filter| UnboundFilter::Not(Box::new(filter))),
        ]
    })
}
