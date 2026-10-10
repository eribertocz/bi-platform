//! Concurrent benchmark runs in the same parent directory never share
//! scratch files and leave nothing behind.

use bi_fixtures::bench::{self, BenchOptions};
use bi_fixtures::sales::FixtureSpec;

#[test]
fn concurrent_runs_are_isolated_and_clean_up() {
    let parent = tempfile::tempdir().unwrap();
    let reports: Vec<_> = std::thread::scope(|scope| {
        let runs = [42, 7].map(|seed| {
            let parent = parent.path();
            scope.spawn(move || {
                bench::run(
                    BenchOptions {
                        spec: FixtureSpec { seed, rows: 50_000 },
                        iterations: 3,
                        warmup: 0,
                    },
                    parent,
                )
            })
        });
        runs.into_iter()
            .map(|run| run.join().unwrap().unwrap())
            .collect()
    });

    for (report, seed) in reports.iter().zip(["42", "7"]) {
        assert_eq!(report.seed, seed);
        assert!(report.correctness.oracle_matches_parquet_roundtrip);
    }
    let leftovers: Vec<_> = std::fs::read_dir(parent.path()).unwrap().collect();
    assert!(leftovers.is_empty(), "scratch left behind: {leftovers:?}");
}
