//! `OSSIE_SQL_2026` analysis: what the language allows, what it rejects and
//! how aggregates are classified for fanout and pre-aggregation.

// Tests fail loudly on purpose: a broken fixture or contract must stop the run.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use sef_ossie::Code;
use sef_ossie::expression::{AggregateClass, Context, Reference, analyze};

fn rejected(text: &str, context: Context) -> Code {
    analyze(text, context)
        .map(|analysis| panic!("{text:?} must be rejected, got {analysis:?}"))
        .unwrap_err()
        .code
}

fn reference(qualifier: Option<&str>, name: &str) -> Reference {
    Reference {
        qualifier: qualifier.map(str::to_owned),
        name: name.to_owned(),
    }
}

#[test]
fn the_constructs_of_the_language_are_accepted() {
    let fields = [
        "customer_id",
        "first_name || ' ' || last_name",
        "CASE WHEN status IN ('a', 'b') THEN 1 WHEN status IS NULL THEN 0 ELSE -1 END",
        "amount BETWEEN 0 AND 100 AND NOT is_test",
        "email ILIKE '%@example.com' OR email LIKE 'x%'",
        "a IS NOT DISTINCT FROM b",
        "CAST(amount AS DECIMAL(18, 2))",
        "TRY_CAST(code AS INTEGER)",
        "EXTRACT(YEAR FROM ordered_at) * 100 + MONTH(ordered_at)",
        "DATE_TRUNC('month', ordered_at)",
        "DATEADD(day, 7, ordered_at)",
        "SUBSTRING(code, 1, 3)",
        "TRIM(name)",
        "POSITION('x' IN name)",
        "COALESCE(NULLIF(amount, 0), 1)",
        "ROUND(amount * 1.21, 2) - FLOOR(amount) + CEIL(amount)",
        "GREATEST(a, b, c)",
        "CURRENT_DATE",
        "'semi;colon inside a string'",
    ];
    for text in fields {
        analyze(text, Context::Field).unwrap_or_else(|e| panic!("{text}: {e}"));
    }
    let metrics = [
        "SUM(orders.amount) / COUNT(DISTINCT customers.id)",
        "COUNT(*)",
        "PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY amount)",
        "SUM(amount) OVER (PARTITION BY region ORDER BY ordered_at)",
        "ROW_NUMBER() OVER (ORDER BY amount DESC)",
        "LAG(amount, 1) OVER (ORDER BY ordered_at)",
        "SUM(CASE WHEN status = 'done' THEN amount ELSE 0 END)",
        "APPROX_COUNT_DISTINCT(visitor) + APPROX_PERCENTILE(latency, 0.95)",
        "STDDEV(amount) + VAR_POP(amount) + MEDIAN(amount)",
    ];
    for text in metrics {
        analyze(text, Context::Metric).unwrap_or_else(|e| panic!("{text}: {e}"));
    }
}

#[test]
fn references_are_normalized_identifiers() {
    let analysis = analyze(
        r#"Orders.Amount + "Exact Name" + "orders".x"#,
        Context::Field,
    )
    .unwrap();
    let expected = [
        reference(None, "Exact Name"),
        reference(Some("ORDERS"), "AMOUNT"),
        reference(Some("orders"), "X"),
    ];
    assert_eq!(
        analysis.references.into_iter().collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn aggregates_are_classified_for_fanout_and_pre_aggregation() {
    let analysis = analyze(
        "SUM(o.amount) + COUNT(DISTINCT c.id) + MIN(o.x) + MAX(o.x) + AVG(o.y) \
         + APPROX_COUNT_DISTINCT(o.z) + APPROX_PERCENTILE(o.z, 0.5) + COUNT(*)",
        Context::Metric,
    )
    .unwrap();
    let summary: Vec<(&str, AggregateClass, bool, bool)> = analysis
        .aggregates
        .iter()
        .map(|a| (a.function.as_str(), a.class, a.distinct, a.fanout_safe))
        .collect();
    assert_eq!(
        summary,
        [
            ("SUM", AggregateClass::Distributive, false, false),
            ("COUNT", AggregateClass::Holistic, true, true),
            ("MIN", AggregateClass::Distributive, false, true),
            ("MAX", AggregateClass::Distributive, false, true),
            ("AVG", AggregateClass::Algebraic, false, false),
            ("APPROX_COUNT_DISTINCT", AggregateClass::Sketch, false, true),
            ("APPROX_PERCENTILE", AggregateClass::Sketch, false, false),
            ("COUNT", AggregateClass::Distributive, false, false),
        ]
    );
    assert_eq!(
        analysis.aggregates[0].references.iter().collect::<Vec<_>>(),
        [&reference(Some("O"), "AMOUNT")]
    );
    assert!(
        analysis.aggregates[7].references.is_empty(),
        "COUNT(*) reads no column"
    );
    assert!(!analysis.windowed);
}

#[test]
fn constructs_outside_the_language_are_rejected() {
    let cases = [
        ("(SELECT max(x) FROM t)", Code::UnsupportedConstruct),
        ("x IN (SELECT y FROM t)", Code::UnsupportedConstruct),
        ("EXISTS (SELECT 1)", Code::UnsupportedConstruct),
        ("a.b.c", Code::UnsupportedConstruct),
        ("x IS TRUE", Code::UnsupportedConstruct),
        ("x SIMILAR TO 'a'", Code::UnsupportedConstruct),
        ("x + ?", Code::UnsupportedConstruct),
        ("CAST(x AS JSON)", Code::UnsupportedConstruct),
        ("x & 1", Code::UnsupportedConstruct),
        ("my_udf(x)", Code::UnknownFunction),
        ("pg_catalog.lower(x)", Code::UnsupportedConstruct),
        ("a; DROP TABLE t", Code::ExpressionSyntax),
        ("a b", Code::ExpressionSyntax),
        ("", Code::ExpressionSyntax),
        ("'unterminated", Code::ExpressionSyntax),
    ];
    for (text, code) in cases {
        assert_eq!(rejected(text, Context::Field), code, "{text}");
    }
}

#[test]
fn fields_are_row_level_and_aggregates_do_not_nest() {
    assert_eq!(
        rejected("SUM(amount)", Context::Field),
        Code::AggregateInField
    );
    assert_eq!(
        rejected("ROW_NUMBER() OVER (ORDER BY x)", Context::Field),
        Code::AggregateInField
    );
    assert_eq!(
        rejected("SUM(MAX(x))", Context::Metric),
        Code::NestedAggregate
    );
    assert_eq!(
        rejected("ROW_NUMBER()", Context::Metric),
        Code::UnsupportedConstruct
    );
    assert_eq!(
        rejected("SUM(x) FILTER (WHERE y > 0)", Context::Metric),
        Code::UnsupportedConstruct
    );
}

#[test]
fn deep_nesting_is_a_diagnostic_not_a_crash() {
    let deep = format!("{}1{}", "(".repeat(10_000), ")".repeat(10_000));
    assert_eq!(rejected(&deep, Context::Field), Code::ExpressionTooLarge);
}

proptest! {
    #[test]
    fn arbitrary_input_never_panics(text in any::<String>(), metric in any::<bool>()) {
        let context = if metric { Context::Metric } else { Context::Field };
        let _ = analyze(&text, context);
    }

    #[test]
    fn analysis_is_stable_across_reprinting(
        terms in prop::collection::vec(
            prop::sample::select(vec![
                "o.amount", "\"Exact\"", "COALESCE(x, 0)", "SUM(o.amount)", "COUNT(DISTINCT c.id)",
                "MIN(o.x)", "CASE WHEN y > 1 THEN 2 ELSE 3 END", "1.5",
            ]),
            1..6,
        ),
        operators in prop::collection::vec(prop::sample::select(vec![" + ", " - ", " * ", " / "]), 5),
    ) {
        let mut text = terms[0].to_owned();
        for (term, op) in terms[1..].iter().zip(&operators) {
            text.push_str(op);
            text.push_str(term);
        }
        let first = analyze(&text, Context::Metric).unwrap();
        let wrapped = analyze(&format!("({text})"), Context::Metric).unwrap();
        prop_assert_eq!(first, wrapped);
    }
}
