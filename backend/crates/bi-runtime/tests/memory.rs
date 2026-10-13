use bi_runtime::{Bootstrap, RuntimeConfig};
use std::time::Instant;

#[tokio::test]
#[ignore = "latency measurement; run with --ignored --nocapture --test-threads=1"]
async fn query_memory_latency() {
    let start = Instant::now();
    let runtime = Bootstrap::open(RuntimeConfig::in_memory(2))
        .unwrap()
        .finish()
        .unwrap();
    println!("startup_ms={:.3}", start.elapsed().as_secs_f64() * 1000.0);
    let mut times = Vec::new();
    for _ in 0..25 {
        let start = Instant::now();
        let sum = runtime
            .run(|conn| {
                conn.query_row(
                    "SELECT sum(i)::BIGINT FROM range(1000000) t(i)",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(sum, 499_999_500_000);
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    println!(
        "streaming_1m_p50_ms={:.3} p95_ms={:.3}",
        times[12], times[23]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_query_has_its_own_budget_and_other_worker_keeps_answering() {
    let runtime = Bootstrap::open(RuntimeConfig {
        query_memory_limit: Some("16MiB".to_owned()),
        duckdb_threads: Some(2),
        ..RuntimeConfig::in_memory(2)
    })
    .unwrap()
    .finish()
    .unwrap();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let big = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            runtime
                .run(move |conn| {
                    conn.execute_batch(
                        "CREATE TABLE pressure AS SELECT random() AS x FROM range(1000000)",
                    )?;
                    started.send(()).unwrap();
                    // Hold managed buffers while the other worker allocates its own.
                    wait.recv().unwrap();
                    conn.query_row(
                        "SELECT count(*) FROM (SELECT DISTINCT i FROM range(10000000) t(i))",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                })
                .await
        }
    });
    ready.await.unwrap();
    let answer = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        runtime.run(|conn| {
            conn.execute_batch(
                "CREATE TABLE pressure AS SELECT random() AS x FROM range(1000000)",
            )?;
            conn.query_row("SELECT count(*) FROM pressure", [], |row| {
                row.get::<_, i64>(0)
            })
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(answer, 1_000_000);
    release.send(()).unwrap();
    assert!(matches!(
        big.await.unwrap(),
        Err(bi_runtime::RuntimeError::MemoryLimit)
    ));
    assert_eq!(runtime.load().workers, 2);
    assert_eq!(
        runtime
            .run(|conn| conn.query_row("SELECT 7", [], |row| row.get::<_, i64>(0)))
            .await
            .unwrap(),
        7
    );
}

#[tokio::test]
async fn memory_rejection_does_not_retire_or_unlock_the_only_worker() {
    let runtime = Bootstrap::open(RuntimeConfig {
        query_memory_limit: Some("16MiB".to_owned()),
        duckdb_threads: Some(1),
        ..RuntimeConfig::in_memory(1)
    })
    .unwrap()
    .finish()
    .unwrap();
    for _ in 0..3 {
        let result = runtime
            .run(|conn| {
                conn.query_row(
                    "SELECT count(*) FROM (SELECT DISTINCT i FROM range(10000000) t(i))",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .await;
        assert!(
            matches!(result, Err(bi_runtime::RuntimeError::MemoryLimit)),
            "{result:?}"
        );
        assert_eq!(
            runtime
                .run(|conn| conn.query_row("SELECT 17", [], |row| row.get::<_, i64>(0)))
                .await
                .unwrap(),
            17
        );
        assert!(
            runtime
                .run(|conn| conn.execute_batch("SET memory_limit='1TB'"))
                .await
                .is_err()
        );
        assert_eq!(runtime.load().workers, 1);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_instances_are_distinct_and_share_no_temporary_catalog() {
    let runtime = Bootstrap::open(RuntimeConfig {
        query_memory_limit: Some("32MiB".to_owned()),
        ..RuntimeConfig::in_memory(2)
    })
    .unwrap()
    .finish()
    .unwrap();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let owner = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            runtime
                .run(move |conn| {
                    conn.execute_batch("CREATE TABLE only_this_instance AS SELECT 1")?;
                    started.send(()).unwrap();
                    wait.recv().unwrap();
                    Ok(())
                })
                .await
        }
    });
    ready.await.unwrap();
    let tables = runtime
        .run(|conn| {
            conn.query_row(
                "SELECT count(*) FROM duckdb_tables() WHERE table_name='only_this_instance'",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(tables, 0);
    release.send(()).unwrap();
    owner.await.unwrap().unwrap();
}
