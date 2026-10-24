//! The audit writer's budget covers every wait, the event's identifier
//! included: entropy runs on the blocking pool, which another task may be
//! holding (#138).

use std::time::{Duration, Instant};

use super::{HttpRecord, record_within};
use crate::db::Database;
use crate::metadata::MetadataMode;

fn record() -> HttpRecord<'static> {
    HttpRecord {
        actor: "ana",
        request_id: "req-audit-budget",
        action: bi_audit::Action::Query,
        result: bi_audit::ResultCode::Failure,
        resource_type: "semantic_request",
        resource_id: "semantic:request",
        policy_decision_id: None,
        query_shape: None,
    }
}

#[test]
fn a_busy_blocking_pool_never_outlives_the_audit_budget() {
    // One blocking thread, held: the identifier's entropy has to queue.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        // No metadata database is reachable: the write could only fail
        // after the identifier, never wait for it unbounded.
        let db = Database::start(
            &"postgres://bi:bi@127.0.0.1:1/bi".parse().unwrap(),
            MetadataMode::Development,
        );
        let (release, held) = std::sync::mpsc::channel::<()>();
        let (started, running) = tokio::sync::oneshot::channel();
        let holder = tokio::task::spawn_blocking(move || {
            let _ = started.send(());
            let _ = held.recv();
        });
        running.await.unwrap();
        let budget = Duration::from_millis(200);
        let begun = Instant::now();
        let refused =
            tokio::time::timeout(Duration::from_secs(5), record_within(&db, record(), budget))
                .await
                .expect("the budget bounds the wait for entropy");
        let elapsed = begun.elapsed();
        assert_eq!(refused, Err("audit_unavailable"));
        assert!(elapsed >= budget, "{elapsed:?}");
        assert!(elapsed < budget * 3, "{elapsed:?} for a {budget:?} budget");
        // Released, the identifier is generated again: what fails now is the
        // unreachable database, promptly and within the same budget.
        release.send(()).unwrap();
        holder.await.unwrap();
        let begun = Instant::now();
        let answered = record_within(&db, record(), Duration::from_secs(5)).await;
        assert!(answered.is_err());
        assert!(begun.elapsed() < Duration::from_secs(5));
    });
}
