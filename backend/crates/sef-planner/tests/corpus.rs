//! The normative corpus through the planner (§27): every valid case gets one
//! deterministic plan whose certificate discharges every obligation, with
//! the operators its semantics require. Regenerate the golden file
//! deliberately with `SEF_BLESS_PLANS=1` and review the diff.
#![allow(clippy::expect_used, clippy::panic)]

mod common;

use std::fmt::Write as _;
use std::path::PathBuf;

use sef_correctness::{SemanticContext, validate};
use sef_model_ir::types::Literal;
use sef_planner::{PlannedQuery, plan};

const VALID: [u8; 18] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 18, 19, 21,
];

fn context(case: u8) -> SemanticContext {
    let mut context = SemanticContext::default();
    if case == 15 {
        context
            .slots
            .insert("tenant_id".to_owned(), Literal::Utf8("acme".to_owned()));
    }
    context
}

fn planned(case: u8) -> PlannedQuery {
    let model = common::model(case);
    let validated = validate(&model, &common::bound(case), &context(case))
        .unwrap_or_else(|errors| panic!("CASE-{case:03} must validate: {errors:#?}"));
    let planned = plan(&model, &validated)
        .unwrap_or_else(|errors| panic!("CASE-{case:03} must plan: {errors:#?}"));
    let obligations: usize = validated
        .blocks
        .iter()
        .map(|block| block.obligations.len())
        .sum();
    assert_eq!(
        planned.certificate.discharges.len(),
        obligations,
        "CASE-{case:03}: every obligation is discharged exactly once"
    );
    planned
}

#[test]
fn every_valid_case_has_its_frozen_plan_and_certificate() {
    let mut text = String::new();
    for case in VALID {
        let planned = planned(case);
        writeln!(text, "=== CASE-{case:03}").expect("in memory");
        write!(
            text,
            "{}{}",
            sef_planner::explain(&common::model(case), &planned.plan),
            planned.certificate
        )
        .expect("in memory");
    }
    let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus-plans.txt");
    if std::env::var_os("SEF_BLESS_PLANS").is_some() {
        std::fs::write(&golden, &text).expect("bless golden plans");
    }
    let frozen = std::fs::read_to_string(&golden).expect("frozen plans");
    assert_eq!(frozen.replace("\r\n", "\n"), text, "a corpus plan changed");
}

/// The operators each case requires (§27), independent of the golden file.
#[test]
fn each_case_plans_the_operations_its_semantics_require() {
    let must = |case: u8, needles: &[&str]| {
        let text = planned(case).plan.explain();
        for needle in needles {
            assert!(
                text.contains(needle),
                "CASE-{case:03} lacks {needle}:\n{text}"
            );
        }
    };
    let order = |case: u8, first: &str, then: &str| {
        let text = planned(case).plan.explain();
        let a = text
            .find(first)
            .unwrap_or_else(|| panic!("CASE-{case:03} lacks {first}"));
        let b = text
            .find(then)
            .unwrap_or_else(|| panic!("CASE-{case:03} lacks {then}"));
        assert!(
            a < b,
            "CASE-{case:03}: {first} must precede {then}:\n{text}"
        );
    };
    must(1, &["ReadSemanticDataset", "AggregateAtGrain"]);
    must(2, &["AttachDimension"]);
    must(3, &["ContractGuard", "AttachDimension"]);
    must(4, &["AlignSemanticBranches"]);
    order(4, "AggregateAtGrain", "AlignSemanticBranches");
    must(6, &["RecomposeMetric"]);
    order(7, "SelectPeriodEnd", "AggregateAtGrain");
    must(8, &["DistinctAggregate"]);
    must(9, &["AlignSemanticBranches", "RecomposeMetric"]);
    must(10, &["AlignSemanticBranches", "RecomposeMetric"]);
    must(11, &["BridgeAllocate", "ContractGuard"]);
    order(12, "NormalizeTime", "TimeBucket");
    must(13, &["TemporalAsOf", "ContractGuard"]);
    order(15, "PolicyGuard", "AggregateAtGrain");
    order(19, "NormalizeTime", "TimeBucket");
    order(21, "AggregateAtGrain", "ComputeWindowMetric");
}

#[test]
fn planning_is_deterministic() {
    for case in VALID {
        assert_eq!(
            planned(case).certificate.seir_hash,
            planned(case).certificate.seir_hash,
            "CASE-{case:03}"
        );
    }
}

#[test]
fn distinct_counts_are_planned_apart_per_block() {
    // CASE-008: by region and overall are two DistinctAggregate nodes; the
    // overall count is never the sum of the regional ones.
    let planned = planned(8);
    let text = planned.plan.explain();
    assert_eq!(text.matches("DistinctAggregate").count(), 2, "{text}");
    assert!(!text.contains("AggregateAtGrain"), "{text}");
}

#[test]
fn a_block_without_metrics_is_an_invariant_violation() {
    // SQIR never binds a block without metrics; one built by hand, skipping
    // binding, is refused instead of producing an empty plan.
    let model = common::model(1);
    let mut validated =
        validate(&model, &common::bound(1), &SemanticContext::default()).expect("valid");
    validated.query.blocks[0].selections.retain(|selection| {
        !matches!(
            selection.item,
            sef_query_ir::query::SelectionItem::Metric(_)
        )
    });
    let errors = plan(&model, &validated).expect_err("invariant violation");
    assert!(errors.iter().any(|error| error.code
        == sef_model_ir::diagnostic::ErrorCode::PlannerInternalInvariantViolation
        && error.phase == sef_model_ir::diagnostic::Phase::Planning));
}

/// Cost of planning on the query path: run with `--ignored --nocapture`.
#[test]
#[ignore = "measurement, not a correctness check"]
fn planning_latency() {
    let model = common::model(10);
    let validated =
        validate(&model, &common::bound(10), &SemanticContext::default()).expect("valid");
    let runs = 2_000_u32;
    let start = std::time::Instant::now();
    for _ in 0..runs {
        plan(&model, &validated).expect("planned");
    }
    println!(
        "CASE-010 (two facts, ratio, time zone): {:.1} µs per plan",
        start.elapsed().as_secs_f64() * 1e6 / f64::from(runs)
    );
}
