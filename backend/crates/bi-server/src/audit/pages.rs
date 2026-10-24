//! Reading the audit trail in pages (#160): an internal repository, not yet
//! exposed to anyone (#161 will, behind its own permission and audit).
//!
//! Each page takes its connection from [`AuditReader::acquire`], which
//! checks the reader's role on every hand-out, and from nothing else: no
//! pool, URL or other connection is accepted, none is kept between pages,
//! and a refused or unavailable reader yields no page.
//!
//! A walk is ordered by `sequence`. Its first page captures the committed
//! head of the chain with a plain read, never locking the writer, and every
//! later page stays at or below it, so events appended meanwhile neither
//! appear nor shift the walk: a logical snapshot of an append-only log, not
//! a transaction held open between pages, and not a verification of the
//! chain (`audit-verify` remains that).
//!
//! Profile v1: one monotonic deadline of 5 s per page from before the
//! connection is requested, enforced by the client and, within the
//! transaction, by PostgreSQL on what remains; a page of at most 4 MiB of
//! JSON, counted while it is written; 1 to 200 events, 50 by default;
//! continuation tokens of at most 1 KiB that expire 15 minutes after the
//! first page.

mod cursor;
mod encode;

use std::fmt;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bi_audit::{Action, Event};
use chrono::{DateTime, Datelike as _, Timelike as _, Utc};
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};
use sqlx::{Connection as _, PgConnection, Row as _};
use tokio::time::{Instant, timeout_at};

pub use cursor::{CursorKey, MAX_ENCODED as MAX_CURSOR_BYTES};

use super::reader::AuditReader;
use cursor::Position;

pub const DEFAULT_LIMIT: u16 = 50;
pub const MAX_LIMIT: u16 = 200;
/// How long a walk's tokens stay valid from its first page; never renewed.
pub const CURSOR_LIFETIME: Duration = Duration::from_mins(15);
/// The longest actor accepted in a filter: the longest one stored.
pub const MAX_ACTOR_BYTES: usize = 128;

/// Why no page. `Display` is the stable code for logs and metrics; nothing
/// of the request, the token or the stored rows is ever part of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageError {
    InvalidLimit,
    InvalidActor,
    InvalidAction,
    InvalidInstant,
    /// `from` is not before `to`.
    InvalidRange,
    /// Malformed, edited, or issued under another key.
    CursorInvalid,
    CursorVersion,
    CursorExpired,
    /// Issued for other filters or another limit.
    CursorMismatch,
    /// The reader refused or could not open a connection, or PostgreSQL
    /// failed.
    Unavailable,
    DeadlineExceeded,
    /// The page would not fit its byte budget; nothing is truncated.
    BudgetExceeded,
    /// A stored row is not a current, valid event.
    InvalidStoredEvent,
}

impl PageError {
    /// Every code, for closed-vocabulary checks.
    pub const ALL: [Self; 13] = [
        Self::InvalidLimit,
        Self::InvalidActor,
        Self::InvalidAction,
        Self::InvalidInstant,
        Self::InvalidRange,
        Self::CursorInvalid,
        Self::CursorVersion,
        Self::CursorExpired,
        Self::CursorMismatch,
        Self::Unavailable,
        Self::DeadlineExceeded,
        Self::BudgetExceeded,
        Self::InvalidStoredEvent,
    ];

    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidLimit => "audit_page_invalid_limit",
            Self::InvalidActor => "audit_page_invalid_actor",
            Self::InvalidAction => "audit_page_invalid_action",
            Self::InvalidInstant => "audit_page_invalid_instant",
            Self::InvalidRange => "audit_page_invalid_range",
            Self::CursorInvalid => "audit_page_cursor_invalid",
            Self::CursorVersion => "audit_page_cursor_version",
            Self::CursorExpired => "audit_page_cursor_expired",
            Self::CursorMismatch => "audit_page_cursor_mismatch",
            Self::Unavailable => "audit_page_unavailable",
            Self::DeadlineExceeded => "audit_page_deadline_exceeded",
            Self::BudgetExceeded => "audit_page_budget_exceeded",
            Self::InvalidStoredEvent => "audit_page_invalid_stored_event",
        }
    }
}

impl fmt::Display for PageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for PageError {}

/// How many events a page holds: 1 to 200, 50 by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit(u16);

impl Limit {
    /// # Errors
    /// [`PageError::InvalidLimit`] outside 1..=200.
    pub fn new(limit: u32) -> Result<Self, PageError> {
        u16::try_from(limit)
            .ok()
            .filter(|limit| (1..=MAX_LIMIT).contains(limit))
            .map(Self)
            .ok_or(PageError::InvalidLimit)
    }

    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl Default for Limit {
    fn default() -> Self {
        Self(DEFAULT_LIMIT)
    }
}

/// An actor to filter by, shaped like every stored one: 1 to 128 bytes of
/// `[A-Za-z0-9_.:-]`. Never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct Actor(String);

impl FromStr for Actor {
    type Err = PageError;

    fn from_str(actor: &str) -> Result<Self, PageError> {
        if !actor.is_empty()
            && actor.len() <= MAX_ACTOR_BYTES
            && actor
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
        {
            Ok(Self(actor.to_owned()))
        } else {
            Err(PageError::InvalidActor)
        }
    }
}

impl fmt::Debug for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Actor(<redacted>)")
    }
}

/// The action as written on the wire (`query`), from the closed set of the
/// current event contract.
///
/// # Errors
/// [`PageError::InvalidAction`] for anything else, case included.
pub fn action_from_wire(action: &str) -> Result<Action, PageError> {
    match action {
        "login" => Ok(Action::Login),
        "logout" => Ok(Action::Logout),
        "query" => Ok(Action::Query),
        "denial" => Ok(Action::Denial),
        _ => Err(PageError::InvalidAction),
    }
}

/// The stored spelling of `action`, exactly what the contract serializes.
const fn action_wire(action: Action) -> &'static str {
    match action {
        Action::Login => "login",
        Action::Logout => "logout",
        Action::Query => "query",
        Action::Denial => "denial",
    }
}

/// An RFC 3339 instant in UTC, written with `Z` (`2026-10-15T12:00:00Z`,
/// up to nanoseconds); an offset, a local time or anything else is refused.
///
/// # Errors
/// [`PageError::InvalidInstant`].
pub fn utc_from_wire(instant: &str) -> Result<DateTime<Utc>, PageError> {
    if instant.len() > 64 || !instant.ends_with('Z') {
        return Err(PageError::InvalidInstant);
    }
    DateTime::parse_from_rfc3339(instant)
        .map(|at| at.to_utc())
        .map_err(|_| PageError::InvalidInstant)
}

/// What a walk selects; every field optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub actor: Option<Actor>,
    pub action: Option<Action>,
    /// Inclusive.
    pub from: Option<DateTime<Utc>>,
    /// Exclusive.
    pub to: Option<DateTime<Utc>>,
}

/// Validated filters and limit: what a cursor is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageQuery {
    filters: Filters,
    limit: Limit,
}

impl PageQuery {
    /// # Errors
    /// [`PageError::InvalidInstant`] for an instant outside years 0 to 9999
    /// (no stored event can be there) or written as a leap second, and
    /// [`PageError::InvalidRange`] unless `from` is before `to`.
    pub fn new(filters: Filters, limit: Limit) -> Result<Self, PageError> {
        let representable =
            |at: &DateTime<Utc>| (0..=9999).contains(&at.year()) && at.nanosecond() < 1_000_000_000;
        if !filters
            .from
            .iter()
            .chain(filters.to.iter())
            .all(representable)
        {
            return Err(PageError::InvalidInstant);
        }
        if let (Some(from), Some(to)) = (filters.from, filters.to)
            && from >= to
        {
            return Err(PageError::InvalidRange);
        }
        Ok(Self { filters, limit })
    }

    #[must_use]
    pub const fn limit(&self) -> Limit {
        self.limit
    }

    /// The normalized form a cursor's fingerprint covers: each field
    /// present or absent, fixed width or length-prefixed, so two different
    /// queries never encode alike.
    fn scope(&self) -> Vec<u8> {
        let mut scope = Vec::with_capacity(64 + MAX_ACTOR_BYTES);
        match &self.filters.actor {
            None => scope.push(0),
            Some(Actor(actor)) => {
                scope.push(1);
                scope.push(u8::try_from(actor.len()).expect("at most 128 bytes"));
                scope.extend_from_slice(actor.as_bytes());
            }
        }
        scope.push(match self.filters.action {
            None => 0,
            Some(Action::Login) => 1,
            Some(Action::Logout) => 2,
            Some(Action::Query) => 3,
            Some(Action::Denial) => 4,
        });
        for bound in [self.filters.from, self.filters.to] {
            match bound {
                None => scope.push(0),
                Some(at) => {
                    scope.push(1);
                    scope.extend_from_slice(&at.timestamp().to_be_bytes());
                    scope.extend_from_slice(&at.timestamp_subsec_nanos().to_be_bytes());
                }
            }
        }
        scope.extend_from_slice(&self.limit.0.to_be_bytes());
        scope
    }
}

/// The budgets of a page. Only [`Profile::V1`] or tighter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    deadline: Duration,
    page_bytes: usize,
}

impl Profile {
    pub const V1: Self = Self {
        deadline: Duration::from_millis(5000),
        page_bytes: 4_194_304,
    };

    /// A profile no looser than v1 in either budget; `None` otherwise.
    #[must_use]
    pub fn tighter(deadline: Duration, page_bytes: usize) -> Option<Self> {
        (!deadline.is_zero()
            && deadline <= Self::V1.deadline
            && page_bytes > 0
            && page_bytes <= Self::V1.page_bytes)
            .then_some(Self {
                deadline,
                page_bytes,
            })
    }

    #[must_use]
    pub const fn deadline(self) -> Duration {
        self.deadline
    }

    #[must_use]
    pub const fn page_bytes(self) -> usize {
        self.page_bytes
    }
}

/// One page: its events in ascending sequence, the token for the next one
/// unless this is the last, and the JSON that carries both,
/// `{"events":[...],"next_cursor":"..."|null}`, within the byte budget.
pub struct Page {
    events: Vec<Event>,
    next_cursor: Option<String>,
    json: String,
}

impl Page {
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    #[must_use]
    pub fn next_cursor(&self) -> Option<&str> {
        self.next_cursor.as_deref()
    }

    #[must_use]
    pub fn json(&self) -> &str {
        &self.json
    }
}

/// Neither the events nor the token.
impl fmt::Debug for Page {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Page")
            .field("events", &self.events.len())
            .field("has_next", &self.next_cursor.is_some())
            .field("bytes", &self.json.len())
            .finish()
    }
}

/// Optional OpenTelemetry instruments, labelled only by a closed result.
#[derive(Default, Clone)]
struct PageMetrics {
    instruments: Option<(Histogram<f64>, Counter<u64>)>,
}

/// Whether meters are on, never the meters.
impl fmt::Debug for PageMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.instruments.is_some() {
            "PageMetrics(on)"
        } else {
            "PageMetrics(off)"
        })
    }
}

/// The repository of audit pages.
#[derive(Clone, Debug)]
pub struct AuditPages {
    reader: AuditReader,
    key: CursorKey,
    profile: Profile,
    metrics: PageMetrics,
}

impl AuditPages {
    /// Pages read through `reader` alone, with tokens under `key`.
    #[must_use]
    pub fn new(reader: AuditReader, key: CursorKey) -> Self {
        Self {
            reader,
            key,
            profile: Profile::V1,
            metrics: PageMetrics::default(),
        }
    }

    #[must_use]
    pub fn with_profile(mut self, profile: Profile) -> Self {
        self.profile = profile;
        self
    }

    /// Records `bi.audit.page.duration` and `bi.audit.page.requests`, with
    /// a `result` of `ok` or an error code and nothing else.
    #[must_use]
    pub fn with_meter(mut self, meter: &Meter) -> Self {
        self.metrics = PageMetrics {
            instruments: Some((
                meter
                    .f64_histogram("bi.audit.page.duration")
                    .with_unit("s")
                    .with_boundaries(vec![
                        0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5,
                        5.0,
                    ])
                    .build(),
                meter.u64_counter("bi.audit.page.requests").build(),
            )),
        };
        self
    }

    /// The page of `query` after `cursor`, or the first one without it.
    ///
    /// # Errors
    /// See [`PageError`]; no error yields events or a token.
    pub async fn page(&self, query: &PageQuery, cursor: Option<&str>) -> Result<Page, PageError> {
        self.page_at(query, cursor, SystemTime::now()).await
    }

    /// [`Self::page`] at the wall-clock time `now`, which issues and checks
    /// token expiry; the deadline always runs on the monotonic clock.
    ///
    /// # Errors
    /// See [`PageError`].
    pub async fn page_at(
        &self,
        query: &PageQuery,
        cursor: Option<&str>,
        now: SystemTime,
    ) -> Result<Page, PageError> {
        let started = Instant::now();
        let outcome = self.serve(query, cursor, now, started).await;
        let elapsed = started.elapsed();
        let result = match &outcome {
            Ok(page) => {
                tracing::info!(
                    event_code = "audit_page_served",
                    first = cursor.is_none(),
                    events = page.events.len(),
                    has_next = page.next_cursor.is_some(),
                    bytes = page.json.len(),
                    elapsed_us = elapsed.as_micros(),
                );
                "ok"
            }
            Err(error) => {
                tracing::warn!(
                    event_code = error.code(),
                    first = cursor.is_none(),
                    elapsed_us = elapsed.as_micros(),
                    "no audit page"
                );
                error.code()
            }
        };
        if let Some((duration, requests)) = &self.metrics.instruments {
            let attributes = [KeyValue::new("result", result)];
            duration.record(elapsed.as_secs_f64(), &attributes);
            requests.add(1, &attributes);
        }
        outcome
    }

    async fn serve(
        &self,
        query: &PageQuery,
        cursor: Option<&str>,
        now: SystemTime,
        started: Instant,
    ) -> Result<Page, PageError> {
        let deadline = started + self.profile.deadline;
        let now_ms = unix_millis(now);
        let scope = query.scope();
        // A token is checked before any connection is requested.
        let start = match cursor {
            None => Start::First {
                expires_ms: now_ms.saturating_add(millis(CURSOR_LIFETIME)),
            },
            Some(token) => Start::After(self.key.open(token, &scope, now_ms)?),
        };
        let mut connection = match timeout_at(deadline, self.reader.acquire()).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(_)) if Instant::now() >= deadline => return Err(PageError::DeadlineExceeded),
            Ok(Err(_)) => return Err(PageError::Unavailable),
            Err(_) => return Err(PageError::DeadlineExceeded),
        };
        let read = timeout_at(deadline, fetch(&mut connection, query, start, deadline)).await;
        let (upper, rows) = match read {
            Ok(Ok(read)) => read,
            // Interrupted or failed: the connection may still be busy or
            // inside the transaction, so it is closed, never handed back.
            Ok(Err(error)) => {
                drop(connection.detach());
                return Err(error);
            }
            Err(_) => {
                drop(connection.detach());
                return Err(PageError::DeadlineExceeded);
            }
        };
        drop(connection);
        let (events, last) = assemble(rows, usize::from(query.limit.0))?;
        let expires_ms = match start {
            Start::First { expires_ms } => expires_ms,
            Start::After(position) => position.expires_ms,
        };
        let next_cursor = last.map(|last| {
            self.key.seal(
                Position {
                    last,
                    upper,
                    expires_ms,
                },
                &scope,
            )
        });
        let json = encode::page(&events, next_cursor.as_deref(), self.profile.page_bytes)?;
        // No page is ever served past its deadline, decoding included.
        if Instant::now() >= deadline {
            return Err(PageError::DeadlineExceeded);
        }
        Ok(Page {
            events,
            next_cursor,
            json,
        })
    }
}

/// Where a page starts: a new walk, or after a checked token.
#[derive(Clone, Copy)]
enum Start {
    First { expires_ms: u64 },
    After(Position),
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Milliseconds since the Unix epoch; zero before it.
fn unix_millis(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).map_or(0, millis)
}

/// The stored instant as fixed-width text with nine fractional digits, the
/// contract's own format (`YYYY-MM-DDTHH:MM:SS[.f{1,9}]Z`, enforced by
/// `audit_append`), so it compares in text order to the nanosecond:
/// PostgreSQL's timestamps would round to microseconds.
macro_rules! at {
    () => {
        "(left((payload::jsonb)->>'occurred_at', 19) || '.' || \
         rpad(coalesce(substring((payload::jsonb)->>'occurred_at' from '\\.([0-9]{1,9})Z$'), ''), 9, '0'))"
    };
}

/// One static statement per combination of indexed filters, so each can
/// use its index (`audit_actor_sequence`, `audit_action_sequence`, or the
/// primary key) in order and stop at the limit. Values are always bound:
/// `$1` last sequence, `$2` upper bound, `$3` rows to read, `$4`/`$5` the
/// instants as text, then the actor and/or the action.
macro_rules! page_sql {
    ($indexed:literal) => {
        concat!(
            "SELECT sequence, payload FROM bi_meta.audit_events WHERE ",
            $indexed,
            "sequence > $1 AND sequence <= $2 \
             AND ($4::text IS NULL OR ",
            at!(),
            " >= $4) AND ($5::text IS NULL OR ",
            at!(),
            " < $5) ORDER BY sequence LIMIT $3"
        )
    };
}

pub(crate) const SQL_ALL: &str = page_sql!("");
pub(crate) const SQL_ACTOR: &str = page_sql!("(payload::jsonb)->>'actor_id' = $6 AND ");
pub(crate) const SQL_ACTION: &str = page_sql!("(payload::jsonb)->>'action' = $6 AND ");
pub(crate) const SQL_ACTOR_ACTION: &str =
    page_sql!("(payload::jsonb)->>'actor_id' = $6 AND (payload::jsonb)->>'action' = $7 AND ");
const SQL_HEAD: &str = "SELECT sequence FROM bi_meta.audit_head WHERE singleton";

/// Every page statement by the indexed filters it serves, for plans and
/// reviews: static text, values always bound.
pub const STATEMENTS: [(&str, &str); 4] = [
    ("all", SQL_ALL),
    ("actor", SQL_ACTOR),
    ("action", SQL_ACTION),
    ("actor+action", SQL_ACTOR_ACTION),
];

/// The page's rows, `limit + 1` at most (one to know whether more remain),
/// read in one read-only transaction bounded by what is left of the page's
/// deadline, and the upper bound they stay under.
async fn fetch(
    connection: &mut PgConnection,
    query: &PageQuery,
    start: Start,
    deadline: Instant,
) -> Result<(u64, Vec<(u64, String)>), PageError> {
    let mut transaction = connection
        .begin_with("BEGIN READ ONLY")
        .await
        .map_err(store)?;
    let (last, upper) = match start {
        Start::After(position) => (position.last, position.upper),
        Start::First { .. } => {
            bound(&mut transaction, deadline).await?;
            // Committed head, plain read: no lock the appender would wait on.
            let head: i64 = sqlx::query_scalar(SQL_HEAD)
                .fetch_one(&mut *transaction)
                .await
                .map_err(store)?;
            (
                0,
                u64::try_from(head).map_err(|_| PageError::InvalidStoredEvent)?,
            )
        }
    };
    bound(&mut transaction, deadline).await?;
    let instant =
        |at: Option<DateTime<Utc>>| at.map(|at| at.format("%Y-%m-%dT%H:%M:%S%.9f").to_string());
    let filters = &query.filters;
    let sql = match (&filters.actor, filters.action) {
        (None, None) => SQL_ALL,
        (Some(_), None) => SQL_ACTOR,
        (None, Some(_)) => SQL_ACTION,
        (Some(_), Some(_)) => SQL_ACTOR_ACTION,
    };
    let mut statement = sqlx::query(sql)
        .bind(i64::try_from(last).map_err(|_| PageError::CursorInvalid)?)
        .bind(i64::try_from(upper).map_err(|_| PageError::CursorInvalid)?)
        .bind(i64::from(query.limit.0) + 1)
        .bind(instant(filters.from))
        .bind(instant(filters.to));
    if let Some(Actor(actor)) = &filters.actor {
        statement = statement.bind(actor.as_str());
    }
    if let Some(action) = filters.action {
        statement = statement.bind(action_wire(action));
    }
    let rows = statement
        .fetch_all(&mut *transaction)
        .await
        .map_err(store)?;
    transaction.commit().await.map_err(store)?;
    let rows = rows
        .iter()
        .map(|row| {
            let sequence: i64 = row.try_get("sequence").map_err(store)?;
            let payload: String = row.try_get("payload").map_err(store)?;
            let sequence = u64::try_from(sequence).map_err(|_| PageError::InvalidStoredEvent)?;
            Ok((sequence, payload))
        })
        .collect::<Result<_, PageError>>()?;
    Ok((upper, rows))
}

/// Lets PostgreSQL end the next statement within what is left, so an
/// abandoned page never outlives its deadline on the server.
async fn bound(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    deadline: Instant,
) -> Result<(), PageError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(PageError::DeadlineExceeded);
    }
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(left.as_millis().max(1).to_string())
        .execute(&mut **transaction)
        .await
        .map_err(store)?;
    Ok(())
}

/// PostgreSQL's `query_canceled`, raised by `statement_timeout`.
const QUERY_CANCELED: &str = "57014";

/// Storage errors may carry stored values or credentials: only their class
/// survives.
#[allow(clippy::needless_pass_by_value)] // Shaped for `map_err`.
fn store(error: sqlx::Error) -> PageError {
    match &error {
        sqlx::Error::Database(database) if database.code().as_deref() == Some(QUERY_CANCELED) => {
            PageError::DeadlineExceeded
        }
        _ => PageError::Unavailable,
    }
}

/// The events of a page from up to `limit + 1` rows in ascending sequence,
/// and the last sequence returned when more rows remain. A stored document
/// that is not a valid event of the current version fails the page without
/// being echoed.
pub(crate) fn assemble(
    mut rows: Vec<(u64, String)>,
    limit: usize,
) -> Result<(Vec<Event>, Option<u64>), PageError> {
    let more = rows.len() > limit;
    rows.truncate(limit);
    let last = rows.last().map(|(sequence, _)| *sequence).filter(|_| more);
    let events = rows
        .iter()
        .map(|(sequence, payload)| {
            serde_json::from_str::<Event>(payload)
                .ok()
                .filter(|event| event.validate().is_ok())
                .ok_or_else(|| {
                    tracing::warn!(
                        event_code = "audit_page_invalid_stored_event",
                        sequence,
                        "a stored audit row is not a valid current event"
                    );
                    PageError::InvalidStoredEvent
                })
        })
        .collect::<Result<_, _>>()?;
    Ok((events, last))
}

#[cfg(test)]
mod tests;
