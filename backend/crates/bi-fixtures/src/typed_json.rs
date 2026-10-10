//! Smoke of typed JSON serialization (G0.13): values travel as
//! `{"type": ..., "value": ...}` with `INT64`, `DECIMAL` and timestamps as
//! strings, following `contratos_referencia/typed-result.json`. The real
//! query result contract is defined in G1; this measures the cost shape.

use arrow_array::cast::AsArray;
use arrow_array::types::{Decimal128Type, Int64Type, TimestampMicrosecondType};
use arrow_array::{Array, RecordBatch};
use serde::Serialize;

use crate::oracle::{format_decimal, format_timestamp};
use crate::sales::DECIMAL_SCALE;

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum TypedValue {
    Int64(String),
    Timestamptz(String),
    String(String),
    Decimal(String),
    Null,
}

/// Converts `sales` batches into typed rows.
#[must_use]
pub fn typed_rows(batches: &[RecordBatch]) -> Vec<Vec<TypedValue>> {
    let mut rows = Vec::with_capacity(batches.iter().map(RecordBatch::num_rows).sum());
    for batch in batches {
        let order_id = batch.column(0).as_primitive::<Int64Type>();
        let ordered_at = batch.column(1).as_primitive::<TimestampMicrosecondType>();
        let region = batch.column(2).as_string::<i32>();
        let quantity = batch.column(3).as_primitive::<Int64Type>();
        let amount = batch.column(4).as_primitive::<Decimal128Type>();
        for i in 0..batch.num_rows() {
            rows.push(vec![
                TypedValue::Int64(order_id.value(i).to_string()),
                TypedValue::Timestamptz(format_timestamp(ordered_at.value(i))),
                if region.is_valid(i) {
                    TypedValue::String(region.value(i).to_owned())
                } else {
                    TypedValue::Null
                },
                if quantity.is_valid(i) {
                    TypedValue::Int64(quantity.value(i).to_string())
                } else {
                    TypedValue::Null
                },
                if amount.is_valid(i) {
                    TypedValue::Decimal(format_decimal(
                        amount.value(i),
                        DECIMAL_SCALE.unsigned_abs(),
                    ))
                } else {
                    TypedValue::Null
                },
            ]);
        }
    }
    rows
}

/// Serializes `batches` as typed JSON rows and returns the bytes.
///
/// # Panics
/// Never in practice: typed values only hold strings.
#[must_use]
pub fn serialize(batches: &[RecordBatch]) -> Vec<u8> {
    serde_json::to_vec(&typed_rows(batches)).expect("typed rows serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sales::{FIRST_ORDER_ID, FixtureSpec, record_batches};

    #[test]
    fn int64_above_2_pow_53_and_decimals_stay_exact() {
        let batches: Vec<_> = record_batches(FixtureSpec { seed: 42, rows: 3 }, 10).collect();
        let json: serde_json::Value = serde_json::from_slice(&serialize(&batches)).unwrap();
        assert_eq!(
            json[0][0],
            serde_json::json!({"type": "int64", "value": FIRST_ORDER_ID.to_string()})
        );
        assert_eq!(
            json[0][4],
            serde_json::json!({"type": "decimal", "value": "9999999999999999.99"})
        );
    }

    #[test]
    fn nulls_are_typed() {
        assert_eq!(
            serde_json::to_string(&TypedValue::Null).unwrap(),
            r#"{"type":"null"}"#
        );
    }
}
