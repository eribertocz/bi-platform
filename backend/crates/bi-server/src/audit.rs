//! PostgreSQL adapter for the independent audit contract. No HTTP routes yet.

use std::fmt;
use std::time::Instant;

use bi_audit::{Checkpoint, Event, Verifier};
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram, Meter},
};
use sqlx::{PgConnection, PgPool, Postgres, Row, Transaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Contract(bi_audit::Error),
    StoreUnavailable,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => error.fmt(f),
            Self::StoreUnavailable => f.write_str("audit_store_unavailable"),
        }
    }
}

impl std::error::Error for Error {}

impl From<sqlx::Error> for Error {
    fn from(_: sqlx::Error) -> Self {
        // Database errors can contain payloads or credentials; do not log them.
        Self::StoreUnavailable
    }
}

/// Explicit meter injection; no global exporter/provider or actor labels.
/// Measures append attempts before commit, not confirmed security actions.
#[derive(Default, Clone)]
pub struct AuditMetrics {
    instruments: Option<(Histogram<f64>, Counter<u64>)>,
}

impl AuditMetrics {
    #[must_use]
    pub fn new(meter: &Meter) -> Self {
        Self {
            instruments: Some((
                meter
                    .f64_histogram("bi.audit.append.duration")
                    .with_unit("s")
                    .with_boundaries(vec![
                        0.0001, 0.00025, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1,
                        0.25, 0.5, 1.0, 5.0,
                    ])
                    .build(),
                meter.u64_counter("bi.audit.append.attempts").build(),
            )),
        }
    }
}

fn checkpoint(row: &sqlx::postgres::PgRow) -> Result<Checkpoint, Error> {
    let sequence: i64 = row.try_get("sequence")?;
    let hash: Vec<u8> = row.try_get("hash")?;
    Ok(Checkpoint {
        sequence: sequence
            .try_into()
            .map_err(|_| Error::Contract(bi_audit::Error::SequenceMismatch))?,
        hash: hash
            .try_into()
            .map_err(|_| Error::Contract(bi_audit::Error::HashMismatch))?,
    })
}

/// Appends in the CALLER's transaction, never commits or retries internally.
/// The caller must roll back the security action if this returns an error.
/// Invalid events are logged by stable code, never by their contents.
/// # Errors
/// Returns redacted validation/storage errors. Every event is written; no sampling.
pub async fn append(
    transaction: &mut Transaction<'_, Postgres>,
    event: &Event,
) -> Result<Checkpoint, Error> {
    append_observed(transaction, event, &AuditMetrics::default()).await
}

/// Same transaction contract as `append`, with an injected OpenTelemetry meter.
/// # Errors
/// Returns validation/storage errors; optional metrics never decide success.
pub async fn append_observed(
    transaction: &mut Transaction<'_, Postgres>,
    event: &Event,
    metrics: &AuditMetrics,
) -> Result<Checkpoint, Error> {
    let started = Instant::now();
    let outcome = append_inner(transaction, event).await;
    if let Some((duration, attempts)) = &metrics.instruments {
        let result = match outcome {
            Ok(_) => "pending",
            Err(Error::Contract(_)) => "invalid",
            Err(Error::StoreUnavailable) => "unavailable",
        };
        let attributes = [KeyValue::new("result", result)];
        duration.record(started.elapsed().as_secs_f64(), &attributes);
        attempts.add(1, &attributes);
    }
    // request_id is logged only for validated events (avoid logging invalid input).
    let request_id = if event.validate().is_ok() {
        event.request_id.as_str()
    } else {
        "unavailable"
    };
    match &outcome {
        Ok(_) => tracing::info!(
            request_id,
            trace_id = %event.trace_id,
            event_code = "audit_append_pending",
            write_us = started.elapsed().as_micros()
        ),
        Err(error) => tracing::error!(request_id, event_code = %error, "audit append failed"),
    }
    outcome
}

async fn append_inner(connection: &mut PgConnection, event: &Event) -> Result<Checkpoint, Error> {
    let payload = event.canonical_json().map_err(Error::Contract)?;
    let row = sqlx::query("SELECT sequence, hash FROM bi_meta.audit_append($1)")
        .bind(payload)
        .fetch_one(connection)
        .await?;
    checkpoint(&row)
}

/// Verifies a consistent, read-only snapshot in bounded pages. If provided,
/// `trusted` must match the checkpoint at that sequence, even if later rows
/// have been appended. Keep trusted checkpoints outside PostgreSQL.
/// # Errors
/// Rejects altered/reordered/missing/invalid rows or an inconsistent head.
pub async fn verify(pool: &PgPool, trusted: Option<Checkpoint>) -> Result<Checkpoint, Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *transaction)
        .await?;
    let head = sqlx::query("SELECT sequence, hash FROM bi_meta.audit_head WHERE singleton")
        .fetch_one(&mut *transaction)
        .await?;
    let head = checkpoint(&head)?;
    let mut verifier = Verifier::default();
    let mut cursor = 0_i64;
    let mut trusted_seen = trusted.is_none() || trusted == Some(Checkpoint::default());
    loop {
        let rows = sqlx::query("SELECT sequence, payload, previous_hash, hash FROM bi_meta.audit_events WHERE sequence > $1 ORDER BY sequence LIMIT 256")
            .bind(cursor).fetch_all(&mut *transaction).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let current = checkpoint(&row)?;
            let previous: Vec<u8> = row.try_get("previous_hash")?;
            let previous: [u8; 32] = previous
                .try_into()
                .map_err(|_| Error::Contract(bi_audit::Error::PreviousHashMismatch))?;
            let payload: String = row.try_get("payload")?;
            verifier
                .push(current.sequence, &payload, &previous, &current.hash)
                .map_err(Error::Contract)?;
            if trusted.is_some_and(|t| t.sequence == current.sequence) {
                if trusted != Some(current) {
                    return Err(Error::Contract(bi_audit::Error::HeadMismatch));
                }
                trusted_seen = true;
            }
            cursor = row.try_get("sequence")?;
        }
    }
    if !trusted_seen {
        return Err(Error::Contract(bi_audit::Error::HeadMismatch));
    }
    let result = verifier.finish(head).map_err(Error::Contract)?;
    transaction.commit().await?;
    Ok(result)
}
