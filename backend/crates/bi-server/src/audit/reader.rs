//! A dedicated, read-only connection to the audit trail (#159): its own
//! role and pool, apart from the runtime's (#115), which still never reads
//! the chain. Nothing here reads events for anyone yet (#160).
//!
//! Every physical connection is checked before the pool admits it, not only
//! the first, and again each time it is handed out: the session is made
//! read-only and bounded, then the role is inspected, so a privilege
//! granted later is refused from the next hand-out on (one granted while a
//! connection is out takes effect when it is handed out again).
//!
//! The role must be a member of no other role, predefined `pg_*` roles
//! included: those grant powers (running server programs, reading server
//! files, signalling backends) that no read-only session prevents. A role that may do anything but
//! read the chain (own it, write it, grant it, append to it, touch sessions
//! or migrations, or create in `bi_meta`), or that cannot read it, is
//! refused, and reading stays disabled. There is no fallback to another
//! role, and the credential never appears in a log.

use std::fmt;
use std::time::Duration;

use sqlx::ConnectOptions as _;
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgConnection, PgPool, Postgres};

use crate::metadata::inspect;

/// Connections of the reader pool: few, it serves audit reads only.
const CONNECTIONS: u32 = 2;
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// Set on every physical connection before anything else runs on it.
const SESSION: &str = "SET default_transaction_read_only = on; \
                       SET statement_timeout = '5s'; \
                       SET idle_in_transaction_session_timeout = '5s'; \
                       SET lock_timeout = '1s'";

/// One role reachable from the reader's login, with what it may do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // One observed PostgreSQL fact per field.
pub struct ReaderRole {
    /// Not the login itself: the reader may be a member of no other role.
    pub other_role: bool,
    pub superuser: bool,
    pub create_role: bool,
    pub create_database: bool,
    pub replication: bool,
    pub bypass_rls: bool,
    pub owns_database: bool,
    /// Owns `bi_meta` or `public`, whose owner may drop what is in it.
    pub owns_schema: bool,
    /// Owns `audit_events` or `audit_head`.
    pub owns_audit_storage: bool,
    /// Owns `audit_append` or `audit_reject_mutation`, the trigger that
    /// keeps the chain append-only.
    pub owns_audit_functions: bool,
    /// Owns `sessions` or `oidc_attempts`.
    pub owns_sessions: bool,
    pub owns_migrations: bool,
    /// `CREATE` on the database: new schemas.
    pub creates_in_database: bool,
    /// `CREATE` on any persistent schema, `bi_meta` and `public` included.
    pub creates_in_schema: bool,
    /// Any privilege on the audit storage but `SELECT`, column grants
    /// included.
    pub writes_audit_storage: bool,
    /// `SELECT` on the audit storage with its grant option.
    pub grants_audit_storage: bool,
    pub executes_audit_append: bool,
    /// Any privilege on `sessions` or `oidc_attempts`.
    pub accesses_sessions: bool,
    pub writes_migrations: bool,
}

/// What the reader's current user can do right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // One required capability per field.
pub struct ReaderEffective {
    pub connect: bool,
    pub metadata_schema_usage: bool,
    pub reads_events: bool,
    pub reads_head: bool,
    /// The session's transactions are read-only by default.
    pub read_only: bool,
    pub objects_present: bool,
}

/// Everything [`inspect::reader_facts`] observes; nothing here is a verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderFacts {
    pub reachable: Vec<ReaderRole>,
    pub effective: ReaderEffective,
}

/// Why a connection may not read the audit trail. `Display` is the stable
/// code used in logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReaderProblem {
    OtherRole,
    Superuser,
    CreateRole,
    CreateDatabase,
    Replication,
    BypassRls,
    OwnsDatabase,
    OwnsSchema,
    OwnsAuditStorage,
    OwnsAuditFunctions,
    OwnsSessions,
    OwnsMigrations,
    CreatesInDatabase,
    CreatesInSchema,
    WritesAuditStorage,
    GrantsAuditStorage,
    ExecutesAuditAppend,
    AccessesSessions,
    WritesMigrations,
    MissingConnect,
    MissingSchemaUsage,
    MissingEventsSelect,
    MissingHeadSelect,
    NotReadOnly,
    ObjectsAbsent,
}

impl ReaderProblem {
    /// The capabilities no reachable role may hold.
    pub const FORBIDDEN: [Self; 19] = [
        Self::OtherRole,
        Self::Superuser,
        Self::CreateRole,
        Self::CreateDatabase,
        Self::Replication,
        Self::BypassRls,
        Self::OwnsDatabase,
        Self::OwnsSchema,
        Self::OwnsAuditStorage,
        Self::OwnsAuditFunctions,
        Self::OwnsSessions,
        Self::OwnsMigrations,
        Self::CreatesInDatabase,
        Self::CreatesInSchema,
        Self::WritesAuditStorage,
        Self::GrantsAuditStorage,
        Self::ExecutesAuditAppend,
        Self::AccessesSessions,
        Self::WritesMigrations,
    ];

    /// Whether `role` holds this forbidden capability.
    #[must_use]
    pub const fn held_by(self, role: &ReaderRole) -> bool {
        match self {
            Self::OtherRole => role.other_role,
            Self::Superuser => role.superuser,
            Self::CreateRole => role.create_role,
            Self::CreateDatabase => role.create_database,
            Self::Replication => role.replication,
            Self::BypassRls => role.bypass_rls,
            Self::OwnsDatabase => role.owns_database,
            Self::OwnsSchema => role.owns_schema,
            Self::OwnsAuditStorage => role.owns_audit_storage,
            Self::OwnsAuditFunctions => role.owns_audit_functions,
            Self::OwnsSessions => role.owns_sessions,
            Self::OwnsMigrations => role.owns_migrations,
            Self::CreatesInDatabase => role.creates_in_database,
            Self::CreatesInSchema => role.creates_in_schema,
            Self::WritesAuditStorage => role.writes_audit_storage,
            Self::GrantsAuditStorage => role.grants_audit_storage,
            Self::ExecutesAuditAppend => role.executes_audit_append,
            Self::AccessesSessions => role.accesses_sessions,
            Self::WritesMigrations => role.writes_migrations,
            Self::MissingConnect
            | Self::MissingSchemaUsage
            | Self::MissingEventsSelect
            | Self::MissingHeadSelect
            | Self::NotReadOnly
            | Self::ObjectsAbsent => false,
        }
    }
}

impl fmt::Display for ReaderProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OtherRole => "other_role",
            Self::Superuser => "superuser",
            Self::CreateRole => "create_role",
            Self::CreateDatabase => "create_database",
            Self::Replication => "replication",
            Self::BypassRls => "bypass_rls",
            Self::OwnsDatabase => "owns_database",
            Self::OwnsSchema => "owns_schema",
            Self::OwnsAuditStorage => "owns_audit_storage",
            Self::OwnsAuditFunctions => "owns_audit_functions",
            Self::OwnsSessions => "owns_sessions",
            Self::OwnsMigrations => "owns_migrations",
            Self::CreatesInDatabase => "creates_in_database",
            Self::CreatesInSchema => "creates_in_schema",
            Self::WritesAuditStorage => "writes_audit_storage",
            Self::GrantsAuditStorage => "grants_audit_storage",
            Self::ExecutesAuditAppend => "executes_audit_append",
            Self::AccessesSessions => "accesses_sessions",
            Self::WritesMigrations => "writes_migrations",
            Self::MissingConnect => "missing_connect",
            Self::MissingSchemaUsage => "missing_schema_usage",
            Self::MissingEventsSelect => "missing_events_select",
            Self::MissingHeadSelect => "missing_head_select",
            Self::NotReadOnly => "not_read_only",
            Self::ObjectsAbsent => "objects_absent",
        })
    }
}

/// Every problem `facts` show, in a stable order; empty means the
/// connection may read the audit trail and nothing else.
#[must_use]
pub fn assess(facts: &ReaderFacts) -> Vec<ReaderProblem> {
    let mut problems: Vec<ReaderProblem> = ReaderProblem::FORBIDDEN
        .into_iter()
        .filter(|problem| facts.reachable.iter().any(|role| problem.held_by(role)))
        .collect();
    let e = &facts.effective;
    for (held, problem) in [
        (e.objects_present, ReaderProblem::ObjectsAbsent),
        (e.connect, ReaderProblem::MissingConnect),
        (e.metadata_schema_usage, ReaderProblem::MissingSchemaUsage),
        (e.reads_events, ReaderProblem::MissingEventsSelect),
        (e.reads_head, ReaderProblem::MissingHeadSelect),
        (e.read_only, ReaderProblem::NotReadOnly),
    ] {
        if !held {
            problems.push(problem);
        }
    }
    // No reachable role at all means nothing was observed: never accepted.
    if facts.reachable.is_empty() {
        problems.push(ReaderProblem::ObjectsAbsent);
    }
    problems.sort();
    problems.dedup();
    problems
}

/// The reader's connection string, never printed.
#[derive(Clone)]
pub struct ReaderUrl(String);

impl ReaderUrl {
    #[must_use]
    pub const fn new(url: String) -> Self {
        Self(url)
    }

    /// The connection options, or `None` for a malformed string; the
    /// string itself is never part of an error.
    #[must_use]
    pub fn options(&self) -> Option<PgConnectOptions> {
        self.0.parse().ok()
    }
}

impl fmt::Debug for ReaderUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReaderUrl(<redacted>)")
    }
}

/// Why no reader connection is available. Nothing of the credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderError {
    /// No physical connection passed its check, or none could be opened
    /// in time.
    Unavailable,
}

/// The pool of read-only connections to the audit trail.
#[derive(Clone, Debug)]
pub struct AuditReader {
    pool: PgPool,
}

impl AuditReader {
    /// A lazy pool: nothing connects until the first [`Self::acquire`].
    #[must_use]
    pub fn start(options: &PgConnectOptions) -> Self {
        let pool = PgPoolOptions::new()
            .max_connections(CONNECTIONS)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .after_connect(|connection, _| Box::pin(admit(connection)))
            // Checked again on every hand-out: a connection opened before a
            // grant is discarded, never reused.
            .before_acquire(|connection, _| Box::pin(async move { Ok(reset(connection).await) }))
            .connect_lazy_with(options.clone().disable_statement_logging());
        Self { pool }
    }

    /// A connection that passed its check when it was opened.
    ///
    /// # Errors
    /// [`ReaderError::Unavailable`] when none passes or none can be opened.
    pub async fn acquire(&self) -> Result<PoolConnection<Postgres>, ReaderError> {
        self.pool
            .acquire()
            .await
            .map_err(|_| ReaderError::Unavailable)
    }

    /// Open connections waiting to be handed out.
    #[must_use]
    pub fn idle(&self) -> usize {
        self.pool.num_idle()
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// Bounds a new session, then refuses it unless its role may only read.
async fn admit(connection: &mut PgConnection) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(SESSION).execute(&mut *connection).await?;
    if check(connection).await {
        Ok(())
    } else {
        Err(sqlx::Error::Protocol(
            "the audit reader's role may do more than read".to_owned(),
        ))
    }
}

/// Before a hand-out: the session's bounds again, whatever the last user
/// changed, and the role checked again. Anything else discards it.
async fn reset(connection: &mut PgConnection) -> bool {
    sqlx::raw_sql(SESSION)
        .execute(&mut *connection)
        .await
        .is_ok()
        && check(connection).await
}

/// Whether the role behind `connection` may still only read the chain; a
/// failure to inspect it counts as no.
async fn check(connection: &mut PgConnection) -> bool {
    let Ok(facts) = inspect::reader_facts(connection).await else {
        tracing::warn!(
            event_code = "audit_reader_unchecked",
            "a connection of the audit reader could not be checked"
        );
        return false;
    };
    let problems = assess(&facts);
    if problems.is_empty() {
        return true;
    }
    let codes: Vec<String> = problems.iter().map(ToString::to_string).collect();
    tracing::warn!(
        event_code = "audit_reader_refused",
        problems = %codes.join(","),
        "a connection of the audit reader was refused"
    );
    false
}

#[cfg(test)]
mod tests;
