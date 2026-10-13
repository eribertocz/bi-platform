//! `OSSIE_SQL_2026` REQUIRED date functions to CSMIR (#77): calendar and
//! clock fields, date arithmetic and what a time zone would change.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use sef_model_ir::expressions::{ScalarExpr, ScalarExprKind};
use sef_model_ir::functions::{DatePart, ScalarFunction};
use sef_model_ir::identity::SourceHandle;
use sef_model_ir::model::ModelDefinition;
use sef_model_ir::time::TimeGrain;
use sef_model_ir::time::TimestampSemantics;
use sef_model_ir::types::{DecimalType, ValueType};
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

// ---- Date functions of OSSIE_SQL_2026 (#77, dates) ------------------------------

/// `case` with a computed dimension `probe` on its first dataset, with an
/// optional `bi-platform` extension payload.
fn probed(
    case: u8,
    text: &str,
    extension: Option<&str>,
) -> Result<ModelDefinition, Vec<Diagnostic>> {
    let mut model = document(case);
    model.datasets[0].fields.push(Field {
        name: "probe".to_owned(),
        expression: portable(text),
        dimension: Some(sef_ossie::document::Dimension { is_time: None }),
        label: None,
        description: None,
        datatype: None,
        ai_context: None,
        custom_extensions: extension
            .map(|data| {
                vec![sef_ossie::document::CustomExtension {
                    vendor_name: "bi-platform".to_owned(),
                    data: data.to_owned(),
                }]
            })
            .unwrap_or_default(),
    });
    convert(case, &model)
}

/// The expression of the dimension `probe` (see [`probed`]).
fn probe(case: u8, text: &str, extension: Option<&str>) -> Result<ScalarExpr, Vec<Diagnostic>> {
    Ok(probed(case, text, extension)?
        .dimensions
        .iter()
        .find(|dimension| {
            dimension
                .metadata
                .id()
                .as_str()
                .rsplit('/')
                .next()
                .is_some_and(|name| name.rsplit('.').next() == Some("probe"))
        })
        .expect("dimension")
        .bindings[0]
        .expression
        .clone())
}

fn function(expr: &ScalarExpr) -> ScalarFunction {
    match &expr.kind {
        ScalarExprKind::Function { function, .. } => *function,
        other => panic!("expected a function, got {other:?}"),
    }
}

const INT64: ValueType = ValueType::Integer {
    signed: true,
    bits: Some(64),
};
const INSTANT: ValueType = ValueType::Timestamp {
    timezone_semantics: TimestampSemantics::Instant,
};
const ZONED: &str = r#"{"version": 1, "time": {"timezone": "America/La_Paz"}}"#;

fn pinned(tzdb: &str) -> String {
    format!(r#"{{"version": 1, "time": {{"timezone": "America/La_Paz", "tzdb": "{tzdb}"}}}}"#)
}

fn rejected(result: Result<ScalarExpr, Vec<Diagnostic>>, code: Code, needle: &str) {
    let errors = result.expect_err("must be rejected");
    assert!(
        errors
            .iter()
            .any(|e| e.code == code && e.message.contains(needle)),
        "expected {} with {needle:?}: {errors:#?}",
        code.as_str()
    );
}

#[test]
fn calendar_fields_of_a_date() {
    // CASE-018: `sales.date` is a DATE.
    for (text, part) in [
        ("YEAR(date)", DatePart::Year),
        ("QUARTER(date)", DatePart::Quarter),
        ("MONTH(date)", DatePart::Month),
        ("DAY(date)", DatePart::Day),
        ("DAYOFYEAR(date)", DatePart::DayOfYear),
        ("EXTRACT(YEAR FROM date)", DatePart::Year),
        ("DATE_PART('week', date)", DatePart::Week),
        ("DATE_PART('dayofweek', date)", DatePart::DayOfWeek),
    ] {
        let expr = probe(18, text, None).unwrap_or_else(|e| panic!("{text}: {e:#?}"));
        assert_eq!(function(&expr), ScalarFunction::Extract { part }, "{text}");
        assert_eq!(expr.value_type, INT64, "{text}");
    }
    rejected(probe(18, "HOUR(date)", None), Code::TypeMismatch, "EXTRACT");
    rejected(
        probe(18, "DATE_PART('millisecond', date)", None),
        Code::NotRepresentable,
        "MILLISECOND",
    );
}

#[test]
fn an_instant_is_read_on_its_local_date_in_the_business_zone() {
    // CASE-019: `events.event_time` is an instant.
    let expr = probe(19, "YEAR(event_time)", Some(ZONED)).expect("converts");
    let ScalarExprKind::Function {
        function,
        arguments,
    } = &expr.kind
    else {
        panic!("EXTRACT");
    };
    assert_eq!(
        *function,
        ScalarFunction::Extract {
            part: DatePart::Year
        }
    );
    let ScalarExprKind::TemporalDerivation { bucket, .. } = &arguments[0].kind else {
        panic!("the local date first");
    };
    assert_eq!(bucket.timezone.as_str(), "America/La_Paz");
    assert_eq!(bucket.grain, TimeGrain::Day);
    assert_eq!(arguments[0].value_type, ValueType::Date);
    // Without a declared zone there is no local date to read.
    rejected(
        probe(19, "YEAR(event_time)", None),
        Code::InvalidExtension,
        "zona",
    );
    // Hours of an instant depend on the zone: refused.
    rejected(
        probe(19, "HOUR(event_time)", Some(ZONED)),
        Code::NotRepresentable,
        "zona horaria",
    );
}

#[test]
fn a_pinned_tzdb_is_kept_or_refused_never_forgotten() {
    // A DATE_TRUNC dimension keeps the pin: two versions, two models.
    let bucket = |tzdb: &str| {
        probed(19, "DATE_TRUNC('MONTH', event_time)", Some(&pinned(tzdb))).expect("converts")
    };
    assert_ne!(bucket("2024a"), bucket("2025b"));
    // The local date read inside an expression has nowhere to keep it.
    for tzdb in ["2024a", "2025b"] {
        rejected(
            probe(19, "YEAR(event_time)", Some(&pinned(tzdb))),
            Code::InvalidExtension,
            "tzdb",
        );
    }
}

#[test]
fn date_arithmetic_keeps_types_and_refuses_what_a_zone_would_change() {
    let added = probe(18, "DATEADD(month, 1, date)", None).expect("converts");
    assert_eq!(
        function(&added),
        ScalarFunction::DateAdd {
            part: DatePart::Month
        }
    );
    assert_eq!(added.value_type, ValueType::Date);
    let between = probe(18, "DATEDIFF(day, date, date)", None).expect("converts");
    assert_eq!(between.value_type, INT64);
    assert_eq!(
        probe(18, "DATEADD('week', -2, date)", None)
            .expect("converts")
            .value_type,
        ValueType::Date
    );
    // A clock part on a date, or a calendar part on an instant: refused.
    rejected(
        probe(18, "DATEADD(hour, 1, date)", None),
        Code::TypeMismatch,
        "DATEADD",
    );
    rejected(
        probe(19, "DATEADD(day, 1, event_time)", None),
        Code::TypeMismatch,
        "DATEADD",
    );
    rejected(
        probe(19, "DATEDIFF(hour, event_time, event_time)", None),
        Code::TypeMismatch,
        "DATEDIFF",
    );
    // Exact durations of an instant do not depend on any zone.
    let later = probe(19, "DATEADD(minute, 5, event_time)", None).expect("converts");
    assert_eq!(later.value_type, INSTANT);
    for part in [DatePart::Second, DatePart::Minute] {
        let text = format!("DATEDIFF({}, event_time, event_time)", part.code());
        let counted = probe(19, &text, None).unwrap_or_else(|e| panic!("{text}: {e:#?}"));
        assert_eq!(
            function(&counted),
            ScalarFunction::DateDiff { part },
            "{text}"
        );
        assert_eq!(counted.value_type, INT64, "{text}");
    }
    // Different types cannot be counted between.
    rejected(
        probe(19, "DATEDIFF(day, event_time, DATE '2026-01-01')", None),
        Code::TypeMismatch,
        "DATEDIFF",
    );
}

#[test]
fn text_becomes_a_date_only_in_iso_8601() {
    let parsed = probe(18, "TO_DATE(category)", None).expect("converts");
    assert_eq!(function(&parsed), ScalarFunction::ToDate);
    assert_eq!(parsed.value_type, ValueType::Date);
    rejected(
        probe(18, "TO_DATE(date)", None),
        Code::TypeMismatch,
        "TO_DATE",
    );
    rejected(
        probe(19, "TO_TIMESTAMP('2026-01-01 10:00')", None),
        Code::NotRepresentable,
        "zona",
    );
}

#[test]
fn the_current_instant_is_refused_for_reproducibility() {
    for text in ["CURRENT_DATE()", "CURRENT_TIMESTAMP()"] {
        let errors = probe(18, text, None).expect_err(text);
        assert!(
            errors.iter().any(|e| e.message.contains("irreproducible")
                || e.code == Code::UnknownFunction
                || e.code == Code::UnsupportedConstruct),
            "{text}: {errors:#?}"
        );
    }
}
