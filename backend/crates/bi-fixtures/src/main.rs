use std::path::PathBuf;
use std::process::ExitCode;

use bi_fixtures::bench::{self, BenchOptions};
use bi_fixtures::oracle::Oracle;
use bi_fixtures::sales::{self, FixtureSpec};
use bi_fixtures::{Error, parquet_io};

const USAGE: &str = "usage:
  bi-fixtures generate [--seed N] [--rows N] --out DIR
      writes DIR/sales.parquet and DIR/sales.oracle.json
  bi-fixtures oracle [--seed N] [--rows N] [--out FILE]
      writes the oracle JSON to FILE or stdout
  bi-fixtures bench [--seed N] [--rows N] [--iterations N] [--warmup N] [--out FILE]
      runs the benchmark and writes the JSON report to FILE or stdout

defaults: --seed 42 --rows 1000000 --iterations 10 --warmup 1";

#[derive(Debug)]
struct Args {
    command: String,
    spec: FixtureSpec,
    iterations: usize,
    warmup: usize,
    out: Option<PathBuf>,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let command = args.next().ok_or("missing command")?;
    let mut parsed = Args {
        command,
        spec: FixtureSpec {
            seed: 42,
            rows: 1_000_000,
        },
        iterations: 10,
        warmup: 1,
        out: None,
    };
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        let number = || {
            value
                .parse::<u64>()
                .map_err(|_| format!("{flag} expects a non-negative integer, got {value:?}"))
        };
        match flag.as_str() {
            "--seed" => parsed.spec.seed = number()?,
            "--rows" => parsed.spec.rows = number()?,
            "--iterations" => {
                parsed.iterations = usize::try_from(number()?).map_err(|e| e.to_string())?;
            }
            "--warmup" => parsed.warmup = usize::try_from(number()?).map_err(|e| e.to_string())?,
            "--out" => parsed.out = Some(PathBuf::from(&value)),
            _ => return Err(format!("unknown flag {flag}")),
        }
    }
    Ok(parsed)
}

fn run(args: Args) -> Result<(), Error> {
    let spec = args.spec;
    match args.command.as_str() {
        "generate" => {
            let dir = args.out.ok_or("generate needs --out DIR")?;
            std::fs::create_dir_all(&dir)?;
            parquet_io::write(
                &dir.join("sales.parquet"),
                sales::schema(),
                sales::record_batches(spec, bench::BATCH_ROWS),
            )?;
            let oracle = Oracle::from_rows(spec, sales::rows(spec));
            std::fs::write(dir.join("sales.oracle.json"), oracle.to_json())?;
        }
        "oracle" => emit(
            args.out,
            &Oracle::from_rows(spec, sales::rows(spec)).to_json(),
        )?,
        "bench" => {
            if args.iterations == 0 {
                return Err("--iterations must be at least 1".into());
            }
            let report = bench::run(
                BenchOptions {
                    spec,
                    iterations: args.iterations,
                    warmup: args.warmup,
                },
                &std::env::temp_dir(),
            )?;
            if !report.correctness.oracle_matches_parquet_roundtrip {
                return Err("Parquet round trip does not match the oracle".into());
            }
            let mut json = serde_json::to_string_pretty(&report)?;
            json.push('\n');
            emit(args.out, &json)?;
        }
        other => return Err(format!("unknown command {other:?}\n{USAGE}").into()),
    }
    Ok(())
}

fn emit(out: Option<PathBuf>, content: &str) -> std::io::Result<()> {
    if let Some(path) = out {
        return std::fs::write(path, content);
    }
    print!("{content}");
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
