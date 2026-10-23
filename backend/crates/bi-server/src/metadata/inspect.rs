//! Read-only observation of the metadata role and schema. Every object is
//! resolved to an OID by schema and name in `pg_catalog`, which any role can
//! read: unlike `to_regclass`, that never fails for a role without `USAGE`
//! on the schema, so a missing grant is diagnosed instead of raising. Every
//! query is fixed text: nothing depends on `search_path`, and no identifier
//! or credential is interpolated into SQL.

use sqlx::{Acquire, PgConnection, Row};

use super::{AppliedMigration, Effective, Facts, History, ReachableRole};

/// The known objects, resolved once per query by qualified name. Macros, not
/// constants, so each query below is one compile-time literal.
macro_rules! target {
    () => {
        "
    target AS (
        SELECT d.oid AS database_oid,
               d.datdba AS database_owner,
               meta.oid AS metadata_schema,
               pub.oid AS public_schema,
               (SELECT c.oid FROM pg_catalog.pg_class c
                WHERE c.relnamespace = meta.oid AND c.relname = 'audit_events') AS audit_events,
               (SELECT c.oid FROM pg_catalog.pg_class c
                WHERE c.relnamespace = meta.oid AND c.relname = 'audit_head') AS audit_head,
               (SELECT p.oid FROM pg_catalog.pg_proc p
                WHERE p.pronamespace = meta.oid AND p.proname = 'audit_append'
                  AND p.pronargs = 1
                  AND p.proargtypes[0] = 'pg_catalog.text'::pg_catalog.regtype) AS audit_append,
               (SELECT p.oid FROM pg_catalog.pg_proc p
                WHERE p.pronamespace = meta.oid AND p.proname = 'audit_reject_mutation'
                  AND p.pronargs = 0) AS audit_reject_mutation,
               (SELECT c.oid FROM pg_catalog.pg_class c
                WHERE c.relnamespace = meta.oid AND c.relname = 'sessions') AS sessions,
               (SELECT c.oid FROM pg_catalog.pg_class c
                WHERE c.relnamespace = meta.oid AND c.relname = 'oidc_attempts') AS oidc_attempts,
               (SELECT c.oid FROM pg_catalog.pg_class c
                WHERE c.relnamespace = pub.oid AND c.relname = '_sqlx_migrations') AS migrations
        FROM pg_catalog.pg_database d
        LEFT JOIN pg_catalog.pg_namespace meta ON meta.nspname = 'bi_meta'
        LEFT JOIN pg_catalog.pg_namespace pub ON pub.nspname = 'public'
        WHERE d.datname = pg_catalog.current_database()
    )"
    };
}

/// One row per role reachable from `session_user` or `current_user`: a
/// login that can `SET ROLE` back to a privileged role is as privileged as
/// that role, and `MEMBER` covers direct and indirect membership regardless
/// of INHERIT or SET options. A reserved role (literal `pg_` prefix, #209)
/// among them is refused by name: its powers show in no grant.
const REACHABLE: &str = concat!(
    "WITH ",
    target!(),
    ",
    principal AS (
        SELECT r.oid FROM pg_catalog.pg_roles r
        WHERE r.rolname = session_user OR r.rolname = current_user
    )
    SELECT r.rolsuper AS superuser,
           r.rolcreaterole AS create_role,
           r.rolcreatedb AS create_database,
           r.rolreplication AS replication,
           r.rolbypassrls AS bypass_rls,
           r.oid = t.database_owner AS owns_database,
           COALESCE(r.oid = (SELECT n.nspowner FROM pg_catalog.pg_namespace n
                             WHERE n.oid = t.metadata_schema), false) AS owns_metadata_schema,
           COALESCE(r.oid IN (SELECT c.relowner FROM pg_catalog.pg_class c
                              WHERE c.oid = t.audit_events OR c.oid = t.audit_head), false)
               AS owns_audit_storage,
           COALESCE(r.oid = (SELECT p.proowner FROM pg_catalog.pg_proc p
                             WHERE p.oid = t.audit_append), false) AS owns_audit_append,
           COALESCE(r.oid = (SELECT c.relowner FROM pg_catalog.pg_class c
                             WHERE c.oid = t.migrations), false) AS owns_migrations,
           COALESCE(pg_catalog.has_schema_privilege(r.oid, t.metadata_schema, 'CREATE'), false)
               AS creates_in_metadata_schema,
           COALESCE(pg_catalog.has_table_privilege(r.oid, t.audit_events,
                        'SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER, MAINTAIN'), false)
           OR COALESCE(pg_catalog.has_table_privilege(r.oid, t.audit_head,
                        'SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER, MAINTAIN'), false)
           OR COALESCE(pg_catalog.has_any_column_privilege(r.oid, t.audit_events,
                        'SELECT, INSERT, UPDATE, REFERENCES'), false)
           OR COALESCE(pg_catalog.has_any_column_privilege(r.oid, t.audit_head,
                        'SELECT, INSERT, UPDATE, REFERENCES'), false) AS accesses_audit_storage,
           COALESCE(pg_catalog.has_table_privilege(r.oid, t.migrations,
                        'INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER, MAINTAIN'), false)
           OR COALESCE(pg_catalog.has_any_column_privilege(r.oid, t.migrations,
                        'INSERT, UPDATE, REFERENCES'), false) AS writes_migrations,
           COALESCE(pg_catalog.has_function_privilege(r.oid, t.audit_append,
                        'EXECUTE WITH GRANT OPTION'), false) AS grants_audit_append,
           pg_catalog.starts_with(r.rolname, 'pg_') AS reserved_role
    FROM pg_catalog.pg_roles r CROSS JOIN target t
    WHERE EXISTS (SELECT 1 FROM principal p WHERE pg_catalog.pg_has_role(p.oid, r.oid, 'MEMBER'))"
);

/// What the current user can do right now, without `SET ROLE`; required
/// privileges are checked one by one (a comma list would mean "any").
const EFFECTIVE: &str = concat!(
    "WITH ",
    target!(),
    "
    SELECT pg_catalog.has_database_privilege(t.database_oid, 'CONNECT') AS connect,
           COALESCE(pg_catalog.has_schema_privilege(t.metadata_schema, 'USAGE'), false)
               AS metadata_schema_usage,
           COALESCE(pg_catalog.has_schema_privilege(t.public_schema, 'USAGE')
                    AND pg_catalog.has_table_privilege(t.migrations, 'SELECT'), false)
               AS read_migrations,
           COALESCE(pg_catalog.has_function_privilege(t.audit_append, 'EXECUTE'), false)
               AS execute_audit_append,
           COALESCE(pg_catalog.has_table_privilege(t.sessions, 'SELECT')
                    AND pg_catalog.has_table_privilege(t.sessions, 'INSERT')
                    AND pg_catalog.has_table_privilege(t.sessions, 'UPDATE'), false) AS sessions,
           COALESCE(pg_catalog.has_table_privilege(t.oidc_attempts, 'SELECT')
                    AND pg_catalog.has_table_privilege(t.oidc_attempts, 'INSERT')
                    AND pg_catalog.has_table_privilege(t.oidc_attempts, 'DELETE'), false)
               AS oidc_attempts,
           t.metadata_schema IS NOT NULL AND t.audit_events IS NOT NULL
               AND t.audit_head IS NOT NULL AND t.audit_append IS NOT NULL
               AND t.sessions IS NOT NULL AND t.oidc_attempts IS NOT NULL
               AND t.migrations IS NOT NULL AS objects_present
    FROM target t"
);

/// One row per role reachable from the audit reader's login (#159): any
/// row but the login's own is refused, predefined roles included. Each
/// row says everything it must not do: any privilege on the audit storage but
/// `SELECT`, column grants and grants to `PUBLIC` included, `SELECT` with
/// its grant option, `EXECUTE` on `audit_append`, any privilege on the
/// sessions, writing the migrations, owning any of them, or `CREATE` on the
/// database or on any schema but a session's temporary ones: persistent DDL
/// anywhere, whatever the session's read-only default says.
const READER_REACHABLE: &str = concat!(
    "WITH ",
    target!(),
    ",
    principal AS (
        SELECT r.oid FROM pg_catalog.pg_roles r
        WHERE r.rolname = session_user OR r.rolname = current_user
    )
    SELECT r.rolname <> session_user AS other_role,
           r.rolsuper AS superuser,
           r.rolcreaterole AS create_role,
           r.rolcreatedb AS create_database,
           r.rolreplication AS replication,
           r.rolbypassrls AS bypass_rls,
           r.oid = t.database_owner AS owns_database,
           COALESCE(r.oid IN (SELECT n.nspowner FROM pg_catalog.pg_namespace n
                              WHERE n.oid IN (t.metadata_schema, t.public_schema)), false)
               AS owns_schema,
           COALESCE(r.oid IN (SELECT c.relowner FROM pg_catalog.pg_class c
                              WHERE c.oid IN (t.audit_events, t.audit_head)), false)
               AS owns_audit_storage,
           COALESCE(r.oid IN (SELECT p.proowner FROM pg_catalog.pg_proc p
                              WHERE p.oid IN (t.audit_append, t.audit_reject_mutation)), false)
               AS owns_audit_functions,
           COALESCE(r.oid IN (SELECT c.relowner FROM pg_catalog.pg_class c
                              WHERE c.oid IN (t.sessions, t.oidc_attempts)), false)
               AS owns_sessions,
           COALESCE(r.oid = (SELECT c.relowner FROM pg_catalog.pg_class c
                             WHERE c.oid = t.migrations), false) AS owns_migrations,
           pg_catalog.has_database_privilege(r.oid, t.database_oid, 'CREATE')
               AS creates_in_database,
           EXISTS (SELECT 1 FROM pg_catalog.pg_namespace n
                   WHERE n.nspname !~ '^pg_(toast_)?temp_'
                     AND pg_catalog.has_schema_privilege(r.oid, n.oid, 'CREATE'))
               AS creates_in_schema,
           COALESCE(pg_catalog.has_table_privilege(r.oid, t.audit_events,
                        'INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER, MAINTAIN'), false)
           OR COALESCE(pg_catalog.has_table_privilege(r.oid, t.audit_head,
                        'INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER, MAINTAIN'), false)
           OR COALESCE(pg_catalog.has_any_column_privilege(r.oid, t.audit_events,
                        'INSERT, UPDATE, REFERENCES'), false)
           OR COALESCE(pg_catalog.has_any_column_privilege(r.oid, t.audit_head,
                        'INSERT, UPDATE, REFERENCES'), false) AS writes_audit_storage,
           COALESCE(pg_catalog.has_table_privilege(r.oid, t.audit_events,
                        'SELECT WITH GRANT OPTION'), false)
           OR COALESCE(pg_catalog.has_table_privilege(r.oid, t.audit_head,
                        'SELECT WITH GRANT OPTION'), false)
           OR COALESCE(pg_catalog.has_any_column_privilege(r.oid, t.audit_events,
                        'SELECT WITH GRANT OPTION'), false)
           OR COALESCE(pg_catalog.has_any_column_privilege(r.oid, t.audit_head,
                        'SELECT WITH GRANT OPTION'), false) AS grants_audit_storage,
           COALESCE(pg_catalog.has_function_privilege(r.oid, t.audit_append, 'EXECUTE'), false)
               AS executes_audit_append,
           COALESCE(pg_catalog.has_table_privilege(r.oid, t.sessions,
                        'SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER, MAINTAIN'), false)
           OR COALESCE(pg_catalog.has_table_privilege(r.oid, t.oidc_attempts,
                        'SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER, MAINTAIN'), false)
           OR COALESCE(pg_catalog.has_any_column_privilege(r.oid, t.sessions,
                        'SELECT, INSERT, UPDATE, REFERENCES'), false)
           OR COALESCE(pg_catalog.has_any_column_privilege(r.oid, t.oidc_attempts,
                        'SELECT, INSERT, UPDATE, REFERENCES'), false) AS accesses_sessions,
           COALESCE(pg_catalog.has_table_privilege(r.oid, t.migrations,
                        'INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER, MAINTAIN'), false)
           OR COALESCE(pg_catalog.has_any_column_privilege(r.oid, t.migrations,
                        'INSERT, UPDATE, REFERENCES'), false) AS writes_migrations
    FROM pg_catalog.pg_roles r CROSS JOIN target t
    WHERE EXISTS (SELECT 1 FROM principal p WHERE pg_catalog.pg_has_role(p.oid, r.oid, 'MEMBER'))"
);

/// What the reader's current user can do right now, and whether its session
/// is read-only by default.
const READER_EFFECTIVE: &str = concat!(
    "WITH ",
    target!(),
    "
    SELECT pg_catalog.has_database_privilege(t.database_oid, 'CONNECT') AS connect,
           COALESCE(pg_catalog.has_schema_privilege(t.metadata_schema, 'USAGE'), false)
               AS metadata_schema_usage,
           COALESCE(pg_catalog.has_table_privilege(t.audit_events, 'SELECT'), false)
               AS reads_events,
           COALESCE(pg_catalog.has_table_privilege(t.audit_head, 'SELECT'), false)
               AS reads_head,
           pg_catalog.current_setting('default_transaction_read_only') = 'on' AS read_only,
           t.metadata_schema IS NOT NULL AND t.audit_events IS NOT NULL
               AND t.audit_head IS NOT NULL AND t.audit_append IS NOT NULL
               AND t.sessions IS NOT NULL AND t.oidc_attempts IS NOT NULL
               AND t.migrations IS NOT NULL AND t.audit_reject_mutation IS NOT NULL
               AS objects_present
    FROM target t"
);

/// Observes the audit reader's role behind `connection` (#159), inside a
/// read-only transaction.
///
/// # Errors
/// Returns the database error when PostgreSQL cannot be queried.
pub async fn reader_facts(
    connection: &mut PgConnection,
) -> Result<crate::audit::reader::ReaderFacts, sqlx::Error> {
    use crate::audit::reader::{ReaderEffective, ReaderFacts, ReaderRole};
    let mut transaction = connection.begin().await?;
    sqlx::query("SET TRANSACTION READ ONLY")
        .execute(&mut *transaction)
        .await?;
    let reachable = sqlx::query(READER_REACHABLE)
        .fetch_all(&mut *transaction)
        .await?
        .iter()
        .map(|row| {
            Ok(ReaderRole {
                other_role: row.try_get("other_role")?,
                superuser: row.try_get("superuser")?,
                create_role: row.try_get("create_role")?,
                create_database: row.try_get("create_database")?,
                replication: row.try_get("replication")?,
                bypass_rls: row.try_get("bypass_rls")?,
                owns_database: row.try_get("owns_database")?,
                owns_schema: row.try_get("owns_schema")?,
                owns_audit_storage: row.try_get("owns_audit_storage")?,
                owns_audit_functions: row.try_get("owns_audit_functions")?,
                owns_sessions: row.try_get("owns_sessions")?,
                owns_migrations: row.try_get("owns_migrations")?,
                creates_in_database: row.try_get("creates_in_database")?,
                creates_in_schema: row.try_get("creates_in_schema")?,
                writes_audit_storage: row.try_get("writes_audit_storage")?,
                grants_audit_storage: row.try_get("grants_audit_storage")?,
                executes_audit_append: row.try_get("executes_audit_append")?,
                accesses_sessions: row.try_get("accesses_sessions")?,
                writes_migrations: row.try_get("writes_migrations")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let row = sqlx::query(READER_EFFECTIVE)
        .fetch_one(&mut *transaction)
        .await?;
    let effective = ReaderEffective {
        connect: row.try_get("connect")?,
        metadata_schema_usage: row.try_get("metadata_schema_usage")?,
        reads_events: row.try_get("reads_events")?,
        reads_head: row.try_get("reads_head")?,
        read_only: row.try_get("read_only")?,
        objects_present: row.try_get("objects_present")?,
    };
    transaction.commit().await?;
    Ok(ReaderFacts {
        reachable,
        effective,
    })
}

/// Observes the role and schema behind `connection`, inside a read-only
/// transaction: no DDL, no writes, no repair.
///
/// # Errors
/// Returns the database error when PostgreSQL cannot be queried.
pub async fn facts(connection: &mut PgConnection) -> Result<Facts, sqlx::Error> {
    let mut transaction = connection.begin().await?;
    sqlx::query("SET TRANSACTION READ ONLY")
        .execute(&mut *transaction)
        .await?;
    let reachable = sqlx::query(REACHABLE)
        .fetch_all(&mut *transaction)
        .await?
        .iter()
        .map(|row| {
            Ok(ReachableRole {
                superuser: row.try_get("superuser")?,
                create_role: row.try_get("create_role")?,
                create_database: row.try_get("create_database")?,
                replication: row.try_get("replication")?,
                bypass_rls: row.try_get("bypass_rls")?,
                owns_database: row.try_get("owns_database")?,
                owns_metadata_schema: row.try_get("owns_metadata_schema")?,
                owns_audit_storage: row.try_get("owns_audit_storage")?,
                owns_audit_append: row.try_get("owns_audit_append")?,
                owns_migrations: row.try_get("owns_migrations")?,
                creates_in_metadata_schema: row.try_get("creates_in_metadata_schema")?,
                accesses_audit_storage: row.try_get("accesses_audit_storage")?,
                writes_migrations: row.try_get("writes_migrations")?,
                grants_audit_append: row.try_get("grants_audit_append")?,
                reserved_role: row.try_get("reserved_role")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let row = sqlx::query(EFFECTIVE).fetch_one(&mut *transaction).await?;
    let effective = Effective {
        connect: row.try_get("connect")?,
        metadata_schema_usage: row.try_get("metadata_schema_usage")?,
        read_migrations: row.try_get("read_migrations")?,
        execute_audit_append: row.try_get("execute_audit_append")?,
        sessions: row.try_get("sessions")?,
        oidc_attempts: row.try_get("oidc_attempts")?,
    };
    let history = if !row.try_get::<bool, _>("objects_present")? {
        History::Absent
    } else if !effective.read_migrations {
        History::Unreadable
    } else {
        History::Applied(
            sqlx::query(
                "SELECT version, success, checksum FROM public._sqlx_migrations ORDER BY version",
            )
            .fetch_all(&mut *transaction)
            .await?
            .iter()
            .map(|row| {
                Ok(AppliedMigration {
                    version: row.try_get("version")?,
                    success: row.try_get("success")?,
                    checksum: row.try_get("checksum")?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()?,
        )
    };
    transaction.commit().await?;
    Ok(Facts {
        reachable,
        effective,
        history,
    })
}
