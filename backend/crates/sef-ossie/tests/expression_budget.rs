//! Resource budgets and a reproducible, opt-in latency measurement.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use sef_ossie::expression::{
    Context, MAX_EXPRESSION_RECURSION, MAX_EXPRESSION_STRUCTURAL_TOKENS, MAX_EXPRESSION_TOKENS,
    Reference, analyze,
};
use sef_ossie::{Code, load};
use std::fmt::Write;
use std::process::Command;
use std::time::Instant;

fn mapping(branches: usize) -> String {
    let mut text = String::from("CASE country");
    for i in 0..branches {
        write!(text, " WHEN {i} THEN 'region{i}'").unwrap();
    }
    text.push_str(" END");
    text
}

fn literal_list(values: usize) -> String {
    format!("country IN ({})", vec!["1"; values].join(","))
}

fn too_large(text: &str, limit: usize, unit: &str) {
    let error = analyze(text, Context::Field).unwrap_err();
    assert_eq!(error.code, Code::ExpressionTooLarge);
    assert_eq!(error.code.as_str(), "expression_too_large");
    assert!(
        error.message.contains(&format!("{limit} {unit}")),
        "{error}"
    );
    assert!(error.message.contains("varios campos"), "{error}");
    assert!(error.message.contains("mapeo a una tabla"), "{error}");
}

#[test]
fn real_mappings_and_literal_lists_preserve_references() {
    let country = Reference {
        qualifier: None,
        name: "COUNTRY".to_owned(),
    };
    let simple = analyze(&mapping(200), Context::Field).unwrap();
    assert_eq!(simple.references, [country.clone()].into_iter().collect());
    let mut branches = String::new();
    for i in 0..200 {
        write!(branches, " WHEN country = 'code{i}' THEN region_name").unwrap();
    }
    let searched = analyze(&format!("CASE{branches} ELSE NULL END"), Context::Field).unwrap();
    assert_eq!(
        searched.references,
        [
            country.clone(),
            Reference {
                qualifier: None,
                name: "REGION_NAME".to_owned(),
            }
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(
        analyze(&literal_list(500), Context::Field)
            .unwrap()
            .references,
        [country].into_iter().collect()
    );
    let metric = analyze(&format!("SUM({})", mapping(200)), Context::Metric).unwrap();
    assert_eq!(metric.aggregates.len(), 1);
    assert_eq!(metric.aggregates[0].references, simple.references);
    for text in [
        "CASE o.x WHEN -1 THEN o.y WHEN +2 THEN NULL ELSE FALSE END",
        "CASE WHEN o.x >= -1 THEN 'a' WHEN o.x <> +2 THEN 'b' END",
        "x NOT IN (-1, +2, NULL, TRUE, FALSE, 'CASE WHEN THEN END')",
        "CASE \"WHEN\" WHEN 1 THEN \"THEN\" WHEN 2 THEN \"ELSE\" END",
    ] {
        analyze(text, Context::Field).unwrap();
    }
}

#[test]
fn each_budget_has_its_own_truthful_diagnostic_and_document_path() {
    // Largest flat mapping under 4096 tokens; one more branch exceeds width.
    analyze(&mapping(1023), Context::Field).unwrap();
    analyze(&format!("+{}", mapping(1023)), Context::Field).unwrap(); // Exactly 4096.
    too_large(
        &format!("++{}", mapping(1023)),
        MAX_EXPRESSION_TOKENS,
        "tokens significativos",
    );
    too_large(
        &mapping(1024),
        MAX_EXPRESSION_TOKENS,
        "tokens significativos",
    );
    analyze(&literal_list(2046), Context::Field).unwrap();
    too_large(
        &literal_list(2047),
        MAX_EXPRESSION_TOKENS,
        "tokens significativos",
    );
    too_large(
        &vec!["1"; 129].join(" + "),
        MAX_EXPRESSION_STRUCTURAL_TOKENS,
        "tokens estructurales",
    );
    too_large(
        &format!("{}1{}", "(".repeat(40), ")".repeat(40)),
        MAX_EXPRESSION_RECURSION,
        "niveles de recursión",
    );
    too_large(
        &format!(
            "{}1{}",
            "CASE WHEN TRUE THEN ".repeat(20),
            " ELSE 0 END".repeat(20)
        ),
        MAX_EXPRESSION_RECURSION,
        "niveles de recursión",
    );
    too_large(
        &format!("{}TRUE", "NOT ".repeat(40)),
        MAX_EXPRESSION_RECURSION,
        "niveles de recursión",
    );
    let max_bytes = sef_ossie::load::MAX_DOCUMENT_BYTES;
    too_large(&" ".repeat(max_bytes + 1), max_bytes, "bytes");
    let document = format!(
        "version: 0.2.0.dev0\nname: m\ndatasets:\n  - name: d\n    source: t\n    fields:\n      - name: x\n        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: '{}'\n",
        vec!["1"; 129].join(" + ")
    );
    let errors = load(&document).unwrap_err();
    assert_eq!(errors[0].code, Code::ExpressionTooLarge);
    assert_eq!(
        errors[0].path,
        "/datasets/0/fields/0/expression/dialects/0/expression"
    );
    for text in [
        "CASE x WHEN 1 THEN END",
        "x IN (1,)",
        "1 +",
        "'unclosed",
        "CASE WHEN TRUE THEN",
    ] {
        assert_eq!(
            analyze(text, Context::Field).unwrap_err().code,
            Code::ExpressionSyntax,
            "{text}"
        );
    }
}

#[test]
fn attacks_and_admitted_boundaries_run_in_an_isolated_process() {
    const CHILD: &str = "SEF_OSSIE_EXPRESSION_BUDGET_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "attacks_and_admitted_boundaries_run_in_an_isolated_process",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env_remove("RUST_MIN_STACK")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{:?}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    for (label, text) in [
        ("10000 operators", vec!["1"; 10_001].join(" + ")),
        (
            "10000 parentheses",
            format!("{}1{}", "(".repeat(10_000), ")".repeat(10_000)),
        ),
        ("10000 NOT", format!("{}TRUE", "NOT ".repeat(10_000))),
        (
            "CASE recursion recovered as identifier",
            format!(
                "{}1{}",
                "CASE WHEN TRUE THEN ".repeat(20),
                " ELSE 0 END".repeat(20)
            ),
        ),
        (
            "NOT recursion recovered as identifier",
            format!("{}TRUE", "NOT ".repeat(40)),
        ),
        (
            "NOT recursion inside CASE",
            format!(
                "CASE WHEN TRUE THEN {}TRUE ELSE FALSE END",
                "NOT ".repeat(40)
            ),
        ),
        (
            "1000 nested CASE",
            format!(
                "{}1{}",
                "CASE WHEN TRUE THEN ".repeat(1000),
                " ELSE 0 END".repeat(1000)
            ),
        ),
        // Inside the total token budget: these must hit the depth budget.
        (
            "nested CASE under token cap",
            format!(
                "{}1{}",
                "CASE WHEN TRUE THEN ".repeat(400),
                " ELSE 0 END".repeat(400)
            ),
        ),
        (
            "nested unary under token cap",
            format!("{}TRUE", "NOT ".repeat(300)),
        ),
        ("chain of shallow CASE", vec![mapping(1); 200].join(" + ")),
        (
            "chain of IN lists",
            vec![literal_list(1); 200].join(" AND "),
        ),
        (
            "keyword identifiers in CASE",
            format!("CASE WHEN {} THEN 1 END", vec!["when"; 300].join(" + ")),
        ),
        (
            "deep IN element",
            format!("x IN ({}, 2)", vec!["1"; 300].join(" + ")),
        ),
        (
            "deep CASE result",
            format!(
                "CASE WHEN TRUE THEN {} ELSE 0 END",
                vec!["1"; 300].join(" + ")
            ),
        ),
    ] {
        eprintln!("isolated: {label}");
        assert_eq!(
            analyze(&text, Context::Field).unwrap_err().code,
            Code::ExpressionTooLarge,
            "{label}"
        );
    }
    admitted_boundaries();
}

fn admitted_boundaries() {
    // Maximum width plus near-maximum structural cost: walk and drop must
    // remain safe together, not only when measured independently.
    let text = format!("{} + {}", vec!["1"; 124].join(" + "), mapping(900));
    analyze(&text, Context::Field).unwrap();
    let text = format!(
        "{} AND {}",
        vec!["TRUE"; 125].join(" AND "),
        literal_list(1800)
    );
    analyze(&text, Context::Field).unwrap();
    analyze(&mapping(1023), Context::Field).unwrap();
    analyze(&format!("+{}", mapping(1023)), Context::Field).unwrap();
    analyze(&literal_list(2046), Context::Field).unwrap();
    // Identical certified CASE fragments can create a left-deep tree too.
    analyze(&vec![mapping(1); 32].join(" + "), Context::Field).unwrap();
}

#[test]
fn width_discounts_do_not_hide_language_violations() {
    for name in ["case", "not", "when", "then", "else"] {
        analyze(name, Context::Field).unwrap();
    }
    for (text, code) in [
        (
            "CASE x WHEN 1 THEN (SELECT 1) END",
            Code::UnsupportedConstruct,
        ),
        ("x IN (1, (SELECT 1))", Code::UnsupportedConstruct),
        ("CASE x WHEN 1 THEN SUM(y) END", Code::AggregateInField),
        ("CASE x WHEN 1 THEN unknown(y) END", Code::UnknownFunction),
        ("x IN (1, ?)", Code::UnsupportedConstruct),
        ("CASE `x` WHEN 1 THEN 2 END", Code::UnsupportedConstruct),
    ] {
        assert_eq!(
            analyze(text, Context::Field).unwrap_err().code,
            code,
            "{text}"
        );
    }
    // Quoted delimiters, strings and comments cannot reset the accounting.
    for term in [
        "\"CASE\"",
        "'WHEN THEN ELSE END'",
        "when",
        "then",
        "else",
        "o.case",
    ] {
        too_large(
            &vec![term; 300].join(" + "),
            MAX_EXPRESSION_STRUCTURAL_TOKENS,
            "tokens estructurales",
        );
    }
    analyze(
        &format!(
            "'{}' /* {} */",
            "+".repeat(10_000),
            "CASE WHEN ".repeat(500)
        ),
        Context::Field,
    )
    .unwrap();
}

proptest! {
    #[test]
    fn valid_nesting_errors_are_resources_not_syntax(depth in 1usize..50) {
        for text in [
            format!("{}1{}", "CASE WHEN TRUE THEN ".repeat(depth), " ELSE 0 END".repeat(depth)),
            format!("{}TRUE", "NOT ".repeat(depth)),
            format!("CASE WHEN TRUE THEN {}TRUE ELSE FALSE END", "NOT ".repeat(depth)),
        ] {
            if let Err(error) = analyze(&text, Context::Field) {
                prop_assert_eq!(error.code, Code::ExpressionTooLarge);
            }
        }
    }

    #[test]
    fn bounded_unicode_input_never_panics(
        characters in prop::collection::vec(any::<char>(), 0..=4096),
        metric in any::<bool>(),
    ) {
        let text: String = characters.into_iter().collect();
        let _ = analyze(&text, if metric { Context::Metric } else { Context::Field });
    }

    #[test]
    fn bounded_token_input_never_panics(
        tokens in prop::collection::vec(prop::sample::select(vec![
            "CASE", "WHEN", "THEN", "ELSE", "END", "IN", "NOT", "(", ")", ",",
            "1", "'x'", "NULL", "x", "\"WHEN\"", "+", "=", "SELECT", "WITH", ".",
        ]), 0..=MAX_EXPRESSION_TOKENS),
        metric in any::<bool>(),
    ) {
        let _ = analyze(&tokens.join(" "), if metric { Context::Metric } else { Context::Field });
    }

    #[test]
    fn flat_mapping_width_is_bounded(branches in 1usize..1200) {
        let result = analyze(&mapping(branches), Context::Field);
        prop_assert_eq!(result.is_ok(), branches * 4 + 3 <= MAX_EXPRESSION_TOKENS);
        if let Err(error) = result { prop_assert_eq!(error.code, Code::ExpressionTooLarge); }
    }

    #[test]
    fn literal_list_width_is_bounded(values in 1usize..2200) {
        let result = analyze(&literal_list(values), Context::Field);
        prop_assert_eq!(result.is_ok(), values * 2 + 3 <= MAX_EXPRESSION_TOKENS);
        if let Err(error) = result { prop_assert_eq!(error.code, Code::ExpressionTooLarge); }
    }
}

#[test]
#[ignore = "latency measurement: run with --ignored --nocapture --test-threads=1"]
fn measure_expression_budget() {
    for (label, text) in [
        ("old_max_tokens", format!("+{}", mapping(63))),
        ("new_max_tokens", format!("+{}", mapping(1023))),
        ("required_case", mapping(200)),
        (
            "required_in",
            format!("country IN ({})", vec!["1"; 500].join(",")),
        ),
    ] {
        let accepted = analyze(&text, Context::Field).is_ok();
        for _ in 0..100 {
            std::hint::black_box(analyze(&text, Context::Field)).ok();
        }
        let mut samples = Vec::new();
        for _ in 0..1000 {
            let start = Instant::now();
            std::hint::black_box(analyze(&text, Context::Field)).ok();
            samples.push(start.elapsed().as_nanos());
        }
        samples.sort_unstable();
        println!(
            "{label}: bytes={} accepted={accepted} n={} p50_ns={} p95_ns={} p99_ns={}",
            text.len(),
            samples.len(),
            samples[500],
            samples[950],
            samples[990]
        );
    }
}
