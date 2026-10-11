//! The one crossing between the routes that the architecture requires:
//! publishing a Polars transformation into DuckLake through a transactional
//! boundary (`ESTANDARES_Y_PORTABILIDAD.md`, M13 §7.3). Polars writes a staging
//! Parquet file; DuckDB inserts it into the lake in one transaction. Polars
//! never touches the lake catalog.

use std::fs::File;
use std::path::Path;

use bi_fixtures::bench::{Phase, Timings, timed};
use bi_fixtures::sales::FixtureSpec;
use bi_runtime::{Bootstrap, LakeConfig, RuntimeConfig};
use polars::prelude::*;
use serde::Serialize;

use crate::results::{Query, Row};

pub const TABLE: &str = "sales_north_south";

#[derive(Debug, Serialize)]
pub struct Report {
    pub description: &'static str,
    pub rows: usize,
    pub staging_parquet_bytes: String,
    pub phases: Vec<Phase>,
    /// The published table, read back by DuckDB, matches the oracle.
    pub exact: bool,
}

/// Transforms with Polars and publishes into the lake `iterations` times
/// (after one warmup), then verifies the published table. Runs on the
/// calling thread; only the verification query uses a Tokio runtime.
///
/// # Errors
/// Polars, I/O or lake errors (e.g. PostgreSQL unreachable).
pub fn run(
    parquet: &Path,
    lake: LakeConfig,
    spec: FixtureSpec,
    iterations: usize,
    expected: &[Row],
) -> Result<Report, bi_fixtures::Error> {
    let scratch = tempfile::tempdir()?;
    let staging = scratch.path().join("staging.parquet");
    let bootstrap = Bootstrap::open(RuntimeConfig {
        lake: Some(lake),
        ..RuntimeConfig::in_memory(1)
    })?;

    let mut transform = Timings::default();
    let mut write = Timings::default();
    let mut commit = Timings::default();
    let mut rows = 0;
    for round in 0..=iterations {
        let measured = round > 0;
        // A "model": the rows of two regions, all columns, in pure Polars.
        let (frame, elapsed) = timed(|| {
            crate::pola::scan(parquet)?
                .filter(
                    col("region")
                        .eq(lit("north"))
                        .or(col("region").eq(lit("south"))),
                )
                .collect()
        });
        let mut frame = frame?;
        rows = frame.height();
        transform.record(measured, elapsed, None);

        let (bytes, elapsed) =
            timed(|| ParquetWriter::new(File::create(&staging)?).finish(&mut frame));
        write.record(measured, elapsed, Some(bytes?));

        // A new fingerprint per round forces a real, atomic replacement.
        let fingerprint = format!(
            "north_south:seed={}:rows={}:round={round}",
            spec.seed, spec.rows
        );
        let (outcome, elapsed) =
            timed(|| bootstrap.load_parquet_fixture(TABLE, &staging, &fingerprint));
        outcome?;
        commit.record(measured, elapsed, None);
    }
    let staging_bytes = std::fs::metadata(&staging)?.len();

    let runtime = bootstrap.finish()?;
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let published: Vec<Row> = tokio.block_on(runtime.run(|conn| {
        conn.prepare(&format!(
            "SELECT region, CAST(sum(amount) AS VARCHAR) FROM lake.main.{TABLE} \
                 GROUP BY region ORDER BY region"
        ))?
        .query_map([], |row| Ok(vec![row.get(0)?, row.get(1)?]))?
        .collect()
    }))?;

    Ok(Report {
        description: "Polars filter (2 regions, all columns) → staging Parquet → DuckDB INSERT into DuckLake in one transaction",
        rows,
        staging_parquet_bytes: staging_bytes.to_string(),
        phases: vec![
            transform.phase("polars_transform"),
            write.phase("polars_write_staging_parquet"),
            commit.phase("ducklake_transactional_insert"),
        ],
        exact: published == expected,
    })
}

/// The rows the published table must aggregate to.
#[must_use]
pub fn expected(oracle: &bi_fixtures::oracle::Oracle) -> Vec<Row> {
    Query::NorthSouth.expected(oracle)
}
