//! The SEF corpus written as Ossie documents compiles to exactly the CSMIR
//! the corpus defines by hand: the same fingerprint as
//! `sef-model-ir/tests/csmir-golden-hashes.txt` (Issue #48).
// Fixtures are trusted test inputs.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use sef_model_ir::identity::SourceHandle;
use sef_model_ir::time::TimestampSemantics;
use sef_model_ir::types::{DecimalType, ValueType};
use sef_ossie::Code;
use sef_ossie::catalog::{Catalog, Column, Relation};
use sef_ossie::csmir::{Options, compile};

fn corpus(file: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// `int64`, `utf8`, `date`, `timestamptz`, `decimal(p,s)`, then ` null`
/// for a nullable column.
fn value_type(spec: &str) -> (ValueType, bool) {
    let (spec, nullable) = match spec.strip_suffix(" null") {
        Some(spec) => (spec, true),
        None => (spec, false),
    };
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
            let (precision, scale) = decimal
                .strip_prefix("decimal(")
                .and_then(|rest| rest.strip_suffix(')'))
                .and_then(|rest| rest.split_once(','))
                .unwrap_or_else(|| panic!("unknown type {decimal}"));
            ValueType::Decimal(
                DecimalType::new(
                    precision.parse().expect("precision"),
                    scale.parse().expect("scale"),
                )
                .expect("decimal type"),
            )
        }
    };
    (value_type, nullable)
}

fn catalog(case: u8) -> Catalog {
    let text = corpus(&format!("CASE-{case:03}.catalog.yaml"));
    let relations: BTreeMap<String, BTreeMap<String, String>> =
        serde_saphyr::from_str(&text).expect("catalog YAML");
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

fn golden(case: u8) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../sef-model-ir/tests/csmir-golden-hashes.txt");
    let text = std::fs::read_to_string(path).expect("golden hashes");
    let line = text
        .lines()
        .find(|line| line.starts_with(&format!("CASE-{case:03} ")))
        .expect("golden line");
    line.split_once(' ').expect("case and hash").1.to_owned()
}

fn assert_same_fingerprint(case: u8) {
    let loaded = sef_ossie::load(&corpus(&format!("CASE-{case:03}.yaml")))
        .unwrap_or_else(|errors| panic!("CASE-{case:03} must load: {errors:#?}"));
    // An as-of relationship cannot point at a unique key of its history:
    // Ossie warns, and our temporal extension states the match instead.
    let expected: &[Code] = if case == 13 {
        &[Code::KeyNotCovered]
    } else {
        &[]
    };
    let warnings: Vec<Code> = loaded.warnings.iter().map(|warning| warning.code).collect();
    assert_eq!(warnings, expected, "{:#?}", loaded.warnings);
    let options = Options {
        namespace: "corpus".to_owned(),
        model: format!("case-{case:03}"),
    };
    let compiled = compile(&loaded.model, &catalog(case), &options).unwrap_or_else(|errors| {
        panic!(
            "CASE-{case:03} must compile:\n{}",
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        )
    });
    assert_eq!(
        compiled.content_hash().to_string(),
        golden(case),
        "CASE-{case:03}: the Ossie document must compile to the corpus CSMIR"
    );
}

macro_rules! corpus_case {
    ($($name:ident => $case:literal),+ $(,)?) => {$(
        #[test]
        fn $name() {
            assert_same_fingerprint($case);
        }
    )+};
}

corpus_case!(
    case_001_additive => 1,
    case_002_star_with_exact_cast => 2,
    case_003_declared_cardinality => 3,
    case_004_independent_grains => 4,
    case_005_multihop => 5,
    case_006_ratio_decomposition => 6,
    case_007_period_end_balance => 7,
    case_008_distinct_recompute => 8,
    case_009_conformed_grain => 9,
    case_010_declared_metric_grain => 10,
    case_011_bridge_allocation => 11,
    case_012_time_bucket => 12,
    case_013_as_of_history => 13,
    case_014_null_contracts => 14,
    case_015_tenant_policy => 15,
    case_016_role_playing => 16,
    case_017_currency_units => 17,
    case_018_rollup_measure => 18,
    case_019_timezone_and_exact_decimal => 19,
    case_020_forbidden_time_axis => 20,
    case_021_cumulative_window => 21,
);
