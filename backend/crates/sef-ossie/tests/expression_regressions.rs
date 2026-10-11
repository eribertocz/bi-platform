//! Regressions for the eight findings in the single review of PR #54.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::process::Command;

use proptest::prelude::*;
use sef_ossie::expression::{
    AggregateClass, AggregatePhase, Context, MAX_EXPRESSION_TOKENS, Reference, analyze,
};
use sef_ossie::{Code, load, to_json};

fn accepted(text: &str, context: Context) -> sef_ossie::expression::Analysis {
    analyze(text, context).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn rejected(text: &str, code: Code) {
    assert_eq!(
        analyze(text, Context::Metric).unwrap_err().code,
        code,
        "{text}"
    );
}

fn refs(names: &[(&str, &str)]) -> BTreeSet<Reference> {
    names
        .iter()
        .map(|(dataset, name)| Reference {
            qualifier: Some((*dataset).to_owned()),
            name: (*name).to_owned(),
        })
        .collect()
}

#[test]
fn token_budget_protects_parse_walk_and_drop_in_an_isolated_process() {
    const CHILD: &str = "SEF_OSSIE_TOKEN_BUDGET_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "token_budget_protects_parse_walk_and_drop_in_an_isolated_process",
                "--nocapture",
            ])
            .env(CHILD, "1")
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
    for (term, op) in [("1", " + "), ("TRUE", " AND "), ("'x'", " || ")] {
        eprintln!("isolated token budget: {op}");
        rejected(&vec![term; 10_000].join(op), Code::ExpressionSyntax);
        // Exercise the maximum accepted left-deep AST and its destruction.
        accepted(
            &vec![term; MAX_EXPRESSION_TOKENS / 2].join(op),
            Context::Field,
        );
        rejected(
            &vec![term; MAX_EXPRESSION_TOKENS / 2 + 1].join(op),
            Code::ExpressionSyntax,
        );
    }
    for text in [
        format!("{}1{}", "(".repeat(10_000), ")".repeat(10_000)),
        format!("{}TRUE", "NOT ".repeat(10_000)),
        format!("{}1{}", "ABS(".repeat(40), ")".repeat(40)),
    ] {
        rejected(&text, Code::ExpressionSyntax);
    }
    // The budget is lexical: operators inside strings/comments do not count.
    accepted(
        &format!("'{}' /* {} */", "+".repeat(10_000), "AND ".repeat(10_000)),
        Context::Field,
    );
}

#[test]
fn every_window_frame_bound_is_visited_and_validated() {
    for frame in [
        "ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW",
        "ROWS BETWEEN 6 PRECEDING AND 2 FOLLOWING",
        "RANGE BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW",
        "ROWS 6 PRECEDING",
        "ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING",
    ] {
        accepted(
            &format!("SUM(o.x) OVER (ORDER BY o.y {frame})"),
            Context::Metric,
        );
    }
    for (bound, code) in [
        ("(SELECT 1)", Code::UnsupportedConstruct),
        ("?", Code::UnsupportedConstruct),
        ("my_udf(1)", Code::UnknownFunction),
        ("-1", Code::UnsupportedConstruct),
        ("1.5", Code::UnsupportedConstruct),
        ("o.n", Code::UnsupportedConstruct),
    ] {
        for frame in [
            format!("ROWS BETWEEN {bound} PRECEDING AND CURRENT ROW"),
            format!("ROWS BETWEEN CURRENT ROW AND {bound} FOLLOWING"),
        ] {
            rejected(&format!("SUM(o.x) OVER (ORDER BY o.y {frame})"), code);
        }
    }
    for text in [
        "SUM(x) OVER w",
        "SUM(x) OVER (w ORDER BY y)",
        "SUM(x) OVER (ORDER BY y GROUPS BETWEEN 1 PRECEDING AND CURRENT ROW)",
        "SUM(x) OVER (ROWS BETWEEN UNBOUNDED FOLLOWING AND CURRENT ROW)",
        "SUM(x) OVER (ROWS BETWEEN CURRENT ROW AND UNBOUNDED PRECEDING)",
        "SUM(x) OVER (ROWS BETWEEN 1 FOLLOWING AND CURRENT ROW)",
        "SUM(x) OVER (ROWS BETWEEN 1 PRECEDING AND 2 PRECEDING)",
    ] {
        rejected(text, Code::UnsupportedConstruct);
    }
}

#[test]
fn window_aggregates_preserve_phase_classification_and_all_dependencies() {
    for suffix in ["", " OVER ()"] {
        let result = accepted(
            &format!("PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY o.amount){suffix}"),
            Context::Metric,
        );
        assert_eq!(result.references, refs(&[("O", "AMOUNT")]));
        assert_eq!(result.aggregates[0].references, result.references);
        assert_eq!(result.aggregates[0].class, AggregateClass::Holistic);
        assert_eq!(
            result.aggregates[0].phase,
            if suffix.is_empty() {
                AggregatePhase::Group
            } else {
                AggregatePhase::Window
            }
        );
    }
    for (call, class, distinct, safe) in [
        ("SUM(o.amount)", AggregateClass::Distributive, false, false),
        ("MIN(o.amount)", AggregateClass::Distributive, false, true),
        (
            "COUNT(DISTINCT o.amount)",
            AggregateClass::Holistic,
            true,
            true,
        ),
    ] {
        let result = accepted(
            &format!(
                "{call} OVER (PARTITION BY o.region ORDER BY o.day ROWS BETWEEN 2 PRECEDING AND CURRENT ROW)"
            ),
            Context::Metric,
        );
        let aggregate = &result.aggregates[0];
        assert_eq!(
            (
                aggregate.phase,
                aggregate.class,
                aggregate.distinct,
                aggregate.fanout_safe
            ),
            (AggregatePhase::Window, class, distinct, safe)
        );
        assert_eq!(
            result.references,
            refs(&[("O", "AMOUNT"), ("O", "REGION"), ("O", "DAY")])
        );
        assert_eq!(aggregate.references, result.references);
    }
    let result = accepted(
        "SUM(SUM(o.amount)) OVER (PARTITION BY o.region ORDER BY SUM(o.tax))",
        Context::Metric,
    );
    assert_eq!(
        result
            .aggregates
            .iter()
            .map(|a| a.phase)
            .collect::<Vec<_>>(),
        [
            AggregatePhase::Group,
            AggregatePhase::Group,
            AggregatePhase::Window
        ]
    );
    assert_eq!(result.aggregates[0].references, refs(&[("O", "AMOUNT")]));
    assert_eq!(
        result.aggregates[2].references,
        refs(&[("O", "AMOUNT"), ("O", "REGION"), ("O", "TAX")])
    );
    accepted(
        "LAG(SUM(o.amount)) OVER (ORDER BY o.region)",
        Context::Metric,
    );
    for text in [
        "SUM(SUM(o.amount) OVER ())",
        "SUM(SUM(SUM(o.amount))) OVER ()",
        "SUM(SUM(o.amount) OVER ()) OVER ()",
        "LAG(SUM(o.amount) OVER ()) OVER ()",
        "SUM(o.x) OVER (ORDER BY SUM(o.y) OVER ())",
    ] {
        rejected(text, Code::NestedAggregate);
    }
}

#[test]
fn all_required_temporal_construction_forms_are_accepted() {
    for text in [
        "DATE '2024-01-15'",
        "TIME '10:30:00'",
        "TIMESTAMP_NTZ '2024-01-15 10:30:00'",
        "TIMESTAMP '2024-01-15 10:30:00'",
        "CAST('2024-01-15' AS DATE)",
        "CAST('10:30:00' AS TIME)",
        "CAST('2024-01-15 10:30:00' AS TIMESTAMP_NTZ)",
        "TO_DATE('2024-01-15')",
        "TO_TIMESTAMP('2024-01-15 10:30:00')",
    ] {
        for context in [Context::Field, Context::Metric] {
            let analysis = accepted(text, context);
            assert!(analysis.references.is_empty());
        }
    }
    for text in ["JSON '{\"x\":1}'", "{d '2024-01-15'}"] {
        rejected(text, Code::UnsupportedConstruct);
    }
}

#[test]
fn unary_signatures_require_exactly_one_operand() {
    for names in [
        &[
            "SUM",
            "COUNT",
            "AVG",
            "MIN",
            "MAX",
            "STDDEV",
            "STDDEV_POP",
            "STDDEV_SAMP",
            "VARIANCE",
            "VAR_POP",
            "VAR_SAMP",
            "MEDIAN",
            "APPROX_COUNT_DISTINCT",
        ][..],
        &[
            "YEAR",
            "QUARTER",
            "MONTH",
            "DAY",
            "DAYOFYEAR",
            "HOUR",
            "MINUTE",
            "SECOND",
            "TO_DATE",
            "TO_TIMESTAMP",
            "LENGTH",
            "LOWER",
            "UPPER",
            "TRIM",
            "LTRIM",
            "RTRIM",
            "ABS",
            "FLOOR",
            "CEIL",
            "CEILING",
            "SIGN",
            "SQRT",
            "EXP",
            "LN",
            "LOG10",
            "SIN",
            "COS",
            "TAN",
            "ASIN",
            "ACOS",
            "ATAN",
            "RADIANS",
            "DEGREES",
            "ZEROIFNULL",
            "NULLIFZERO",
        ][..],
    ] {
        for name in names {
            accepted(&format!("{name}(x)"), Context::Metric);
            assert!(
                analyze(&format!("{name}()"), Context::Metric).is_err(),
                "{name}()"
            );
            assert!(
                analyze(&format!("{name}(x, y)"), Context::Metric).is_err(),
                "{name}(x, y)"
            );
        }
    }
}

#[test]
fn scalar_signatures_validate_fixed_and_optional_arities() {
    for name in [
        "APPROX_PERCENTILE",
        "LEFT",
        "RIGHT",
        "CHARINDEX",
        "CONTAINS",
        "STARTSWITH",
        "ENDSWITH",
        "REGEXP_LIKE",
        "REGEXP_EXTRACT",
        "REGEXP_COUNT",
        "MOD",
        "POWER",
        "LOG",
        "ATAN2",
        "NULLIF",
        "IFNULL",
        "NVL",
    ] {
        accepted(&format!("{name}(x, y)"), Context::Metric);
        rejected(&format!("{name}(x)"), Code::UnsupportedConstruct);
        rejected(&format!("{name}(x, y, z)"), Code::UnsupportedConstruct);
    }
    for name in [
        "REPLACE",
        "SPLIT_PART",
        "REGEXP_REPLACE",
        "IF",
        "IFF",
        "NVL2",
    ] {
        accepted(&format!("{name}(x, y, z)"), Context::Metric);
        rejected(&format!("{name}(x, y)"), Code::UnsupportedConstruct);
        rejected(&format!("{name}(x, y, z, w)"), Code::UnsupportedConstruct);
    }
    for name in ["CURRENT_DATE", "CURRENT_TIME", "CURRENT_TIMESTAMP", "PI"] {
        accepted(&format!("{name}()"), Context::Field);
        rejected(&format!("{name}(x)"), Code::UnsupportedConstruct);
    }
    for name in ["ROUND", "TRUNC", "TRUNCATE"] {
        for args in ["x", "x, 2"] {
            accepted(&format!("{name}({args})"), Context::Field);
        }
        rejected(&format!("{name}()"), Code::UnsupportedConstruct);
        rejected(&format!("{name}(x, 2, 3)"), Code::UnsupportedConstruct);
    }
    for name in ["COALESCE", "CONCAT", "GREATEST", "LEAST"] {
        for args in ["x, y", "x, y, z"] {
            accepted(&format!("{name}({args})"), Context::Field);
        }
        rejected(&format!("{name}(x)"), Code::UnsupportedConstruct);
    }
}

#[test]
fn window_signatures_validate_ranking_and_offset_arities() {
    for name in [
        "ROW_NUMBER",
        "RANK",
        "DENSE_RANK",
        "PERCENT_RANK",
        "CUME_DIST",
    ] {
        accepted(&format!("{name}() OVER ()"), Context::Metric);
        rejected(&format!("{name}(x) OVER ()"), Code::UnsupportedConstruct);
    }
    for (name, args) in [
        ("NTILE", "4"),
        ("FIRST_VALUE", "x"),
        ("LAST_VALUE", "x"),
        ("NTH_VALUE", "x, 2"),
    ] {
        accepted(&format!("{name}({args}) OVER ()"), Context::Metric);
        rejected(&format!("{name}() OVER ()"), Code::UnsupportedConstruct);
        rejected(
            &format!("{name}({args}, y) OVER ()"),
            Code::UnsupportedConstruct,
        );
    }
    for name in ["LAG", "LEAD"] {
        for args in ["x", "x, 1", "x, 1, 0"] {
            accepted(&format!("{name}({args}) OVER ()"), Context::Metric);
        }
        rejected(&format!("{name}() OVER ()"), Code::UnsupportedConstruct);
        rejected(
            &format!("{name}(x, 1, 0, y) OVER ()"),
            Code::UnsupportedConstruct,
        );
    }
}

#[test]
fn aggregate_signatures_validate_modifiers_wildcards_and_ordering() {
    for name in ["PERCENTILE_CONT", "PERCENTILE_DISC"] {
        accepted(
            &format!("{name}(0.5) WITHIN GROUP (ORDER BY x)"),
            Context::Metric,
        );
        for args in ["", "0.5, x"] {
            rejected(
                &format!("{name}({args}) WITHIN GROUP (ORDER BY x)"),
                Code::UnsupportedConstruct,
            );
        }
        rejected(&format!("{name}(0.5)"), Code::UnsupportedConstruct);
        rejected(&format!("{name}(0.5) OVER ()"), Code::UnsupportedConstruct);
        rejected(
            &format!("{name}(0.5) WITHIN GROUP (ORDER BY x, y)"),
            Code::UnsupportedConstruct,
        );
    }
    for text in [
        "COUNT(*)",
        "COUNT(ALL x)",
        "SUM(DISTINCT x)",
        "COUNT(DISTINCT x)",
        "SUM(DISTINCT x) OVER ()",
        "COUNT(DISTINCT x) OVER ()",
        "SUBSTRING(x, 1, 3)",
        "POSITION('a' IN x)",
    ] {
        accepted(text, Context::Metric);
    }
    for text in [
        "ABS(DISTINCT x)",
        "COUNT(DISTINCT *)",
        "SUM(*)",
        "COUNT(t.*)",
        "AVG(DISTINCT x)",
        "APPROX_PERCENTILE(DISTINCT x, 0.5)",
        "ABS(x) WITHIN GROUP (ORDER BY y)",
        "SUM(x) WITHIN GROUP (ORDER BY y) OVER ()",
        "LAG(DISTINCT x) OVER ()",
        "COUNT(x ORDER BY y)",
        "SUM(x => y)",
        "CEIL(x, 2)",
        "FLOOR(x TO DAY)",
        "SUBSTRING(x)",
        "SUBSTRING(x, 1)",
        "TRIM(x, 'a')",
        "TRIM(x, my_udf(y))",
    ] {
        rejected(text, Code::UnsupportedConstruct);
    }
}

#[test]
fn temporal_units_are_constants_and_never_column_references() {
    for part in [
        "year", "quarter", "month", "week", "day", "hour", "minute", "second",
    ] {
        for (name, rest) in [
            ("DATEADD", "7, o.ordered_at"),
            ("DATEDIFF", "o.start_at, o.ordered_at"),
        ] {
            for unit in [part.to_owned(), format!("'{part}'"), part.to_uppercase()] {
                let result = accepted(&format!("{name}({unit}, {rest})"), Context::Field);
                let expected = if name == "DATEADD" {
                    refs(&[("O", "ORDERED_AT")])
                } else {
                    refs(&[("O", "START_AT"), ("O", "ORDERED_AT")])
                };
                assert_eq!(result.references, expected);
            }
            rejected(&format!("{name}({part}, o.x)"), Code::UnsupportedConstruct);
            rejected(
                &format!("{name}({part}, o.x, o.y, o.z)"),
                Code::UnsupportedConstruct,
            );
        }
        assert_eq!(
            accepted(
                &format!("DATE_TRUNC('{part}', o.ordered_at)"),
                Context::Field
            )
            .references,
            refs(&[("O", "ORDERED_AT")])
        );
    }
    for part in [
        "year",
        "quarter",
        "month",
        "week",
        "day",
        "dayofweek",
        "dayofyear",
        "hour",
        "minute",
        "second",
        "millisecond",
    ] {
        for text in [
            format!("DATE_PART('{part}', o.x)"),
            format!("EXTRACT({part} FROM o.x)"),
        ] {
            assert_eq!(
                accepted(&text, Context::Field).references,
                refs(&[("O", "X")])
            );
        }
    }
    for text in [
        "DATEADD(fortnight, 7, o.x)",
        "DATEDIFF(millisecond, o.x, o.y)",
        "DATEADD(o.day, 7, o.x)",
        "DATEADD(\"day\", 7, o.x)",
        "DATEADD(?, 7, o.x)",
        "DATEADD(LOWER('day'), 7, o.x)",
        "DATE_TRUNC('dayofyear', o.x)",
        "DATE_PART('epoch', o.x)",
        "EXTRACT(EPOCH FROM o.x)",
    ] {
        rejected(text, Code::UnsupportedConstruct);
    }
}

#[test]
fn identifiers_require_ansi_quotes_and_128_characters_per_part() {
    for text in ["`x`", "`o`.`x`", "`ABS`(x)", "[x]", "bad$name"] {
        assert!(analyze(text, Context::Field).is_err(), "{text}");
    }
    let result = accepted(r#""o"."a""b" + "Exact Name""#, Context::Field);
    assert!(result.references.contains(&Reference {
        qualifier: Some("o".to_owned()),
        name: "a\"b".to_owned()
    }));
    for c in ["a", "é"] {
        for (length, valid) in [(128, true), (129, false)] {
            let name = c.repeat(length);
            for text in [
                name.clone(),
                format!("o.{name}"),
                format!("{name}.x"),
                format!("\"{name}\""),
                format!("\"{name}\".x"),
            ] {
                assert_eq!(analyze(&text, Context::Field).is_ok(), valid, "{text}");
            }
        }
    }
    accepted(&format!("\"{}\"", "\"\"".repeat(128)), Context::Field);
    rejected(
        &format!("\"{}\"", "\"\"".repeat(129)),
        Code::InvalidIdentifier,
    );
    accepted("'`not an identifier`'", Context::Field);
}

#[test]
fn ansi_sql_is_preserved_as_opaque_and_ossie_is_checked_at_its_path() {
    let text = "version: 0.2.0.dev0\nname: m\ndatasets:\n  - name: d\n    source: t\n    fields:\n      - name: x\n        expression:\n          dialects:\n            - dialect: ANSI_SQL\n              expression: x IS TRUE\n";
    let loaded = load(text).unwrap();
    let exported = to_json(&loaded.model).unwrap();
    assert_eq!(load(&exported).unwrap().model, loaded.model);
    assert_eq!(
        loaded.model.datasets[0].fields[0].expression.dialects[0].expression,
        "x IS TRUE"
    );
    let errors = load(&text.replace("ANSI_SQL", "OSSIE_SQL_2026")).unwrap_err();
    assert_eq!(errors[0].code, Code::UnsupportedConstruct);
    assert_eq!(
        errors[0].path,
        "/datasets/0/fields/0/expression/dialects/0/expression"
    );
}

proptest! {
    #[test]
    fn structured_operator_chains_are_bounded(terms in 1usize..300, operator in prop::sample::select(vec![" + ", " AND ", " || "])) {
        let term = match operator { " AND " => "TRUE", " || " => "'x'", _ => "1" };
        let text = vec![term; terms].join(operator);
        let result = analyze(&text, Context::Field);
        prop_assert_eq!(result.is_ok(), terms * 2 - 1 <= MAX_EXPRESSION_TOKENS);
    }
}
