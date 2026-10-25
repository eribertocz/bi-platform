//! Governed execution end to end on real DuckDB: exact against the
//! independent oracle, bounded by the profile, with one deadline, and
//! cancelled, refused or discarded as a whole. The queue and the worker are
//! synchronized through the pool itself: a held worker and its observed
//! queue, never a blind sleep.

use std::time::{Duration, Instant};

use bi_runtime::{Runtime, RuntimeConfig};
use proptest::prelude::*;
use serde_json::{Value, json};

use super::*;
use crate::analytics::Analytics;
use crate::db::Database;
use crate::metadata::MetadataMode;
#[cfg(feature = "postgres-tests")]
use crate::semantic::prepare::tests::composition::ROWS;
use crate::semantic::prepare::tests::composition::{
    ROW_POLICY, analytics, analytics_on, dimension, metric, model, project, request, selections,
    session, state, zoned_model,
};

const MEMBERS: &str =
    "version: 1\nusers:\n  ana: [viewer]\n  a: [viewer]\n  b: [viewer]\n  jefa: [admin]\n";

async fn run(
    state: &AppState,
    session: Session,
    request: ExplainRequest,
) -> Result<SemanticRun, RunFailure> {
    execute(state, session, request, "semantic-run-test", Instant::now())
        .await
        .map(|answer| answer.run)
        .map_err(|failed| failed.failure)
}

fn ana() -> Session {
    session("ana", &[])
}

/// The rows of the first block as the client reads them, sorted.
fn rows(run: &SemanticRun) -> Vec<Vec<Value>> {
    let mut rows = delivered(run);
    rows.sort_by_key(|row| row.iter().map(ToString::to_string).collect::<Vec<_>>());
    rows
}

/// The rows of the first block in the order delivered.
fn delivered(run: &SemanticRun) -> Vec<Vec<Value>> {
    serde_json::from_value(serde_json::to_value(&run.result.blocks[0].rows).unwrap()).unwrap()
}

fn total() -> ExplainRequest {
    selections(&json!([metric("t", "importe_total"), metric("f", "filas")]))
}

/// With no order: the default orders it by region.
fn by_region() -> ExplainRequest {
    selections(&json!([
        dimension("r", "ventas.region"),
        metric("t", "importe_total"),
        metric("f", "filas")
    ]))
}

/// `selections`, ordered by the selection `by`.
fn ordered(selections: &Value, by: &str) -> ExplainRequest {
    request(&json!({ "selections": selections, "order_by": [{ "selection": by }] }))
}

/// The Issue's oracle, written out by hand.
fn oracle_total() -> Vec<Vec<Value>> {
    vec![vec![json!("300.30"), json!("4")]]
}

fn oracle_regions() -> Vec<Vec<Value>> {
    vec![
        vec![json!("Norte"), json!("100.30"), json!("2")],
        vec![json!("Sur"), json!("200.00"), json!("2")],
    ]
}

fn runtime_of(state: &AppState) -> Runtime {
    state.analytics.runtime().unwrap()
}

/// One worker and room for `queue` waiting jobs.
async fn single(queue: usize) -> Analytics {
    analytics_on(RuntimeConfig {
        queue_capacity: queue,
        ..RuntimeConfig::in_memory(1)
    })
    .await
}

/// Occupies a worker until the returned sender sends or is dropped.
async fn hold(runtime: &Runtime) -> (std::sync::mpsc::Sender<()>, tokio::task::JoinHandle<()>) {
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let (started, running) = tokio::sync::oneshot::channel();
    let runtime = runtime.clone();
    let held = tokio::spawn(async move {
        let _ = runtime
            .run(move |_| {
                let _ = started.send(());
                let _ = wait.recv();
                Ok(())
            })
            .await;
    });
    running.await.unwrap();
    (release, held)
}

/// Waits until `queued` jobs wait in the pool.
async fn queued(runtime: &Runtime, queued: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while runtime.load().queued != queued {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{queued} queued: {:?}", runtime.load()));
}

async fn drained(runtime: &Runtime) {
    queued(runtime, 0).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while runtime.load().running != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

/// No exact-arithmetic function survives an execution.
async fn no_functions_left(runtime: &Runtime) {
    let left = runtime
        .run(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM duckdb_functions() WHERE function_name LIKE 'sef_%'",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(left, 0);
}

async fn sql(runtime: &Runtime, statement: &'static str) {
    runtime
        .run(move |conn| conn.execute_batch(statement))
        .await
        .unwrap();
}

/// A preparation lowered for `session`, within the default timeout.
async fn ready(state: &AppState, session: Session, mut request: ExplainRequest) -> Ready {
    bounded(&mut request.query, state.semantic_runs.profile().budgets()).unwrap();
    let Ok(ready) = prepared(state, session, &request, later()).await else {
        panic!("prepared and lowered");
    };
    ready
}

fn later() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

/// Executes and assembles `ready` as [`governed`] does.
async fn finished(state: &AppState, ready: Ready, user: &str) -> Result<SemanticRun, RunFailure> {
    let Ready { prepared, plan } = ready;
    let events = executed(state, &prepared, plan, "semantic-run-test", later()).await?;
    assembled(
        prepared,
        events,
        state.context_key.clone(),
        owned(state, user),
        later(),
    )
    .await
}

fn owned(state: &AppState, user: &str) -> Owned {
    Owned {
        project: state
            .models
            .directory
            .as_deref()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default(),
        principal: user.to_owned(),
    }
}

fn budgets(state: &mut AppState, change: impl FnOnce(&mut Budgets)) {
    let mut budgets = *state.semantic_runs.profile().budgets();
    change(&mut budgets);
    state.semantic_runs = Runs::new(Profile::new(budgets).unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_is_complete_and_exact_against_the_oracle() {
    let p = project(&model("", ""), MEMBERS);
    let state = state(p.path(), analytics().await);
    let answer = run(&state, ana(), total()).await.unwrap();
    assert_eq!(rows(&answer), oracle_total());
    // One row by construction: no subset, no limit.
    let block = &answer.result.blocks[0];
    assert_eq!((block.row_count, block.limit), (1, None));
    let answer = run(&state, ana(), by_region()).await.unwrap();
    assert_eq!(delivered(&answer), oracle_regions());
    let block = &answer.result.blocks[0];
    assert_eq!((block.row_count, block.limit), (2, Some(1000)));
    assert!(answer.receipt.id.starts_with("run:"));
    // An in-memory source has no version to read again.
    assert!(
        answer
            .receipt
            .sources
            .iter()
            .all(|s| s.data_snapshot.is_none())
    );
    // The private fingerprints stay private.
    let ready = ready(&state, ana(), by_region()).await;
    let private = [
        ready.prepared.plan_fingerprint().to_owned(),
        ready.prepared.policies().to_owned(),
        ready.prepared.decision().fingerprint.to_string(),
        ready.prepared.model().content_hash().to_string(),
    ];
    let answer = finished(&state, ready, "ana").await.unwrap();
    let text = serde_json::to_string(&answer).unwrap();
    for fingerprint in private {
        assert!(!text.contains(&fingerprint), "{fingerprint}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_integer_beyond_two_to_the_53_stays_exact() {
    let sum_ids = "  - name: suma_id\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: SUM(ventas.id)\n";
    let p = project(&model("", sum_ids), MEMBERS);
    let state = state(p.path(), analytics().await);
    sql(
        &runtime_of(&state),
        "INSERT INTO lake.main.ventas VALUES (9007199254740993, 'Sur', 'Oruro', 0.00, \
         TIMESTAMPTZ '2026-01-03 00:00:00+00', NULL)",
    )
    .await;
    let ids = selections(&json!([metric("s", "suma_id"), metric("f", "filas")]));
    // 1 + 2 + 3 + 4 + 2^53 + 1, written out: one more than a double holds.
    assert_eq!(
        rows(&run(&state, ana(), ids).await.unwrap()),
        vec![vec![json!("9007199254741003"), json!("5")]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn row_policies_bind_only_the_verified_session_even_concurrently() {
    let p = project(&model(ROW_POLICY, ""), MEMBERS);
    let state = state(p.path(), analytics().await);
    let (a, b) = tokio::join!(
        run(&state, session("a", &[("region", json!("Norte"))]), total()),
        run(&state, session("b", &[("region", json!("Sur"))]), total()),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(rows(&a), vec![vec![json!("100.30"), json!("2")]]);
    assert_eq!(rows(&b), vec![vec![json!("200.00"), json!("2")]]);
    // The same definitions and query; their contexts differ.
    assert_eq!(a.receipt.definitions, b.receipt.definitions);
    assert_eq!(a.receipt.fingerprints.query, b.receipt.fingerprints.query);
    assert_ne!(
        a.receipt.fingerprints.context,
        b.receipt.fingerprints.context
    );
    // An administrator is bound too: without the attribute, refused.
    assert!(matches!(
        run(&state, session("jefa", &[]), total()).await,
        Err(RunFailure::Refused(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_limit_is_normalized_before_preparing_and_explain_keeps_its_own() {
    let p = project(&model("", ""), MEMBERS);
    let state = state(p.path(), analytics().await);
    let omitted = run(&state, ana(), by_region()).await.unwrap();
    let mut explicit = by_region();
    explicit.query.limit = Some(1000);
    explicit.query.order_by = serde_json::from_value(json!([{ "selection": "r" }])).unwrap();
    let explicit = run(&state, ana(), explicit).await.unwrap();
    assert_eq!(omitted.result.blocks[0].limit, Some(1000));
    // The query executed is the one with the limit and its order: same
    // SQIR, same fingerprint.
    assert_eq!(omitted.receipt.query, explicit.receipt.query);
    assert_eq!(
        omitted.receipt.fingerprints.query,
        explicit.receipt.fingerprints.query
    );
    // Explain prepares the request as it came.
    let Ok(explained) = prepare::prepare(&state, ana(), &by_region()).await else {
        panic!("explain prepares");
    };
    assert_eq!(explained.query().blocks[0].limit, None);
    // The bounds themselves are accepted.
    for (limit, offset, count) in [
        (Some(1), None, 1),
        (Some(10_000), None, 2),
        (None, Some(10_000), 0),
    ] {
        let mut request = ordered(
            &json!([dimension("r", "ventas.region"), metric("f", "filas")]),
            "r",
        );
        request.query.limit = limit;
        request.query.offset = offset;
        let answer = run(&state, ana(), request).await.unwrap();
        assert_eq!(
            answer.result.blocks[0].row_count, count,
            "{limit:?} {offset:?}"
        );
    }
    // Outside them, refused before anything is read: with no project and
    // no engine, a valid request would be refused by the preparation.
    let empty = AppState::new(Database::start(
        &"postgres://bi:bi@127.0.0.1:1/bi".parse().unwrap(),
        MetadataMode::Development,
    ));
    for (limit, offset) in [(Some(0), None), (Some(10_001), None), (None, Some(10_001))] {
        let mut request = by_region();
        request.query.limit = limit;
        request.query.offset = offset;
        assert!(matches!(
            run(&empty, ana(), request).await,
            Err(RunFailure::LimitInvalid)
        ));
    }
    assert!(matches!(
        run(&empty, ana(), by_region()).await,
        Err(RunFailure::Refused(_))
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Exactly the profile's limits pass; the effective limit is the one
    /// asked or the default, the offset is kept, the client's order is
    /// kept, and only the default limit brings the grain's order. An
    /// aggregate with no grain and no pagination is left as it came.
    #[test]
    fn only_the_profiles_limits_pass(
        limit in proptest::option::of(0_u64..20_000),
        offset in proptest::option::of(0_u64..20_000),
        grain in any::<bool>(),
        order in any::<bool>(),
    ) {
        let mut selected = vec![metric("t", "importe_total")];
        if grain {
            selected.insert(0, dimension("r", "ventas.region"));
        }
        let order_by = if order {
            json!([{ "selection": "t", "descending": true }])
        } else {
            json!([])
        };
        let mut query: SemanticQuery = serde_json::from_value(json!({
            "selections": selected,
            "order_by": order_by,
            "limit": limit,
            "offset": offset,
        }))
        .unwrap();
        let untouched = !grain && limit.is_none() && offset.is_none() && !order;
        let valid = untouched
            || (limit.is_none_or(|l| (1..=10_000).contains(&l))
                && offset.is_none_or(|o| o <= 10_000));
        let bounded = bounded(&mut query, &INITIAL);
        prop_assert_eq!(bounded.is_ok(), valid);
        let orders: Vec<(String, bool, bool)> = query
            .order_by
            .iter()
            .map(|o| (o.selection.clone(), o.descending, o.nulls_first))
            .collect();
        if untouched {
            prop_assert_eq!(query.limit, None);
            prop_assert!(orders.is_empty());
        } else if valid {
            prop_assert_eq!(query.limit, Some(limit.unwrap_or(1_000)));
            prop_assert_eq!(query.offset, offset);
            let expected = if order {
                vec![("t".to_owned(), true, false)]
            } else if limit.is_none() && grain {
                vec![("r".to_owned(), false, false)]
            } else {
                Vec::new()
            };
            prop_assert_eq!(orders, expected);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_default_order_is_the_grain_and_the_client_keeps_its_own() {
    let p = project(&model("", ""), MEMBERS);
    let state = state(p.path(), analytics().await);
    // Several dimensions and a NULL: region, then note, nulls last, as
    // delivered.
    let notes = selections(&json!([
        dimension("r", "ventas.region"),
        dimension("n", "ventas.nota"),
        metric("f", "filas")
    ]));
    let answer = run(&state, session("jefa", &[]), notes).await.unwrap();
    assert_eq!(
        delivered(&answer),
        vec![
            vec![json!("Norte"), json!("n"), json!("1")],
            vec![json!("Norte"), Value::Null, json!("1")],
            vec![json!("Sur"), json!("s"), json!("1")],
            vec![json!("Sur"), Value::Null, json!("1")],
        ]
    );
    let explicit = request(&json!({
        "selections": [
            dimension("r", "ventas.region"),
            dimension("n", "ventas.nota"),
            metric("f", "filas")
        ],
        "order_by": [{ "selection": "r" }, { "selection": "n" }],
        "limit": 1000
    }));
    let explicit = run(&state, session("jefa", &[]), explicit).await.unwrap();
    assert_eq!(answer.receipt.query, explicit.receipt.query);
    // The client's order is kept exactly; the omitted limit is the default.
    let descending = request(&json!({
        "selections": [
            dimension("r", "ventas.region"),
            metric("t", "importe_total"),
            metric("f", "filas")
        ],
        "order_by": [{ "selection": "r", "descending": true }]
    }));
    let answer = run(&state, ana(), descending).await.unwrap();
    let mut reversed = oracle_regions();
    reversed.reverse();
    assert_eq!(delivered(&answer), reversed);
    assert_eq!(answer.result.blocks[0].limit, Some(1000));
    // An explicit offset, even 0, is kept, with the default limit and the
    // grain's order.
    let mut zero = by_region();
    zero.query.offset = Some(0);
    let zero = run(&state, ana(), zero).await.unwrap();
    let mut written = by_region();
    written.query.offset = Some(0);
    written.query.limit = Some(1000);
    written.query.order_by = serde_json::from_value(json!([{ "selection": "r" }])).unwrap();
    let written = run(&state, ana(), written).await.unwrap();
    assert_eq!(zero.receipt.query, written.receipt.query);
    // A limit written without an order, or an offset with no grain and no
    // order, stays the preparation's refusal.
    let mut unordered = by_region();
    unordered.query.limit = Some(5);
    let mut offset = total();
    offset.query.offset = Some(1);
    for request in [unordered, offset] {
        assert!(matches!(
            run(&state, ana(), request).await,
            Err(RunFailure::Refused(_))
        ));
    }
    // A total a HAVING removes: no row, still no limit.
    let removed = request(&json!({
        "selections": [metric("t", "importe_total"), metric("f", "filas")],
        "having": {
            "op": "compare",
            "target": { "kind": "metric", "selection": "f" },
            "comparison": ">",
            "value": { "type": "number", "value": "100" }
        }
    }));
    let removed = run(&state, ana(), removed).await.unwrap();
    let block = &removed.result.blocks[0];
    assert_eq!((block.row_count, block.limit), (0, None));
}

/// `ventas` with 10 500 more cities, one row each.
async fn many_cities() -> (tempfile::TempDir, AppState) {
    let p = project(&model("", ""), MEMBERS);
    let analytics = analytics().await;
    sql(
        &analytics.runtime().unwrap(),
        "INSERT INTO lake.main.ventas SELECT 100 + i, 'Norte', 'c' || i, 1.00, \
         TIMESTAMPTZ '2026-01-03 00:00:00+00', NULL FROM range(10500) t(i)",
    )
    .await;
    let state = state(p.path(), analytics);
    (p, state)
}

fn by_city(limit: Option<u64>) -> ExplainRequest {
    let mut request = ordered(
        &json!([dimension("c", "ventas.ciudad"), metric("f", "filas")]),
        "c",
    );
    request.query.limit = limit;
    request
}

#[tokio::test(flavor = "multi_thread")]
async fn a_limited_set_says_the_limit_it_applied() {
    let (_p, state) = many_cities().await;
    for (limit, count) in [(None, 1_000), (Some(10_000), 10_000), (Some(1), 1)] {
        let answer = run(&state, ana(), by_city(limit)).await.unwrap();
        let block = &answer.result.blocks[0];
        assert_eq!(block.row_count, count);
        // Never presented as all there is: the limit applied is stated.
        assert_eq!(block.limit, Some(limit.unwrap_or(1_000)));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_budget_exceeded_discards_everything_and_the_next_query_runs() {
    let (_p, mut state) = many_cities().await;
    let runtime = runtime_of(&state);
    // Rows over budget after some batches were delivered: nothing of them
    // comes out.
    budgets(&mut state, |b| b.max_rows = 2_500);
    assert!(matches!(
        run(&state, ana(), by_city(Some(10_000))).await,
        Err(RunFailure::Budget)
    ));
    budgets(&mut state, |b| *b = INITIAL);
    budgets(&mut state, |b| b.max_arrow_bytes = 1_024);
    assert!(matches!(
        run(&state, ana(), by_city(Some(10_000))).await,
        Err(RunFailure::Budget)
    ));
    budgets(&mut state, |b| *b = INITIAL);
    let answer = run(&state, ana(), total()).await.unwrap();
    assert_eq!(rows(&answer), vec![vec![json!("10800.30"), json!("10504")]]);
    // One cell over 64 KiB.
    sql(
        &runtime,
        "INSERT INTO lake.main.ventas VALUES (99999, 'Sur', repeat('x', 70000), 1.00, \
         TIMESTAMPTZ '2026-01-03 00:00:00+00', NULL)",
    )
    .await;
    let mut long = by_city(Some(1));
    long.query.order_by =
        serde_json::from_value(json!([{ "selection": "c", "descending": true }])).unwrap();
    assert!(matches!(
        run(&state, ana(), long).await,
        Err(RunFailure::Budget)
    ));
    drained(&runtime).await;
    no_functions_left(&runtime).await;
    assert!(run(&state, ana(), total()).await.is_ok());
}

/// A metric beyond DECIMAL(38): `importe` cubed is DECIMAL(56,6), summed
/// by the adapter's exact Rust emulation.
const CUBED: &str = "  - name: cubo\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: SUM(ventas.importe * ventas.importe * ventas.importe)\n";

#[tokio::test(flavor = "multi_thread")]
async fn the_emulations_intermediates_are_bounded_and_exact() {
    let p = project(&model("", CUBED), MEMBERS);
    let mut state = state(p.path(), analytics().await);
    let runtime = runtime_of(&state);
    let cubed = selections(&json!([metric("c", "cubo")]));
    // Written out by hand: 100.10³ + 0.20³ + 200.20³ + (-0.20)³.
    let answer = run(&state, ana(), cubed).await.unwrap();
    assert_eq!(
        answer.result.blocks[0].columns[0].value_type,
        "DECIMAL(56,6)"
    );
    assert_eq!(rows(&answer), vec![vec![json!("9027027.009000")]]);
    budgets(&mut state, |b| b.max_intermediate_bytes = 1);
    let cubed = selections(&json!([metric("c", "cubo")]));
    assert!(matches!(
        run(&state, ana(), cubed).await,
        Err(RunFailure::Budget)
    ));
    budgets(&mut state, |b| *b = INITIAL);
    drained(&runtime).await;
    no_functions_left(&runtime).await;
    assert_eq!(
        rows(&run(&state, ana(), total()).await.unwrap()),
        oracle_total()
    );
}

/// The emulated cube by region through the single enforcement point: each
/// principal sees only its own region, exact, and a small window never
/// bypasses the intermediate budget (#256).
#[tokio::test(flavor = "multi_thread")]
async fn the_emulation_by_region_is_governed_and_bounded() {
    let p = project(&model(ROW_POLICY, CUBED), MEMBERS);
    let mut state = state(p.path(), analytics().await);
    let cube = || {
        selections(&json!([
            dimension("r", "ventas.region"),
            metric("c", "cubo")
        ]))
    };
    let a = run(&state, session("a", &[("region", json!("Norte"))]), cube())
        .await
        .unwrap();
    let b = run(&state, session("b", &[("region", json!("Sur"))]), cube())
        .await
        .unwrap();
    assert_eq!(
        rows(&a),
        vec![vec![json!("Norte"), json!("1003003.009000")]]
    );
    assert_eq!(rows(&b), vec![vec![json!("Sur"), json!("8024024.000000")]]);
    assert_ne!(
        a.receipt.fingerprints.context,
        b.receipt.fingerprints.context
    );
    budgets(&mut state, |b| b.max_intermediate_bytes = 1);
    let small = request(&json!({
        "selections": [dimension("r", "ventas.region"), metric("c", "cubo")],
        "order_by": [{ "selection": "r" }],
        "limit": 1
    }));
    assert!(matches!(
        run(&state, session("a", &[("region", json!("Norte"))]), small).await,
        Err(RunFailure::Budget)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn one_deadline_covers_admission_the_queue_and_the_execution() {
    let p = project(&model("", ""), MEMBERS);
    let mut state = state(p.path(), single(4).await);
    let runtime = runtime_of(&state);
    // Admitted too long ago: nothing starts.
    let admitted = Instant::now().checked_sub(Duration::from_secs(11)).unwrap();
    assert!(matches!(
        execute(&state, ana(), total(), "stale", admitted)
            .await
            .map_err(|failed| failed.failure),
        Err(RunFailure::Deadline)
    ));
    // Waiting for the engine to prepare.
    budgets(&mut state, |b| b.timeout = Duration::from_millis(200));
    let (release, held) = hold(&runtime).await;
    assert!(matches!(
        run(&state, ana(), total()).await,
        Err(RunFailure::Deadline)
    ));
    release.send(()).unwrap();
    held.await.unwrap();
    drained(&runtime).await;
    // Prepared, then waiting for the engine to execute: the same deadline
    // from admission, never restarted by the phase.
    let ready = ready(&state, ana(), total()).await;
    let (release, held) = hold(&runtime).await;
    let Ready { prepared, plan } = ready;
    let deadline = Instant::now() + Duration::from_millis(200);
    assert!(matches!(
        executed(&state, &prepared, plan, "queued", deadline).await,
        Err(RunFailure::Deadline)
    ));
    release.send(()).unwrap();
    held.await.unwrap();
    drained(&runtime).await;
    budgets(&mut state, |b| *b = INITIAL);
    assert_eq!(
        rows(&run(&state, ana(), total()).await.unwrap()),
        oracle_total()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_releases_the_worker_and_spares_every_other_call() {
    let p = project(&model(ROW_POLICY, ""), MEMBERS);
    let state = state(p.path(), single(4).await);
    let runtime = runtime_of(&state);
    let a = || session("a", &[("region", json!("Norte"))]);
    let b = || session("b", &[("region", json!("Sur"))]);
    let spawn = |ready: Ready, user: &'static str| {
        let state = state.clone();
        tokio::spawn(async move { finished(&state, ready, user).await })
    };
    // Two executions waiting for the engine; the first is abandoned.
    let (first, second) = (
        ready(&state, a(), total()).await,
        ready(&state, b(), total()).await,
    );
    let (release, held) = hold(&runtime).await;
    let abandoned = spawn(first, "a");
    queued(&runtime, 1).await;
    let kept = spawn(second, "b");
    queued(&runtime, 2).await;
    abandoned.abort();
    assert!(abandoned.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    held.await.unwrap();
    let kept = kept.await.unwrap().unwrap();
    assert_eq!(rows(&kept), vec![vec![json!("200.00"), json!("2")]]);
    drained(&runtime).await;
    // The whole call dropped while it prepares.
    let (release, held) = hold(&runtime).await;
    let whole = {
        let state = state.clone();
        tokio::spawn(async move { run(&state, a(), total()).await.map(|_| ()) })
    };
    queued(&runtime, 1).await;
    whole.abort();
    assert!(whole.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    held.await.unwrap();
    drained(&runtime).await;
    no_functions_left(&runtime).await;
    let answer = run(&state, a(), total()).await.unwrap();
    assert_eq!(rows(&answer), vec![vec![json!("100.30"), json!("2")]]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_queue_is_busy_with_no_partial_answer() {
    let p = project(&model("", ""), MEMBERS);
    let state = state(p.path(), single(1).await);
    let runtime = runtime_of(&state);
    let ready = ready(&state, ana(), total()).await;
    let (release, held) = hold(&runtime).await;
    let filler = {
        let runtime = runtime.clone();
        tokio::spawn(async move { runtime.run(|_| Ok(())).await.unwrap() })
    };
    queued(&runtime, 1).await;
    // To prepare, and to execute.
    assert!(matches!(
        run(&state, ana(), total()).await,
        Err(RunFailure::Busy)
    ));
    assert!(matches!(
        finished(&state, ready, "ana").await,
        Err(RunFailure::Busy)
    ));
    release.send(()).unwrap();
    held.await.unwrap();
    filler.await.unwrap();
    drained(&runtime).await;
    assert_eq!(
        rows(&run(&state, ana(), total()).await.unwrap()),
        oracle_total()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn two_executions_of_the_same_preparation_never_mix() {
    let p = project(&model("", ""), MEMBERS);
    let state = state(p.path(), single(4).await);
    let runtime = runtime_of(&state);
    // The same query, plan and certificate; a write lands between the two
    // executions, in the pool's own order.
    let (x, y) = (
        ready(&state, ana(), total()).await,
        ready(&state, ana(), total()).await,
    );
    assert_eq!(x.prepared.plan_fingerprint(), y.prepared.plan_fingerprint());
    let spawn = |ready: Ready| {
        let state = state.clone();
        tokio::spawn(async move { finished(&state, ready, "ana").await })
    };
    let (release, held) = hold(&runtime).await;
    let before = spawn(x);
    queued(&runtime, 1).await;
    let write = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            sql(
                &runtime,
                "INSERT INTO lake.main.ventas VALUES (5, 'Norte', 'Potosí', 1.00, \
                 TIMESTAMPTZ '2026-01-03 00:00:00+00', NULL)",
            )
            .await;
        })
    };
    queued(&runtime, 2).await;
    let after = spawn(y);
    queued(&runtime, 3).await;
    release.send(()).unwrap();
    held.await.unwrap();
    let (before, after) = (
        before.await.unwrap().unwrap(),
        after.await.unwrap().unwrap(),
    );
    write.await.unwrap();
    assert_eq!(rows(&before), oracle_total());
    assert_eq!(rows(&after), vec![vec![json!("301.30"), json!("5")]]);
    // The same evidence: an in-memory source has no version to tell the
    // two reads apart, and only this call's ownership of its stream does.
    assert_eq!(before.receipt.fingerprints, after.receipt.fingerprints);
    // Different certificates at once, each with its own answer.
    let state = crate::semantic::prepare::tests::composition::state(p.path(), analytics().await);
    let (regions, all) = tokio::join!(run(&state, ana(), by_region()), run(&state, ana(), total()));
    assert_eq!(rows(&regions.unwrap()), oracle_regions());
    assert_eq!(rows(&all.unwrap()), oracle_total());
}

/// A runtime on a real DuckLake whose catalog lives in PostgreSQL:
/// `schema` names its metadata, `data` holds its Parquet files.
#[cfg(feature = "postgres-tests")]
async fn ducklake(schema: &str, data: &std::path::Path, quota: Option<&str>) -> Runtime {
    let config = RuntimeConfig {
        lake: Some(bi_runtime::LakeConfig {
            catalog: std::env::var("BI_TEST_LAKE_CATALOG")
                .unwrap_or_else(|_| bi_runtime::config::DEV_LAKE_CATALOG.to_owned()),
            metadata_schema: Some(format!("{schema}_{}", std::process::id())),
            data_path: data.to_owned(),
        }),
        query_memory_limit: quota.map(str::to_owned),
        ..RuntimeConfig::in_memory(1)
    };
    tokio::task::spawn_blocking(move || {
        bi_runtime::Bootstrap::open(config)
            .unwrap()
            .finish()
            .unwrap()
    })
    .await
    .unwrap()
}

#[cfg(feature = "postgres-tests")]
const VENTAS: &str = "CREATE TABLE lake.main.ventas (id BIGINT NOT NULL, \
    region VARCHAR NOT NULL, ciudad VARCHAR NOT NULL, importe DECIMAL(18,2) NOT NULL, \
    instante TIMESTAMPTZ NOT NULL, nota VARCHAR)";

#[cfg(feature = "postgres-tests")]
#[tokio::test(flavor = "multi_thread")]
async fn a_real_ducklake_read_names_its_snapshot_and_a_write_moves_it() {
    let data = tempfile::tempdir().unwrap();
    let runtime = ducklake("semantic_run", data.path(), None).await;
    runtime
        .run(|conn| {
            conn.execute_batch(&format!(
                "{VENTAS}; INSERT INTO lake.main.ventas VALUES {ROWS};"
            ))
        })
        .await
        .unwrap();
    let p = project(&model("", ""), MEMBERS);
    let state = state(p.path(), Analytics::ready(runtime.clone()));
    let first = run(&state, ana(), total()).await.unwrap();
    assert_eq!(rows(&first), oracle_total());
    sql(
        &runtime,
        "INSERT INTO lake.main.ventas VALUES (5, 'Norte', 'Potosí', 1.00, \
         TIMESTAMPTZ '2026-01-03 00:00:00+00', NULL)",
    )
    .await;
    let second = run(&state, ana(), total()).await.unwrap();
    assert_eq!(rows(&second), vec![vec![json!("301.30"), json!("5")]]);
    let snapshot = |run: &SemanticRun| run.receipt.sources[0].data_snapshot.clone().unwrap();
    // Each answer names the snapshot it read; the write moved it.
    assert_ne!(snapshot(&first), snapshot(&second));
    assert_ne!(first.receipt.id, second.receipt.id);
    drained(&runtime).await;
}

#[cfg(feature = "postgres-tests")]
#[tokio::test(flavor = "multi_thread")]
async fn an_exhausted_memory_quota_fails_whole_and_the_next_query_runs() {
    // Two million distinct cities in Parquet, written without a quota.
    let data = tempfile::tempdir().unwrap();
    let writer = ducklake("semantic_quota", data.path(), None).await;
    sql(
        &writer,
        "CREATE TABLE lake.main.ventas (id BIGINT NOT NULL, region VARCHAR NOT NULL, \
         ciudad VARCHAR NOT NULL, importe DECIMAL(18,2) NOT NULL, \
         instante TIMESTAMPTZ NOT NULL, nota VARCHAR); \
         INSERT INTO lake.main.ventas SELECT i, 'Norte', repeat('c', 64) || i, 1.00, \
         TIMESTAMPTZ '2026-01-01 00:00:00+00', NULL FROM range(2000000) t(i)",
    )
    .await;
    let p = project(&model("", ""), MEMBERS);
    // Without a quota the query completes: the failure below is the
    // quota's.
    let unbounded = state(p.path(), Analytics::ready(writer.clone()));
    let answer = run(&unbounded, ana(), by_city(None)).await.unwrap();
    assert_eq!(answer.result.blocks[0].row_count, 1_000);
    // The quota the runtime was started with, per query and without spill.
    let bounded = ducklake("semantic_quota", data.path(), Some("64MiB")).await;
    let quota = state(p.path(), Analytics::ready(bounded.clone()));
    assert!(matches!(
        run(&quota, ana(), by_city(None)).await,
        Err(RunFailure::Engine)
    ));
    drained(&bounded).await;
    assert_eq!(
        rows(&run(&quota, ana(), total()).await.unwrap()),
        vec![vec![json!("2000000.00"), json!("2000000")]]
    );
}

fn bucket(zone: Option<&str>) -> ExplainRequest {
    let mut day =
        json!({ "kind": "time_dimension", "id": "d", "name": "ventas.instante", "grain": "day" });
    if let Some(zone) = zone {
        day["timezone"] = json!(zone);
    }
    ordered(&json!([day, metric("f", "filas")]), "d")
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ungoverned_bucket_of_an_instant_is_refused_before_it_runs() {
    let p = project(&model("", ""), MEMBERS);
    let state = state(p.path(), analytics().await);
    for zone in ["America/La_Paz", "UTC"] {
        assert!(
            matches!(
                run(&state, ana(), bucket(Some(zone))).await,
                Err(RunFailure::TemporalUnsupported)
            ),
            "{zone}"
        );
    }
    // With no zone at all, the preparation already refuses it.
    assert!(matches!(
        run(&state, ana(), bucket(None)).await,
        Err(RunFailure::Refused(_))
    ));
    // The instant itself, with no bucket, keeps its exact moments.
    let instants = ordered(
        &json!([dimension("i", "ventas.instante"), metric("f", "filas")]),
        "i",
    );
    assert_eq!(
        rows(&run(&state, ana(), instants).await.unwrap()),
        vec![
            vec![json!("2026-01-01T00:00:00Z"), json!("2")],
            vec![json!("2026-01-02T00:00:00Z"), json!("2")],
        ]
    );
    // A governed bucket keeps its months in La Paz.
    let zoned = project(&zoned_model(), MEMBERS);
    let state =
        crate::semantic::prepare::tests::composition::state(zoned.path(), analytics().await);
    let months = ordered(
        &json!([
            { "kind": "time_dimension", "id": "m", "name": "ventas.mes", "grain": "month" },
            metric("f", "filas")
        ]),
        "m",
    );
    let answer = run(&state, ana(), months).await.unwrap();
    assert_eq!(
        answer.result.blocks[0].columns[0].timezone.as_deref(),
        Some("America/La_Paz")
    );
    assert_eq!(
        rows(&answer),
        vec![
            vec![json!("2025-12-01"), json!("2")],
            vec![json!("2026-01-01"), json!("2")],
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_masked_decision_is_refused_whole_before_it_runs() {
    let p = project(&model("", ""), MEMBERS);
    let state = state(p.path(), analytics().await);
    let runtime = runtime_of(&state);
    // `margen` is confidential: the viewer may only see it masked.
    for request in [
        selections(&json!([metric("m", "margen")])),
        selections(&json!([
            metric("m", "margen"),
            metric("t", "importe_total")
        ])),
    ] {
        assert!(matches!(
            run(&state, ana(), request).await,
            Err(RunFailure::MaskedUnsupported)
        ));
    }
    // Filtering or ordering by it is still the preparation's denial.
    let having = request(&json!({
        "selections": [metric("m", "margen"), metric("t", "importe_total")],
        "having": { "op": "compare", "target": { "kind": "metric", "selection": "m" },
                    "comparison": ">", "value": { "type": "number", "value": "0" } }
    }));
    let by_margin = request(&json!({
        "selections": [dimension("r", "ventas.region"), metric("m", "margen")],
        "order_by": [{ "selection": "m" }]
    }));
    for request in [having, by_margin] {
        assert!(matches!(
            run(&state, ana(), request).await,
            Err(RunFailure::Refused(_))
        ));
    }
    drained(&runtime).await;
    assert_eq!(
        rows(&run(&state, ana(), total()).await.unwrap()),
        oracle_total()
    );
    // An ordinary NULL runs, and is no mask.
    let notes = ordered(
        &json!([dimension("n", "ventas.nota"), metric("f", "filas")]),
        "n",
    );
    let answer = run(&state, session("jefa", &[]), notes).await.unwrap();
    assert!(!answer.result.blocks[0].columns[0].masked);
    assert_eq!(
        rows(&answer),
        vec![
            vec![json!("n"), json!("1")],
            vec![json!("s"), json!("1")],
            vec![Value::Null, json!("2")],
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_source_changed_after_preparing_is_refused_and_the_next_query_runs() {
    let p = project(&model("", ""), MEMBERS);
    let state = state(p.path(), analytics().await);
    let runtime = runtime_of(&state);
    let ready = ready(&state, ana(), by_region()).await;
    sql(
        &runtime,
        "ALTER TABLE lake.main.ventas RENAME COLUMN region TO zona",
    )
    .await;
    assert!(matches!(
        finished(&state, ready, "ana").await,
        Err(RunFailure::Engine)
    ));
    sql(
        &runtime,
        "ALTER TABLE lake.main.ventas RENAME COLUMN zona TO region",
    )
    .await;
    drained(&runtime).await;
    assert_eq!(
        rows(&run(&state, ana(), by_region()).await.unwrap()),
        oracle_regions()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_says_only_its_code() {
    let p = project(
        "version: '0.2.0.dev0'\nname: secreto_centinela\ndatasets: [",
        MEMBERS,
    );
    let state = state(p.path(), analytics().await);
    let failure = run(&state, ana(), total()).await.unwrap_err();
    assert!(matches!(failure, RunFailure::Refused(_)));
    assert_eq!(format!("{failure:?}"), "refused");
}

#[test]
fn only_a_finite_profile_prepares_the_service() {
    assert_eq!(Profile::new(INITIAL), Ok(Profile::initial()));
    let invalid = |change: fn(&mut Budgets)| {
        let mut budgets = INITIAL;
        change(&mut budgets);
        Profile::new(budgets).unwrap_err().0
    };
    assert_eq!(invalid(|b| b.capacity = 0), "capacity");
    assert_eq!(invalid(|b| b.max_rows = 0), "rows");
    assert_eq!(invalid(|b| b.max_arrow_bytes = 0), "bytes");
    assert_eq!(invalid(|b| b.max_intermediate_bytes = 0), "bytes");
    assert_eq!(invalid(|b| b.max_limit = 0), "limit");
    assert_eq!(invalid(|b| b.default_limit = 0), "limit");
    assert_eq!(invalid(|b| b.default_limit = b.max_limit + 1), "limit");
    assert_eq!(invalid(|b| b.timeout = Duration::ZERO), "timeout");
    assert_eq!(invalid(|b| b.timeout = Duration::MAX), "timeout");
}

#[test]
fn execution_leaves_the_audit_its_reserve_of_the_timeout() {
    // R = min(500 ms, T/4): below, at and above 2 s.
    for (timeout, execution) in [(1_000, 750), (2_000, 1_500), (10_000, 9_500)] {
        let profile = Profile::new(Budgets {
            timeout: Duration::from_millis(timeout),
            ..INITIAL
        })
        .unwrap();
        assert_eq!(
            profile.execution_timeout(),
            Duration::from_millis(execution)
        );
    }
    // Too short to share: refused when the service prepares.
    let mut tiny = INITIAL;
    tiny.timeout = Duration::from_nanos(3);
    assert_eq!(Profile::new(tiny).unwrap_err().0, "timeout");
}

#[test]
fn every_failure_has_a_closed_code() {
    let codes: Vec<&str> = [
        RunFailure::LimitInvalid,
        RunFailure::MaskedUnsupported,
        RunFailure::TemporalUnsupported,
        RunFailure::Busy,
        RunFailure::Deadline,
        RunFailure::Cancelled,
        RunFailure::Budget,
        RunFailure::Engine,
        RunFailure::Incomplete,
        RunFailure::Evidence,
        RunFailure::Codec,
        RunFailure::Unavailable,
    ]
    .iter()
    .map(RunFailure::code)
    .collect();
    assert!(
        codes
            .iter()
            .all(|code| code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
    );
    let mut unique = codes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), codes.len());
}

/// A measured request: its name, its state and how to build it.
type Case<'a> = (&'a str, &'a AppState, fn() -> ExplainRequest);

fn summary(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    json!({
        "p50_ms": values[values.len() / 2],
        "p95_ms": values[values.len() * 95 / 100],
        "max_ms": values[values.len() - 1],
    })
}

/// Reproducible cost of one governed execution, against the preparation
/// alone (what explain already does) in the same harness: cold on a fresh
/// runtime, warm p50/p95, the Issue's four rows and a 10 000-row answer,
/// and the process's peak RSS, which DuckDB's quota does not bound. Run
/// explicitly:
/// `cargo test --release -p bi-server --no-default-features --features dev-auth --lib semantic::run::tests::latency -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "measurement, not a CI latency threshold"]
async fn latency() {
    const WARMUP: usize = 20;
    const SAMPLES: usize = 300;
    const COLD: usize = 10;
    let pid = sysinfo::get_current_pid().unwrap();
    let mut system = sysinfo::System::new();
    let mut rss = move || {
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        system.process(pid).map_or(0, sysinfo::Process::memory)
    };
    let started_rss = rss();
    let mut peak = started_rss;
    let ms = |started: Instant| started.elapsed().as_secs_f64() * 1000.0;
    // Cold: a fresh runtime and project, its first execution.
    let mut cold = Vec::new();
    for _ in 0..COLD {
        let p = project(&model("", ""), MEMBERS);
        let state = state(p.path(), analytics().await);
        let started = Instant::now();
        run(&state, ana(), by_region()).await.unwrap();
        cold.push(ms(started));
    }
    let mut report = serde_json::Map::new();
    report.insert("cold_by_region".into(), summary(cold));
    let p = project(&model("", ""), MEMBERS);
    let small = state(p.path(), analytics().await);
    let (_q, large) = many_cities().await;
    // The emulation beyond DECIMAL(38): its total, and by region in the
    // API's canonical order and limit, which the emulation applies (#256).
    let cubed = project(&model("", CUBED), MEMBERS);
    let wide = state(cubed.path(), analytics().await);
    let cases: [Case<'_>; 4] = [
        ("by_region_2_rows", &small, by_region),
        ("by_city_10000_rows", &large, || by_city(Some(10_000))),
        ("wide_total", &wide, || {
            selections(&json!([metric("c", "cubo")]))
        }),
        ("wide_by_region_ordered", &wide, || {
            selections(&json!([
                dimension("r", "ventas.region"),
                metric("c", "cubo")
            ]))
        }),
    ];
    for (name, state, request) in cases {
        let (mut prepared, mut executed) = (Vec::new(), Vec::new());
        let mut bytes = 0;
        for i in 0..WARMUP + SAMPLES {
            let started = Instant::now();
            let Ok(_) = prepare::prepare(state, ana(), &request()).await else {
                panic!("prepared");
            };
            let preparing = ms(started);
            let started = Instant::now();
            let answer = run(state, ana(), request()).await.unwrap();
            let executing = ms(started);
            peak = peak.max(rss());
            if i >= WARMUP {
                prepared.push(preparing);
                executed.push(executing);
                bytes = serde_json::to_vec(&answer).unwrap().len();
            }
        }
        report.insert(
            name.into(),
            json!({
                "preparation_only": summary(prepared),
                "execution": summary(executed),
                "body_bytes": bytes,
            }),
        );
    }
    let system = sysinfo::System::new_all();
    println!(
        "{}",
        json!({
            "task": "137",
            "path": "semantic::run::execute, in-memory DuckDB, profile INITIAL",
            "os": sysinfo::System::long_os_version(),
            "cpu": system.cpus()[0].brand(),
            "logical_cpus": system.cpus().len(),
            "ram_bytes": system.total_memory(),
            "warmup": WARMUP,
            "samples": SAMPLES,
            "cold_samples": COLD,
            "rss_start_bytes": started_rss,
            "rss_peak_bytes": peak,
            "report": report,
        })
    );
}
