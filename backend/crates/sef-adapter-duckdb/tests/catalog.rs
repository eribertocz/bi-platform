#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use duckdb::Connection;
use proptest::prelude::*;
use sef_adapter_duckdb::{CatalogError, describe, value_type};
use sef_model_ir::time::TimestampSemantics;
use sef_model_ir::types::{DecimalType, ValueType};

#[test]
fn exact_types_and_nullability_from_real_describe() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE types (big BIGINT NOT NULL, int INTEGER, dec DECIMAL(18,2), dbl DOUBLE, str VARCHAR, flag BOOLEAN, day DATE, instant TIMESTAMP WITH TIME ZONE)").unwrap();
    let relation = describe(&conn, "types").unwrap();
    let expected = [
        ValueType::Integer {
            signed: true,
            bits: Some(64),
        },
        ValueType::Integer {
            signed: true,
            bits: Some(32),
        },
        ValueType::Decimal(DecimalType::new(18, 2).unwrap()),
        ValueType::Float { bits: 64 },
        ValueType::Utf8,
        ValueType::Boolean,
        ValueType::Date,
        ValueType::Timestamp {
            timezone_semantics: TimestampSemantics::Instant,
        },
    ];
    assert_eq!(relation.columns.len(), expected.len());
    for (i, (column, ty)) in relation.columns.iter().zip(expected).enumerate() {
        assert_eq!(column.value_type, ty);
        assert_eq!(column.nullable, i != 0);
    }
}

#[test]
fn unsupported_types_never_approximate() {
    let conn = Connection::open_in_memory().unwrap();
    for ty in [
        "TIMESTAMP",
        "HUGEINT",
        "INTEGER[]",
        "FLOAT",
        "UBIGINT",
        "BLOB",
        "TIME",
        "INTERVAL",
        "STRUCT(a INTEGER)",
    ] {
        conn.execute_batch(&format!("CREATE OR REPLACE TABLE bad (v {ty})"))
            .unwrap();
        let error = describe(&conn, "bad").unwrap_err();
        assert!(
            matches!(error, CatalogError::UnsupportedType { .. }),
            "{ty}: {error}"
        );
        assert!(error.to_string().contains("exactamente"));
    }
}

#[test]
fn lookup_rejects_sql_paths_and_functions_and_keeps_case_in_handles() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE sales (x BIGINT); CREATE TABLE a (\"X\" BIGINT)")
        .unwrap();
    for source in [
        "missing",
        "sales; DROP TABLE sales",
        "read_csv('/secret')",
        "../sales",
        "sales --",
        "SELECT * FROM sales",
        "sales.x.y.z",
    ] {
        assert!(matches!(
            describe(&conn, source),
            Err(CatalogError::UnknownSource)
        ));
    }
    assert_eq!(
        describe(&conn, "sales").unwrap(),
        describe(&conn, "MAIN.SALES").unwrap()
    );
    assert_ne!(
        describe(&conn, "sales").unwrap().columns[0].handle,
        describe(&conn, "a").unwrap().columns[0].handle
    );
}

proptest! {
    #[test]
    fn all_duckdb_decimal_shapes_are_exact(p in 1u8..=38, seed in any::<u8>()) {
        let s = seed % (p + 1);
        prop_assert_eq!(value_type(&format!("DECIMAL({p},{s})")), Ok(ValueType::Decimal(DecimalType::new(p, s).unwrap())));
    }
    #[test]
    fn unknown_types_have_no_fallback(s in "[a-z]{1,50}") {
        prop_assert!(value_type(&s).is_err());
    }
    #[test]
    fn source_injection_never_becomes_sql(payload in "[a-zA-Z0-9_ ;'()/-]{0,70}") {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE sales (x BIGINT)").unwrap();
        let source = format!("sales;{payload}");
        prop_assert!(matches!(describe(&conn, &source), Err(CatalogError::UnknownSource)));
        prop_assert!(describe(&conn, "sales").is_ok());
    }
}
