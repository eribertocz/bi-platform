//! Synthetic `sales` fact table: deterministic for a given [`FixtureSpec`],
//! with the types that must survive end to end without loss (G0.10):
//! `BIGINT` above 2^53, `DECIMAL(18,2)`, `TIMESTAMPTZ` with microseconds,
//! categories and explicit `NULL`s.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_array::builder::{
    Decimal128Builder, Int64Builder, StringBuilder, TimestampMicrosecondBuilder,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};

use crate::rng::SplitMix64;

pub const DECIMAL_PRECISION: u8 = 18;
pub const DECIMAL_SCALE: i8 = 2;
/// Largest unscaled `DECIMAL(18,2)` value: 9999999999999999.99.
pub const MAX_AMOUNT_CENTS: i64 = 999_999_999_999_999_999;
/// First order id. Above 2^53, so any trip through a float64 visibly corrupts it.
pub const FIRST_ORDER_ID: i64 = 9_007_199_254_740_993;
pub const REGIONS: [&str; 5] = ["central", "east", "north", "south", "west"];

/// 2026-01-01T00:00:00Z in microseconds since the Unix epoch.
const START_MICROS: i64 = 1_767_225_600_000_000;
const YEAR_MICROS: u64 = 365 * 24 * 3600 * 1_000_000;
const MAX_REGULAR_AMOUNT_CENTS: u64 = 10_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixtureSpec {
    pub seed: u64,
    pub rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaleRow {
    pub order_id: i64,
    pub ordered_at_micros: i64,
    pub region: Option<&'static str>,
    pub quantity: Option<i64>,
    /// Unscaled `DECIMAL(18,2)`, i.e. cents.
    pub amount_cents: Option<i64>,
}

/// Rows of the fixture, generated lazily and always in the same order.
pub fn rows(spec: FixtureSpec) -> impl Iterator<Item = SaleRow> {
    let mut rng = SplitMix64::new(spec.seed);
    (0..spec.rows).map(move |index| generate_row(index, &mut rng))
}

fn generate_row(index: u64, rng: &mut SplitMix64) -> SaleRow {
    let order_id = FIRST_ORDER_ID + i64::try_from(index).expect("row index fits in i64");
    let ordered_at_micros = START_MICROS + to_i64(rng.below(YEAR_MICROS));
    let region = if rng.percent(3) {
        None
    } else {
        Some(REGIONS[usize::try_from(rng.below(REGIONS.len() as u64)).expect("small index")])
    };
    let quantity = if rng.percent(2) {
        None
    } else {
        Some(1 + to_i64(rng.below(50)))
    };
    let amount_is_null = rng.percent(2);
    let amount_cents = if index == 0 {
        // The extreme of the type range, so totals exceed any float64 precision.
        Some(MAX_AMOUNT_CENTS)
    } else if amount_is_null {
        None
    } else {
        let cents = 1 + to_i64(rng.below(MAX_REGULAR_AMOUNT_CENTS));
        // About 1% refunds, so minimums and sums cross zero.
        Some(if rng.percent(1) { -cents } else { cents })
    };
    SaleRow {
        order_id,
        ordered_at_micros,
        region,
        quantity,
        amount_cents,
    }
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).expect("generated value fits in i64")
}

#[must_use]
pub fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("order_id", DataType::Int64, false),
        Field::new(
            "ordered_at",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            false,
        ),
        Field::new("region", DataType::Utf8, true),
        Field::new("quantity", DataType::Int64, true),
        Field::new(
            "amount",
            DataType::Decimal128(DECIMAL_PRECISION, DECIMAL_SCALE),
            true,
        ),
    ]))
}

/// The fixture as Arrow record batches of at most `batch_rows` rows.
///
/// # Panics
/// If `batch_rows` is zero.
pub fn record_batches(spec: FixtureSpec, batch_rows: usize) -> impl Iterator<Item = RecordBatch> {
    assert!(batch_rows > 0, "batch_rows must be positive");
    let mut rows = rows(spec).peekable();
    std::iter::from_fn(move || {
        rows.peek()?;
        Some(to_record_batch(rows.by_ref().take(batch_rows)))
    })
}

fn to_record_batch(rows: impl Iterator<Item = SaleRow>) -> RecordBatch {
    let mut order_id = Int64Builder::new();
    let mut ordered_at = TimestampMicrosecondBuilder::new().with_timezone("UTC");
    let mut region = StringBuilder::new();
    let mut quantity = Int64Builder::new();
    let mut amount = Decimal128Builder::new()
        .with_precision_and_scale(DECIMAL_PRECISION, DECIMAL_SCALE)
        .expect("DECIMAL(18,2) is a valid Arrow decimal");

    for row in rows {
        order_id.append_value(row.order_id);
        ordered_at.append_value(row.ordered_at_micros);
        region.append_option(row.region);
        quantity.append_option(row.quantity);
        amount.append_option(row.amount_cents.map(i128::from));
    }

    RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(order_id.finish()),
            Arc::new(ordered_at.finish()),
            Arc::new(region.finish()),
            Arc::new(quantity.finish()),
            Arc::new(amount.finish()),
        ],
    )
    .expect("columns match the schema")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: FixtureSpec = FixtureSpec {
        seed: 42,
        rows: 10_000,
    };

    #[test]
    fn same_seed_same_rows_and_different_seed_different_rows() {
        let a: Vec<_> = rows(SPEC).collect();
        let b: Vec<_> = rows(SPEC).collect();
        let c: Vec<_> = rows(FixtureSpec { seed: 43, ..SPEC }).collect();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn covers_extremes_and_nulls() {
        let rows: Vec<_> = rows(SPEC).collect();
        assert_eq!(rows.len(), 10_000);
        assert!(rows.iter().all(|r| r.order_id > (1 << 53)));
        assert_eq!(rows[0].amount_cents, Some(MAX_AMOUNT_CENTS));
        assert!(rows.iter().any(|r| r.region.is_none()));
        assert!(rows.iter().any(|r| r.quantity.is_none()));
        assert!(rows.iter().any(|r| r.amount_cents.is_none()));
        assert!(rows.iter().any(|r| r.amount_cents.is_some_and(|c| c < 0)));
        assert!(
            rows.windows(2)
                .all(|pair| pair[1].order_id == pair[0].order_id + 1)
        );
    }

    #[test]
    fn batches_cover_all_rows_in_order() {
        let batches: Vec<_> = record_batches(SPEC, 3_000).collect();
        let sizes: Vec<_> = batches.iter().map(RecordBatch::num_rows).collect();
        assert_eq!(sizes, [3_000, 3_000, 3_000, 1_000]);
        assert!(batches.iter().all(|b| b.schema() == schema()));
    }
}
