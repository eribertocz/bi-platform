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

/// PostgreSQL's `data_exception` class for a value rejected by the function
/// itself (`RAISE ... USING ERRCODE = '22023'`): a contract violation the
/// caller wrote, not an availability problem. Every other code (connection,
/// timeout, serialization, internal errors...) stays `StoreUnavailable`.
const INVALID_EVENT_SQLSTATE: &str = "22023";

impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        // Database errors can contain payloads or credentials; do not log them.
        match &error {
            sqlx::Error::Database(database)
                if database.code().as_deref() == Some(INVALID_EVENT_SQLSTATE) =>
            {
                Self::Contract(bi_audit::Error::InvalidEvent)
            }
            _ => Self::StoreUnavailable,
        }
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
    // A cryptographic break found while reaching here would already have
    // returned above (`?` on `push`), taking priority over any earlier
    // content defect. Only once the chain is cryptographically complete do
    // we also require every row's content to be a well-formed `Event`:
    // this call stays strict, unlike `report`.
    if verifier.invalid_content_count() > 0 {
        return Err(Error::Contract(bi_audit::Error::InvalidEvent));
    }
    let result = verifier.finish(head).map_err(Error::Contract)?;
    transaction.commit().await?;
    Ok(result)
}

/// A point-in-time account of the audit history, bounded in memory
/// (paginated, no accumulated history or per-row diagnostics): how far the
/// hash chain stayed intact, whether every row along the way was a
/// well-formed `Event`, and where the first defect of each kind was.
///
/// Unlike [`verify`], a cryptographic or content defect is reported, not an
/// error: only a genuine storage or connection failure returns `Err`, since
/// then nothing could be printed as a trustworthy report at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// `false` only for a cryptographic break (sequence, link, hash or an
    /// external checkpoint mismatch); independent of content validity.
    pub chain_valid: bool,
    /// `None` when the walk stopped (a cryptographic break) before every
    /// row up to the head was seen, and no content defect was found yet: a
    /// defect may exist in the unseen remainder. `Some(false)` as soon as
    /// one is found, whether or not the walk later stops. `Some(true)` only
    /// once the entire history was walked with none.
    pub content_valid: Option<bool>,
    pub invalid_content_count: u64,
    pub first_invalid_content_sequence: Option<u64>,
    /// The first cryptographic break, if the chain did not stay intact to
    /// its expected head (and external checkpoint, if one was given).
    pub integrity_error: Option<bi_audit::Error>,
    pub first_integrity_error_sequence: Option<u64>,
    /// Only `Some` once the chain was followed, cryptographically intact,
    /// all the way to the expected head (and external checkpoint, if any).
    pub checkpoint: Option<Checkpoint>,
}

/// Same scan as [`verify`], reported instead of enforced.
/// # Errors
/// Storage or connection failures only; a cryptographic or content defect
/// is a field of the returned [`Report`], not an `Err`.
pub async fn report(pool: &PgPool, trusted: Option<Checkpoint>) -> Result<Report, Error> {
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
    let mut integrity_error = None;
    let mut first_integrity_error_sequence = None;
    'scan: loop {
        let rows = sqlx::query("SELECT sequence, payload, previous_hash, hash FROM bi_meta.audit_events WHERE sequence > $1 ORDER BY sequence LIMIT 256")
            .bind(cursor).fetch_all(&mut *transaction).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let sequence: i64 = row.try_get("sequence")?;
            let previous: Vec<u8> = row.try_get("previous_hash")?;
            let hash: Vec<u8> = row.try_get("hash")?;
            let payload: String = row.try_get("payload")?;
            // Our own storage producing a value outside its declared shape
            // (never attacker-controlled JSON) is itself an integrity
            // defect, at the same severity as a broken link.
            let (Ok(current_sequence), Ok(previous), Ok(hash)) = (
                u64::try_from(sequence),
                <[u8; 32]>::try_from(previous),
                <[u8; 32]>::try_from(hash),
            ) else {
                integrity_error = Some(bi_audit::Error::HashMismatch);
                first_integrity_error_sequence = u64::try_from(sequence).ok();
                break 'scan;
            };
            if let Err(error) = verifier.push(current_sequence, &payload, &previous, &hash) {
                integrity_error = Some(error);
                first_integrity_error_sequence = Some(current_sequence);
                break 'scan;
            }
            if trusted.is_some_and(|t| t.sequence == current_sequence) {
                if trusted
                    != Some(Checkpoint {
                        sequence: current_sequence,
                        hash,
                    })
                {
                    integrity_error = Some(bi_audit::Error::HeadMismatch);
                    first_integrity_error_sequence = Some(current_sequence);
                    break 'scan;
                }
                trusted_seen = true;
            }
            cursor = sequence;
        }
    }
    transaction.commit().await?;

    let mut checkpoint_out = None;
    if integrity_error.is_none() {
        if !trusted_seen {
            integrity_error = Some(bi_audit::Error::HeadMismatch);
        } else if verifier.checkpoint() == head {
            checkpoint_out = Some(head);
        } else {
            integrity_error = Some(bi_audit::Error::HeadMismatch);
            first_integrity_error_sequence = Some(head.sequence);
        }
    }

    let invalid_content_count = verifier.invalid_content_count();
    let content_valid = if invalid_content_count > 0 {
        Some(false)
    } else if checkpoint_out.is_some() {
        Some(true)
    } else {
        None
    };
    Ok(Report {
        chain_valid: integrity_error.is_none(),
        content_valid,
        invalid_content_count,
        first_invalid_content_sequence: verifier.first_invalid_content_sequence(),
        integrity_error,
        first_integrity_error_sequence,
        checkpoint: checkpoint_out,
    })
}
