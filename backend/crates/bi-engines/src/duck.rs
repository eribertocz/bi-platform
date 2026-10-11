//! Pure DuckDB SQL route: DuckDB reads the Parquet file and aggregates; the
//! result leaves as Arrow and is formatted exactly.

use std::path::Path;
use std::time::Duration;

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Decimal128Type, Int32Type, Int64Type, TimestampMicrosecondType, UInt32Type, UInt64Type,
};
use arrow_array::{Array, RecordBatch};
use arrow_schema::{DataType, TimeUnit};
use bi_fixtures::bench::timed;
use bi_fixtures::oracle::{format_decimal, format_timestamp};
use bi_runtime::duckdb::{self, Connection};
use bi_runtime::sql_string;

use crate::results::{Query, Row};

pub struct Run {
    pub build: Duration,
    pub plan: Duration,
    pub execute: Duration,
    pub serialize: Duration,
    pub rows: Vec<Row>,
}

#[must_use]
pub fn sql(query: Query, parquet: &Path) -> String {
    let source = format!("read_parquet({})", sql_string(&parquet.to_string_lossy()));
    match query {
        Query::Totals => format!(
            "SELECT count(*), sum(order_id), sum(amount), min(amount), max(amount), \
             sum(quantity), min(ordered_at), max(ordered_at), \
             count(*) FILTER (WHERE region IS NULL) FROM {source}"
        ),
        Query::ByRegion => format!(
            "SELECT region, count(*), sum(quantity), sum(amount) FROM {source} \
             GROUP BY region ORDER BY region ASC NULLS LAST"
        ),
        Query::NorthSouth => format!(
            "SELECT region, sum(amount) FROM {source} WHERE region IN ('north', 'south') \
             GROUP BY region ORDER BY region"
        ),
    }
}

/// Runs `query` once, timing each phase.
///
/// # Errors
/// Engine errors.
pub fn run(conn: &Connection, query: Query, parquet: &Path) -> duckdb::Result<Run> {
    let (sql, build) = timed(|| sql(query, parquet));
    let (statement, plan) = timed(|| conn.prepare(&sql));
    let mut statement = statement?;
    let (batches, execute) = timed(|| {
        statement
            .query_arrow([])
            .map(Iterator::collect::<Vec<RecordBatch>>)
    });
    let batches = batches?;
    let (rows, serialize) = timed(|| to_rows(&batches));
    Ok(Run {
        build,
        plan,
        execute,
        serialize,
        rows,
    })
}

fn to_rows(batches: &[RecordBatch]) -> Vec<Row> {
    let mut rows = Vec::new();
    for batch in batches {
        for i in 0..batch.num_rows() {
            rows.push(
                batch
                    .columns()
                    .iter()
                    .map(|column| value(column.as_ref(), i))
                    .collect(),
            );
        }
    }
    rows
}

/// Exact text of one Arrow value. DuckDB exports HUGEINT as `Decimal128(38, 0)`.
///
/// # Panics
/// On an Arrow type the benchmark queries never produce.
#[must_use]
pub fn value(array: &dyn Array, i: usize) -> Option<String> {
    if array.is_null(i) {
        return None;
    }
    Some(match array.data_type() {
        DataType::Int64 => array.as_primitive::<Int64Type>().value(i).to_string(),
        DataType::Int32 => array.as_primitive::<Int32Type>().value(i).to_string(),
        DataType::UInt64 => array.as_primitive::<UInt64Type>().value(i).to_string(),
        DataType::UInt32 => array.as_primitive::<UInt32Type>().value(i).to_string(),
        DataType::Decimal128(_, scale) => format_decimal(
            array.as_primitive::<Decimal128Type>().value(i),
            u8::try_from(*scale).expect("non-negative scale"),
        ),
        DataType::Timestamp(TimeUnit::Microsecond, _) => {
            format_timestamp(array.as_primitive::<TimestampMicrosecondType>().value(i))
        }
        DataType::Utf8 => array.as_string::<i32>().value(i).to_owned(),
        DataType::LargeUtf8 => array.as_string::<i64>().value(i).to_owned(),
        DataType::Utf8View => array.as_string_view().value(i).to_owned(),
        other => panic!("unexpected Arrow type {other} from DuckDB"),
    })
}
