//! DuckDB SQL vs Polars LazyFrame on the `sales` fixture (G1, issue #15).
//! Exactness against the oracle first; then build/plan/execute/serialize
//! timings, cold (first run) and warm, and the Polars → DuckLake publication
//! boundary. Writes a JSON report in the `PERFORMANCE_DESDE_G0.md` shape.

mod duck;
mod pola;
mod publish;
mod results;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use bi_fixtures::bench::{self, Machine, Phase, RssSampler, Timings};
use bi_fixtures::oracle::Oracle;
use bi_fixtures::parquet_io;
use bi_fixtures::sales::{self, FixtureSpec};
use bi_runtime::RuntimeConfig;
use bi_runtime::duckdb::Connection;
use serde::Serialize;

use results::{Query, Row};

const USAGE: &str =
    "usage: bi-engines [--rows N] [--seed N] [--iterations N] [--out FILE] [--no-publish]
  Compares DuckDB SQL and Polars LazyFrame on the synthetic sales fixture.
  The publication step needs the DuckLake catalog configured by BI_LAKE_*
  (default: the development PostgreSQL); --no-publish skips it.
  defaults: --rows 1000000 --seed 42 --iterations 10";

#[derive(Debug, Serialize)]
struct Report {
    task: &'static str,
    sha: String,
    machine: Machine,
    versions: Versions,
    seed: String,
    rows: String,
    parquet_bytes: String,
    iterations: usize,
    results: Vec<EngineResult>,
    findings: Vec<String>,
    publication: serde_json::Value,
    rss_peak_bytes_sampled: String,
    limitations: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct Versions {
    rustc: &'static str,
    duckdb: String,
    polars: &'static str,
}

#[derive(Debug, Serialize)]
struct EngineResult {
    engine: &'static str,
    query: &'static str,
    /// The result equals the oracle value by value.
    exact: bool,
    /// First run in the process: build + plan + execute + serialize.
    cold_total_ms: f64,
    /// Warm runs, per phase.
    phases: Vec<Phase>,
}

struct Args {
    spec: FixtureSpec,
    iterations: usize,
    out: Option<PathBuf>,
    publish: bool,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args {
        spec: FixtureSpec {
            seed: 42,
            rows: 1_000_000,
        },
        iterations: 10,
        out: None,
        publish: true,
    };
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        if flag == "--no-publish" {
            parsed.publish = false;
            continue;
        }
        let value = iter.next().ok_or_else(|| format!("{flag} needs a value"))?;
        let number = || {
            value
                .parse::<u64>()
                .map_err(|_| format!("{flag} expects a non-negative integer"))
        };
        match flag.as_str() {
            "--rows" => parsed.spec.rows = number()?,
            "--seed" => parsed.spec.seed = number()?,
            "--iterations" => {
                parsed.iterations = usize::try_from(number()?)
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--iterations must be at least 1")?;
            }
            "--out" => parsed.out = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown flag {flag}")),
        }
    }
    Ok(parsed)
}

/// Phases of one run of either engine.
struct Sample {
    build: Duration,
    plan: Duration,
    execute: Duration,
    serialize: Duration,
}

fn measure(
    engine: &'static str,
    query: Query,
    iterations: usize,
    expected: &[Row],
    mut run: impl FnMut() -> Result<(Sample, Vec<Row>), String>,
) -> Result<EngineResult, String> {
    let mut phases: [Timings; 4] = Default::default();
    let mut cold = Duration::ZERO;
    let mut exact = true;
    for round in 0..=iterations {
        let (sample, rows) = run()?;
        exact &= rows == expected;
        let parts = [sample.build, sample.plan, sample.execute, sample.serialize];
        if round == 0 {
            cold = parts.iter().sum();
        }
        for (timings, elapsed) in phases.iter_mut().zip(parts) {
            timings.record(round > 0, elapsed, None);
        }
    }
    let [build, plan, execute, serialize] = phases;
    Ok(EngineResult {
        engine,
        query: query.name(),
        exact,
        cold_total_ms: (cold.as_secs_f64() * 1_000_000.0).round() / 1_000.0,
        phases: vec![
            build.phase("build"),
            plan.phase("plan"),
            execute.phase("execute"),
            serialize.phase("serialize"),
        ],
    })
}

/// Runs both engines on this thread: neither DuckDB nor Polars may run on a
/// Tokio executor (Polars refuses to; DuckDB would block it).
fn run(args: &Args) -> Result<Report, bi_fixtures::Error> {
    let spec = args.spec;
    let scratch = tempfile::tempdir()?;
    let parquet = scratch.path().join("sales.parquet");
    let parquet_bytes = parquet_io::write(
        &parquet,
        sales::schema(),
        sales::record_batches(spec, bench::BATCH_ROWS),
    )?;
    let oracle = Oracle::from_rows(spec, sales::rows(spec));
    let rss = RssSampler::start();

    let conn = Connection::open_in_memory()?;
    let duckdb_version: String = conn.query_row("SELECT version()", [], |row| row.get(0))?;
    let mut results = Vec::new();
    for query in Query::ALL {
        let expected = query.expected(&oracle);
        results.push(measure(
            "duckdb_sql",
            query,
            args.iterations,
            &expected,
            || {
                let run = duck::run(&conn, query, &parquet).map_err(|e| e.to_string())?;
                Ok((
                    Sample {
                        build: run.build,
                        plan: run.plan,
                        execute: run.execute,
                        serialize: run.serialize,
                    },
                    run.rows,
                ))
            },
        )?);
        results.push(measure(
            "polars_lazy",
            query,
            args.iterations,
            &expected,
            || {
                let run = pola::run(query, &parquet).map_err(|e| e.to_string())?;
                Ok((
                    Sample {
                        build: run.build,
                        plan: run.plan,
                        execute: run.execute,
                        serialize: run.serialize,
                    },
                    run.rows,
                ))
            },
        )?);
    }

    let mut findings = Vec::new();
    let naive = pola::naive_order_id_sum(&parquet)?;
    if naive != oracle.totals.order_id.sum {
        findings.push(format!(
            "Polars sums Int64 in 64 bits and wraps on overflow without error: \
             order_id sum without a cast = {}, oracle = {}. Every integer sum must \
             cast to Int128 first; DuckDB widens SUM(BIGINT) to HUGEINT by itself.",
            naive.as_deref().unwrap_or("null"),
            oracle.totals.order_id.sum.as_deref().unwrap_or("null"),
        ));
    }

    let publication = if args.publish {
        match publication(&parquet, spec, args.iterations, &oracle, scratch.path()) {
            Ok(report) => serde_json::to_value(report)?,
            Err(error) => serde_json::json!({ "skipped": error.to_string() }),
        }
    } else {
        serde_json::json!({ "skipped": "--no-publish" })
    };

    Ok(Report {
        task: "G1 engines: DuckDB SQL vs Polars LazyFrame on the sales fixture",
        sha: bench::commit_sha(),
        machine: bench::machine(),
        versions: Versions {
            rustc: bench::RUSTC_VERSION,
            duckdb: duckdb_version,
            polars: polars::VERSION,
        },
        seed: spec.seed.to_string(),
        rows: spec.rows.to_string(),
        parquet_bytes: parquet_bytes.to_string(),
        iterations: args.iterations,
        results,
        findings,
        publication,
        rss_peak_bytes_sampled: rss.stop().to_string(),
        limitations: vec![
            "Both engines read the same Parquet file in one process; the OS file cache is warm after the first run, so cold only means the first run in the process.",
            "Polars plan time is describe_optimized_plan(); collect() optimizes again, so its execute includes optimization.",
            "Peak RSS is sampled and covers both engines in the same process.",
            "Compare only runs from the same machine; few samples make p95 approach the maximum.",
        ],
    })
}

fn publication(
    parquet: &Path,
    spec: FixtureSpec,
    iterations: usize,
    oracle: &Oracle,
    scratch: &Path,
) -> Result<publish::Report, bi_fixtures::Error> {
    let config = RuntimeConfig::from_lookup(|key| std::env::var(key).ok())?;
    let mut lake = config.lake.ok_or("no lake configured")?;
    // Its own metadata schema and data directory: never touches other lakes.
    lake.metadata_schema = Some(format!("bi_engines_{}", std::process::id()));
    lake.data_path = scratch.join("lake");
    publish::run(parquet, lake, spec, iterations, &publish::expected(oracle))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse(&args) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let out = args.out.clone();
    let report = match run(&args) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let exact = report.results.iter().all(|r| r.exact);
    let json = serde_json::to_string_pretty(&report).expect("report serializes") + "\n";
    let written = if let Some(path) = out {
        std::fs::write(&path, json)
    } else {
        print!("{json}");
        Ok(())
    };
    if let Err(error) = written {
        eprintln!("error: {error}");
        return ExitCode::FAILURE;
    }
    if exact {
        ExitCode::SUCCESS
    } else {
        eprintln!("error: an engine result differs from the oracle");
        ExitCode::FAILURE
    }
}
