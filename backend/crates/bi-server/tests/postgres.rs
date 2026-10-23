//! Readiness and migrations against a real PostgreSQL. Run with
//! `cargo test --features postgres-tests` and `DATABASE_URL` pointing to a
//! server where the user may create databases (each test gets a fresh one).
#![cfg(feature = "postgres-tests")]

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use bi_server::db::{Database, MIGRATOR};
use bi_server::health::{Health, HealthCheck, HealthStatus};
use bi_server::metadata::MetadataMode;
use bi_server::{AppState, router};
use common::get_health;
use sqlx::PgPool;

const MIGRATION_TIMEOUT: Duration = Duration::from_secs(30);

async fn migrated_database(pool: &PgPool) -> Database {
    let db = Database::start(&pool.connect_options(), MetadataMode::Development);
    let migrated = tokio::time::timeout(MIGRATION_TIMEOUT, db.wait_until_ready())
        .await
        .expect("migrations finish in time");
    assert!(migrated);
    db
}

fn health(status: HealthStatus, postgres: HealthStatus, migrations: HealthStatus) -> Health {
    let check = |name: &str, status| HealthCheck {
        name: name.to_owned(),
        status,
    };
    Health {
        status,
        checks: vec![check("postgres", postgres), check("migrations", migrations)],
    }
}

#[sqlx::test(migrations = false)]
async fn ready_after_migrations_are_applied(pool: PgPool) {
    let db = migrated_database(&pool).await;
    let (status, body) = get_health(&router(AppState::new(db.clone())), "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        health(HealthStatus::Ok, HealthStatus::Ok, HealthStatus::Ok)
    );

    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(applied, i64::try_from(MIGRATOR.iter().count()).unwrap());

    let schema_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.schemata WHERE schema_name = 'bi_meta')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(schema_exists);
    db.close().await;
}

#[sqlx::test(migrations = false)]
async fn migrations_are_idempotent_across_restarts(pool: PgPool) {
    migrated_database(&pool).await.close().await;
    // A restarted API finds every migration applied and becomes ready.
    let db = migrated_database(&pool).await;
    let (status, _) = get_health(&router(AppState::new(db.clone())), "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    db.close().await;
}

#[sqlx::test(migrations = false)]
async fn ready_turns_unavailable_when_database_is_lost(pool: PgPool) {
    let db = migrated_database(&pool).await;
    let app = router(AppState::new(db.clone()));
    db.close().await;

    let (status, body) = get_health(&app, "/health/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    // Nothing about the schema is trusted after a loss until revalidated.
    assert_eq!(
        body,
        health(
            HealthStatus::Unavailable,
            HealthStatus::Unavailable,
            HealthStatus::Unavailable
        )
    );
    let (live, _) = get_health(&app, "/health/live").await;
    assert_eq!(live, StatusCode::OK);
}

/// The restricted mode (#115) against real PostgreSQL 18 roles: an owner
/// that migrates, a least-privilege runtime role and a helper, named after
/// this test's own database so parallel tests never collide, and dropped at
/// the end.
#[cfg(feature = "dev-auth")]
mod restricted {
    use std::collections::BTreeSet;
    use std::net::SocketAddr;
    use std::time::{Duration, Instant};

    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use bi_server::auth::AuthConfig;
    use bi_server::db::{self, Database, MetadataMetrics, MetadataStatus};
    use bi_server::metadata::{Assessment, Forbidden, MetadataMode, SchemaProblem};
    use bi_server::{AppState, audit, router};
    use http_body_util::BodyExt;
    use sqlx::postgres::PgConnectOptions;
    use sqlx::{AssertSqlSafe, ConnectOptions as _, PgPool};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::{JoinHandle, JoinSet};
    use tower::ServiceExt;

    const GRANTS: &str = include_str!("../admin/runtime-grants.sql");
    /// A decoy, never a real secret: no log line may ever contain it.
    const PASSWORD: &str = "decoy-password-7f3a";
    const ORIGIN: &str = "http://127.0.0.1:8080";
    const READY: Duration = Duration::from_secs(30);

    struct Roles {
        superuser: PgPool,
        database: String,
        admin: String,
        runtime: String,
        helper: String,
    }

    impl Roles {
        async fn create(pool: &PgPool) -> Self {
            let database: String = sqlx::query_scalar("SELECT current_database()")
                .fetch_one(pool)
                .await
                .unwrap();
            assert!(
                database
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            );
            // Role names must stay unique within PostgreSQL's 63-byte limit,
            // which the test database name alone almost reaches.
            let tag: String = database
                .chars()
                .rev()
                .filter(char::is_ascii_alphanumeric)
                .take(16)
                .collect::<String>()
                .to_ascii_lowercase();
            let roles = Self {
                superuser: pool.clone(),
                admin: format!("t{tag}_admin"),
                runtime: format!("t{tag}_runtime"),
                helper: format!("t{tag}_helper"),
                database,
            };
            // Test database names are deterministic; a run that panicked may
            // have left these roles behind after its database was dropped.
            roles
                .sql(
                    "DROP ROLE IF EXISTS {runtime}; DROP ROLE IF EXISTS {helper}; \
                     DROP ROLE IF EXISTS {admin}; \
                     CREATE ROLE {admin} LOGIN PASSWORD '{password}'; \
                     CREATE ROLE {runtime} LOGIN PASSWORD '{password}'; \
                     CREATE ROLE {helper} NOLOGIN; \
                     ALTER DATABASE {database} OWNER TO {admin};",
                )
                .await;
            roles
        }

        fn expand(&self, sql: &str) -> String {
            sql.replace("{admin}", &self.admin)
                .replace("{runtime}", &self.runtime)
                .replace("{helper}", &self.helper)
                // Quoted: generated database names are mixed case.
                .replace("{database}", &format!("\"{}\"", self.database))
                .replace("{password}", PASSWORD)
        }

        /// Runs fixture SQL as the superuser of the test cluster.
        async fn sql(&self, sql: &str) {
            sqlx::raw_sql(AssertSqlSafe(self.expand(sql)))
                .execute(&self.superuser)
                .await
                .unwrap();
        }

        fn options(&self, role: &str) -> PgConnectOptions {
            self.superuser
                .connect_options()
                .as_ref()
                .clone()
                .username(role)
                .password(PASSWORD)
        }

        /// The administrative step, as the owner: what `bi-server migrate` runs.
        async fn migrate(&self) {
            db::migrate(&self.options(&self.admin), db::MIGRATION_TIMEOUT)
                .await
                .unwrap();
        }

        /// The documented procedure, verbatim, run by the owner.
        async fn grant(&self) {
            let owner = PgPool::connect_lazy_with(self.options(&self.admin));
            sqlx::raw_sql(AssertSqlSafe(GRANTS.replace("bi_runtime", &self.runtime)))
                .execute(&owner)
                .await
                .unwrap();
            owner.close().await;
        }

        fn runtime(&self, mode: MetadataMode) -> Database {
            Database::start(&self.options(&self.runtime), mode)
        }

        async fn drop(self) {
            self.sql(
                "ALTER DATABASE {database} OWNER TO CURRENT_USER; \
                 REASSIGN OWNED BY {admin}, {runtime}, {helper} TO CURRENT_USER; \
                 DROP OWNED BY {admin}, {runtime}, {helper}; \
                 DROP ROLE {runtime}; DROP ROLE {helper}; DROP ROLE {admin};",
            )
            .await;
        }
    }

    /// The first verdict the supervisor publishes.
    async fn assessed(db: &Database) -> Assessment {
        let deadline = Instant::now() + READY;
        loop {
            let status = db.status();
            if let MetadataStatus::Assessed(assessment) = status {
                return assessment;
            }
            assert!(
                Instant::now() < deadline,
                "no assessment in time: {status:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn ready(db: &Database) {
        assert!(
            tokio::time::timeout(READY, db.wait_until_ready())
                .await
                .expect("ready in time"),
            "pool closed before ready"
        );
    }

    fn app(db: Database) -> Router {
        let auth = AuthConfig::from_lookup(&|key| match key {
            "BI_PUBLIC_ORIGIN" => Some(ORIGIN.to_owned()),
            "BI_DEV_USERS" => Some(r#"[{"id":"analyst","display_name":"Analista"}]"#.to_owned()),
            _ => None,
        })
        .unwrap();
        router(AppState {
            auth,
            ..AppState::new(db)
        })
    }

    async fn send(
        app: &Router,
        method: &str,
        path: &str,
        cookie: Option<&str>,
    ) -> (StatusCode, String, Option<String>) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", ORIGIN)
            .header("content-type", "application/json");
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        let body = if path == "/api/v1/dev/login" {
            r#"{"user_id":"analyst"}"#
        } else {
            ""
        };
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let cookie = response.headers().get("set-cookie").map(|value| {
            value
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned()
        });
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(body.to_vec()).unwrap(), cookie)
    }

    async fn sessions(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM bi_meta.sessions")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    fn event(id: &str) -> bi_audit::Event {
        bi_audit::Event {
            event_id: id.into(),
            occurred_at: "2026-10-15T12:00:00Z".parse().unwrap(),
            actor_id: "analyst".into(),
            actor_type: bi_audit::ActorType::User,
            session_id: None,
            action: bi_audit::Action::Login,
            resource_type: "session".into(),
            resource_id: "own".into(),
            result: bi_audit::ResultCode::Success,
            request_id: "request-1".into(),
            trace_id: "a".repeat(32),
            policy_decision_id: None,
            source: bi_audit::Source::Http,
            metadata_redacted: bi_audit::MetadataRedacted::default(),
            schema_version: 1,
        }
    }

    async fn history(pool: &PgPool) -> Vec<(i64, bool, Vec<u8>)> {
        sqlx::query_as(
            "SELECT version, success, checksum FROM public._sqlx_migrations ORDER BY version",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    /// Case 1: the command migrates idempotently, and the limited role does
    /// its real work (session, revocation, an atomic audit event) while every
    /// direct path to the audit storage and the history is refused.
    #[sqlx::test(migrations = false)]
    #[allow(clippy::too_many_lines)] // One end-to-end administrative sequence.
    async fn migrate_command_then_limited_role_works_and_cannot_touch_audit(pool: PgPool) {
        let roles = Roles::create(&pool).await;
        let url = roles.options(&roles.admin).to_url_lossy().to_string();
        let mut snapshots = Vec::new();
        for _ in 0..2 {
            let url = url.clone();
            let output = tokio::task::spawn_blocking(move || {
                std::process::Command::new(env!("CARGO_BIN_EXE_bi-server"))
                    .arg("migrate")
                    .env("BI_DATABASE_URL", url)
                    .output()
                    .unwrap()
            })
            .await
            .unwrap();
            assert_eq!(output.status.code(), Some(0), "{output:?}");
            let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                receipt["migrations"],
                db::MIGRATOR.iter().count().to_string()
            );
            let printed = String::from_utf8_lossy(&output.stdout).to_string()
                + &String::from_utf8_lossy(&output.stderr);
            assert!(!printed.contains(PASSWORD) && !printed.contains(&roles.admin));
            snapshots.push(history(&pool).await);
        }
        assert_eq!(snapshots[0], snapshots[1], "a second run changes nothing");
        assert_eq!(snapshots[0].len(), db::MIGRATOR.iter().count());
        roles.grant().await;

        let db = roles.runtime(MetadataMode::Restricted);
        ready(&db).await;
        assert!(assessed(&db).await.secure());
        let app = app(db.clone());
        let (status, _, cookie) = send(&app, "POST", "/api/v1/dev/login", None).await;
        assert_eq!(status, StatusCode::OK);
        let cookie = cookie.unwrap();
        assert_eq!(
            send(&app, "GET", "/api/v1/session", Some(&cookie)).await.0,
            StatusCode::OK
        );
        assert_eq!(
            send(&app, "POST", "/api/v1/logout", Some(&cookie)).await.0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            send(&app, "GET", "/api/v1/session", Some(&cookie)).await.0,
            StatusCode::UNAUTHORIZED
        );

        // A session and its audit event commit together, or neither does.
        let insert_session = "INSERT INTO bi_meta.sessions(token_hash, user_id, display_name, data_scope, expires_at) \
             VALUES ($1, 'analyst', 'Analista', 'synthetic', clock_timestamp() + interval '1 hour')";
        let mut transaction = db.pool().begin().await.unwrap();
        sqlx::query(insert_session)
            .bind(vec![7_u8; 32])
            .execute(&mut *transaction)
            .await
            .unwrap();
        let checkpoint = audit::append(&mut transaction, &event("event-1"))
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        let mut transaction = db.pool().begin().await.unwrap();
        sqlx::query(insert_session)
            .bind(vec![8_u8; 32])
            .execute(&mut *transaction)
            .await
            .unwrap();
        assert!(
            audit::append(&mut transaction, &event("event-1"))
                .await
                .is_err()
        );
        transaction.rollback().await.unwrap();
        assert_eq!(sessions(&pool).await, 2);

        for sql in [
            "UPDATE bi_meta.audit_events SET payload = '{}'",
            "UPDATE bi_meta.audit_events SET payload = payload",
            "DELETE FROM bi_meta.audit_events",
            "TRUNCATE bi_meta.audit_events",
            "ALTER TABLE bi_meta.audit_events DISABLE TRIGGER audit_immutable",
            "INSERT INTO bi_meta.audit_events(sequence, payload, previous_hash, hash) SELECT 2, payload, hash, hash FROM bi_meta.audit_events",
            "UPDATE bi_meta.audit_head SET sequence = 0",
            "SELECT * FROM bi_meta.audit_events",
            "CREATE TABLE bi_meta.shadow (id int)",
            "DELETE FROM public._sqlx_migrations",
        ] {
            let error = sqlx::query(sql).execute(db.pool()).await.unwrap_err();
            assert_eq!(
                error.as_database_error().unwrap().code().unwrap(),
                "42501",
                "{sql}"
            );
        }
        assert_eq!(audit::verify(&pool, None).await.unwrap(), checkpoint);
        assert_eq!(history(&pool).await, snapshots[0]);
        db.close().await;
        roles.drop().await;
    }

    /// Case 2: each forbidden capability, alone and however it is obtained,
    /// keeps the server not ready and refuses a write before any mutation.
    #[sqlx::test(migrations = false)]
    async fn each_forbidden_capability_alone_blocks_admission_before_writing(pool: PgPool) {
        let roles = Roles::create(&pool).await;
        roles.migrate().await;
        roles.grant().await;
        let mut cases: Vec<(String, String, Forbidden)> = [
            ("ALTER ROLE {runtime} SUPERUSER", "ALTER ROLE {runtime} NOSUPERUSER", Forbidden::Superuser),
            ("ALTER ROLE {runtime} CREATEROLE", "ALTER ROLE {runtime} NOCREATEROLE", Forbidden::CreateRole),
            ("ALTER ROLE {runtime} CREATEDB", "ALTER ROLE {runtime} NOCREATEDB", Forbidden::CreateDatabase),
            ("ALTER ROLE {runtime} REPLICATION", "ALTER ROLE {runtime} NOREPLICATION", Forbidden::Replication),
            ("ALTER ROLE {runtime} BYPASSRLS", "ALTER ROLE {runtime} NOBYPASSRLS", Forbidden::BypassRls),
            ("ALTER DATABASE {database} OWNER TO {runtime}", "ALTER DATABASE {database} OWNER TO {admin}", Forbidden::OwnsDatabase),
            ("ALTER SCHEMA bi_meta OWNER TO {runtime}", "ALTER SCHEMA bi_meta OWNER TO {admin}", Forbidden::OwnsMetadataSchema),
            ("ALTER TABLE bi_meta.audit_head OWNER TO {runtime}", "ALTER TABLE bi_meta.audit_head OWNER TO {admin}", Forbidden::OwnsAuditStorage),
            ("ALTER FUNCTION bi_meta.audit_append(text) OWNER TO {runtime}", "ALTER FUNCTION bi_meta.audit_append(text) OWNER TO {admin}", Forbidden::OwnsAuditAppend),
            ("ALTER TABLE public._sqlx_migrations OWNER TO {runtime}", "ALTER TABLE public._sqlx_migrations OWNER TO {admin}", Forbidden::OwnsMigrations),
            ("GRANT CREATE ON SCHEMA bi_meta TO {runtime}", "REVOKE CREATE ON SCHEMA bi_meta FROM {runtime}", Forbidden::CreatesInMetadataSchema),
            // Column grants, to the role itself and to PUBLIC.
            ("GRANT UPDATE (payload) ON bi_meta.audit_events TO {runtime}", "REVOKE UPDATE (payload) ON bi_meta.audit_events FROM {runtime}", Forbidden::AccessesAuditStorage),
            ("GRANT SELECT (hash) ON bi_meta.audit_head TO PUBLIC", "REVOKE SELECT (hash) ON bi_meta.audit_head FROM PUBLIC", Forbidden::AccessesAuditStorage),
            ("GRANT SELECT ON bi_meta.audit_events TO PUBLIC", "REVOKE SELECT ON bi_meta.audit_events FROM PUBLIC", Forbidden::AccessesAuditStorage),
            // Inherited, and reachable only through SET ROLE.
            ("GRANT INSERT ON bi_meta.audit_head TO {helper}; GRANT {helper} TO {runtime}", "REVOKE {helper} FROM {runtime}; REVOKE INSERT ON bi_meta.audit_head FROM {helper}", Forbidden::AccessesAuditStorage),
            ("GRANT DELETE ON bi_meta.audit_events TO {helper}; GRANT {helper} TO {runtime} WITH INHERIT FALSE, SET TRUE", "REVOKE {helper} FROM {runtime}; REVOKE DELETE ON bi_meta.audit_events FROM {helper}", Forbidden::AccessesAuditStorage),
            ("ALTER ROLE {helper} CREATEROLE; GRANT {helper} TO {runtime} WITH INHERIT FALSE, SET TRUE", "REVOKE {helper} FROM {runtime}; ALTER ROLE {helper} NOCREATEROLE", Forbidden::CreateRole),
            // The owner is still the owner after revoking its own ordinary ACL.
            ("REVOKE ALL ON bi_meta.audit_events FROM {admin}; GRANT {admin} TO {runtime} WITH INHERIT FALSE, SET TRUE", "REVOKE {admin} FROM {runtime}; GRANT ALL ON bi_meta.audit_events TO {admin}", Forbidden::OwnsAuditStorage),
            ("GRANT INSERT ON public._sqlx_migrations TO {runtime}", "REVOKE INSERT ON public._sqlx_migrations FROM {runtime}", Forbidden::WritesMigrations),
            ("GRANT UPDATE (checksum) ON public._sqlx_migrations TO {runtime}", "REVOKE UPDATE (checksum) ON public._sqlx_migrations FROM {runtime}", Forbidden::WritesMigrations),
            ("GRANT EXECUTE ON FUNCTION bi_meta.audit_append(text) TO {runtime} WITH GRANT OPTION", "REVOKE GRANT OPTION FOR EXECUTE ON FUNCTION bi_meta.audit_append(text) FROM {runtime}", Forbidden::GrantsAuditAppend),
            // Reserved roles (#209): powers over the server's programs and
            // files, signals, settings and all data, granted on nothing in
            // bi_meta. Each alone, directly, through one and two
            // intermediaries, and without inheritance, even without SET.
            ("GRANT pg_execute_server_program TO {runtime}", "REVOKE pg_execute_server_program FROM {runtime}", Forbidden::ReservedRole),
            ("GRANT pg_read_server_files TO {runtime}", "REVOKE pg_read_server_files FROM {runtime}", Forbidden::ReservedRole),
            ("GRANT pg_write_server_files TO {runtime}", "REVOKE pg_write_server_files FROM {runtime}", Forbidden::ReservedRole),
            ("GRANT pg_signal_backend TO {runtime}", "REVOKE pg_signal_backend FROM {runtime}", Forbidden::ReservedRole),
            ("GRANT pg_monitor TO {runtime}", "REVOKE pg_monitor FROM {runtime}", Forbidden::ReservedRole),
            ("GRANT pg_read_all_settings TO {runtime}", "REVOKE pg_read_all_settings FROM {runtime}", Forbidden::ReservedRole),
            ("GRANT pg_read_all_data TO {runtime}", "REVOKE pg_read_all_data FROM {runtime}", Forbidden::ReservedRole),
            ("GRANT pg_write_all_data TO {runtime}", "REVOKE pg_write_all_data FROM {runtime}", Forbidden::ReservedRole),
            ("GRANT pg_execute_server_program TO {helper}; GRANT {helper} TO {runtime}", "REVOKE {helper} FROM {runtime}; REVOKE pg_execute_server_program FROM {helper}", Forbidden::ReservedRole),
            ("CREATE ROLE {helper}_b NOLOGIN; GRANT pg_read_server_files TO {helper}_b; GRANT {helper}_b TO {helper}; GRANT {helper} TO {runtime}", "REVOKE {helper} FROM {runtime}; REVOKE {helper}_b FROM {helper}; DROP ROLE {helper}_b", Forbidden::ReservedRole),
            ("GRANT pg_execute_server_program TO {helper}; GRANT {helper} TO {runtime} WITH INHERIT FALSE, SET TRUE", "REVOKE {helper} FROM {runtime}; REVOKE pg_execute_server_program FROM {helper}", Forbidden::ReservedRole),
            ("GRANT pg_execute_server_program TO {helper}; GRANT {helper} TO {runtime} WITH INHERIT FALSE, SET FALSE", "REVOKE {helper} FROM {runtime}; REVOKE pg_execute_server_program FROM {helper}", Forbidden::ReservedRole),
        ]
        .into_iter()
        .map(|(grant, revoke, expected)| (grant.to_owned(), revoke.to_owned(), expected))
        .collect();
        for privilege in [
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "TRUNCATE",
            "REFERENCES",
            "TRIGGER",
            "MAINTAIN",
        ] {
            for table in ["audit_events", "audit_head"] {
                cases.push((
                    format!("GRANT {privilege} ON bi_meta.{table} TO {{runtime}}"),
                    format!("REVOKE {privilege} ON bi_meta.{table} FROM {{runtime}}"),
                    Forbidden::AccessesAuditStorage,
                ));
            }
        }
        for (grant, revoke, expected) in cases {
            roles.sql(&grant).await;
            let db = roles.runtime(MetadataMode::Restricted);
            let assessment = assessed(&db).await;
            assert!(
                assessment.forbidden.contains(&expected),
                "{grant}: {:?}",
                assessment.forbidden
            );
            assert!(!assessment.admits() && !db.admits(), "{grant}");
            let (status, body, cookie) =
                send(&app(db.clone()), "POST", "/api/v1/dev/login", None).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{grant}");
            assert!(
                body.contains("metadata_not_ready") && cookie.is_none(),
                "{grant}: {body}"
            );
            assert_eq!(sessions(&pool).await, 0, "{grant}");
            db.close().await;
            roles.sql(&revoke).await;
            // Ownership round trips rewrite ACLs: restore the documented grants.
            roles.grant().await;
        }
        // The prefix is literal: a harmless group of our own named `pgx_…`
        // keeps the runtime admitted.
        roles
            .sql("CREATE ROLE pgx_{helper} NOLOGIN; GRANT pgx_{helper} TO {runtime}")
            .await;
        let db = roles.runtime(MetadataMode::Restricted);
        ready(&db).await;
        assert!(assessed(&db).await.secure());
        db.close().await;
        roles
            .sql("REVOKE pgx_{helper} FROM {runtime}; DROP ROLE pgx_{helper}")
            .await;
        // A dangerous login that starts as the limited role is still the
        // dangerous login: refused by what session_user reaches.
        roles
            .sql(
                "CREATE ROLE {helper}_login LOGIN PASSWORD '{password}'; \
                 GRANT pg_execute_server_program TO {helper}_login; \
                 GRANT {runtime} TO {helper}_login",
            )
            .await;
        let disguised = roles
            .options(&format!("{}_login", roles.helper))
            .options([("role", roles.runtime.as_str())]);
        let db = Database::start(&disguised, MetadataMode::Restricted);
        let assessment = assessed(&db).await;
        assert!(
            assessment.forbidden.contains(&Forbidden::ReservedRole),
            "{:?}",
            assessment.forbidden
        );
        assert!(!db.admits());
        db.close().await;
        roles
            .sql("DROP OWNED BY {helper}_login; DROP ROLE {helper}_login")
            .await;
        let db = roles.runtime(MetadataMode::Restricted);
        ready(&db).await;
        db.close().await;
        roles.drop().await;
    }

    /// Lacking any privilege today's handlers need is also "not ready", with
    /// its own stable reason, rather than an error at the first request.
    #[sqlx::test(migrations = false)]
    async fn each_missing_required_grant_blocks_admission(pool: PgPool) {
        use bi_server::metadata::Missing;
        let roles = Roles::create(&pool).await;
        roles.migrate().await;
        for (revoke, expected) in [
            (
                "REVOKE USAGE ON SCHEMA bi_meta FROM {runtime}",
                Missing::MetadataSchemaUsage,
            ),
            (
                "REVOKE SELECT ON public._sqlx_migrations FROM {runtime}",
                Missing::ReadMigrations,
            ),
            (
                "REVOKE EXECUTE ON FUNCTION bi_meta.audit_append(text) FROM {runtime}",
                Missing::ExecuteAuditAppend,
            ),
            (
                "REVOKE UPDATE ON bi_meta.sessions FROM {runtime}",
                Missing::Sessions,
            ),
            (
                "REVOKE INSERT ON bi_meta.sessions FROM {runtime}",
                Missing::Sessions,
            ),
            (
                "REVOKE DELETE ON bi_meta.oidc_attempts FROM {runtime}",
                Missing::OidcAttempts,
            ),
            (
                "REVOKE SELECT ON bi_meta.oidc_attempts FROM {runtime}",
                Missing::OidcAttempts,
            ),
        ] {
            roles.grant().await;
            roles.sql(revoke).await;
            let db = roles.runtime(MetadataMode::Restricted);
            let assessment = assessed(&db).await;
            assert_eq!(assessment.missing, BTreeSet::from([expected]), "{revoke}");
            assert!(assessment.forbidden.is_empty(), "{revoke}");
            assert_eq!(assessment.reason(), Some(expected.to_string()), "{revoke}");
            assert!(!db.admits(), "{revoke}");
            db.close().await;
        }
        roles.drop().await;
    }

    /// Case 3: a login that can switch back to a privileged role is as
    /// privileged as that role, whichever of the two is `current_user`.
    #[sqlx::test(migrations = false)]
    async fn a_session_user_able_to_return_to_privilege_is_refused(pool: PgPool) {
        let roles = Roles::create(&pool).await;
        roles.migrate().await;
        roles.grant().await;
        roles.sql("GRANT {runtime} TO {admin}").await;
        for (login, label) in [
            (roles.options(&roles.admin), "owner login"),
            (pool.connect_options().as_ref().clone(), "superuser login"),
        ] {
            // The connection's current_user is the limited role from the start.
            let options = login.options([("role", roles.runtime.as_str())]);
            let db = Database::start(&options, MetadataMode::Restricted);
            let assessment = assessed(&db).await;
            assert!(!assessment.admits(), "{label}");
            assert!(
                assessment.forbidden.contains(&Forbidden::OwnsDatabase)
                    || assessment.forbidden.contains(&Forbidden::Superuser),
                "{label}: {:?}",
                assessment.forbidden
            );
            db.close().await;
        }
        roles.drop().await;
    }

    /// Case 4: a missing, pending, failed, edited or future schema is never
    /// repaired by the server; the administrative step recovers it.
    #[sqlx::test(migrations = false)]
    async fn schema_states_block_without_repair_and_recover_after_migrate(pool: PgPool) {
        let roles = Roles::create(&pool).await;
        let db = roles.runtime(MetadataMode::Restricted);
        assert_eq!(assessed(&db).await.schema, Err(SchemaProblem::Absent));
        tokio::time::sleep(Duration::from_millis(500)).await;
        let untouched: bool = sqlx::query_scalar(
            "SELECT to_regnamespace('bi_meta') IS NULL AND to_regclass('public._sqlx_migrations') IS NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(untouched, "the restricted server never runs DDL");
        roles.migrate().await;
        roles.grant().await;
        ready(&db).await;
        db.close().await;

        let latest = db::MIGRATOR.iter().map(|m| m.version).max().unwrap();
        let saved = history(&pool).await;
        for (tamper, expected) in [
            (
                format!("DELETE FROM public._sqlx_migrations WHERE version = {latest}"),
                SchemaProblem::Pending { version: latest },
            ),
            (
                format!("UPDATE public._sqlx_migrations SET success = false WHERE version = {latest}"),
                SchemaProblem::Failed { version: latest },
            ),
            (
                format!("UPDATE public._sqlx_migrations SET checksum = '\\x00' WHERE version = {latest}"),
                SchemaProblem::ChecksumMismatch { version: latest },
            ),
            (
                "INSERT INTO public._sqlx_migrations (version, description, success, checksum, execution_time) \
                 VALUES (99999999, 'future', true, '\\x00', 0)"
                    .to_owned(),
                SchemaProblem::Unknown { version: 99_999_999 },
            ),
        ] {
            roles.sql(&tamper).await;
            let tampered = history(&pool).await;
            let db = roles.runtime(MetadataMode::Restricted);
            assert_eq!(assessed(&db).await.schema, Err(expected), "{tamper}");
            assert!(!db.admits(), "{tamper}");
            db.close().await;
            assert_eq!(history(&pool).await, tampered, "no repair attempt: {tamper}");
            roles.sql("DELETE FROM public._sqlx_migrations").await;
            for (version, success, checksum) in &saved {
                sqlx::query(
                    "INSERT INTO public._sqlx_migrations (version, description, success, checksum, execution_time) \
                     VALUES ($1, 'restored', $2, $3, 0)",
                )
                .bind(version)
                .bind(success)
                .bind(checksum)
                .execute(&pool)
                .await
                .unwrap();
            }
        }
        let db = roles.runtime(MetadataMode::Restricted);
        ready(&db).await;
        db.close().await;
        roles.drop().await;
    }

    /// A TCP proxy in front of PostgreSQL that can be cut and restored, so a
    /// test sees a real outage: open connections reset, new ones refused.
    struct Proxy {
        port: u16,
        target: SocketAddr,
        serving: Option<JoinHandle<()>>,
    }

    impl Proxy {
        async fn start(target: SocketAddr) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            Self {
                port,
                target,
                serving: Some(tokio::spawn(Self::serve(listener, target))),
            }
        }

        async fn serve(listener: TcpListener, target: SocketAddr) {
            let mut connections = JoinSet::new();
            while let Ok((mut client, _)) = listener.accept().await {
                connections.spawn(async move {
                    if let Ok(mut server) = TcpStream::connect(target).await {
                        tokio::io::copy_bidirectional(&mut client, &mut server)
                            .await
                            .ok();
                    }
                });
            }
        }

        async fn cut(&mut self) {
            if let Some(serving) = self.serving.take() {
                serving.abort();
                serving.await.ok();
            }
        }

        async fn restore(&mut self) {
            let listener = TcpListener::bind(("127.0.0.1", self.port)).await.unwrap();
            self.serving = Some(tokio::spawn(Self::serve(listener, self.target)));
        }
    }

    /// Case 5: losing PostgreSQL stops admission at once while liveness
    /// stays up; on reconnection the role is checked again before serving.
    #[sqlx::test(migrations = false)]
    async fn losing_postgres_stops_admission_and_reconnection_rechecks_the_role(pool: PgPool) {
        let roles = Roles::create(&pool).await;
        roles.migrate().await;
        roles.grant().await;
        let options = pool.connect_options();
        let target = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await
            .unwrap()
            .find(SocketAddr::is_ipv4)
            .unwrap();
        let mut proxy = Proxy::start(target).await;
        let through_proxy = roles
            .options(&roles.runtime)
            .host("127.0.0.1")
            .port(proxy.port);
        let db = Database::start(&through_proxy, MetadataMode::Restricted);
        ready(&db).await;
        let app = app(db.clone());
        let (_, _, cookie) = send(&app, "POST", "/api/v1/dev/login", None).await;
        let cookie = cookie.unwrap();

        proxy.cut().await;
        let deadline = Instant::now() + Duration::from_secs(10);
        while db.admits() {
            assert!(Instant::now() < deadline, "the lost connection is noticed");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let (status, body, _) = send(&app, "GET", "/api/v1/session", Some(&cookie)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.contains("metadata_not_ready"), "{body}");
        assert_eq!(
            send(&app, "GET", "/health/ready", None).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            send(&app, "GET", "/health/live", None).await.0,
            StatusCode::OK
        );

        // The role changes while the server cannot reach the database.
        roles
            .sql("GRANT SELECT ON bi_meta.audit_events TO {runtime}")
            .await;
        proxy.restore().await;
        let deadline = Instant::now() + READY;
        let assessment = loop {
            if let MetadataStatus::Assessed(assessment) = db.status() {
                break assessment;
            }
            assert!(Instant::now() < deadline, "revalidated after reconnecting");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert!(
            assessment
                .forbidden
                .contains(&Forbidden::AccessesAuditStorage)
        );
        assert_eq!(
            send(&app, "GET", "/api/v1/session", Some(&cookie)).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        roles
            .sql("REVOKE SELECT ON bi_meta.audit_events FROM {runtime}")
            .await;
        ready(&db).await;
        assert_eq!(
            send(&app, "GET", "/api/v1/session", Some(&cookie)).await.0,
            StatusCode::OK
        );
        // Again with a reserved role (#209): granted while unreachable, it
        // refuses the replacement connection; revoked, the server recovers.
        proxy.cut().await;
        let deadline = Instant::now() + Duration::from_secs(10);
        while db.admits() {
            assert!(Instant::now() < deadline, "the lost connection is noticed");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        roles
            .sql("GRANT pg_execute_server_program TO {runtime}")
            .await;
        proxy.restore().await;
        let deadline = Instant::now() + READY;
        let assessment = loop {
            if let MetadataStatus::Assessed(assessment) = db.status() {
                break assessment;
            }
            assert!(Instant::now() < deadline, "revalidated after reconnecting");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert!(assessment.forbidden.contains(&Forbidden::ReservedRole));
        assert_eq!(
            send(&app, "GET", "/api/v1/session", Some(&cookie)).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        roles
            .sql("REVOKE pg_execute_server_program FROM {runtime}")
            .await;
        ready(&db).await;
        assert_eq!(
            send(&app, "GET", "/api/v1/session", Some(&cookie)).await.0,
            StatusCode::OK
        );
        db.close().await;
        proxy.cut().await;
        roles.drop().await;
    }

    /// Case 7, metrics: across development, ready, invalid and unavailable
    /// states the validation counter only ever carries one closed label.
    #[sqlx::test(migrations = false)]
    async fn validation_metrics_keep_closed_labels_in_every_state(pool: PgPool) {
        use opentelemetry::metrics::MeterProvider as _;
        use opentelemetry_sdk::metrics::data::{
            AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics,
        };
        use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

        let roles = Roles::create(&pool).await;
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let metrics = || MetadataMetrics::new(&provider.meter("test"));
        let runtime = |roles: &Roles| roles.options(&roles.runtime);

        // Development: migrates itself as the owner, and says so.
        let development = Database::start_observed(
            &roles.options(&roles.admin),
            MetadataMode::Development,
            metrics(),
        );
        ready(&development).await;
        assert!(!assessed(&development).await.secure());
        development.close().await;
        roles.grant().await;
        // Restricted, ready.
        let secure =
            Database::start_observed(&runtime(&roles), MetadataMode::Restricted, metrics());
        ready(&secure).await;
        secure.close().await;
        // Restricted, invalid.
        roles.sql("GRANT {admin} TO {runtime}").await;
        let invalid =
            Database::start_observed(&runtime(&roles), MetadataMode::Restricted, metrics());
        assert!(!assessed(&invalid).await.admits());
        invalid.close().await;
        // Restricted, unavailable: nothing listens on port 1.
        let unavailable = Database::start_observed(
            &roles.options(&roles.runtime).host("127.0.0.1").port(1),
            MetadataMode::Restricted,
            metrics(),
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        // Wait for its own first validation, not for an earlier log line.
        while unavailable.status() != MetadataStatus::Unavailable {
            assert!(
                Instant::now() < deadline,
                "the unreachable store is assessed"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        unavailable.close().await;

        provider.force_flush().unwrap();
        let mut results = BTreeSet::new();
        for metric in exporter
            .get_finished_metrics()
            .unwrap()
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .filter(|metric| metric.name() == "bi.metadata.validation.attempts")
        {
            let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                panic!("counter expected");
            };
            for point in sum.data_points() {
                let labels: Vec<_> = point.attributes().collect();
                assert_eq!(labels.len(), 1);
                assert_eq!(labels[0].key.as_str(), "result");
                results.insert(labels[0].value.to_string());
            }
        }
        assert_eq!(
            results,
            BTreeSet::from([
                "invalid".to_owned(),
                "ready".to_owned(),
                "unavailable".to_owned()
            ])
        );
        roles.drop().await;
    }

    fn percentile(samples: &mut [Duration], fraction: f64) -> f64 {
        samples.sort();
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )] // A rank within a few hundred samples.
        let rank = ((samples.len() as f64 * fraction).ceil() as usize).max(1) - 1;
        samples[rank].as_secs_f64() * 1000.0
    }

    /// Measurement for #115, without thresholds on shared machines: what
    /// one validation (role and schema inspection) costs, the latency of hot
    /// authenticated requests through the admission point, and proof that
    /// those requests never re-validate (the validation counter is unchanged).
    #[sqlx::test(migrations = false)]
    async fn admission_cost_baseline(pool: PgPool) {
        use opentelemetry::metrics::MeterProvider as _;
        use opentelemetry_sdk::metrics::data::{
            AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics, SumDataPoint,
        };
        use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
        const WARMUP: usize = 20;
        const SAMPLES: usize = 200;
        const INSPECTIONS: usize = 30;

        let roles = Roles::create(&pool).await;
        roles.migrate().await;
        roles.grant().await;
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let db = Database::start_observed(
            &roles.options(&roles.runtime),
            MetadataMode::Restricted,
            MetadataMetrics::new(&provider.meter("test")),
        );
        ready(&db).await;
        let validations = || {
            provider.force_flush().unwrap();
            exporter
                .get_finished_metrics()
                .unwrap()
                .iter()
                .flat_map(ResourceMetrics::scope_metrics)
                .flat_map(ScopeMetrics::metrics)
                .filter(|metric| metric.name() == "bi.metadata.validation.attempts")
                .filter_map(|metric| match metric.data() {
                    AggregatedMetrics::U64(MetricData::Sum(sum)) => {
                        Some(sum.data_points().map(SumDataPoint::value).sum::<u64>())
                    }
                    _ => None,
                })
                .max()
                .unwrap_or_default()
        };

        let mut connection = db.pool().acquire().await.unwrap();
        let mut inspection = Vec::with_capacity(INSPECTIONS);
        for _ in 0..INSPECTIONS {
            let started = Instant::now();
            bi_server::metadata::inspect::facts(&mut connection)
                .await
                .unwrap();
            inspection.push(started.elapsed());
        }
        drop(connection);

        let app = app(db.clone());
        let (_, _, cookie) = send(&app, "POST", "/api/v1/dev/login", None).await;
        let cookie = cookie.unwrap();
        for _ in 0..WARMUP {
            send(&app, "GET", "/api/v1/session", Some(&cookie)).await;
        }
        let connections = db.pool().size();
        let before = validations();
        let mut hot = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = Instant::now();
            let (status, _, _) = send(&app, "GET", "/api/v1/session", Some(&cookie)).await;
            hot.push(started.elapsed());
            assert_eq!(status, StatusCode::OK);
        }
        let after = validations();
        // SQLx returns a released connection to the pool asynchronously, so
        // a request may still find none idle and open another one; each new
        // physical connection validates once. A validated connection is never
        // inspected again, whatever the number of requests.
        let opened = db.pool().size().saturating_sub(connections);
        assert_eq!(
            after - before,
            u64::from(opened),
            "hot requests never re-validate a connection"
        );
        println!(
            "metadata_admission inspections={INSPECTIONS} inspection_p50_ms={:.3} inspection_p95_ms={:.3} \
             warmup={WARMUP} n={SAMPLES} session_p50_ms={:.3} session_p95_ms={:.3} \
             validations_during_hot_requests={} connections_opened_during_hot_requests={opened}",
            percentile(&mut inspection, 0.50),
            percentile(&mut inspection, 0.95),
            percentile(&mut hot, 0.50),
            percentile(&mut hot, 0.95),
            after - before,
        );
        db.close().await;
        roles.drop().await;
    }

    /// Runs the real `bi-server`, with its production log setup (default
    /// `info` filter, text format), until its output names `expected`, and
    /// returns everything it printed. A separate process per state, so no
    /// other test's logging can interfere.
    async fn server_output(env: Vec<(&'static str, String)>, expected: &'static str) -> String {
        tokio::task::spawn_blocking(move || {
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let log = tempfile::NamedTempFile::new().unwrap();
            let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_bi-server"));
            command
                .env("BI_BIND_ADDR", format!("127.0.0.1:{port}"))
                .env("BI_ANALYTICS", "off")
                .env("BI_LOG_FORMAT", "text")
                .env_remove("RUST_LOG")
                .stdout(log.reopen().unwrap())
                .stderr(log.reopen().unwrap());
            for (key, value) in env {
                command.env(key, value);
            }
            let mut child = command.spawn().unwrap();
            let deadline = Instant::now() + READY;
            let output = loop {
                let output = std::fs::read_to_string(log.path()).unwrap_or_default();
                if output.contains(expected) || Instant::now() > deadline {
                    break output;
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            child.kill().ok();
            child.wait().ok();
            output
        })
        .await
        .unwrap()
    }

    /// Cases 6 and 7, logs: explicit development warns that its role is
    /// privileged, and every restricted state is named by its stable code,
    /// while no line ever carries the password, a role name, a URL or SQL.
    #[sqlx::test(migrations = false)]
    async fn operator_logs_name_each_state_without_secrets(pool: PgPool) {
        let roles = Roles::create(&pool).await;
        let url = |options: PgConnectOptions| options.to_url_lossy().to_string();
        let runtime = || url(roles.options(&roles.runtime));
        let mut logs = server_output(
            vec![
                ("BI_METADATA_MODE", "development".to_owned()),
                (
                    "BI_DEV_USERS",
                    r#"[{"id":"analyst","display_name":"Analista"}]"#.to_owned(),
                ),
                ("BI_DATABASE_URL", url(roles.options(&roles.admin))),
            ],
            "metadata_privileged_development",
        )
        .await;
        roles.grant().await;
        logs += &server_output(vec![("BI_DATABASE_URL", runtime())], "metadata_ready").await;
        roles.sql("GRANT {admin} TO {runtime}").await;
        logs += &server_output(vec![("BI_DATABASE_URL", runtime())], "metadata_not_ready").await;
        logs += &server_output(
            vec![(
                "BI_DATABASE_URL",
                url(roles.options(&roles.runtime).host("127.0.0.1").port(1)),
            )],
            "metadata_unavailable",
        )
        .await;
        for expected in [
            "metadata_privileged_development",
            "metadata_ready",
            "metadata_not_ready",
            "reason=\"owns_database\"",
            "metadata_unavailable",
        ] {
            assert!(logs.contains(expected), "{expected} missing from:\n{logs}");
        }
        // The database name is logged on purpose (where the server connects);
        // credentials, roles and SQL never are.
        for secret in [
            PASSWORD,
            roles.admin.as_str(),
            roles.runtime.as_str(),
            "postgres://",
            "pg_catalog",
            "SELECT",
            "GRANT",
        ] {
            assert!(!logs.contains(secret), "{secret} leaked into:\n{logs}");
        }
        roles.drop().await;
    }

    /// The restricted server keeps the G1 query route closed, even for a
    /// synthetic dev session, and records the refusal through the limited
    /// role's append function alone (#118).
    #[sqlx::test(migrations = false)]
    async fn the_restricted_server_closes_the_legacy_query_route_and_records_it(pool: PgPool) {
        let roles = Roles::create(&pool).await;
        roles.migrate().await;
        roles.grant().await;
        let db = roles.runtime(MetadataMode::Restricted);
        ready(&db).await;
        let app = app(db.clone());
        let (status, _, cookie) = send(&app, "POST", "/api/v1/dev/login", None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, body, _) = send(&app, "POST", "/api/v1/queries", Some(&cookie.unwrap())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(body.contains("\"legacy_query_disabled\""), "{body}");
        let denied: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM bi_meta.audit_events
             WHERE (payload::jsonb)->>'resource_id' = 'legacy_queries'
               AND (payload::jsonb)->>'action' = 'denial'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(denied, 1);
        db.close().await;
        roles.drop().await;
    }

    /// The restricted server decides the project catalog (#124) and records
    /// each decision through the limited role's append function alone; that
    /// role still cannot read the chain it writes to.
    #[sqlx::test(migrations = false)]
    async fn the_restricted_server_records_catalog_decisions(pool: PgPool) {
        let roles = Roles::create(&pool).await;
        roles.migrate().await;
        roles.grant().await;
        let db = roles.runtime(MetadataMode::Restricted);
        ready(&db).await;
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join("metrics")).unwrap();
        std::fs::create_dir(project.path().join("policies")).unwrap();
        let members = project.path().join("policies/members.yaml");
        std::fs::write(&members, "version: 1\nusers:\n  analyst: [modeler]\n").unwrap();
        let auth = AuthConfig::from_lookup(&|key| match key {
            "BI_PUBLIC_ORIGIN" => Some(ORIGIN.to_owned()),
            "BI_DEV_USERS" => Some(r#"[{"id":"analyst","display_name":"Analista"}]"#.to_owned()),
            _ => None,
        })
        .unwrap();
        let app = router(AppState {
            auth,
            models: bi_server::models::Models::new(Some(project.path().to_owned())),
            ..AppState::new(db.clone())
        });
        let (status, _, cookie) = send(&app, "POST", "/api/v1/dev/login", None).await;
        assert_eq!(status, StatusCode::OK);
        let cookie = cookie.unwrap();
        let (status, body, _) = send(&app, "GET", "/api/v1/models", Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("\"project_status\":\"ready\""), "{body}");
        std::fs::write(&members, "version: 1\nusers:\n  analyst: [viewer]\n").unwrap();
        let (status, body, _) = send(&app, "GET", "/api/v1/models", Some(&cookie)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(body.contains("\"models_forbidden\""), "{body}");
        let recorded: Vec<(String, String, bool)> = sqlx::query_as(
            "SELECT (payload::jsonb)->>'action', (payload::jsonb)->>'result',
                    (payload::jsonb)->>'policy_decision_id' IS NOT NULL
             FROM bi_meta.audit_events
             WHERE (payload::jsonb)->>'resource_id' = 'project:catalog' ORDER BY sequence",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            recorded,
            vec![
                ("query".to_owned(), "success".to_owned(), true),
                ("denial".to_owned(), "denied".to_owned(), true),
            ]
        );
        // The role that wrote them can neither read nor change the chain.
        let runtime = PgPool::connect_lazy_with(roles.options(&roles.runtime));
        for statement in [
            "SELECT count(*) FROM bi_meta.audit_events",
            "UPDATE bi_meta.audit_events SET payload = payload",
            "DELETE FROM bi_meta.audit_events",
        ] {
            assert!(
                sqlx::query(statement).execute(&runtime).await.is_err(),
                "{statement}"
            );
        }
        runtime.close().await;
        bi_server::audit::verify(&pool, None).await.unwrap();
        db.close().await;
        roles.drop().await;
    }

    /// The pool replaces one lost work connection while the sentinel stays
    /// connected: the new connection validates itself before serving, so a
    /// dangerous privilege that appeared meanwhile stops admission before
    /// any write, and work resumes once the role is corrected (#125).
    #[sqlx::test(migrations = false)]
    async fn a_replaced_work_connection_revalidates_before_serving(pool: PgPool) {
        let roles = Roles::create(&pool).await;
        roles.migrate().await;
        roles.grant().await;
        let db = roles.runtime(MetadataMode::Restricted);
        ready(&db).await;
        let app = app(db.clone());
        let mut connection = db.acquire().await.unwrap();
        let lost: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *connection)
            .await
            .unwrap();
        roles
            .sql("GRANT SELECT ON bi_meta.audit_events TO {runtime}")
            .await;
        let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
            .bind(lost)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(terminated);
        drop(connection);
        let deadline = Instant::now() + READY;
        loop {
            let alive: i64 =
                sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE pid = $1")
                    .bind(lost)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            if alive == 0 {
                break;
            }
            assert!(Instant::now() < deadline, "backend {lost} still alive");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            db.admits(),
            "the sentinel is alive: nothing else noticed the loss"
        );

        let (status, body, cookie) = send(&app, "POST", "/api/v1/dev/login", None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(body.contains("\"metadata_not_ready\""), "{body}");
        assert!(cookie.is_none());
        assert_eq!(sessions(&pool).await, 0, "refused before writing");
        assert!(!db.admits());
        assert!(
            matches!(db.acquire().await, Err(db::AcquireError::NotAdmitted)),
            "no connection is handed out while the role is unsafe"
        );
        let deadline = Instant::now() + READY;
        loop {
            if let MetadataStatus::Assessed(assessment) = db.status() {
                assert!(!assessment.admits());
                assert!(
                    assessment
                        .forbidden
                        .contains(&Forbidden::AccessesAuditStorage)
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "no verdict in time: {:?}",
                db.status()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        roles
            .sql("REVOKE SELECT ON bi_meta.audit_events FROM {runtime}")
            .await;
        ready(&db).await;
        let (status, body, cookie) = send(&app, "POST", "/api/v1/dev/login", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(cookie.is_some());
        assert_eq!(sessions(&pool).await, 1);
        db.close().await;
        roles.drop().await;
    }

    /// `bi-server migrate` ends within its bound while another session holds
    /// the history table, leaves no backend waiting on the server and no
    /// history change, and completes once the lock is released (#125).
    #[sqlx::test(migrations = false)]
    async fn migration_is_bounded_by_a_held_lock_and_leaves_nothing_behind(pool: PgPool) {
        let roles = Roles::create(&pool).await;
        roles.migrate().await;
        let before = history(&pool).await;
        let mut locked = pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE public._sqlx_migrations IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *locked)
            .await
            .unwrap();
        let limit = Duration::from_secs(2);
        let started = Instant::now();
        let outcome = db::migrate(&roles.options(&roles.admin), limit).await;
        let elapsed = started.elapsed();
        assert!(
            matches!(outcome, Err(db::MigrationError::TimedOut)),
            "{outcome:?}"
        );
        assert!(
            elapsed >= limit && elapsed < limit + Duration::from_secs(3),
            "{elapsed:?}"
        );
        let deadline = Instant::now() + READY;
        loop {
            let left: i64 =
                sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE usename = $1")
                    .bind(&roles.admin)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            if left == 0 {
                break;
            }
            assert!(Instant::now() < deadline, "{left} migration backends left");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        locked.commit().await.unwrap();
        assert_eq!(history(&pool).await, before);
        db::migrate(&roles.options(&roles.admin), limit)
            .await
            .unwrap();
        assert_eq!(history(&pool).await, before, "idempotent once released");
        roles.drop().await;
    }

    /// A validation slowed by a lock on the history table logs no SQL with
    /// the production filter (#125).
    #[sqlx::test(migrations = false)]
    async fn a_slow_validation_logs_no_sql(pool: PgPool) {
        let roles = Roles::create(&pool).await;
        roles.migrate().await;
        roles.grant().await;
        let mut locked = pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE public._sqlx_migrations IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *locked)
            .await
            .unwrap();
        let url = roles.options(&roles.runtime).to_url_lossy().to_string();
        let output = server_output(vec![("BI_DATABASE_URL", url)], "metadata_ready");
        // Longer than the one-second slow-statement threshold of SQLx.
        let release = async move {
            tokio::time::sleep(Duration::from_millis(2500)).await;
            locked.commit().await.unwrap();
        };
        let (logs, ()) = tokio::join!(output, release);
        assert!(logs.contains("metadata_ready"), "{logs}");
        for sql in [
            "SELECT",
            "_sqlx_migrations",
            "db.statement",
            "slow statement",
        ] {
            assert!(!logs.contains(sql), "{sql} leaked into:\n{logs}");
        }
        roles.drop().await;
    }
}
