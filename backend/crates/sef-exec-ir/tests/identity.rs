//! Content identity of SEIR (§5.1–5.2, §24): the same meaning is the same
//! plan, whatever order it was built in.
#![allow(clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use sef_exec_ir::{GraphError, NodeId, Operator, PlanBlock, SeirGraph, SemanticPlan};
use sef_model_ir::identity::StableId;
use sef_query_ir::query::{QualifiedDimensionRef, RequestedAtom};

fn id(text: &str) -> StableId {
    StableId::new(text).expect("stable id")
}

fn region() -> Vec<RequestedAtom> {
    vec![RequestedAtom::Dimension(QualifiedDimensionRef {
        dimension: id("corpus/dimension/region"),
        roles: Vec::new(),
    })]
}

/// CASE-004's shape: two facts, each aggregated to region, then aligned.
fn branch(graph: &mut SeirGraph, dataset: &str, measure: &str) -> NodeId {
    let read = graph
        .add(Operator::ReadSemanticDataset {
            dataset: id(dataset),
            fields: vec![id(&format!("{dataset}.amount"))],
        })
        .expect("read");
    graph
        .add(Operator::AggregateAtGrain {
            input: read,
            grain: region(),
            measures: vec![id(measure)],
        })
        .expect("aggregate")
}

fn plan(order_first: bool) -> SemanticPlan {
    let mut graph = SeirGraph::new();
    let (orders, items) = if order_first {
        let orders = branch(
            &mut graph,
            "corpus/dataset/orders",
            "corpus/measure/order_revenue",
        );
        let items = branch(
            &mut graph,
            "corpus/dataset/order_items",
            "corpus/measure/item_count",
        );
        (orders, items)
    } else {
        let items = branch(
            &mut graph,
            "corpus/dataset/order_items",
            "corpus/measure/item_count",
        );
        let orders = branch(
            &mut graph,
            "corpus/dataset/orders",
            "corpus/measure/order_revenue",
        );
        (orders, items)
    };
    let inputs = if order_first {
        vec![orders, items]
    } else {
        vec![items, orders]
    };
    let root = graph
        .add(Operator::AlignSemanticBranches {
            inputs,
            grain: region(),
        })
        .expect("align");
    SemanticPlan {
        graph,
        blocks: vec![PlanBlock {
            block_id: "result".to_owned(),
            root,
        }],
    }
}

#[test]
fn construction_order_never_changes_the_plan() {
    let a = plan(true);
    let b = plan(false);
    assert_eq!(a.blocks, b.blocks, "same root identity");
    assert_eq!(
        a.fingerprint().expect("encoding"),
        b.fingerprint().expect("encoding")
    );
    assert_eq!(a.explain(), b.explain());
}

#[test]
fn identical_subplans_are_shared() {
    let mut graph = SeirGraph::new();
    let first = branch(
        &mut graph,
        "corpus/dataset/orders",
        "corpus/measure/order_revenue",
    );
    let before = graph.len();
    let second = branch(
        &mut graph,
        "corpus/dataset/orders",
        "corpus/measure/order_revenue",
    );
    assert_eq!(first, second);
    assert_eq!(graph.len(), before, "no duplicated nodes");
}

#[test]
fn a_node_can_only_read_existing_nodes() {
    let mut other = SeirGraph::new();
    let foreign = branch(
        &mut other,
        "corpus/dataset/orders",
        "corpus/measure/order_revenue",
    );
    let mut graph = SeirGraph::new();
    let error = graph
        .add(Operator::RecomposeMetric {
            input: foreign,
            metric: id("corpus/metric/average_order_value"),
        })
        .expect_err("unknown input");
    assert_eq!(error, GraphError::UnknownInput(foreign));
    assert!(graph.is_empty());
}

#[test]
fn leftover_nodes_do_not_change_the_plan() {
    let clean = plan(true);
    let mut noisy = plan(true);
    branch(
        &mut noisy.graph,
        "corpus/dataset/unused",
        "corpus/measure/unused",
    );
    assert_eq!(
        clean.fingerprint().expect("encoding"),
        noisy.fingerprint().expect("encoding")
    );
}

#[test]
fn a_different_meaning_is_a_different_plan() {
    let mut graph = SeirGraph::new();
    let read = graph
        .add(Operator::ReadSemanticDataset {
            dataset: id("corpus/dataset/orders"),
            fields: vec![id("corpus/dataset/orders.amount")],
        })
        .expect("read");
    let by_region = graph
        .add(Operator::AggregateAtGrain {
            input: read,
            grain: region(),
            measures: vec![id("corpus/measure/order_revenue")],
        })
        .expect("by region");
    let overall = graph
        .add(Operator::AggregateAtGrain {
            input: read,
            grain: Vec::new(),
            measures: vec![id("corpus/measure/order_revenue")],
        })
        .expect("overall");
    assert_ne!(by_region, overall);
}

#[test]
fn the_explanation_lists_inputs_before_their_readers() {
    let text = plan(true).explain();
    let position = |needle: &str| text.find(needle).expect(needle);
    assert!(position("ReadSemanticDataset") < position("AggregateAtGrain"));
    assert!(position("AggregateAtGrain") < position("AlignSemanticBranches"));
    assert!(text.ends_with(&format!("block result = {}\n", plan(true).blocks[0].root)));
}

proptest! {
    /// Any insertion order of the same independent reads, aligned in any
    /// order, gives one plan.
    #[test]
    fn insertion_order_is_irrelevant(order in Just((0..4_usize).collect::<Vec<_>>()).prop_shuffle()) {
        let datasets = ["a", "b", "c", "d"];
        let build = |sequence: &[usize]| {
            let mut graph = SeirGraph::new();
            let mut nodes: Vec<(usize, NodeId)> = sequence
                .iter()
                .map(|&i| (i, branch(&mut graph, &format!("corpus/dataset/{}", datasets[i]), "corpus/measure/m")))
                .collect();
            let inputs: Vec<NodeId> = nodes.iter().map(|(_, node)| *node).collect();
            let root = graph
                .add(Operator::AlignSemanticBranches { inputs, grain: region() })
                .expect("align");
            nodes.sort();
            SemanticPlan { graph, blocks: vec![PlanBlock { block_id: "result".to_owned(), root }] }
        };
        let canonical = build(&[0, 1, 2, 3]);
        let shuffled = build(&order);
        prop_assert_eq!(canonical.fingerprint().expect("encoding"), shuffled.fingerprint().expect("encoding"));
    }
}
