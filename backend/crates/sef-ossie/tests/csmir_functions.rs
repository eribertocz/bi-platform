//! `OSSIE_SQL_2026` REQUIRED scalar functions to CSMIR (#77): exact types,
//! refusals with their reason, and the shared typing rule in validation.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use proptest::prelude::*;
use sef_model_ir::expressions::{BooleanOp, ScalarExpr, ScalarExprKind, UnaryOp};
use sef_model_ir::functions::ScalarFunction;
use sef_model_ir::identity::SourceHandle;
use sef_model_ir::model::ModelDefinition;
use sef_model_ir::time::TimestampSemantics;
use sef_model_ir::types::{DecimalType, ValueType, ZeroDivisionPolicy};
use sef_ossie::catalog::{Catalog, Column, Relation};
use sef_ossie::csmir::{Options, to_csmir};
use sef_ossie::document::{Dialect, DialectExpression, Expression, Field, SemanticModel};
use sef_ossie::{Code, Diagnostic};

fn read(file: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(file);
    std::fs::read_to_string(path).expect("fixture")
}

fn document(case: u8) -> SemanticModel {
    sef_ossie::load(&read(&format!("CASE-{case:03}.yaml")))
        .expect("corpus document")
        .model
}

fn value_type(spec: &str) -> (ValueType, bool) {
    let (spec, nullable) = spec
        .strip_suffix(" null")
        .map_or((spec, false), |spec| (spec, true));
    let value_type = match spec {
        "int64" => ValueType::Integer {
            signed: true,
            bits: Some(64),
        },
        "utf8" => ValueType::Utf8,
        "date" => ValueType::Date,
        "timestamptz" => ValueType::Timestamp {
            timezone_semantics: TimestampSemantics::Instant,
        },
        decimal => {
            let (p, s) = decimal
                .strip_prefix("decimal(")
                .and_then(|rest| rest.strip_suffix(')'))
                .and_then(|rest| rest.split_once(','))
                .expect("decimal spec");
            ValueType::Decimal(
                DecimalType::new(p.parse().expect("p"), s.parse().expect("s")).expect("type"),
            )
        }
    };
    (value_type, nullable)
}

fn catalog(case: u8) -> Catalog {
    let relations: BTreeMap<String, BTreeMap<String, String>> =
        serde_saphyr::from_str(&read(&format!("CASE-{case:03}.catalog.yaml"))).expect("catalog");
    Catalog {
        relations: relations
            .into_iter()
            .map(|(name, columns)| {
                let relation = Relation {
                    source: SourceHandle::new("corpus").expect("handle"),
                    relation: SourceHandle::new(name.as_str()).expect("handle"),
                    columns: columns
                        .into_iter()
                        .map(|(column, spec)| {
                            let (value_type, nullable) = value_type(&spec);
                            Column {
                                handle: SourceHandle::new(column.as_str()).expect("handle"),
                                name: column,
                                value_type,
                                nullable,
                            }
                        })
                        .collect(),
                };
                (name, relation)
            })
            .collect(),
    }
}

fn options(case: u8) -> Options {
    Options {
        namespace: "corpus".to_owned(),
        model: format!("case-{case:03}"),
    }
}

fn convert(case: u8, model: &SemanticModel) -> Result<ModelDefinition, Vec<Diagnostic>> {
    to_csmir(model, &catalog(case), &options(case))
}

fn portable(text: &str) -> Expression {
    Expression {
        dialects: vec![DialectExpression {
            dialect: Dialect::OssieSql2026,
            expression: text.to_owned(),
        }],
    }
}

// ---- Scalar functions of OSSIE_SQL_2026 (#77, block 1) --------------------------

/// The dimension built from a computed field `probe` of CASE-001's `sales`
/// (`sale_id` int64, `region` utf8, `amount` decimal(18,2)).
fn field(text: &str) -> Result<ScalarExpr, Vec<Diagnostic>> {
    let mut model = document(1);
    model.datasets[0].fields.push(Field {
        name: "probe".to_owned(),
        expression: portable(text),
        dimension: Some(sef_ossie::document::Dimension { is_time: None }),
        label: None,
        description: None,
        datatype: None,
        ai_context: None,
        custom_extensions: Vec::new(),
    });
    let converted = convert(1, &model)?;
    Ok(converted
        .dimensions
        .iter()
        .find(|dimension| dimension.metadata.id().as_str().ends_with("sales.probe"))
        .expect("dimension")
        .bindings[0]
        .expression
        .clone())
}

/// The scalar argument of `SUM(text)` in CASE-001's metric.
fn summed(text: &str) -> Result<ScalarExpr, Vec<Diagnostic>> {
    let mut model = document(1);
    model.metrics[0].expression = portable(&format!("SUM({text})"));
    let converted = convert(1, &model)?;
    Ok(converted.measures[0]
        .scalar_expression
        .clone()
        .expect("scalar"))
}

fn function(expr: &ScalarExpr) -> ScalarFunction {
    match &expr.kind {
        ScalarExprKind::Function { function, .. } => *function,
        other => panic!("expected a function, got {other:?}"),
    }
}

fn decimal(precision: u8, scale: u8) -> ValueType {
    ValueType::Decimal(DecimalType::new(precision, scale).expect("decimal"))
}

const INT64: ValueType = ValueType::Integer {
    signed: true,
    bits: Some(64),
};

#[test]
fn text_functions_are_typed_exactly() {
    for (text, expected, value_type) in [
        (
            "CONCAT(region, '-', region)",
            ScalarFunction::Concat,
            ValueType::Utf8,
        ),
        ("region || '!'", ScalarFunction::Concat, ValueType::Utf8),
        ("LENGTH(region)", ScalarFunction::Length, INT64),
        ("LOWER(region)", ScalarFunction::Lower, ValueType::Utf8),
        ("UPPER(region)", ScalarFunction::Upper, ValueType::Utf8),
        ("TRIM(region)", ScalarFunction::Trim, ValueType::Utf8),
        (
            "TRIM(LEADING 'x' FROM region)",
            ScalarFunction::LTrim,
            ValueType::Utf8,
        ),
        (
            "TRIM(TRAILING 'x' FROM region)",
            ScalarFunction::RTrim,
            ValueType::Utf8,
        ),
        ("LTRIM(region)", ScalarFunction::LTrim, ValueType::Utf8),
        ("LEFT(region, 3)", ScalarFunction::Left, ValueType::Utf8),
        ("RIGHT(region, 2)", ScalarFunction::Right, ValueType::Utf8),
        (
            "SUBSTRING(region FROM 2 FOR 3)",
            ScalarFunction::Substring,
            ValueType::Utf8,
        ),
        (
            "SUBSTRING(region, 2, 3)",
            ScalarFunction::Substring,
            ValueType::Utf8,
        ),
        (
            "REPLACE(region, 'a', 'b')",
            ScalarFunction::Replace,
            ValueType::Utf8,
        ),
        (
            "SPLIT_PART(region, '-', -1)",
            ScalarFunction::SplitPart,
            ValueType::Utf8,
        ),
        ("POSITION('a' IN region)", ScalarFunction::Position, INT64),
        ("CHARINDEX('a', region)", ScalarFunction::Position, INT64),
        (
            "CONTAINS(region, 'n')",
            ScalarFunction::Contains,
            ValueType::Boolean,
        ),
        (
            "STARTSWITH(region, 'n')",
            ScalarFunction::StartsWith,
            ValueType::Boolean,
        ),
        (
            "ENDSWITH(region, 'h')",
            ScalarFunction::EndsWith,
            ValueType::Boolean,
        ),
        ("region LIKE 'n%'", ScalarFunction::Like, ValueType::Boolean),
        (
            "region ILIKE 'N_rth'",
            ScalarFunction::ILike,
            ValueType::Boolean,
        ),
    ] {
        let expr = field(text).unwrap_or_else(|errors| panic!("{text}: {errors:#?}"));
        assert_eq!(function(&expr), expected, "{text}");
        assert_eq!(expr.value_type, value_type, "{text}");
        assert_eq!(expr.unit, None, "{text}");
    }
}

#[test]
fn not_like_negates_the_match() {
    let expr = field("region NOT LIKE 'n%'").expect("converts");
    let ScalarExprKind::Unary { op, operand } = &expr.kind else {
        panic!("NOT");
    };
    assert_eq!(*op, UnaryOp::Not);
    assert_eq!(function(operand), ScalarFunction::Like);
}

#[test]
fn exact_math_keeps_decimals_exact() {
    for (text, expected, value_type) in [
        ("ABS(sales.amount)", ScalarFunction::Abs, decimal(18, 2)),
        ("FLOOR(sales.amount)", ScalarFunction::Floor, decimal(17, 0)),
        ("CEIL(sales.amount)", ScalarFunction::Ceil, decimal(17, 0)),
        (
            "CEILING(sales.amount)",
            ScalarFunction::Ceil,
            decimal(17, 0),
        ),
        // 16 integral digits, one decimal, and room for a carry (9.99 → 10.0).
        (
            "ROUND(sales.amount, 1)",
            ScalarFunction::Round,
            decimal(18, 1),
        ),
        ("ROUND(sales.amount)", ScalarFunction::Round, decimal(17, 0)),
        // Truncation cannot carry.
        (
            "TRUNC(sales.amount, 1)",
            ScalarFunction::Truncate,
            decimal(17, 1),
        ),
        // Asking for more digits than the type has changes nothing.
        (
            "ROUND(sales.amount, 4)",
            ScalarFunction::Round,
            decimal(18, 2),
        ),
        (
            "GREATEST(sales.amount, 0)",
            ScalarFunction::Greatest,
            decimal(18, 2),
        ),
        (
            "LEAST(sales.amount, 100.5, 7)",
            ScalarFunction::Least,
            decimal(18, 2),
        ),
        // |a mod 7| < 7: one integral digit, the dividend's scale.
        (
            "MOD(sales.amount, 7)",
            ScalarFunction::Mod {
                zero: ZeroDivisionPolicy::Null,
            },
            decimal(3, 2),
        ),
        (
            "sales.sale_id % 7",
            ScalarFunction::Mod {
                zero: ZeroDivisionPolicy::Null,
            },
            INT64,
        ),
    ] {
        let expr = summed(text).unwrap_or_else(|errors| panic!("{text}: {errors:#?}"));
        assert_eq!(function(&expr), expected, "{text}");
        assert_eq!(expr.value_type, value_type, "{text}");
    }
    let sign = summed("SIGN(sales.amount)").expect("converts");
    assert_eq!(sign.value_type, INT64);
}

#[test]
fn conditional_shorthands_become_case_and_null_safe_logic() {
    let nvl2 = field("NVL2(region, 'known', 'unknown')").expect("converts");
    assert!(matches!(nvl2.kind, ScalarExprKind::Case { .. }));
    assert_eq!(nvl2.value_type, ValueType::Utf8);
    let zero = summed("NULLIFZERO(sales.amount)").expect("converts");
    assert!(matches!(zero.kind, ScalarExprKind::Case { .. }));
    assert_eq!(zero.value_type, decimal(18, 2));
    // Never NULL: two NULLs are not distinct.
    let distinct = field("region IS DISTINCT FROM 'north'").expect("converts");
    let ScalarExprKind::Unary {
        op: UnaryOp::Not,
        operand,
    } = &distinct.kind
    else {
        panic!("NOT (… IS NOT DISTINCT FROM …)");
    };
    assert!(matches!(
        operand.kind,
        ScalarExprKind::BooleanLogic {
            op: BooleanOp::Or,
            ..
        }
    ));
    let same = field("region IS NOT DISTINCT FROM 'north'").expect("converts");
    assert_eq!(same.value_type, ValueType::Boolean);
}

#[test]
fn wrong_arguments_are_type_mismatches_with_the_function_named() {
    for text in [
        "UPPER(amount)",
        "LENGTH(sale_id)",
        "LEFT(region, 'x')",
        "CONCAT(region, amount)",
        "TRIM(BOTH 'xy' FROM region)",
        "STARTSWITH(region, 1)",
        "region LIKE 1",
    ] {
        let errors = field(text).expect_err(text);
        assert!(
            errors.iter().any(|e| e.code == Code::TypeMismatch),
            "{text}: {errors:#?}"
        );
    }
    for text in [
        "ABS(region)",
        "ROUND(sales.amount, sales.sale_id)",
        "ROUND(sales.amount, -1)",
        "GREATEST(sales.amount, sales.sale_id)",
        "MOD(sales.amount, region)",
    ] {
        let errors = summed(text).expect_err(text);
        assert!(
            errors
                .iter()
                .any(|e| e.code == Code::TypeMismatch || e.code == Code::NotRepresentable),
            "{text}: {errors:#?}"
        );
    }
}

#[test]
fn functions_with_unsettled_semantics_are_refused_with_the_reason() {
    for (text, reason) in [
        ("REGEXP_LIKE(region, 'n.*')", "expresiones regulares"),
        ("SQRT(sales.amount)", "coma flotante"),
        ("POWER(sales.amount, 2)", "coma flotante"),
        ("LN(sales.amount)", "coma flotante"),
    ] {
        let errors = summed(text)
            .err()
            .or_else(|| field(text).err())
            .unwrap_or_else(|| panic!("{text} must be refused"));
        assert!(
            errors
                .iter()
                .any(|e| e.code == Code::NotRepresentable && e.message.contains(reason)),
            "{text}: {errors:#?}"
        );
    }
}

#[test]
fn csmir_validation_applies_the_same_typing_rule() {
    // A function node whose declared type disagrees with its rule is
    // rejected by CSMIR itself, whoever built it.
    let mut model = document(1);
    model.datasets[0].fields.push(Field {
        name: "probe".to_owned(),
        expression: portable("UPPER(region)"),
        dimension: Some(sef_ossie::document::Dimension { is_time: None }),
        label: None,
        description: None,
        datatype: None,
        ai_context: None,
        custom_extensions: Vec::new(),
    });
    let mut definition = convert(1, &model).expect("converts");
    let dimension = definition
        .dimensions
        .iter_mut()
        .find(|d| d.metadata.id().as_str().ends_with("sales.probe"))
        .expect("dimension");
    dimension.bindings[0].expression.value_type = INT64;
    dimension.value_type = INT64;
    let errors = definition.compile().expect_err("type disagrees with UPPER");
    assert!(
        errors
            .iter()
            .any(|e| e.code == sef_model_ir::diagnostic::ErrorCode::IncompatibleTypes)
    );
}

proptest! {
    /// Conversion is total: any call of a known or unknown name with
    /// arbitrary simple arguments converts or is refused, never panics.
    #[test]
    fn function_conversion_never_panics(
        name in proptest::sample::select(vec![
            "CONCAT", "LENGTH", "LOWER", "UPPER", "TRIM", "LTRIM", "RTRIM", "LEFT", "RIGHT",
            "SUBSTRING", "REPLACE", "SPLIT_PART", "POSITION", "CHARINDEX", "CONTAINS",
            "STARTSWITH", "ENDSWITH", "ABS", "SIGN", "FLOOR", "CEIL", "ROUND", "TRUNC", "MOD",
            "GREATEST", "LEAST", "NVL2", "NULLIFZERO", "REGEXP_LIKE", "SQRT", "FOO",
        ]),
        args in proptest::collection::vec(
            proptest::sample::select(vec![
                "region", "sales.amount", "sales.sale_id", "'x'", "''", "0", "-3", "2.5", "NULL",
            ]),
            0..5,
        ),
    ) {
        let text = format!("{name}({})", args.join(", "));
        let _ = field(&text);
        let _ = summed(&text);
    }
}

#[test]
fn csmir_rejects_a_remainder_declared_narrower_than_its_rule() {
    // Review of #92: UINT64::MAX mod 20000000000000000000 needs 20 digits;
    // a node declaring 19 is refused by CSMIR itself.
    let mut model = document(1);
    model.datasets[0].fields.push(Field {
        name: "probe".to_owned(),
        expression: portable("UPPER(region)"),
        dimension: Some(sef_ossie::document::Dimension { is_time: None }),
        label: None,
        description: None,
        datatype: None,
        ai_context: None,
        custom_extensions: Vec::new(),
    });
    let mut definition = convert(1, &model).expect("converts");
    let wide = sef_model_ir::types::Literal::UnsignedInteger(u128::from(u64::MAX));
    let divisor = sef_model_ir::types::Literal::Decimal(
        sef_model_ir::types::DecimalValue::new(
            "20000000000000000000",
            DecimalType::new(38, 0).expect("type"),
        )
        .expect("value"),
    );
    let literal = |value: sef_model_ir::types::Literal| ScalarExpr {
        value_type: value.value_type(),
        kind: ScalarExprKind::Literal(value),
        unit: None,
    };
    let narrow = decimal(19, 0);
    let dimension = definition
        .dimensions
        .iter_mut()
        .find(|d| d.metadata.id().as_str().ends_with("sales.probe"))
        .expect("dimension");
    dimension.bindings[0].expression = ScalarExpr {
        kind: ScalarExprKind::Function {
            function: ScalarFunction::Mod {
                zero: ZeroDivisionPolicy::Null,
            },
            arguments: vec![literal(wide), literal(divisor)],
        },
        value_type: narrow.clone(),
        unit: None,
    };
    dimension.value_type = narrow;
    let errors = definition
        .compile()
        .expect_err("19 digits cannot hold the remainder");
    assert!(
        errors
            .iter()
            .any(|e| e.code == sef_model_ir::diagnostic::ErrorCode::IncompatibleTypes)
    );
}
