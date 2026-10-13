//! Actual pool composition: bounded delivery, deadlines and exact Arrow data.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "../../sef-planner/tests/common/mod.rs"]
mod common;
use bi_runtime::{Bootstrap, Runtime, RuntimeConfig};
use bi_server::semantic_execution::{Event, Failure, SemanticExecutor};
use sef_adapter_duckdb::execution::{
    DuckDbPlan, ExecutionError, ExecutionLimits, PhysicalRelation,
};
use sef_correctness::SemanticContext;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn plan(case: u8, protected: bool, lake: bool) -> Arc<DuckDbPlan> {
    let model = common::model(case);
    let context = if protected {
        SemanticContext {
            slots: [(
                "tenant_id".into(),
                sef_model_ir::types::Literal::Utf8("A".into()),
            )]
            .into(),
        }
    } else {
        SemanticContext::default()
    };
    let validated = sef_correctness::validate(&model, &common::bound(case), &context).unwrap();
    let semantic = sef_planner::plan(&model, &validated).unwrap();
    let catalog = common::catalog(case)
        .relations
        .into_values()
        .map(|r| {
            (
                (r.source, r.relation.clone()),
                PhysicalRelation {
                    name: if lake {
                        vec!["lake".into(), "main".into(), r.relation.as_str().to_owned()]
                    } else {
                        vec![r.relation.as_str().to_owned()]
                    },
                    columns: r.columns.into_iter().map(|c| (c.handle, c.name)).collect(),
                },
            )
        })
        .collect();
    Arc::new(
        sef_adapter_duckdb::execution::lower(
            &model,
            &validated,
            &semantic.plan,
            &semantic.certificate,
            &catalog,
        )
        .unwrap(),
    )
}

async fn runtime() -> Runtime {
    let runtime = Bootstrap::open(RuntimeConfig::in_memory(2))
        .unwrap()
        .finish()
        .unwrap();
    runtime.run(|conn| {
        conn.execute_batch("CREATE TABLE sales(region VARCHAR NOT NULL, amount DECIMAL(18,2) NOT NULL); INSERT INTO sales SELECT CAST(i AS VARCHAR), 1.00 FROM range(10000) t(i);
            CREATE TABLE orders(order_id BIGINT NOT NULL, amount DECIMAL(18,2) NOT NULL); INSERT INTO orders VALUES (1,1.00),(2,2.00),(3,3.00),(4,4.00)")?;
        Ok(())
    }).await.unwrap();
    runtime
}

fn limits() -> ExecutionLimits {
    ExecutionLimits {
        max_rows: 12000,
        max_bytes: 4 * 1024 * 1024,
        max_intermediate_bytes: 4 * 1024 * 1024,
        deadline: Some(Instant::now() + Duration::from_secs(10)),
        ..Default::default()
    }
}

async fn drained(runtime: &Runtime) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while runtime.load().running != 0 || runtime.load().queued != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_workers_return_exact_batches_and_one_completion_each() {
    async fn consume(stream: &mut bi_server::semantic_execution::SemanticStream) {
        let mut rows = 0;
        let mut complete = 0;
        while let Some(event) = stream.next().await {
            match event {
                Event::Batch(block) => {
                    for batch in block.batches {
                        let amounts = batch
                            .column(0)
                            .as_any()
                            .downcast_ref::<bi_runtime::duckdb::arrow::array::Decimal128Array>()
                            .unwrap();
                        assert!((0..amounts.len()).all(|i| amounts.value(i) == 100));
                        assert_eq!(
                            batch.schema().field(0).metadata()["sef.metric_id"],
                            "corpus/metric/revenue"
                        );
                        rows += batch.num_rows();
                    }
                }
                Event::Complete(receipt) => {
                    assert_eq!(receipt.rows, rows);
                    assert_eq!(receipt.rows, 10000);
                    complete += 1;
                }
                Event::Failed(failure) => panic!("{failure:?}"),
            }
        }
        assert_eq!(complete, 1);
    }

    let runtime = runtime().await;
    let executor = SemanticExecutor::new(runtime.clone());
    let plan = plan(1, false, false);
    let mut first = executor
        .start(
            plan.clone(),
            SemanticContext::default(),
            "semantic-first",
            limits(),
            2,
        )
        .unwrap();
    let mut second = executor
        .start(
            plan,
            SemanticContext::default(),
            "semantic-second",
            limits(),
            2,
        )
        .unwrap();
    tokio::join!(consume(&mut first), consume(&mut second));
    drained(&runtime).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn backpressure_drop_and_deadline_release_the_worker() {
    let runtime = runtime().await;
    let executor = SemanticExecutor::new(runtime.clone());
    let plan = plan(1, false, false);
    let mut stream = executor
        .start(
            plan.clone(),
            SemanticContext::default(),
            "semantic-drop",
            limits(),
            1,
        )
        .unwrap();
    assert!(matches!(stream.next().await, Some(Event::Batch(_))));
    // Ten thousand distinct groups need several batches; one undrained
    // buffer cannot let the worker finish and certify the whole result.
    assert_eq!(runtime.load().running, 1);
    drop(stream);
    drained(&runtime).await;
    assert_eq!(
        runtime
            .run(|conn| conn.query_row("SELECT 7", [], |r| r.get::<_, i64>(0)))
            .await
            .unwrap(),
        7
    );
    let mut bound = limits();
    bound.deadline = Some(Instant::now() + Duration::from_millis(100));
    let mut stream = executor
        .start(
            plan,
            SemanticContext::default(),
            "semantic-deadline",
            bound,
            1,
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let mut failed = false;
    while let Some(event) = stream.next().await {
        match event {
            Event::Batch(_) => {}
            Event::Failed(Failure::Deadline | Failure::Rejected(ExecutionError::Deadline)) => {
                failed = true;
            }
            _ => panic!("a timed-out stream cannot complete"),
        }
    }
    assert!(failed);
    drained(&runtime).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_result_budget_failure_never_publishes_a_completion() {
    let runtime = runtime().await;
    let executor = SemanticExecutor::new(runtime.clone());
    let mut bound = limits();
    bound.max_rows = 2500;
    let mut stream = executor
        .start(
            plan(1, false, false),
            SemanticContext::default(),
            "semantic-budget",
            bound,
            1,
        )
        .unwrap();
    let mut failed = false;
    let mut rows = 0;
    while let Some(event) = stream.next().await {
        match event {
            Event::Batch(block) => {
                rows += block
                    .batches
                    .iter()
                    .map(bi_runtime::duckdb::arrow::record_batch::RecordBatch::num_rows)
                    .sum::<usize>();
            }
            Event::Failed(Failure::Rejected(ExecutionError::Budget)) => failed = true,
            _ => panic!("a failed budget cannot certify a result"),
        }
    }
    assert!(failed);
    assert!(rows <= 2500);
    drained(&runtime).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ratio_functions_are_isolated_between_concurrent_workers() {
    async fn consume(mut stream: bi_server::semantic_execution::SemanticStream) {
        let mut complete = false;
        while let Some(event) = stream.next().await {
            match event {
                Event::Batch(block) => {
                    let values = block.batches[0]
                        .column(0)
                        .as_any()
                        .downcast_ref::<bi_runtime::duckdb::arrow::array::Decimal128Array>()
                        .unwrap();
                    assert_eq!(values.value(0), 250);
                }
                Event::Complete(receipt) => {
                    assert_eq!(receipt.rows, 1);
                    complete = true;
                }
                Event::Failed(error) => panic!("{error:?}"),
            }
        }
        assert!(complete);
    }

    let runtime = runtime().await;
    let executor = SemanticExecutor::new(runtime.clone());
    let plan = plan(6, false, false);
    let first = executor
        .start(
            plan.clone(),
            SemanticContext::default(),
            "ratio-first",
            limits(),
            1,
        )
        .unwrap();
    let second = executor
        .start(
            plan,
            SemanticContext::default(),
            "ratio-second",
            limits(),
            1,
        )
        .unwrap();
    tokio::join!(consume(first), consume(second));
    drained(&runtime).await;
    assert_eq!(runtime.run(|conn| conn.query_row("SELECT COUNT(*) FROM duckdb_functions() WHERE function_name LIKE 'sef_exact_ratio_%'", [], |row| row.get::<_, i64>(0))).await.unwrap(), 0);
}

#[cfg(feature = "postgres-tests")]
#[tokio::test(flavor = "multi_thread")]
async fn certified_execution_reads_real_ducklake_and_records_its_snapshot() {
    let data = tempfile::tempdir().unwrap();
    let runtime = Bootstrap::open(RuntimeConfig {
        lake: Some(bi_runtime::LakeConfig {
            catalog: std::env::var("BI_TEST_LAKE_CATALOG")
                .unwrap_or_else(|_| bi_runtime::config::DEV_LAKE_CATALOG.to_owned()),
            metadata_schema: Some(format!("semantic_test_{}", std::process::id())),
            data_path: data.path().to_owned(),
        }),
        ..RuntimeConfig::in_memory(2)
    })
    .unwrap()
    .finish()
    .unwrap();
    runtime.run(|conn| {
        conn.execute_batch("CREATE TABLE lake.main.sales(region VARCHAR NOT NULL, amount DECIMAL(18,2) NOT NULL); INSERT INTO lake.main.sales VALUES ('north',90071992547409.93),('north',0.07)")?;
        Ok(())
    }).await.unwrap();
    let executor = SemanticExecutor::new(runtime.clone());
    let mut stream = executor
        .start(
            plan(1, false, true),
            SemanticContext::default(),
            "semantic-ducklake",
            limits(),
            1,
        )
        .unwrap();
    let mut complete = false;
    while let Some(event) = stream.next().await {
        match event {
            Event::Batch(block) => {
                let values = block.batches[0]
                    .column(0)
                    .as_any()
                    .downcast_ref::<bi_runtime::duckdb::arrow::array::Decimal128Array>()
                    .unwrap();
                assert_eq!(values.value(0), 9_007_199_254_741_000);
            }
            Event::Complete(receipt) => {
                assert_eq!(receipt.rows, 1);
                assert!(receipt.sources.iter().all(|s| s.data_snapshot.is_some()));
                complete = true;
            }
            Event::Failed(error) => panic!("{error:?}"),
        }
    }
    assert!(complete);
    drop(stream);
    drained(&runtime).await;
}
