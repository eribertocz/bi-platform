//! Synthetic fixtures in the lake (G1 uses only synthetic data): loading the
//! `sales` fixture and recomputing its oracle with DuckDB, so engine results
//! can be compared value by value with `bi_fixtures::oracle`.

use bi_fixtures::oracle::{
    Column, Oracle, RegionTotals, SCHEMA_VERSION, TimeRange, Totals, ValueStats, format_timestamp,
};
use bi_fixtures::sales::{self, FixtureSpec};
use bi_fixtures::{bench, parquet_io};
use duckdb::Connection;

use crate::{Bootstrap, LoadOutcome, Runtime, RuntimeError};

pub const SALES_TABLE: &str = "sales";

/// Identifies the fixture content; a different value triggers a reload.
#[must_use]
pub fn sales_fingerprint(spec: FixtureSpec) -> String {
    format!(
        "sales:schema={SCHEMA_VERSION}:seed={}:rows={}",
        spec.seed, spec.rows
    )
}

/// Generates the `sales` fixture as Parquet and loads it into
/// `lake.main.sales`, unless that exact fixture is already there.
///
/// # Errors
/// I/O, Parquet or engine errors.
pub fn load_sales(bootstrap: &Bootstrap, spec: FixtureSpec) -> Result<LoadOutcome, RuntimeError> {
    let scratch = tempfile::tempdir()?;
    let parquet = scratch.path().join("sales.parquet");
    parquet_io::write(
        &parquet,
        sales::schema(),
        sales::record_batches(spec, bench::BATCH_ROWS),
    )
    .map_err(|error| RuntimeError::Io(std::io::Error::other(error.to_string())))?;
    bootstrap.load_parquet_fixture(SALES_TABLE, &parquet, &sales_fingerprint(spec))
}

/// Recomputes the `sales` oracle with DuckDB over `lake.main.sales`.
///
/// # Errors
/// Engine errors, e.g. when the table does not exist.
pub async fn sales_oracle(runtime: &Runtime, spec: FixtureSpec) -> Result<Oracle, RuntimeError> {
    runtime.run(move |conn| oracle_query(conn, spec)).await
}

fn oracle_query(conn: &Connection, spec: FixtureSpec) -> duckdb::Result<Oracle> {
    let columns = conn
        .prepare(
            "SELECT column_name, data_type, is_nullable = 'YES' FROM information_schema.columns
             WHERE table_catalog = 'lake' AND table_schema = 'main' AND table_name = 'sales'
             ORDER BY ordinal_position",
        )?
        .query_map([], |row| {
            Ok(Column {
                name: row.get(0)?,
                sql_type: row.get(1)?,
                nullable: row.get(2)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;

    // Aggregates are cast to text inside DuckDB, so DECIMAL and HUGEINT
    // values never pass through a float on the way out.
    let totals = conn.query_row(
        "SELECT count(*)::VARCHAR,
            count(order_id)::VARCHAR, (count(*) - count(order_id))::VARCHAR,
            sum(order_id)::VARCHAR, min(order_id)::VARCHAR, max(order_id)::VARCHAR,
            epoch_us(min(ordered_at)), epoch_us(max(ordered_at)),
            count(*) FILTER (WHERE region IS NULL)::VARCHAR,
            count(quantity)::VARCHAR, (count(*) - count(quantity))::VARCHAR,
            sum(quantity)::VARCHAR, min(quantity)::VARCHAR, max(quantity)::VARCHAR,
            count(amount)::VARCHAR, (count(*) - count(amount))::VARCHAR,
            sum(amount)::VARCHAR, min(amount)::VARCHAR, max(amount)::VARCHAR
         FROM lake.main.sales",
        [],
        |row| {
            let stats = |first: usize| -> duckdb::Result<ValueStats> {
                Ok(ValueStats {
                    count: row.get(first)?,
                    null_count: row.get(first + 1)?,
                    sum: row.get(first + 2)?,
                    min: row.get(first + 3)?,
                    max: row.get(first + 4)?,
                })
            };
            Ok(Totals {
                row_count: row.get(0)?,
                order_id: stats(1)?,
                ordered_at: TimeRange {
                    min: row.get::<_, Option<i64>>(6)?.map(format_timestamp),
                    max: row.get::<_, Option<i64>>(7)?.map(format_timestamp),
                },
                region_null_count: row.get(8)?,
                quantity: stats(9)?,
                amount: stats(14)?,
            })
        },
    )?;

    let by_region = conn
        .prepare(
            "SELECT region, count(*)::VARCHAR, sum(quantity)::VARCHAR, sum(amount)::VARCHAR
             FROM lake.main.sales GROUP BY region ORDER BY region ASC NULLS LAST",
        )?
        .query_map([], |row| {
            Ok(RegionTotals {
                region: row.get(0)?,
                row_count: row.get(1)?,
                quantity_sum: row.get(2)?,
                amount_sum: row.get(3)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;

    Ok(Oracle {
        fixture: "sales".to_owned(),
        schema_version: SCHEMA_VERSION,
        seed: spec.seed.to_string(),
        rows: spec.rows.to_string(),
        columns,
        totals,
        by_region,
    })
}
