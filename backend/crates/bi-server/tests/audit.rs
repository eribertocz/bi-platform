//! Real PostgreSQL, including malicious-owner manipulation and restricted roles.
#![cfg(feature = "postgres-tests")]

use bi_audit::{
    Action, ActorType, Checkpoint, Error as IntegrityError, Event, MetadataRedacted, ResultCode,
    Source,
};
use bi_server::{audit, db::MIGRATOR};
use sqlx::{AssertSqlSafe, PgPool};

fn event(id: &str) -> Event {
    Event {
        event_id: id.into(),
        occurred_at: "2026-10-11T12:00:00Z".parse().unwrap(),
        actor_id: "actor-1".into(),
        actor_type: ActorType::User,
        session_id: Some("session-1".into()),
        action: Action::Query,
        resource_type: "dataset".into(),
        resource_id: "sales".into(),
        result: ResultCode::Success,
        request_id: "request-1".into(),
        trace_id: "a".repeat(32),
        policy_decision_id: None,
        source: Source::Http,
        metadata_redacted: MetadataRedacted::default(),
        schema_version: 1,
    }
}

async fn append(pool: &PgPool, id: &str) -> Checkpoint {
    let mut transaction = pool.begin().await.unwrap();
    let receipt = audit::append(&mut transaction, &event(id)).await.unwrap();
    transaction.commit().await.unwrap();
    receipt
}

#[sqlx::test(migrations = false)]
async fn concurrent_writers_and_paged_verification_match_rust_hashes(pool: PgPool) {
    MIGRATOR.run(&pool).await.unwrap();
    assert_eq!(
        audit::verify(&pool, None).await.unwrap(),
        Checkpoint::default()
    );
    let mut tasks = tokio::task::JoinSet::new();
    for id in 0..270 {
        let pool = pool.clone();
        tasks.spawn(async move { append(&pool, &format!("event-{id}")).await });
    }
    let mut receipts = Vec::new();
    while let Some(result) = tasks.join_next().await {
        receipts.push(result.unwrap());
        if receipts.len().is_multiple_of(25) {
            assert!(audit::verify(&pool, None).await.unwrap().sequence <= 270);
        }
    }
    receipts.sort_by_key(|c| c.sequence);
    assert_eq!(
        receipts.iter().map(|c| c.sequence).collect::<Vec<_>>(),
        (1..=270).collect::<Vec<_>>()
    );
    assert_eq!(
        audit::verify(&pool, Some(receipts[100])).await.unwrap(),
        receipts[269]
    );
    let mut fake = receipts[100];
    fake.hash[0] ^= 1;
    assert_eq!(
        audit::verify(&pool, Some(fake)).await,
        Err(audit::Error::Contract(IntegrityError::HeadMismatch))
    );
    let future = Checkpoint {
        sequence: 271,
        hash: [1; 32],
    };
    assert!(audit::verify(&pool, Some(future)).await.is_err());
}

#[sqlx::test(migrations = false)]
async fn rollback_and_duplicate_id_do_not_consume_sequence(pool: PgPool) {
    MIGRATOR.run(&pool).await.unwrap();
    let mut transaction = pool.begin().await.unwrap();
    audit::append(&mut transaction, &event("rolled-back"))
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    let first = append(&pool, "event-1").await;
    assert_eq!(first.sequence, 1);
    let mut transaction = pool.begin().await.unwrap();
    assert!(
        audit::append(&mut transaction, &event("event-1"))
            .await
            .is_err()
    );
    transaction.rollback().await.unwrap();
    assert_eq!(append(&pool, "event-2").await.sequence, 2);
    assert_eq!(audit::verify(&pool, Some(first)).await.unwrap().sequence, 2);
}

#[sqlx::test(migrations = false)]
async fn invalid_events_log_redacted_code_and_write_nothing(pool: PgPool) {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};
    use tracing::instrument::WithSubscriber as _;
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    MIGRATOR.run(&pool).await.unwrap();
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let output = Capture(bytes.clone());
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || output.clone())
        .finish();
    let mut transaction = pool.begin().await.unwrap();
    let mut invalid = event("invalid");
    invalid.request_id = "password=secret@example.test".into();
    // Instrument the future so the subscriber follows async polls on any worker.
    assert_eq!(
        audit::append(&mut transaction, &invalid)
            .with_subscriber(subscriber)
            .await,
        Err(audit::Error::Contract(IntegrityError::InvalidEvent))
    );
    transaction.rollback().await.unwrap();
    let logs = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("audit_invalid_event"));
    assert!(!logs.contains("password") && !logs.contains("secret@example.test"));
    assert_eq!(
        audit::verify(&pool, None).await.unwrap(),
        Checkpoint::default()
    );
    // The database boundary also rejects raw missing fields/secret metadata.
    let mut value = serde_json::to_value(event("event-1")).unwrap();
    value["metadata_redacted"]["password"] = serde_json::json!("secret");
    for invalid in ["{}".to_owned(), value.to_string()] {
        assert!(
            sqlx::query("SELECT * FROM bi_meta.audit_append($1)")
                .bind(invalid)
                .execute(&pool)
                .await
                .is_err()
        );
    }
    assert_eq!(
        audit::verify(&pool, None).await.unwrap(),
        Checkpoint::default()
    );
}

#[sqlx::test(migrations = false)]
async fn query_shape_round_trips_and_database_rejects_text_and_invalid_counts(pool: PgPool) {
    MIGRATOR.run(&pool).await.unwrap();
    let mut query = event("valid-shape");
    query.metadata_redacted.query_shape = Some(bi_audit::QueryShape {
        dimensions: 0,
        measures: 2,
        filters: 3,
        limit: u32::MAX,
    });
    let mut transaction = pool.begin().await.unwrap();
    let receipt = audit::append(&mut transaction, &query).await.unwrap();
    transaction.commit().await.unwrap();
    assert_eq!(audit::verify(&pool, None).await.unwrap(), receipt);
    let mut value = serde_json::to_value(query).unwrap();
    value["event_id"] = serde_json::json!("invalid-shape");
    for invalid in [
        serde_json::json!(-1),
        serde_json::json!(4_294_967_296_u64),
        serde_json::json!("private@example.test"),
        serde_json::Value::Null,
        serde_json::json!(true),
        serde_json::json!(1.5),
    ] {
        let mut attempt = value.clone();
        attempt["metadata_redacted"]["query_shape"]["filters"] = invalid;
        let error = sqlx::query("SELECT * FROM bi_meta.audit_append($1)")
            .bind(attempt.to_string())
            .execute(&pool)
            .await
            .unwrap_err();
        assert_eq!(error.as_database_error().unwrap().code().unwrap(), "22023");
    }
    value["metadata_redacted"]["query_shape"]["sql"] = serde_json::json!("SELECT private_value");
    assert!(
        sqlx::query("SELECT * FROM bi_meta.audit_append($1)")
            .bind(value.to_string())
            .execute(&pool)
            .await
            .is_err()
    );
    assert_eq!(audit::verify(&pool, None).await.unwrap(), receipt);
}

#[sqlx::test(migrations = false)]
async fn runtime_has_only_append_function_and_security_action_rolls_back(pool: PgPool) {
    MIGRATOR.run(&pool).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    );
    let role = format!("{database}_audit_app");
    sqlx::raw_sql(AssertSqlSafe(format!("CREATE ROLE {role} NOLOGIN; GRANT USAGE ON SCHEMA bi_meta TO {role}; GRANT EXECUTE ON FUNCTION bi_meta.audit_append(TEXT) TO {role};"))).execute(&pool).await.unwrap();
    let mut transaction = pool.begin().await.unwrap();
    sqlx::query(AssertSqlSafe(format!("SET LOCAL ROLE {role}")))
        .execute(&mut *transaction)
        .await
        .unwrap();
    assert_eq!(
        audit::append(&mut transaction, &event("allowed"))
            .await
            .unwrap()
            .sequence,
        1
    );
    transaction.commit().await.unwrap();
    for sql in [
        "UPDATE bi_meta.audit_events SET payload = '{}'",
        "DELETE FROM bi_meta.audit_events",
        "TRUNCATE bi_meta.audit_events",
        "INSERT INTO bi_meta.audit_events(sequence, payload, previous_hash, hash) SELECT 2, payload, hash, hash FROM bi_meta.audit_events",
        "UPDATE bi_meta.audit_head SET sequence = 0",
        "SELECT * FROM bi_meta.audit_events",
    ] {
        let mut transaction = pool.begin().await.unwrap();
        sqlx::query(AssertSqlSafe(format!("SET LOCAL ROLE {role}")))
            .execute(&mut *transaction)
            .await
            .unwrap();
        let error = sqlx::query(sql)
            .execute(&mut *transaction)
            .await
            .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().unwrap(),
            "42501",
            "{sql}"
        );
        transaction.rollback().await.unwrap();
    }
    // Even the owner is guarded against accidental mutation by the trigger.
    assert!(
        sqlx::query("DELETE FROM bi_meta.audit_events")
            .execute(&pool)
            .await
            .is_err()
    );
    // A real session write and duplicate audit event share one transaction.
    let mut transaction = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO bi_meta.sessions(token_hash, user_id, display_name, data_scope, expires_at) VALUES ($1, 'actor-1', 'Synthetic', 'synthetic', clock_timestamp() + interval '1 hour')")
        .bind(vec![1_u8; 32]).execute(&mut *transaction).await.unwrap();
    assert!(
        audit::append(&mut transaction, &event("allowed"))
            .await
            .is_err()
    );
    transaction.rollback().await.unwrap();
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM bi_meta.sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0);
    assert_eq!(audit::verify(&pool, None).await.unwrap().sequence, 1);
    sqlx::raw_sql(AssertSqlSafe(format!("REVOKE ALL ON SCHEMA bi_meta FROM {role}; REVOKE ALL ON FUNCTION bi_meta.audit_append(TEXT) FROM {role}; DROP ROLE {role};"))).execute(&pool).await.unwrap();
}

async fn tamper(pool: &PgPool, sql: &'static str) {
    MIGRATOR.run(pool).await.unwrap();
    for id in 1..=3 {
        append(pool, &format!("event-{id}")).await;
    }
    // Simulates an operator/owner bypassing the append-only guard.
    sqlx::query("ALTER TABLE bi_meta.audit_events DISABLE TRIGGER audit_immutable")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(sql).execute(pool).await.unwrap();
    assert!(audit::verify(pool, None).await.is_err());
}

#[sqlx::test(migrations = false)]
async fn changed_row_detected(pool: PgPool) {
    tamper(&pool, "UPDATE bi_meta.audit_events SET payload = replace(payload, 'actor-1', 'actor-2') WHERE sequence = 2").await;
}

#[sqlx::test(migrations = false)]
async fn missing_middle_detected(pool: PgPool) {
    tamper(&pool, "DELETE FROM bi_meta.audit_events WHERE sequence = 2").await;
}

#[sqlx::test(migrations = false)]
async fn missing_tail_detected(pool: PgPool) {
    tamper(&pool, "DELETE FROM bi_meta.audit_events WHERE sequence = 3").await;
}

#[sqlx::test(migrations = false)]
async fn entire_history_deleted_detected(pool: PgPool) {
    tamper(&pool, "TRUNCATE bi_meta.audit_events").await;
}

#[sqlx::test(migrations = false)]
async fn out_of_order_insert_detected(pool: PgPool) {
    tamper(&pool, "INSERT INTO bi_meta.audit_events(sequence, payload, previous_hash, hash) SELECT 5, replace(payload, 'event-1', 'injected'), previous_hash, hash FROM bi_meta.audit_events WHERE sequence = 1").await;
}

#[sqlx::test(migrations = false)]
async fn external_checkpoint_detects_owner_rewinding_head_and_tail(pool: PgPool) {
    MIGRATOR.run(&pool).await.unwrap();
    let first = append(&pool, "event-1").await;
    let trusted = append(&pool, "event-2").await;
    sqlx::raw_sql("ALTER TABLE bi_meta.audit_events DISABLE TRIGGER audit_immutable; DELETE FROM bi_meta.audit_events WHERE sequence = 2;").execute(&pool).await.unwrap();
    sqlx::query("UPDATE bi_meta.audit_head SET sequence = 1, hash = $1")
        .bind(first.hash.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(audit::verify(&pool, None).await.unwrap(), first);
    assert_eq!(
        audit::verify(&pool, Some(trusted)).await,
        Err(audit::Error::Contract(IntegrityError::HeadMismatch))
    );
}

#[sqlx::test(migrations = false)]
async fn verifier_command_is_read_only_and_reports_tampering(pool: PgPool) {
    use sqlx::ConnectOptions as _;
    MIGRATOR.run(&pool).await.unwrap();
    append(&pool, "event-1").await;
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    );
    let reader = format!("{database}_reader");
    sqlx::raw_sql(AssertSqlSafe(format!("CREATE ROLE {reader} NOLOGIN; GRANT USAGE ON SCHEMA bi_meta TO {reader}; GRANT SELECT ON bi_meta.audit_events, bi_meta.audit_head TO {reader};"))).execute(&pool).await.unwrap();
    let url = pool
        .connect_options()
        .as_ref()
        .clone()
        .options([("role", reader.as_str())])
        .to_url_lossy()
        .to_string();
    let execute = |url: String| {
        tokio::task::spawn_blocking(move || {
            std::process::Command::new(env!("CARGO_BIN_EXE_bi-server"))
                .arg("audit-verify")
                .env("BI_DATABASE_URL", url)
                .output()
                .unwrap()
        })
    };
    let output = execute(url.clone()).await.unwrap();
    assert!(output.status.success());
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["sequence"], "1");
    assert_eq!(receipt["hash"].as_str().unwrap().len(), 64);
    assert_eq!(output.stderr.as_slice(), b"");
    sqlx::raw_sql("ALTER TABLE bi_meta.audit_events DISABLE TRIGGER audit_immutable; DELETE FROM bi_meta.audit_events;").execute(&pool).await.unwrap();
    let output = execute(url).await.unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "audit_head_mismatch"
    );
    assert_eq!(output.stdout.as_slice(), b"");
    sqlx::raw_sql(AssertSqlSafe(format!("REVOKE ALL ON SCHEMA bi_meta FROM {reader}; REVOKE ALL ON bi_meta.audit_events, bi_meta.audit_head FROM {reader}; DROP ROLE {reader};"))).execute(&pool).await.unwrap();
}

#[sqlx::test(migrations = false)]
async fn otel_attempt_metrics_have_bounded_labels_and_include_failures(pool: PgPool) {
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::data::{
        AggregatedMetrics, HistogramDataPoint, MetricData, ResourceMetrics, ScopeMetrics,
    };
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
    MIGRATOR.run(&pool).await.unwrap();
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter.clone())
        .build();
    let metrics = audit::AuditMetrics::new(&provider.meter("test"));
    let mut transaction = pool.begin().await.unwrap();
    assert!(
        audit::append_observed(&mut transaction, &event("event-1"), &metrics)
            .await
            .is_ok()
    );
    let mut invalid = event("private-event-id");
    invalid.actor_id.clear();
    assert!(
        audit::append_observed(&mut transaction, &invalid, &metrics)
            .await
            .is_err()
    );
    assert!(
        audit::append_observed(&mut transaction, &event("event-1"), &metrics)
            .await
            .is_err()
    );
    provider.force_flush().unwrap();
    let exports = exporter.get_finished_metrics().unwrap();
    let mut attempts = 0;
    let mut durations = 0;
    for metric in exports
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(ScopeMetrics::metrics)
    {
        match metric.data() {
            AggregatedMetrics::U64(MetricData::Sum(sum)) => {
                assert_eq!(metric.name(), "bi.audit.append.attempts");
                for point in sum.data_points() {
                    let labels: Vec<_> = point.attributes().collect();
                    assert_eq!(labels.len(), 1);
                    assert_eq!(labels[0].key.as_str(), "result");
                    assert!(
                        ["pending", "invalid", "unavailable"]
                            .contains(&labels[0].value.to_string().as_str())
                    );
                    attempts += point.value();
                }
            }
            AggregatedMetrics::F64(MetricData::Histogram(histogram)) => {
                assert_eq!(metric.name(), "bi.audit.append.duration");
                assert_eq!(metric.unit(), "s");
                assert!(
                    histogram
                        .data_points()
                        .all(|p| p.bounds().any(|b| b < 0.001))
                );
                durations += histogram
                    .data_points()
                    .map(HistogramDataPoint::count)
                    .sum::<u64>();
            }
            other => panic!("unexpected metric: {other:?}"),
        }
    }
    assert_eq!(attempts, 3);
    assert_eq!(durations, 3);
    transaction.rollback().await.unwrap();
}

/// Same connection/commit path with and without an audit write; no thresholds on
/// shared CI machines. Includes WAL flush/commit, not just hashing or INSERT.
#[sqlx::test(migrations = false)]
async fn audit_write_cost_baseline(pool: PgPool) {
    use sqlx::Connection as _;
    use std::time::Instant;
    MIGRATOR.run(&pool).await.unwrap();
    let mut connection = pool.acquire().await.unwrap();
    let mut baseline = Vec::new();
    let mut audited = Vec::new();
    for id in 0..110 {
        for with_audit in [false, true] {
            let started = Instant::now();
            let mut transaction = connection.begin().await.unwrap();
            sqlx::query("SELECT 1")
                .execute(&mut *transaction)
                .await
                .unwrap();
            if with_audit {
                audit::append(&mut transaction, &event(&format!("bench-{id}")))
                    .await
                    .unwrap();
            }
            transaction.commit().await.unwrap();
            if id >= 10 {
                let samples = if with_audit {
                    &mut audited
                } else {
                    &mut baseline
                };
                samples.push(started.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    baseline.sort_by(f64::total_cmp);
    audited.sort_by(f64::total_cmp);
    println!(
        "audit_write_cost n=100 warmup=10 baseline_p50_ms={:.3} baseline_p95_ms={:.3} audited_p50_ms={:.3} audited_p95_ms={:.3}",
        baseline[49], baseline[94], audited[49], audited[94]
    );
    assert_eq!(audit::verify(&pool, None).await.unwrap().sequence, 110);
}
