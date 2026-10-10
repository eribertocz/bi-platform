//! Minimal benchmark harness (G0.13): machine profile, commit, versions,
//! timings (p50/p95), sampled peak RSS and a correctness check, emitted as
//! JSON in the shape of `PERFORMANCE_DESDE_G0.md`. It records baselines; it
//! does not enforce thresholds.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use arrow_array::RecordBatch;
use serde::Serialize;
use sysinfo::{
    CpuRefreshKind, MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System,
};

use crate::oracle::Oracle;
use crate::sales::{self, FixtureSpec};
use crate::{Error, parquet_io, typed_json};

pub const BATCH_ROWS: usize = 64 * 1024;
const RSS_SAMPLE_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Debug, Clone, Copy)]
pub struct BenchOptions {
    pub spec: FixtureSpec,
    pub iterations: usize,
    pub warmup: usize,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub task: String,
    pub sha: String,
    pub machine: Machine,
    pub versions: Versions,
    pub seed: String,
    pub rows: String,
    pub iterations: usize,
    pub warmup: usize,
    pub phases: Vec<Phase>,
    pub rss_peak_bytes_sampled: String,
    pub rss_sample_interval_ms: u64,
    pub correctness: Correctness,
    pub limitations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Machine {
    pub os: String,
    pub arch: String,
    pub cpu: String,
    pub logical_cpus: usize,
    pub ram_bytes: String,
}

#[derive(Debug, Serialize)]
pub struct Versions {
    pub rustc: String,
    pub bi_fixtures: String,
    pub parquet: String,
}

#[derive(Debug, Serialize)]
pub struct Phase {
    pub name: String,
    pub samples: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    /// Output size, when the phase produces bytes.
    pub bytes: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Correctness {
    /// The oracle recomputed from the Parquet file equals the generator's oracle.
    pub oracle_matches_parquet_roundtrip: bool,
}

/// Runs every phase `warmup + iterations` times, writing scratch files in `workdir`.
///
/// # Errors
/// I/O or Parquet errors while writing or reading the scratch file.
///
/// # Panics
/// If `iterations` is zero.
pub fn run(options: BenchOptions, workdir: &Path) -> Result<Report, Error> {
    assert!(options.iterations > 0, "at least one iteration");
    let spec = options.spec;
    let parquet_path = workdir.join("bench-sales.parquet");
    let rss = RssSampler::start();

    let mut generate = Timings::default();
    let mut oracle_phase = Timings::default();
    let mut write = Timings::default();
    let mut read = Timings::default();
    let mut json = Timings::default();
    let mut correct = true;

    for round in 0..options.warmup + options.iterations {
        let measured = round >= options.warmup;

        let (batches, elapsed) =
            timed(|| sales::record_batches(spec, BATCH_ROWS).collect::<Vec<_>>());
        generate.record(measured, elapsed, None);

        let (oracle, elapsed) = timed(|| Oracle::from_rows(spec, sales::rows(spec)));
        oracle_phase.record(measured, elapsed, None);

        let (size, elapsed) =
            timed(|| parquet_io::write(&parquet_path, sales::schema(), batches.iter().cloned()));
        write.record(measured, elapsed, Some(size?));

        let (read_back, elapsed) = timed(|| parquet_io::read(&parquet_path));
        let read_back: Vec<RecordBatch> = read_back?;
        read.record(measured, elapsed, None);

        let (bytes, elapsed) = timed(|| typed_json::serialize(&read_back));
        json.record(measured, elapsed, Some(bytes.len() as u64));

        correct &= Oracle::from_batches(spec, &read_back) == oracle;
    }
    std::fs::remove_file(&parquet_path)?;
    let rss_peak = rss.stop();

    Ok(Report {
        task: "G0.13 baseline: sales fixture generate/oracle/parquet/typed JSON".to_owned(),
        sha: commit_sha(),
        machine: machine(),
        versions: Versions {
            rustc: env!("BI_RUSTC_VERSION").to_owned(),
            bi_fixtures: env!("CARGO_PKG_VERSION").to_owned(),
            parquet: parquet::file::properties::DEFAULT_CREATED_BY.to_owned(),
        },
        seed: spec.seed.to_string(),
        rows: spec.rows.to_string(),
        iterations: options.iterations,
        warmup: options.warmup,
        phases: vec![
            generate.phase("generate_arrow"),
            oracle_phase.phase("oracle_exact"),
            write.phase("parquet_write_snappy"),
            read.phase("parquet_read"),
            json.phase("json_typed_serialize"),
        ],
        rss_peak_bytes_sampled: rss_peak.to_string(),
        rss_sample_interval_ms: u64::try_from(RSS_SAMPLE_INTERVAL.as_millis()).unwrap_or(u64::MAX),
        correctness: Correctness {
            oracle_matches_parquet_roundtrip: correct,
        },
        limitations: vec![
            "Peak RSS is sampled, so spikes shorter than the interval can be missed.".to_owned(),
            "Single process on a shared machine; compare only runs from the same machine."
                .to_owned(),
            "Few samples: p95 approaches the maximum below ~20 iterations.".to_owned(),
        ],
    })
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let value = f();
    (value, started.elapsed())
}

#[derive(Default)]
struct Timings {
    samples: Vec<Duration>,
    bytes: Option<u64>,
}

impl Timings {
    fn record(&mut self, measured: bool, elapsed: Duration, bytes: Option<u64>) {
        if measured {
            self.samples.push(elapsed);
            self.bytes = bytes.or(self.bytes);
        }
    }

    fn phase(mut self, name: &str) -> Phase {
        self.samples.sort_unstable();
        let ms = |d: Duration| (d.as_secs_f64() * 1_000_000.0).round() / 1_000.0;
        Phase {
            name: name.to_owned(),
            samples: self.samples.len(),
            p50_ms: ms(percentile(&self.samples, 50)),
            p95_ms: ms(percentile(&self.samples, 95)),
            min_ms: ms(self.samples[0]),
            max_ms: ms(self.samples[self.samples.len() - 1]),
            bytes: self.bytes.map(|b| b.to_string()),
        }
    }
}

/// Nearest-rank percentile of an ascending, non-empty slice.
fn percentile(sorted: &[Duration], percent: usize) -> Duration {
    let rank = (percent * sorted.len()).div_ceil(100).max(1);
    sorted[rank.min(sorted.len()) - 1]
}

/// Samples this process's resident set size on a background thread.
struct RssSampler {
    stop: Arc<AtomicBool>,
    peak: Arc<AtomicU64>,
    handle: thread::JoinHandle<()>,
}

impl RssSampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(0));
        let handle = {
            let (stop, peak) = (Arc::clone(&stop), Arc::clone(&peak));
            thread::spawn(move || {
                let Ok(pid) = sysinfo::get_current_pid() else {
                    return;
                };
                let mut system = System::new();
                let refresh = ProcessRefreshKind::nothing().with_memory();
                loop {
                    system.refresh_processes_specifics(
                        ProcessesToUpdate::Some(&[pid]),
                        false,
                        refresh,
                    );
                    if let Some(process) = system.process(pid) {
                        peak.fetch_max(process.memory(), Ordering::Relaxed);
                    }
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(RSS_SAMPLE_INTERVAL);
                }
            })
        };
        Self { stop, peak, handle }
    }

    fn stop(self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.join().ok();
        self.peak.load(Ordering::Relaxed)
    }
}

fn machine() -> Machine {
    let system = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::nothing())
            .with_memory(MemoryRefreshKind::nothing().with_ram()),
    );
    Machine {
        os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_owned()),
        arch: std::env::consts::ARCH.to_owned(),
        cpu: system
            .cpus()
            .first()
            .map_or_else(|| "unknown".to_owned(), |cpu| cpu.brand().trim().to_owned()),
        logical_cpus: system.cpus().len(),
        ram_bytes: system.total_memory().to_string(),
    }
}

/// `GITHUB_SHA` in CI, otherwise `git rev-parse HEAD`, otherwise `unknown`.
fn commit_sha() -> String {
    if let Ok(sha) = std::env::var("GITHUB_SHA") {
        return sha;
    }
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unknown".to_owned(), |sha| sha.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(values: &[u64]) -> Vec<Duration> {
        values.iter().map(|&v| Duration::from_millis(v)).collect()
    }

    #[test]
    fn nearest_rank_percentiles() {
        let samples = ms(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(percentile(&samples, 50), Duration::from_millis(5));
        assert_eq!(percentile(&samples, 95), Duration::from_millis(10));
        assert_eq!(percentile(&ms(&[7]), 95), Duration::from_millis(7));
    }
}
