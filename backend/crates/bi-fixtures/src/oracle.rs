//! Exact expected results for the `sales` fixture. Engines under test
//! (DuckDB, Polars) must reproduce these values; every number is computed
//! with integer arithmetic and serialized as a string, so `DECIMAL`, `INT64`
//! and `HUGEINT` sums never pass through a float.

use std::collections::BTreeMap;

use arrow_array::cast::AsArray;
use arrow_array::types::{Decimal128Type, Int64Type, TimestampMicrosecondType};
use arrow_array::{Array, RecordBatch};
use chrono::{DateTime, SecondsFormat};
use serde::{Deserialize, Serialize};

use crate::sales::{DECIMAL_SCALE, FixtureSpec, SaleRow};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Oracle {
    pub fixture: String,
    pub schema_version: u32,
    pub seed: String,
    pub rows: String,
    pub columns: Vec<Column>,
    pub totals: Totals,
    /// One entry per region, ascending, `NULL` last (DuckDB's default order).
    pub by_region: Vec<RegionTotals>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub sql_type: String,
    pub nullable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Totals {
    pub row_count: String,
    pub order_id: ValueStats,
    pub ordered_at: TimeRange,
    pub region_null_count: String,
    pub quantity: ValueStats,
    pub amount: ValueStats,
}

/// `count` excludes `NULL`s; `sum`/`min`/`max` are `null` when `count` is 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueStats {
    pub count: String,
    pub null_count: String,
    pub sum: Option<String>,
    pub min: Option<String>,
    pub max: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeRange {
    pub min: Option<String>,
    pub max: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionTotals {
    pub region: Option<String>,
    pub row_count: String,
    pub quantity_sum: Option<String>,
    pub amount_sum: Option<String>,
}

impl Oracle {
    /// Computes the oracle from the generator's rows.
    #[must_use]
    pub fn from_rows(spec: FixtureSpec, rows: impl IntoIterator<Item = SaleRow>) -> Self {
        let mut acc = Accumulator::default();
        for row in rows {
            acc.push(
                row.order_id,
                row.ordered_at_micros,
                row.region,
                row.quantity,
                row.amount_cents,
            );
        }
        acc.finish(spec)
    }

    /// Computes the oracle from Arrow batches with the fixture schema, e.g.
    /// read back from Parquet, to prove a file holds exactly the fixture.
    ///
    /// # Panics
    /// If a batch does not have the `sales` fixture schema.
    #[must_use]
    pub fn from_batches<'a>(
        spec: FixtureSpec,
        batches: impl IntoIterator<Item = &'a RecordBatch>,
    ) -> Self {
        let mut acc = Accumulator::default();
        for batch in batches {
            let order_id = batch.column(0).as_primitive::<Int64Type>();
            let ordered_at = batch.column(1).as_primitive::<TimestampMicrosecondType>();
            let region = batch.column(2).as_string::<i32>();
            let quantity = batch.column(3).as_primitive::<Int64Type>();
            let amount = batch.column(4).as_primitive::<Decimal128Type>();
            assert_eq!(order_id.null_count(), 0, "order_id is NOT NULL");
            assert_eq!(ordered_at.null_count(), 0, "ordered_at is NOT NULL");
            for i in 0..batch.num_rows() {
                acc.push(
                    order_id.value(i),
                    ordered_at.value(i),
                    region.is_valid(i).then(|| region.value(i)),
                    quantity.is_valid(i).then(|| quantity.value(i)),
                    amount.is_valid(i).then(|| {
                        i64::try_from(amount.value(i)).expect("DECIMAL(18,2) fits in i64")
                    }),
                );
            }
        }
        acc.finish(spec)
    }

    /// Pretty JSON with a trailing newline; identical bytes on every OS.
    ///
    /// # Panics
    /// Never in practice: the oracle only holds strings and integers.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(self).expect("oracle serializes");
        json.push('\n');
        json
    }
}

#[derive(Default)]
struct Stats {
    count: u64,
    null_count: u64,
    sum: i128,
    min: Option<i64>,
    max: Option<i64>,
}

impl Stats {
    fn push(&mut self, value: Option<i64>) {
        let Some(value) = value else {
            self.null_count += 1;
            return;
        };
        self.count += 1;
        self.sum += i128::from(value);
        self.min = Some(self.min.map_or(value, |m| m.min(value)));
        self.max = Some(self.max.map_or(value, |m| m.max(value)));
    }

    fn sum(&self) -> Option<i128> {
        (self.count > 0).then_some(self.sum)
    }

    fn finish(&self, format: impl Fn(i128) -> String) -> ValueStats {
        ValueStats {
            count: self.count.to_string(),
            null_count: self.null_count.to_string(),
            sum: self.sum().map(&format),
            min: self.min.map(|v| format(i128::from(v))),
            max: self.max.map(|v| format(i128::from(v))),
        }
    }
}

#[derive(Default)]
struct RegionAcc {
    rows: u64,
    quantity: Stats,
    amount: Stats,
}

#[derive(Default)]
struct Accumulator {
    rows: u64,
    order_id: Stats,
    ordered_at: Stats,
    region_nulls: u64,
    quantity: Stats,
    amount: Stats,
    /// Keyed so that `NULL` sorts last: `(is_null, name)`.
    by_region: BTreeMap<(bool, String), RegionAcc>,
}

impl Accumulator {
    fn push(
        &mut self,
        order_id: i64,
        ordered_at_micros: i64,
        region: Option<&str>,
        quantity: Option<i64>,
        amount_cents: Option<i64>,
    ) {
        self.rows += 1;
        self.order_id.push(Some(order_id));
        self.ordered_at.push(Some(ordered_at_micros));
        self.quantity.push(quantity);
        self.amount.push(amount_cents);
        if region.is_none() {
            self.region_nulls += 1;
        }
        let key = (region.is_none(), region.unwrap_or_default().to_owned());
        let group = self.by_region.entry(key).or_default();
        group.rows += 1;
        group.quantity.push(quantity);
        group.amount.push(amount_cents);
    }

    fn finish(self, spec: FixtureSpec) -> Oracle {
        let integer = |v: i128| v.to_string();
        let decimal = |v: i128| format_decimal(v, DECIMAL_SCALE.unsigned_abs());
        Oracle {
            fixture: "sales".to_owned(),
            schema_version: SCHEMA_VERSION,
            seed: spec.seed.to_string(),
            rows: spec.rows.to_string(),
            columns: columns(),
            totals: Totals {
                row_count: self.rows.to_string(),
                order_id: self.order_id.finish(integer),
                ordered_at: TimeRange {
                    min: self.ordered_at.min.map(format_timestamp),
                    max: self.ordered_at.max.map(format_timestamp),
                },
                region_null_count: self.region_nulls.to_string(),
                quantity: self.quantity.finish(integer),
                amount: self.amount.finish(decimal),
            },
            by_region: self
                .by_region
                .into_iter()
                .map(|((is_null, name), group)| RegionTotals {
                    region: (!is_null).then_some(name),
                    row_count: group.rows.to_string(),
                    quantity_sum: group.quantity.sum().map(integer),
                    amount_sum: group.amount.sum().map(decimal),
                })
                .collect(),
        }
    }
}

fn columns() -> Vec<Column> {
    [
        ("order_id", "BIGINT", false),
        ("ordered_at", "TIMESTAMP WITH TIME ZONE", false),
        ("region", "VARCHAR", true),
        ("quantity", "BIGINT", true),
        ("amount", "DECIMAL(18,2)", true),
    ]
    .into_iter()
    .map(|(name, sql_type, nullable)| Column {
        name: name.to_owned(),
        sql_type: sql_type.to_owned(),
        nullable,
    })
    .collect()
}

/// Formats an unscaled decimal exactly, e.g. `(-5, 2)` → `"-0.05"`.
#[must_use]
pub fn format_decimal(unscaled: i128, scale: u8) -> String {
    let digits = unscaled.unsigned_abs().to_string();
    let scale = usize::from(scale);
    let digits = format!("{digits:0>width$}", width = scale + 1);
    let (int_part, frac_part) = digits.split_at(digits.len() - scale);
    let sign = if unscaled < 0 { "-" } else { "" };
    if scale == 0 {
        format!("{sign}{int_part}")
    } else {
        format!("{sign}{int_part}.{frac_part}")
    }
}

/// RFC 3339 in UTC with microseconds, e.g. `2026-01-01T00:00:00.000000Z`.
///
/// # Panics
/// If `micros` is outside chrono's supported range (not reachable for fixtures).
#[must_use]
pub fn format_timestamp(micros: i64) -> String {
    DateTime::from_timestamp_micros(micros)
        .expect("timestamp within range")
        .to_rfc3339_opts(SecondsFormat::Micros, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_decimals_exactly() {
        assert_eq!(format_decimal(0, 2), "0.00");
        assert_eq!(format_decimal(5, 2), "0.05");
        assert_eq!(format_decimal(-5, 2), "-0.05");
        assert_eq!(format_decimal(12_345, 2), "123.45");
        assert_eq!(format_decimal(-100, 2), "-1.00");
        assert_eq!(
            format_decimal(999_999_999_999_999_999, 2),
            "9999999999999999.99"
        );
        assert_eq!(format_decimal(42, 0), "42");
    }

    #[test]
    fn formats_timestamps_in_utc_with_micros() {
        assert_eq!(
            format_timestamp(1_767_225_600_000_001),
            "2026-01-01T00:00:00.000001Z"
        );
    }
}
