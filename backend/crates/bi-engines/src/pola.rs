//! Pure Polars LazyFrame route: Polars scans the Parquet file and aggregates.
//!
//! Rule found by this benchmark: Polars sums `Int64` in 64 bits and wraps on
//! overflow without error, so every integer sum casts to `Int128` first
//! (DuckDB widens `SUM(BIGINT)` to `HUGEINT` by itself).

use std::path::Path;
use std::time::Duration;

use bi_fixtures::bench::timed;
use bi_fixtures::oracle::{format_decimal, format_timestamp};
use polars::prelude::*;

use crate::results::{Query, Row};

pub struct Run {
    pub build: Duration,
    pub plan: Duration,
    pub execute: Duration,
    pub serialize: Duration,
    pub rows: Vec<Row>,
}

/// Lazily scans the fixture.
///
/// # Errors
/// When the file cannot be scanned.
pub fn scan(parquet: &Path) -> PolarsResult<LazyFrame> {
    LazyFrame::scan_parquet(
        PlRefPath::new(parquet.to_string_lossy().as_ref()),
        ScanArgsParquet::default(),
    )
}

fn exact_sum(name: &str) -> Expr {
    col(name).cast(DataType::Int128).sum()
}

pub fn plan(query: Query, source: LazyFrame) -> LazyFrame {
    match query {
        Query::Totals => source.select([
            len(),
            exact_sum("order_id"),
            col("amount").sum().alias("amount_sum"),
            col("amount").min().alias("amount_min"),
            col("amount").max().alias("amount_max"),
            exact_sum("quantity"),
            col("ordered_at").min().alias("ordered_at_min"),
            col("ordered_at").max().alias("ordered_at_max"),
            col("region").null_count().alias("region_nulls"),
        ]),
        Query::ByRegion => source
            .group_by([col("region")])
            .agg([len(), exact_sum("quantity"), col("amount").sum()])
            .sort(
                ["region"],
                SortMultipleOptions::default().with_nulls_last(true),
            ),
        Query::NorthSouth => source
            .filter(
                col("region")
                    .eq(lit("north"))
                    .or(col("region").eq(lit("south"))),
            )
            .group_by([col("region")])
            .agg([col("amount").sum()])
            .sort(["region"], SortMultipleOptions::default()),
    }
}

/// Runs `query` once, timing each phase.
///
/// # Errors
/// Polars errors.
pub fn run(query: Query, parquet: &Path) -> PolarsResult<Run> {
    let (lazy, build) = timed(|| scan(parquet).map(|source| plan(query, source)));
    let lazy = lazy?;
    let (optimized, plan_time) = timed(|| lazy.describe_optimized_plan());
    optimized?;
    let (frame, execute) = timed(|| lazy.collect());
    let frame = frame?;
    let (rows, serialize) = timed(|| to_rows(&frame));
    Ok(Run {
        build,
        plan: plan_time,
        execute,
        serialize,
        rows: rows?,
    })
}

/// The integer sum without the `Int128` cast, kept as evidence of the
/// silent overflow.
///
/// # Errors
/// Polars errors.
pub fn naive_order_id_sum(parquet: &Path) -> PolarsResult<Option<String>> {
    let frame = scan(parquet)?.select([col("order_id").sum()]).collect()?;
    value(&frame.column("order_id")?.get(0)?)
}

fn to_rows(frame: &DataFrame) -> PolarsResult<Vec<Row>> {
    (0..frame.height())
        .map(|i| {
            frame
                .columns()
                .iter()
                .map(|column| value(&column.get(i)?))
                .collect()
        })
        .collect()
}

/// Exact text of one Polars value.
///
/// # Errors
/// On a value type the benchmark queries never produce.
pub fn value(value: &AnyValue) -> PolarsResult<Option<String>> {
    Ok(Some(match value {
        AnyValue::Null => return Ok(None),
        AnyValue::UInt32(v) => v.to_string(),
        AnyValue::UInt64(v) => v.to_string(),
        AnyValue::Int64(v) => v.to_string(),
        AnyValue::Int128(v) => v.to_string(),
        AnyValue::Decimal(v, _, scale) => format_decimal(
            *v,
            u8::try_from(*scale).map_err(|_| polars_err!(ComputeError: "scale {scale}"))?,
        ),
        AnyValue::Datetime(v, TimeUnit::Microseconds, _)
        | AnyValue::DatetimeOwned(v, TimeUnit::Microseconds, _) => format_timestamp(*v),
        AnyValue::String(v) => (*v).to_owned(),
        AnyValue::StringOwned(v) => v.to_string(),
        other => polars_bail!(ComputeError: "unexpected Polars value {other:?}"),
    }))
}
