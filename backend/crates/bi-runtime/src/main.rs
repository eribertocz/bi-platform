use std::process::ExitCode;

use bi_fixtures::sales::FixtureSpec;
use bi_runtime::fixtures::{self, sales_oracle};
use bi_runtime::{Bootstrap, LoadOutcome, RuntimeConfig};

const USAGE: &str = "usage: bi-runtime load-sales [--seed N] [--rows N]
  Loads the synthetic sales fixture into the DuckLake configured by BI_LAKE_*
  (default: the development PostgreSQL of compose.yaml) and verifies it
  against the exact oracle. Idempotent.";

fn parse(args: &[String]) -> Result<FixtureSpec, String> {
    let mut spec = FixtureSpec {
        seed: 42,
        rows: 1_000_000,
    };
    let [command, flags @ ..] = args else {
        return Err("missing command".to_owned());
    };
    if command != "load-sales" {
        return Err(format!("unknown command {command:?}"));
    }
    for pair in flags.chunks(2) {
        let [flag, value] = pair else {
            return Err(format!("{} needs a value", pair[0]));
        };
        let value: u64 = value
            .parse()
            .map_err(|_| format!("{flag} expects a non-negative integer"))?;
        match flag.as_str() {
            "--seed" => spec.seed = value,
            "--rows" => spec.rows = value,
            _ => return Err(format!("unknown flag {flag}")),
        }
    }
    Ok(spec)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let spec = match parse(&args) {
        Ok(spec) => spec,
        Err(error) => {
            eprintln!("{error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(spec) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Bootstraps and loads on this thread (native, blocking work); only the
/// verification query runs on a Tokio executor.
fn run(spec: FixtureSpec) -> Result<(), Box<dyn std::error::Error>> {
    let config = RuntimeConfig::from_lookup(|key| std::env::var(key).ok())?;
    let bootstrap = Bootstrap::open(config)?;
    let outcome = fixtures::load_sales(&bootstrap, spec)?;
    if outcome == LoadOutcome::Altered {
        return Err(
            "the lake marks this fixture as loaded but its rows differ; left untouched".into(),
        );
    }
    let runtime = bootstrap.finish()?;
    let actual = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(sales_oracle(&runtime, spec))?;
    let expected = bi_fixtures::oracle::Oracle::from_rows(spec, bi_fixtures::sales::rows(spec));
    if actual.totals != expected.totals || actual.by_region != expected.by_region {
        return Err("lake contents do not match the oracle".into());
    }
    println!(
        "sales fixture (seed {}, {} rows): {outcome:?}; lake matches the oracle",
        spec.seed, spec.rows
    );
    Ok(())
}
