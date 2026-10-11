//! DuckLake over a real PostgreSQL catalog. Run with
//! `cargo test --features postgres-tests`; the catalog database comes from
//! `BI_TEST_LAKE_CATALOG` (default: `bi_lake_catalog` on the development
//! PostgreSQL). Each test uses its own metadata schema and data directory.
#![cfg(feature = "postgres-tests")]

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use bi_fixtures::oracle::Oracle;
use bi_fixtures::sales::{self, FixtureSpec};
use bi_runtime::config::DEV_LAKE_CATALOG;
use bi_runtime::fixtures::{load_sales, sales_oracle};
use bi_runtime::{Bootstrap, LakeConfig, LoadOutcome, RuntimeConfig, RuntimeError};

fn lake_config(data_path: &Path) -> RuntimeConfig {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let schema = format!(
        "bi_test_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    RuntimeConfig {
        lake: Some(LakeConfig {
            catalog: std::env::var("BI_TEST_LAKE_CATALOG")
                .unwrap_or_else(|_| DEV_LAKE_CATALOG.to_owned()),
            metadata_schema: Some(schema),
            data_path: data_path.to_owned(),
        }),
        ..RuntimeConfig::in_memory(2)
    }
}

fn expected(spec: FixtureSpec) -> Oracle {
    Oracle::from_rows(spec, sales::rows(spec))
}

fn assert_matches_oracle(actual: &Oracle, spec: FixtureSpec) {
    let expected = expected(spec);
    assert_eq!(actual.totals, expected.totals);
    assert_eq!(actual.by_region, expected.by_region);
    let types = |oracle: &Oracle| -> Vec<(String, String)> {
        oracle
            .columns
            .iter()
            .map(|c| (c.name.clone(), c.sql_type.clone()))
            .collect()
    };
    assert_eq!(types(actual), types(&expected));
}

fn files_by_extension(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(
                    path.extension()
                        .map_or_else(String::new, |e| e.to_string_lossy().into_owned()),
                );
            }
        }
    }
    found.sort();
    found
}

#[tokio::test(flavor = "multi_thread")]
async fn lake_reproduces_the_oracle_and_survives_a_restart() {
    let data = tempfile::tempdir().unwrap();
    let config = lake_config(data.path());
    let spec = FixtureSpec {
        seed: 42,
        rows: 200_000,
    };

    let bootstrap = Bootstrap::open(config.clone()).unwrap();
    assert_eq!(load_sales(&bootstrap, spec).unwrap(), LoadOutcome::Loaded);
    let runtime = bootstrap.finish().unwrap();
    assert_matches_oracle(&sales_oracle(&runtime, spec).await.unwrap(), spec);
    drop(runtime);
    let files_after_load = files_by_extension(data.path());
    assert!(files_after_load.contains(&"parquet".to_owned()));
    assert!(
        files_after_load.iter().all(|ext| ext == "parquet"),
        "only Parquet in DATA_PATH, no .duckdb: {files_after_load:?}"
    );

    // A new process: attach the same catalog and data path, no reload.
    let bootstrap = Bootstrap::open(config).unwrap();
    assert_eq!(
        load_sales(&bootstrap, spec).unwrap(),
        LoadOutcome::AlreadyLoaded
    );
    let runtime = bootstrap.finish().unwrap();
    assert_matches_oracle(&sales_oracle(&runtime, spec).await.unwrap(), spec);
    assert_eq!(files_by_extension(data.path()), files_after_load);
}

#[tokio::test(flavor = "multi_thread")]
async fn loading_a_different_fixture_replaces_the_table() {
    let data = tempfile::tempdir().unwrap();
    let config = lake_config(data.path());
    let first = FixtureSpec {
        seed: 1,
        rows: 20_000,
    };
    let second = FixtureSpec {
        seed: 2,
        rows: 30_000,
    };

    let bootstrap = Bootstrap::open(config.clone()).unwrap();
    assert_eq!(load_sales(&bootstrap, first).unwrap(), LoadOutcome::Loaded);
    drop(bootstrap);

    let bootstrap = Bootstrap::open(config).unwrap();
    assert_eq!(load_sales(&bootstrap, second).unwrap(), LoadOutcome::Loaded);
    let runtime = bootstrap.finish().unwrap();
    assert_matches_oracle(&sales_oracle(&runtime, second).await.unwrap(), second);
}

#[tokio::test(flavor = "multi_thread")]
async fn locked_down_connections_read_the_lake_but_nothing_else() {
    let data = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        seed: 3,
        rows: 10_000,
    };
    bi_fixtures::parquet_io::write(
        &outside.path().join("other.parquet"),
        sales::schema(),
        sales::record_batches(spec, 4096),
    )
    .unwrap();

    let bootstrap = Bootstrap::open(lake_config(data.path())).unwrap();
    load_sales(&bootstrap, spec).unwrap();
    let runtime = bootstrap.finish().unwrap();

    // Reading lake data (not just metadata) works after the lockdown.
    assert_matches_oracle(&sales_oracle(&runtime, spec).await.unwrap(), spec);

    let other = outside
        .path()
        .join("other.parquet")
        .to_string_lossy()
        .replace('\'', "''");
    let read_outside = runtime
        .run(move |conn| {
            conn.query_row(
                &format!("SELECT count(*) FROM read_parquet('{other}')"),
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .await;
    assert!(
        matches!(read_outside, Err(RuntimeError::Engine(_))),
        "{read_outside:?}"
    );
}

#[test]
fn an_unreachable_catalog_is_reported_without_credentials() {
    for (catalog, secret) in [
        (
            "dbname=bi_lake_catalog host=127.0.0.1 port=1 user=bi password=s3cret",
            "s3cret",
        ),
        (
            "dbname=bi_lake_catalog host=127.0.0.1 port=1 user=bi password='s3cret with spaces'",
            "with spaces",
        ),
        (
            "postgresql://bi:p%40ss%20word@127.0.0.1:1/bi_lake_catalog",
            "p%40ss",
        ),
    ] {
        let data = tempfile::tempdir().unwrap();
        let mut config = lake_config(data.path());
        if let Some(lake) = &mut config.lake {
            catalog.clone_into(&mut lake.catalog);
        }
        let Err(RuntimeError::Lake(message)) = Bootstrap::open(config) else {
            panic!("attaching an unreachable catalog must fail: {catalog}");
        };
        assert!(!message.contains(secret), "{message}");
        assert!(!message.contains("p@ss word"), "{message}");
    }
}
