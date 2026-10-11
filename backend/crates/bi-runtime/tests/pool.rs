//! Query pool behaviour on an in-memory database: results, cancellation,
//! admission, fault isolation and the post-bootstrap lockdown.

use std::time::{Duration, Instant};

use bi_runtime::{Bootstrap, Runtime, RuntimeConfig, RuntimeError};

/// Runs for hours unless interrupted.
const ENDLESS: &str = "SELECT sum(a.range * b.range) FROM range(10000000) a, range(10000000) b";

fn runtime(workers: usize, queue_capacity: usize) -> Runtime {
    Bootstrap::open(RuntimeConfig {
        queue_capacity,
        ..RuntimeConfig::in_memory(workers)
    })
    .unwrap()
    .finish()
    .unwrap()
}

async fn scalar(runtime: &Runtime, sql: &'static str) -> Result<i64, RuntimeError> {
    runtime
        .run(move |conn| conn.query_row(sql, [], |row| row.get(0)))
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_queries_return_their_own_results() {
    let runtime = runtime(4, 16);
    let tasks: Vec<_> = (1..=16_i64)
        .map(|n| {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .run(move |conn| {
                        conn.query_row("SELECT sum(range) FROM range(?)", [n * 1000], |row| {
                            row.get::<_, i64>(0)
                        })
                    })
                    .await
                    .map(|sum| (n, sum))
            })
        })
        .collect();
    for task in tasks {
        let (n, sum) = task.await.unwrap().unwrap();
        let m = n * 1000;
        assert_eq!(sum, m * (m - 1) / 2, "n = {n}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_future_interrupts_the_query_and_frees_the_worker() {
    let runtime = runtime(1, 4);
    let started = Instant::now();
    let timed_out =
        tokio::time::timeout(Duration::from_millis(300), scalar(&runtime, ENDLESS)).await;
    assert!(timed_out.is_err(), "the endless query cannot finish");

    // The single worker is free again only if the query was interrupted.
    let next = tokio::time::timeout(Duration::from_secs(10), scalar(&runtime, "SELECT 7"))
        .await
        .expect("worker freed by the interrupt");
    assert_eq!(next.unwrap(), 7);
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_work_when_the_queue_is_full() {
    let runtime = runtime(1, 1);
    let running = tokio::spawn({
        let runtime = runtime.clone();
        async move { scalar(&runtime, ENDLESS).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let queued = tokio::spawn({
        let runtime = runtime.clone();
        async move { scalar(&runtime, "SELECT 1").await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(matches!(
        scalar(&runtime, "SELECT 2").await,
        Err(RuntimeError::Busy)
    ));

    queued.abort();
    running.abort();
    let recovered = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match scalar(&runtime, "SELECT 3").await {
                Err(RuntimeError::Busy) => tokio::time::sleep(Duration::from_millis(50)).await,
                other => return other,
            }
        }
    })
    .await
    .expect("aborted jobs release the pool");
    assert_eq!(recovered.unwrap(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_cancelled_while_queued_never_runs() {
    let runtime = runtime(1, 4);
    let blocker = tokio::spawn({
        let runtime = runtime.clone();
        async move { scalar(&runtime, ENDLESS).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let queued = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            runtime
                .run(|conn| conn.execute_batch("CREATE TABLE side_effect AS SELECT 1 AS x"))
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    queued.abort();
    blocker.abort();

    let exists = tokio::time::timeout(
        Duration::from_secs(10),
        scalar(
            &runtime,
            "SELECT count(*) FROM duckdb_tables() WHERE table_name = 'side_effect'",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(exists, 0, "the cancelled job must not have run");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_panicking_job_does_not_take_the_worker_down() {
    let runtime = runtime(1, 4);
    let panicked = runtime
        .run(|_conn| -> duckdb::Result<()> { panic!("job bug") })
        .await;
    assert!(matches!(panicked, Err(RuntimeError::Panicked)));
    assert_eq!(scalar(&runtime, "SELECT 5").await.unwrap(), 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn query_connections_are_locked_down() {
    let runtime = runtime(1, 4);
    let scratch = tempfile::tempdir().unwrap();
    let file = scratch
        .path()
        .join("x.csv")
        .to_string_lossy()
        .replace('\'', "''");
    let forbidden = [
        format!("SELECT * FROM read_csv('{file}')"),
        format!("COPY (SELECT 1) TO '{file}'"),
        format!("ATTACH '{file}.duckdb' AS f"),
        "ATTACH 'dbname=bi host=127.0.0.1' AS meta (TYPE postgres)".to_owned(),
        "INSTALL httpfs".to_owned(),
        "SET enable_external_access = true".to_owned(),
        "SET memory_limit = '100TB'".to_owned(),
    ];
    for sql in forbidden {
        let result = runtime
            .run({
                let sql = sql.clone();
                move |conn| conn.execute_batch(&sql)
            })
            .await;
        assert!(
            matches!(result, Err(RuntimeError::Engine(_))),
            "must be refused: {sql} → {result:?}"
        );
    }
    assert!(!scratch.path().join("x.csv").exists());
}

#[test]
fn dropping_the_last_runtime_never_blocks_the_executor() {
    // A single-threaded executor: any blocking wait would stall everything.
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    executor.block_on(async {
        let runtime = runtime(1, 4);
        let (started, has_started) = std::sync::mpsc::channel();
        let job = tokio::spawn({
            let runtime = runtime.clone();
            async move {
                runtime
                    .run(move |_conn| {
                        started.send(()).ok();
                        // Native work that an interrupt cannot stop.
                        std::thread::sleep(Duration::from_millis(500));
                        Ok(())
                    })
                    .await
            }
        });
        tokio::task::spawn_blocking(move || has_started.recv())
            .await
            .unwrap()
            .unwrap();
        job.abort();
        assert!(job.await.unwrap_err().is_cancelled());

        let started = Instant::now();
        drop(runtime);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "dropping the runtime blocked for {:?}",
            started.elapsed()
        );
    });
}
