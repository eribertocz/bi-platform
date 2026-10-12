//! Correctness properties (§23) over arbitrary requests: they must hold for
//! every input, not only for the corpus examples.
#![allow(clippy::expect_used, clippy::panic)]

mod common;

use proptest::prelude::*;
use sef_correctness::{Obligation, SemanticContext, validate};
use sef_model_ir::expressions::ComparisonOp;
use sef_model_ir::types::Literal;
use sef_query_ir::unbound::{UnboundDimension, UnboundFilter, UnboundTarget, Value};

/// Cases whose single block has several selections, multi-fact included.
const CASES: [u8; 6] = [2, 4, 9, 10, 18, 21];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// The order in which a caller lists selections is presentation: the
    /// proven facts and the obligations are the same.
    #[test]
    fn selection_order_never_changes_meaning((case, order) in shuffled_case()) {
        let model = common::model(case);
        let original = common::request(case);
        let mut shuffled = original.clone();
        shuffled.blocks[0].selections = order
            .iter()
            .map(|&i| original.blocks[0].selections[i].clone())
            .collect();
        let bind = |request| sef_query_ir::bind(request, model.definition()).expect("bound");
        let context = SemanticContext::default();
        let a = validate(&model, &bind(&original), &context).expect("valid");
        let b = validate(&model, &bind(&shuffled), &context).expect("valid");
        prop_assert_eq!(&a.blocks[0].facts, &b.blocks[0].facts);
        prop_assert_eq!(&a.blocks[0].obligations, &b.blocks[0].obligations);
    }

    /// Policy non-broadening (§20.1): whatever the caller filters by, the
    /// mandatory tenant policy is still attached.
    #[test]
    fn caller_filters_never_remove_a_mandatory_policy(filter in filters()) {
        let model = common::model(15);
        let mut request = common::request(15);
        request.blocks[0].filter = Some(filter);
        let query = sef_query_ir::bind(&request, model.definition()).expect("bound");
        let mut context = SemanticContext::default();
        context.slots.insert("tenant_id".to_owned(), Literal::Utf8("acme".to_owned()));
        let validated = validate(&model, &query, &context).expect("valid");
        let attached = validated.blocks[0].obligations.iter().any(|obligation| matches!(
            obligation,
            Obligation::ApplyMandatoryPolicy { policy, .. }
                if policy.as_str() == "corpus/policy/tenant"
        ));
        prop_assert!(attached, "the tenant policy was dropped");
    }
}

/// A case and a permutation of its block's selections.
fn shuffled_case() -> impl Strategy<Value = (u8, Vec<usize>)> {
    prop::sample::select(CASES.to_vec()).prop_flat_map(|case| {
        let len = common::request(case).blocks[0].selections.len();
        (
            Just(case),
            Just((0..len).collect::<Vec<_>>()).prop_shuffle(),
        )
    })
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
        Just(UnboundFilter::IsNotNull(region())),
    ];
    leaf.prop_recursive(3, 16, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 1..4).prop_map(UnboundFilter::And),
            prop::collection::vec(inner.clone(), 1..4).prop_map(UnboundFilter::Or),
            inner.prop_map(|filter| UnboundFilter::Not(Box::new(filter))),
        ]
    })
}
