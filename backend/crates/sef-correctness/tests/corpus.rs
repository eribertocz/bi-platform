//! The normative corpus through the correctness engine (§25): every valid
//! case yields exactly its frozen facts and obligations, and the three
//! normative rejections fail with their exact codes. Regenerate the golden
//! file deliberately with `SEF_BLESS_CORRECTNESS=1` and review the diff.
#![allow(clippy::expect_used, clippy::panic)]

mod common;

use std::fmt::Write as _;
use std::path::PathBuf;

use sef_correctness::{SemanticContext, SemanticErrorCode as Code, ValidatedQuery, validate};
use sef_model_ir::types::Literal;
use sef_ossie::document::{Dialect, DialectExpression, Expression, Metric};
use sef_query_ir::unbound::{UnboundDimension, UnboundItem, UnboundSelection};

/// CASE-015's tenant is bound; every other case needs no context.
fn context(case: u8) -> SemanticContext {
    let mut context = SemanticContext::default();
    if case == 15 {
        context
            .slots
            .insert("tenant_id".to_owned(), Literal::Utf8("acme".to_owned()));
    }
    context
}

fn validated(case: u8) -> ValidatedQuery {
    validate(&common::model(case), &common::bound(case), &context(case))
        .unwrap_or_else(|errors| panic!("CASE-{case:03} must validate: {errors:#?}"))
}

fn rejected(case: u8) -> Vec<sef_correctness::SemanticError> {
    validate(&common::model(case), &common::bound(case), &context(case))
        .expect_err("must be rejected")
}

const VALID: [u8; 18] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 18, 19, 21,
];

#[test]
fn every_valid_case_yields_its_frozen_obligations() {
    let mut text = String::new();
    for case in VALID {
        writeln!(text, "=== CASE-{case:03}").expect("in memory");
        write!(text, "{}", validated(case)).expect("in memory");
    }
    let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus-obligations.txt");
    if std::env::var_os("SEF_BLESS_CORRECTNESS").is_some() {
        std::fs::write(&golden, &text).expect("bless golden obligations");
    }
    let frozen = std::fs::read_to_string(&golden).expect("frozen obligations");
    assert_eq!(
        frozen.replace("\r\n", "\n"),
        text,
        "the facts or obligations of a corpus case changed"
    );
}

/// The corpus matrix (§25), stated independently of the golden file.
#[test]
fn each_case_carries_the_obligation_its_semantics_require() {
    let must = |case: u8, needles: &[&str]| {
        let text = validated(case).to_string();
        for needle in needles {
            assert!(
                text.contains(needle),
                "CASE-{case:03} lacks {needle:?}:\n{text}"
            );
        }
    };
    must(3, &["proven at-most-one-match"]);
    must(
        4,
        &[
            "must PreserveMetricGrain corpus/measure/order_revenue",
            "must PreserveMetricGrain corpus/measure/item_count",
            "must AggregateBeforeAlignment",
        ],
    );
    must(
        5,
        &["through [corpus/relationship/store, corpus/relationship/region]"],
    );
    must(
        6,
        &["must RecomputeAtTargetGrain corpus/metric/average_order_value"],
    );
    must(
        7,
        &["must ApplyPeriodEndSelection corpus/metric/balance period-end"],
    );
    must(8, &["must DoNotReaggregate corpus/metric/active_customers"]);
    must(
        9,
        &[
            "must AggregateBeforeAlignment",
            "must RecomputeAtTargetGrain corpus/metric/net_revenue",
        ],
    );
    must(
        10,
        &[
            "must AggregateBeforeAlignment",
            "must RecomputeAtTargetGrain corpus/metric/attainment",
        ],
    );
    must(
        11,
        &[
            "must ApplyBridgeAllocation corpus/metric/allocated_revenue",
            "must ValidateAllocationNormalization",
        ],
    );
    must(
        12,
        &[
            "must ConvertTimezoneBeforeBucketing corpus/dimension/month on corpus/dataset/events to UTC",
        ],
    );
    must(
        13,
        &["must UseTemporalRelationship corpus/relationship/historical_customer"],
    );
    must(14, &["count-rows", "count-non-null"]);
    must(
        15,
        &["must ApplyMandatoryPolicy corpus/policy/tenant on corpus/dataset/sales"],
    );
    must(
        19,
        &[
            "to America/La_Paz then Day",
            "must PreserveExactDecimalSemantics corpus/metric/revenue",
        ],
    );
    must(
        21,
        &[
            "must ApplyOrderedCumulative corpus/metric/cumulative_revenue over corpus/metric/daily_revenue",
        ],
    );
}

#[test]
fn case_016_rejects_the_ambiguous_customer_role() {
    let errors = rejected(16);
    assert!(
        errors
            .iter()
            .any(|error| error.code == Code::AmbiguousRelationshipPath
                && error.path == "/blocks/0/selections/1"
                && error.suggestions.len() == 2)
    );
}

#[test]
fn case_016_validates_once_the_role_is_stated() {
    let mut request = common::request(16);
    request.blocks[0].selections[1] = UnboundSelection {
        selection_id: "country".to_owned(),
        item: UnboundItem::Dimension(UnboundDimension {
            name: "country".to_owned(),
            roles: vec!["billing_customer".to_owned()],
        }),
        output_label: None,
    };
    let model = common::model(16);
    let query = sef_query_ir::bind(&request, model.definition()).expect("bound");
    let text = validate(&model, &query, &SemanticContext::default())
        .expect("disambiguated")
        .to_string();
    assert!(text.contains("UseDeclaredRelationshipRole corpus/relationship/billing_customer"));
    assert!(!text.contains("shipping_customer"));
}

#[test]
fn case_017_never_reaches_the_engine() {
    // The corpus document holds the two valid metrics; the requested
    // `combined = revenue_usd + revenue_bob` is rejected when the model
    // compiles (`expected/error.json`: INCOMPATIBLE_UNITS naming USD and
    // BOB), so no query can ever reach the engine with it.
    let errors = common::compiled_with(17, |document| {
        document.metrics.push(Metric {
            name: "combined".to_owned(),
            expression: Expression {
                dialects: vec![DialectExpression {
                    dialect: Dialect::OssieSql2026,
                    expression: "revenue_usd + revenue_bob".to_owned(),
                }],
            },
            description: None,
            datatype: None,
            ai_context: None,
            custom_extensions: Vec::new(),
        });
    })
    .expect_err("incompatible units");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("INCOMPATIBLE_UNITS")
                && error.message.contains("USD")
                && error.message.contains("BOB")),
        "{errors:#?}"
    );
}

#[test]
fn case_020_rejects_summing_balances_across_months() {
    let errors = rejected(20);
    assert!(
        errors
            .iter()
            .any(|error| error.code == Code::IllegalTemporalReaggregation),
        "{errors:#?}"
    );
}

#[test]
fn case_015_without_its_tenant_is_rejected() {
    let errors = validate(
        &common::model(15),
        &common::bound(15),
        &SemanticContext::default(),
    )
    .expect_err("tenant required");
    assert!(
        errors
            .iter()
            .any(|error| error.code == Code::PolicyContextMissing
                && error.path == "/context/slots/tenant_id")
    );
}

#[test]
fn validation_is_deterministic() {
    for case in VALID {
        assert_eq!(validated(case), validated(case), "CASE-{case:03}");
    }
}

/// Cost of validation on the query path: run with `--ignored --nocapture`.
#[test]
#[ignore = "measurement, not a correctness check"]
fn validation_latency() {
    let model = common::model(10);
    let query = common::bound(10);
    let context = SemanticContext::default();
    let runs = 2_000_u32;
    let start = std::time::Instant::now();
    for _ in 0..runs {
        validate(&model, &query, &context).expect("valid");
    }
    println!(
        "CASE-010 (two facts, ratio, time zone): {:.1} µs per validation",
        start.elapsed().as_secs_f64() * 1e6 / f64::from(runs)
    );
}
