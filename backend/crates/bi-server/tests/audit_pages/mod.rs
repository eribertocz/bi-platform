//! Audit pages (#160) against real PostgreSQL 18 roles: an owner that
//! migrates, the runtime that appends through `audit_append`, and the
//! least-privilege reader every page goes through. Role names come from the
//! test's own database, so parallel tests never collide; they are dropped at
//! the end. Fixture preparation and checks use the cluster's superuser, a
//! separate connection that is never a path of the product.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use bi_audit::{Action, ActorType, Event, MetadataRedacted, ResultCode, Source};
use bi_server::audit::pages::{
    AuditPages, CURSOR_LIFETIME, CursorKey, Filters, Limit, Page, PageError, PageQuery, Profile,
    STATEMENTS, action_from_wire, utc_from_wire,
};
use bi_server::audit::{self, reader::AuditReader};
use bi_server::db;
use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, PgPool};
use tracing::instrument::WithSubscriber as _;

const GRANTS: &str = include_str!("../../admin/runtime-grants.sql");
const READER_GRANTS: &str = include_str!("../../admin/audit-reader-grants.sql");
/// A decoy, never a real secret: no log line may ever contain it.
const PASSWORD: &str = "decoy-password-160a";
const READY: Duration = Duration::from_secs(30);
/// The wall clock of the fixture's first page.
const T0: Duration = Duration::from_hours(500_000);

fn at(offset: Duration) -> SystemTime {
    SystemTime::UNIX_EPOCH + T0 + offset
}

/// Seven events: actors A and B, every action, instants down to the
/// nanosecond.
const SEVEN: [(&str, Action, &str); 7] = [
    ("actor-a", Action::Login, "2026-10-15T10:00:00Z"),
    ("actor-b", Action::Query, "2026-10-15T10:30:00.5Z"),
    ("actor-a", Action::Query, "2026-10-15T11:00:00Z"),
    ("actor-b", Action::Logout, "2026-10-15T11:00:00.000000001Z"),
    ("actor-a", Action::Denial, "2026-10-15T12:00:00Z"),
    ("actor-a", Action::Query, "2026-10-15T12:30:00.123456Z"),
    ("actor-b", Action::Query, "2026-10-15T13:00:00Z"),
];

fn event(sequence: usize, actor: &str, action: Action, occurred_at: &str) -> Event {
    Event {
        event_id: format!("event-{sequence}"),
        occurred_at: occurred_at.parse().unwrap(),
        actor_id: actor.into(),
        actor_type: ActorType::User,
        session_id: None,
        action,
        resource_type: "dataset".into(),
        resource_id: "sales".into(),
        result: ResultCode::Success,
        request_id: format!("request-{sequence}"),
        trace_id: "b".repeat(32),
        policy_decision_id: None,
        source: Source::Http,
        metadata_redacted: MetadataRedacted::default(),
        schema_version: bi_audit::SCHEMA_VERSION,
    }
}

struct Chain {
    superuser: PgPool,
    database: String,
    admin: String,
    runtime: String,
    reader: String,
    writer: PgPool,
}

impl Chain {
    /// Migrated by the owner, both documented grants applied verbatim, and
    /// the seven events appended by the runtime.
    async fn seven(pool: &PgPool) -> Self {
        let chain = Self::empty(pool).await;
        for (index, (actor, action, occurred_at)) in SEVEN.iter().enumerate() {
            chain
                .append(&event(index + 1, actor, *action, occurred_at))
                .await;
        }
        chain
    }

    async fn empty(pool: &PgPool) -> Self {
        let database: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(pool)
            .await
            .unwrap();
        assert!(
            database
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        );
        let tag: String = database
            .chars()
            .rev()
            .filter(char::is_ascii_alphanumeric)
            .take(16)
            .collect::<String>()
            .to_ascii_lowercase();
        let options = |role: &str| {
            pool.connect_options()
                .as_ref()
                .clone()
                .username(role)
                .password(PASSWORD)
        };
        let runtime = format!("p{tag}_runtime");
        let chain = Self {
            superuser: pool.clone(),
            admin: format!("p{tag}_admin"),
            reader: format!("p{tag}_reader"),
            writer: PgPool::connect_lazy_with(options(&runtime)),
            runtime,
            database,
        };
        chain
            .sql(
                "DROP ROLE IF EXISTS {runtime}; DROP ROLE IF EXISTS {reader};                  DROP ROLE IF EXISTS {admin};                  CREATE ROLE {admin} LOGIN PASSWORD '{password}';                  CREATE ROLE {runtime} LOGIN PASSWORD '{password}';                  CREATE ROLE {reader} LOGIN PASSWORD '{password}';                  ALTER DATABASE {database} OWNER TO {admin};",
            )
            .await;
        db::migrate(&chain.options(&chain.admin), db::MIGRATION_TIMEOUT)
            .await
            .unwrap();
        let owner = PgPool::connect_lazy_with(chain.options(&chain.admin));
        for (grants, example, role) in [
            (GRANTS, "bi_runtime", &chain.runtime),
            (READER_GRANTS, "bi_audit_reader", &chain.reader),
        ] {
            sqlx::raw_sql(AssertSqlSafe(grants.replace(example, role)))
                .execute(&owner)
                .await
                .unwrap();
        }
        owner.close().await;
        chain
    }

    fn expand(&self, sql: &str) -> String {
        sql.replace("{admin}", &self.admin)
            .replace("{runtime}", &self.runtime)
            .replace("{reader}", &self.reader)
            .replace("{database}", &format!("\"{}\"", self.database))
            .replace("{password}", PASSWORD)
    }

    /// Fixture SQL as the superuser of the test cluster.
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

    fn reader(&self) -> AuditReader {
        AuditReader::start(&self.options(&self.reader))
    }

    fn pages(&self) -> AuditPages {
        AuditPages::new(self.reader(), CursorKey::generate().unwrap())
    }

    /// Through the production append path, as the runtime.
    async fn append(&self, event: &Event) {
        let mut transaction = self.writer.begin().await.unwrap();
        audit::append(&mut transaction, event).await.unwrap();
        transaction.commit().await.unwrap();
    }

    async fn head(&self) -> i64 {
        sqlx::query_scalar("SELECT sequence FROM bi_meta.audit_head")
            .fetch_one(&self.superuser)
            .await
            .unwrap()
    }

    async fn drop(self) {
        self.writer.close().await;
        self.sql(
            "ALTER DATABASE {database} OWNER TO CURRENT_USER; \
             REASSIGN OWNED BY {admin}, {runtime}, {reader} TO CURRENT_USER; \
             DROP OWNED BY {admin}, {runtime}, {reader}; \
             DROP ROLE {runtime}; DROP ROLE {reader}; DROP ROLE {admin};",
        )
        .await;
    }
}

fn query(filters: Filters, limit: u32) -> PageQuery {
    PageQuery::new(filters, Limit::new(limit).unwrap()).unwrap()
}

fn all(limit: u32) -> PageQuery {
    query(Filters::default(), limit)
}

#[allow(clippy::unnecessary_wraps)] // Shaped for the optional filter fields.
fn utc(instant: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    Some(utc_from_wire(instant).unwrap())
}

/// The sequences of a page, from its events' ids, checked against its JSON.
fn sequences(page: &Page) -> Vec<usize> {
    let json: serde_json::Value = serde_json::from_str(page.json()).unwrap();
    assert_eq!(
        serde_json::from_value::<Vec<Event>>(json["events"].clone()).unwrap(),
        page.events()
    );
    assert_eq!(json["next_cursor"].as_str(), page.next_cursor());
    page.events()
        .iter()
        .map(|e| e.event_id["event-".len()..].parse().unwrap())
        .collect()
}

/// Every page of a walk, by sequences; only the last lacks a cursor.
async fn walk(pages: &AuditPages, query: &PageQuery) -> Vec<Vec<usize>> {
    let mut walked = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = pages
            .page_at(query, cursor.as_deref(), at(Duration::ZERO))
            .await
            .unwrap();
        walked.push(sequences(&page));
        match page.next_cursor() {
            Some(next) => cursor = Some(next.to_owned()),
            None => return walked,
        }
    }
}

async fn idle_again(reader: &AuditReader, expected: usize) {
    let deadline = Instant::now() + READY;
    while reader.idle() != expected {
        assert!(Instant::now() < deadline, "the pool settles in time");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Registers the repository's log callsites, served and refused, before
/// any capture.
async fn warm(pages: &AuditPages) {
    assert!(pages.page(&all(1), None).await.is_ok());
    assert!(pages.page(&all(1), Some("not a cursor!")).await.is_err());
}

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What `work` logged, down to the driver's traces, and its result. The
/// callsites it must see are registered beforehand by [`warm`].
async fn logged<T>(work: impl Future<Output = T>) -> (String, T) {
    static ANOTHER: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let output = Capture(bytes.clone());
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || output.clone())
        .finish();
    // With a single subscriber registered, tracing caches a new callsite's
    // interest from the registering thread's own default: a parallel test
    // with none would silence it for this capture. A second registration
    // kept for the whole process makes every registration consult all live
    // subscribers, this one included.
    ANOTHER.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
    let result = work.with_subscriber(subscriber).await;
    let logs = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    (logs, result)
}

/// Seven events in pages of three: 3, 3 and 1, through one cursor chain
/// that replays identically; events 8 and 9 appended between pages stay
/// out of the walk begun at 7, and only a new walk sees them. Reading
/// leaves the head and the chain as they were.
#[sqlx::test(migrations = false)]
async fn seven_events_walk_three_three_one_and_appends_meanwhile_stay_out(pool: PgPool) {
    let chain = Chain::seven(&pool).await;
    let pages = chain.pages();
    let q = all(3);
    let first = pages.page_at(&q, None, at(Duration::ZERO)).await.unwrap();
    assert_eq!(sequences(&first), [1, 2, 3]);
    let c1 = first.next_cursor().unwrap().to_owned();
    let second = pages
        .page_at(&q, Some(&c1), at(Duration::from_secs(60)))
        .await
        .unwrap();
    assert_eq!(sequences(&second), [4, 5, 6]);
    let c2 = second.next_cursor().unwrap().to_owned();
    for (index, (actor, action, occurred_at)) in [
        ("actor-a", Action::Query, "2026-10-15T14:00:00Z"),
        ("actor-b", Action::Query, "2026-10-15T15:00:00Z"),
    ]
    .iter()
    .enumerate()
    {
        chain
            .append(&event(8 + index, actor, *action, occurred_at))
            .await;
    }
    assert_eq!(chain.head().await, 9);
    let third = pages
        .page_at(&q, Some(&c2), at(Duration::from_secs(120)))
        .await
        .unwrap();
    assert_eq!(sequences(&third), [7]);
    assert_eq!(third.next_cursor(), None, "the final page has no cursor");
    // The same cursor replays the same page, its token included: the
    // expiry is the first page's, not renewed.
    let replay = pages
        .page_at(&q, Some(&c1), at(Duration::from_secs(600)))
        .await
        .unwrap();
    assert_eq!(replay.json(), second.json());
    // A new walk starts at the new head.
    assert_eq!(walk(&pages, &all(200)).await, [(1..=9).collect::<Vec<_>>()]);
    assert_eq!(chain.head().await, 9, "reading never moves the head");
    assert_eq!(audit::verify(&pool, None).await.unwrap().sequence, 9);
    chain.drop().await;
}

/// Every limit from 1 to 8 and every filter walks the fixture visiting each
/// selected event exactly once, in order, in full pages but the last, which
/// alone has no cursor.
#[sqlx::test(migrations = false)]
async fn every_limit_and_filter_visits_each_selected_event_once(pool: PgPool) {
    let chain = Chain::seven(&pool).await;
    let pages = chain.pages();
    let actor = |a: &str| Some(a.parse().unwrap());
    let cases: [(Filters, &[usize]); 9] = [
        (Filters::default(), &[1, 2, 3, 4, 5, 6, 7]),
        (
            Filters {
                actor: actor("actor-a"),
                ..Filters::default()
            },
            &[1, 3, 5, 6],
        ),
        (
            Filters {
                action: Some(action_from_wire("query").unwrap()),
                ..Filters::default()
            },
            &[2, 3, 6, 7],
        ),
        (
            Filters {
                actor: actor("actor-a"),
                action: Some(Action::Query),
                ..Filters::default()
            },
            &[3, 6],
        ),
        // `from` inclusive, `to` exclusive, to the nanosecond.
        (
            Filters {
                from: utc("2026-10-15T11:00:00Z"),
                to: utc("2026-10-15T12:30:00.123456Z"),
                ..Filters::default()
            },
            &[3, 4, 5],
        ),
        (
            Filters {
                from: utc("2026-10-15T11:00:00Z"),
                to: utc("2026-10-15T11:00:00.000000001Z"),
                ..Filters::default()
            },
            &[3],
        ),
        (
            Filters {
                from: utc("2026-10-15T11:00:00.000000001Z"),
                to: utc("2026-10-15T11:00:00.000000002Z"),
                ..Filters::default()
            },
            &[4],
        ),
        (
            Filters {
                actor: actor("actor-b"),
                action: Some(Action::Logout),
                from: utc("2026-10-15T10:30:00.5Z"),
                ..Filters::default()
            },
            &[4],
        ),
        (
            Filters {
                actor: actor("actor-c"),
                ..Filters::default()
            },
            &[],
        ),
    ];
    for (filters, expected) in cases {
        for limit in 1..=8 {
            let q = query(filters.clone(), limit);
            let walked = walk(&pages, &q).await;
            let flat: Vec<usize> = walked.iter().flatten().copied().collect();
            assert_eq!(flat, expected, "{filters:?} limit {limit}");
            let full = usize::try_from(limit).unwrap();
            let (last, rest) = walked.split_last().unwrap();
            assert!(rest.iter().all(|page| page.len() == full), "{walked:?}");
            assert!(last.len() <= full && (walked.len() == 1 || !last.is_empty()));
        }
    }
    chain.drop().await;
}

/// An empty chain yields one empty page without a cursor.
#[sqlx::test(migrations = false)]
async fn an_empty_chain_yields_one_empty_final_page(pool: PgPool) {
    let chain = Chain::empty(&pool).await;
    let page = chain.pages().page(&all(50), None).await.unwrap();
    assert_eq!(page.events(), []);
    assert_eq!(page.next_cursor(), None);
    assert_eq!(page.json(), r#"{"events":[],"next_cursor":null}"#);
    chain.drop().await;
}

/// Edited, expired, rebound to other filters or limit, of an unknown
/// version or under another key: every such cursor is refused, with its
/// own code, before any connection is requested.
#[sqlx::test(migrations = false)]
async fn cursors_edited_expired_rebound_or_foreign_are_refused(pool: PgPool) {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let chain = Chain::seven(&pool).await;
    let reader = chain.reader();
    let pages = AuditPages::new(reader.clone(), CursorKey::generate().unwrap());
    let q = all(3);
    let first = pages.page_at(&q, None, at(Duration::ZERO)).await.unwrap();
    let c1 = first.next_cursor().unwrap().to_owned();
    let lifetime = CURSOR_LIFETIME;
    // Valid to its last millisecond, refused from its expiry on.
    assert!(
        pages
            .page_at(
                &q,
                Some(&c1),
                at(lifetime.checked_sub(Duration::from_millis(1)).unwrap())
            )
            .await
            .is_ok()
    );
    // Not renewed: a cursor issued ten minutes in still expires at 15.
    let c2 = pages
        .page_at(&q, Some(&c1), at(Duration::from_secs(600)))
        .await
        .unwrap()
        .next_cursor()
        .unwrap()
        .to_owned();
    for cursor in [&c1, &c2] {
        assert_eq!(
            pages
                .page_at(&q, Some(cursor), at(lifetime))
                .await
                .unwrap_err(),
            PageError::CursorExpired
        );
    }
    let mut bytes = URL_SAFE_NO_PAD.decode(&c1).unwrap();
    let mut edited = bytes.clone();
    edited[8] ^= 1; // The last sequence: 3 becomes 2.
    let mut version = bytes.clone();
    version[0] = 2;
    bytes.truncate(40);
    let other_key = AuditPages::new(reader.clone(), CursorKey::generate().unwrap());
    let actor_a = Filters {
        actor: Some("actor-a".parse().unwrap()),
        ..Filters::default()
    };
    let refused: [(&AuditPages, PageQuery, String, PageError); 7] = [
        (
            &pages,
            all(3),
            URL_SAFE_NO_PAD.encode(&edited),
            PageError::CursorInvalid,
        ),
        (
            &pages,
            all(3),
            URL_SAFE_NO_PAD.encode(&bytes),
            PageError::CursorInvalid,
        ),
        (&pages, all(3), format!("{c1}!"), PageError::CursorInvalid),
        (
            &pages,
            all(3),
            URL_SAFE_NO_PAD.encode(&version),
            PageError::CursorVersion,
        ),
        (&pages, all(4), c1.clone(), PageError::CursorMismatch),
        (
            &pages,
            query(actor_a, 3),
            c1.clone(),
            PageError::CursorMismatch,
        ),
        (&other_key, all(3), c1.clone(), PageError::CursorInvalid),
    ];
    warm(&pages).await;
    idle_again(&reader, 1).await;
    for (pages, q, cursor, expected) in refused {
        let (logs, outcome) = logged(pages.page_at(&q, Some(&cursor), at(Duration::ZERO))).await;
        assert_eq!(outcome.unwrap_err(), expected, "{cursor}");
        assert!(logs.contains(expected.code()), "{logs}");
        assert!(!logs.contains(&cursor) && !logs.contains(&c1), "{logs}");
        // Refused before any connection was requested: the idle one stays.
        assert_eq!(reader.idle(), 1);
    }
    reader.close().await;
    chain.drop().await;
}

/// The byte budget, with the page's real size: exactly that fits, one byte
/// less refuses the page whole. The deadline, injected tighter: a page held
/// up by a lock fails in time, its connection is closed rather than handed
/// back, PostgreSQL leaves nothing waiting, and the next page is served.
/// Then v1's own 5 s: an exhausted pool ends the page at its deadline.
#[sqlx::test(migrations = false)]
async fn the_byte_budget_and_the_deadline_are_honored(pool: PgPool) {
    let chain = Chain::seven(&pool).await;
    let reader = chain.reader();
    let key = CursorKey::generate().unwrap();
    let pages = AuditPages::new(reader.clone(), key.clone());
    let q = all(3);
    let page = pages.page_at(&q, None, at(Duration::ZERO)).await.unwrap();
    let size = page.json().len();
    println!(
        "profile v1: deadline {:?}, page budget {} bytes; page of 3 events: {size} bytes",
        Profile::V1.deadline(),
        Profile::V1.page_bytes()
    );
    let budget = |bytes| {
        AuditPages::new(reader.clone(), key.clone())
            .with_profile(Profile::tighter(Profile::V1.deadline(), bytes).unwrap())
    };
    let exact = budget(size)
        .page_at(&q, None, at(Duration::ZERO))
        .await
        .unwrap();
    assert_eq!(exact.json(), page.json());
    assert_eq!(
        budget(size - 1)
            .page_at(&q, None, at(Duration::ZERO))
            .await
            .unwrap_err(),
        PageError::BudgetExceeded
    );

    // A lock no read may pass: the reader's statement waits for it. A pool
    // of its own holds exactly one connection, opened by a first page.
    let solo = chain.reader();
    AuditPages::new(solo.clone(), key.clone())
        .page_at(&q, None, at(Duration::ZERO))
        .await
        .unwrap();
    let tight = Duration::from_millis(300);
    let hurried = AuditPages::new(solo.clone(), key.clone())
        .with_profile(Profile::tighter(tight, Profile::V1.page_bytes()).unwrap());
    idle_again(&solo, 1).await;
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE bi_meta.audit_head IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let started = Instant::now();
    let outcome = hurried.page_at(&q, None, at(Duration::ZERO)).await;
    let elapsed = started.elapsed();
    println!("deadline {tight:?} under a held lock: {elapsed:?}");
    assert_eq!(outcome.unwrap_err(), PageError::DeadlineExceeded);
    assert!(
        elapsed >= tight && elapsed < tight + Duration::from_millis(500),
        "{elapsed:?}"
    );
    assert_eq!(
        solo.idle(),
        0,
        "the interrupted connection is not handed back"
    );
    let deadline = Instant::now() + READY;
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE usename = $1 AND state <> 'idle'",
        )
        .bind(&chain.reader)
        .fetch_one(&pool)
        .await
        .unwrap();
        if waiting == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{waiting} reader statements left"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    lock.commit().await.unwrap();
    assert_eq!(
        sequences(&hurried.page_at(&q, None, at(Duration::ZERO)).await.unwrap()),
        [1, 2, 3]
    );

    // Every connection of the reader taken: v1's deadline ends the wait.
    let held = (
        reader.acquire().await.unwrap(),
        reader.acquire().await.unwrap(),
    );
    let started = Instant::now();
    let outcome = pages.page_at(&q, None, at(Duration::ZERO)).await;
    let elapsed = started.elapsed();
    println!("v1 deadline with the pool exhausted: {elapsed:?}");
    assert_eq!(outcome.unwrap_err(), PageError::DeadlineExceeded);
    assert!(
        elapsed >= Profile::V1.deadline()
            && elapsed < Profile::V1.deadline() + Duration::from_millis(500),
        "{elapsed:?}"
    );
    drop(held);
    assert!(pages.page_at(&q, None, at(Duration::ZERO)).await.is_ok());
    solo.close().await;
    reader.close().await;
    chain.drop().await;
}

/// A refused reader keeps retrying its admission until its own 5 s
/// acquisition timeout (#159), so a refusal surfaces as `Unavailable` or,
/// once the page's deadline passes first, `DeadlineExceeded`: no page
/// either way. A 2 s deadline keeps these tests short.
fn refusal() -> Profile {
    Profile::tighter(Duration::from_secs(2), Profile::V1.page_bytes()).unwrap()
}

fn assert_no_page(outcome: &Result<Page, PageError>, case: &str) {
    assert!(
        matches!(
            outcome,
            Err(PageError::Unavailable | PageError::DeadlineExceeded)
        ),
        "{case}: {outcome:?}"
    );
}

/// A page served, its connection back in the pool; then a forbidden grant,
/// persistent DDL included: the next page yields no data and the
/// connection is discarded. Revoked, the same repository serves again.
#[sqlx::test(migrations = false)]
async fn a_grant_after_a_page_refuses_the_next_and_a_revoke_recovers(pool: PgPool) {
    let chain = Chain::seven(&pool).await;
    let key = CursorKey::generate().unwrap();
    let q = all(3);
    for (grant, revoke) in [
        (
            "GRANT INSERT ON bi_meta.audit_events TO {reader}",
            "REVOKE INSERT ON bi_meta.audit_events FROM {reader}",
        ),
        (
            "GRANT CREATE ON SCHEMA public TO {reader}",
            "REVOKE CREATE ON SCHEMA public FROM {reader}",
        ),
        (
            "GRANT CREATE ON SCHEMA bi_meta TO {reader}",
            "REVOKE CREATE ON SCHEMA bi_meta FROM {reader}",
        ),
        (
            "GRANT CREATE ON DATABASE {database} TO {reader}",
            "REVOKE CREATE ON DATABASE {database} FROM {reader}",
        ),
        (
            "GRANT {runtime} TO {reader}",
            "REVOKE {runtime} FROM {reader}",
        ),
    ] {
        // A pool of its own: one connection, opened by the first page.
        let reader = chain.reader();
        let pages = AuditPages::new(reader.clone(), key.clone()).with_profile(refusal());
        let first = pages.page_at(&q, None, at(Duration::ZERO)).await.unwrap();
        let cursor = first.next_cursor().unwrap().to_owned();
        idle_again(&reader, 1).await;
        chain.sql(grant).await;
        for continuation in [None, Some(cursor.as_str())] {
            let refused = pages.page_at(&q, continuation, at(Duration::ZERO)).await;
            assert_no_page(&refused, grant);
            assert_eq!(
                reader.idle(),
                0,
                "{grant}: the refused connection is discarded"
            );
        }
        chain.sql(revoke).await;
        let recovered = pages
            .page_at(&q, Some(&cursor), at(Duration::ZERO))
            .await
            .unwrap();
        assert_eq!(sequences(&recovered), [4, 5, 6], "{revoke}");
        reader.close().await;
    }
    chain.drop().await;
}

/// The owner, the runtime and the cluster's superuser, each presented as
/// the reader, yield no page.
#[sqlx::test(migrations = false)]
async fn an_owner_runtime_or_superuser_presented_as_reader_yields_no_page(pool: PgPool) {
    let chain = Chain::seven(&pool).await;
    for options in [
        chain.options(&chain.admin),
        chain.options(&chain.runtime),
        pool.connect_options().as_ref().clone(),
    ] {
        let reader = AuditReader::start(&options);
        let pages =
            AuditPages::new(reader.clone(), CursorKey::generate().unwrap()).with_profile(refusal());
        let refused = pages.page(&all(50), None).await;
        assert_no_page(&refused, options.get_username());
        assert_eq!(reader.idle(), 0);
        reader.close().await;
    }
    chain.drop().await;
}

/// An append in flight holds the head's row lock: a first page neither
/// waits for it nor sees it, and the append still commits.
#[sqlx::test(migrations = false)]
async fn reading_never_waits_for_or_blocks_the_appender(pool: PgPool) {
    let chain = Chain::seven(&pool).await;
    let pages = chain.pages();
    let mut appending = chain.writer.begin().await.unwrap();
    audit::append(
        &mut appending,
        &event(8, "actor-a", Action::Query, "2026-10-15T14:00:00Z"),
    )
    .await
    .unwrap();
    let started = Instant::now();
    let page = pages.page(&all(200), None).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(sequences(&page), (1..=7).collect::<Vec<_>>());
    appending.commit().await.unwrap();
    assert_eq!(chain.head().await, 8);
    assert_eq!(page.next_cursor(), None);
    chain.drop().await;
}

/// A stored row that is not a valid current event fails its page closed,
/// echoing nothing of it; the rows before it are served.
#[sqlx::test(migrations = false)]
async fn an_invalid_stored_event_fails_its_page_without_echo(pool: PgPool) {
    let chain = Chain::seven(&pool).await;
    // Written around the append function, as only a superuser could.
    let mut stored =
        serde_json::to_value(event(8, "actor-a", Action::Query, "2026-10-15T14:00:00Z")).unwrap();
    stored["schema_version"] = 2.into();
    stored["actor_id"] = "person-secret-marker".into();
    sqlx::query(
        "INSERT INTO bi_meta.audit_events(sequence, payload, previous_hash, hash) \
         VALUES (8, $1, decode(repeat('00', 32), 'hex'), decode(repeat('00', 32), 'hex'))",
    )
    .bind(stored.to_string())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE bi_meta.audit_head SET sequence = 8")
        .execute(&pool)
        .await
        .unwrap();
    let pages = chain.pages();
    let q = all(7);
    let first = pages.page_at(&q, None, at(Duration::ZERO)).await.unwrap();
    assert_eq!(sequences(&first), (1..=7).collect::<Vec<_>>());
    let cursor = first.next_cursor().unwrap().to_owned();
    let (logs, outcome) = logged(pages.page_at(&q, Some(&cursor), at(Duration::ZERO))).await;
    assert_eq!(outcome.unwrap_err(), PageError::InvalidStoredEvent);
    assert!(logs.contains("audit_page_invalid_stored_event"), "{logs}");
    assert!(!logs.contains("person-secret-marker"), "{logs}");
    chain.drop().await;
}

/// Logs and metrics of served and refused pages carry codes and counts
/// only: no actor, cursor, credential or SQL.
#[sqlx::test(migrations = false)]
async fn logs_and_metrics_name_no_actor_cursor_or_secret(pool: PgPool) {
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::data::{
        AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics,
    };
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
    let chain = Chain::seven(&pool).await;
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter.clone())
        .build();
    let pages = chain
        .pages()
        .with_meter(&provider.meter("audit-pages-test"));
    let q = query(
        Filters {
            actor: Some("actor-a".parse().unwrap()),
            ..Filters::default()
        },
        2,
    );
    warm(&pages).await;
    let (logs, cursor) = logged(async {
        let first = pages.page(&q, None).await.unwrap();
        let cursor = first.next_cursor().unwrap().to_owned();
        pages.page(&q, Some(&cursor)).await.unwrap();
        assert!(pages.page(&all(2), Some(&cursor)).await.is_err());
        cursor
    })
    .await;
    assert!(
        logs.contains("audit_page_served") && logs.contains("audit_page_cursor_mismatch"),
        "{logs}"
    );
    for secret in [
        cursor.as_str(),
        "actor-a",
        PASSWORD,
        &chain.reader,
        "SELECT",
        "payload",
    ] {
        assert!(!logs.contains(secret), "{secret} leaked into:\n{logs}");
    }
    provider.force_flush().unwrap();
    let mut results = std::collections::BTreeMap::new();
    for metric in exporter
        .get_finished_metrics()
        .unwrap()
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(ScopeMetrics::metrics)
    {
        let labels: Vec<Vec<(String, String)>> = match metric.data() {
            AggregatedMetrics::U64(MetricData::Sum(sum)) => {
                assert_eq!(metric.name(), "bi.audit.page.requests");
                sum.data_points()
                    .map(|p| {
                        *results
                            .entry(
                                p.attributes()
                                    .map(|kv| kv.value.to_string())
                                    .collect::<Vec<_>>(),
                            )
                            .or_insert(0) += p.value();
                        p.attributes()
                            .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                            .collect()
                    })
                    .collect()
            }
            AggregatedMetrics::F64(MetricData::Histogram(histogram)) => {
                assert_eq!(metric.name(), "bi.audit.page.duration");
                assert_eq!(metric.unit(), "s");
                histogram
                    .data_points()
                    .map(|p| {
                        p.attributes()
                            .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                            .collect()
                    })
                    .collect()
            }
            other => panic!("unexpected metric: {other:?}"),
        };
        for point in labels {
            assert_eq!(point.len(), 1);
            assert_eq!(point[0].0, "result");
            assert!(
                point[0].1 == "ok" || PageError::ALL.iter().any(|e| e.code() == point[0].1),
                "{point:?}"
            );
        }
    }
    // The warm-up's page and refusal, then the walk's two pages and one
    // refusal.
    let expected: std::collections::BTreeMap<Vec<String>, u64> = [
        ("ok", 3),
        ("audit_page_cursor_invalid", 1),
        ("audit_page_cursor_mismatch", 1),
    ]
    .into_iter()
    .map(|(result, count)| (vec![result.to_owned()], count))
    .collect();
    assert_eq!(results, expected);
    chain.drop().await;
}

/// Many events, then statistics, as a deployed chain: a hundred actors
/// (1 % each), `denial` rare (1 %), the other actions in turn.
async fn large_chain(pool: &PgPool, events: usize) -> Chain {
    let chain = Chain::empty(pool).await;
    let common = [Action::Login, Action::Logout, Action::Query];
    for sequence in 1..=events {
        let actor = format!("actor-{}", sequence % 100);
        let action = if sequence % 100 == 0 {
            Action::Denial
        } else {
            common[sequence % 3]
        };
        chain
            .append(&event(sequence, &actor, action, "2026-10-15T12:00:00Z"))
            .await;
    }
    sqlx::query("ANALYZE bi_meta.audit_events")
        .execute(pool)
        .await
        .unwrap();
    chain
}

/// The plan PostgreSQL chooses for `sql` (simple protocol), as text.
async fn plan(pool: &PgPool, sql: &str) -> String {
    sqlx::raw_sql(AssertSqlSafe(sql.to_owned()))
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|row| sqlx::Row::get::<String, _>(row, 0))
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

/// Every page statement reads an index, never the table nor a row lock: in
/// its generic plan, as a cached prepared statement may run it, in sequence
/// order up to the limit without sorting; and in its custom plan for
/// selective values, where each filter uses its own expression index.
#[sqlx::test(migrations = false)]
async fn every_page_statement_reads_its_index_in_order(pool: PgPool) {
    let chain = large_chain(&pool, 2000).await;
    let indexes = [
        ("all", &["audit_events_pkey"][..]),
        ("actor", &["audit_actor_sequence"][..]),
        ("action", &["audit_action_sequence"][..]),
        (
            "actor+action",
            &["audit_actor_sequence", "audit_action_sequence"][..],
        ),
    ];
    for ((name, sql), (named, expected)) in STATEMENTS.into_iter().zip(indexes) {
        assert_eq!(name, named);
        let generic = plan(&pool, &format!("EXPLAIN (GENERIC_PLAN) {sql}")).await;
        let custom = plan(
            &pool,
            &format!(
                "PREPARE page(bigint, bigint, bigint, text, text, text, text) AS {sql};                  EXPLAIN EXECUTE page(0, 2000, 51, NULL, NULL,                      {});                  DEALLOCATE page;",
                match name {
                    "actor+action" => "'actor-0', 'denial'",
                    "action" => "'denial', NULL",
                    _ => "'actor-7', NULL",
                }
            ),
        )
        .await;
        println!(
            "page statement `{name}`, generic plan:
{generic}
custom plan:
{custom}
"
        );
        assert!(
            generic.contains("Limit") && generic.contains("Index Scan using"),
            "{name}: {generic}"
        );
        for absent in ["Seq Scan", "Sort", "LockRows"] {
            assert!(!generic.contains(absent), "{name}: {generic}");
        }
        // With values, the planner may also read only the index's matching
        // rows and order those few: never the table, never a lock.
        assert!(
            expected.iter().any(|index| custom.contains(index)),
            "{name}: {custom}"
        );
        for absent in ["Seq Scan", "LockRows"] {
            assert!(!custom.contains(absent), "{name}: {custom}");
        }
    }
    chain.drop().await;
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

/// Page latency on 2000 events, without thresholds on shared machines:
/// `cold` is a new reader's first page (connection, admission and read);
/// the rest reuse one repository, whose every page still re-checks the
/// reader's role on hand-out. Run explicitly:
/// `cargo test -p bi-server --no-default-features --features postgres-tests --test postgres audit_pages::page_latency_baseline -- --ignored --nocapture`
#[sqlx::test(migrations = false)]
#[ignore = "measurement, run explicitly"]
async fn page_latency_baseline(pool: PgPool) {
    const COLD: usize = 30;
    const WARMUP: usize = 20;
    const SAMPLES: usize = 300;
    let chain = large_chain(&pool, 2000).await;
    let key = CursorKey::generate().unwrap();
    let mut cold = Vec::with_capacity(COLD);
    for _ in 0..COLD {
        let reader = chain.reader();
        let pages = AuditPages::new(reader.clone(), key.clone());
        let started = Instant::now();
        pages.page(&all(50), None).await.unwrap();
        cold.push(started.elapsed());
        reader.close().await;
    }
    println!(
        "audit page latency (ms), 2000 events, {} build",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    println!(
        "cold first page, limit 50: p50 {:.2} p95 {:.2} (n={COLD})",
        percentile(&mut cold, 0.5),
        percentile(&mut cold, 0.95)
    );
    let reader = chain.reader();
    let pages = AuditPages::new(reader.clone(), key);
    let actor = query(
        Filters {
            actor: Some("actor-7".parse().unwrap()),
            ..Filters::default()
        },
        50,
    );
    let middle = pages
        .page(&all(50), None)
        .await
        .unwrap()
        .next_cursor()
        .unwrap()
        .to_owned();
    let cases: [(&str, PageQuery, Option<&str>); 4] = [
        ("first page, limit 50", all(50), None),
        ("next page, limit 50", all(50), Some(middle.as_str())),
        ("first page, limit 200", all(200), None),
        ("actor filter, limit 50", actor, None),
    ];
    for (name, q, cursor) in cases {
        for _ in 0..WARMUP {
            pages.page(&q, cursor).await.unwrap();
        }
        let mut samples = Vec::with_capacity(SAMPLES);
        let mut bytes = 0;
        for _ in 0..SAMPLES {
            let started = Instant::now();
            let page = pages.page(&q, cursor).await.unwrap();
            samples.push(started.elapsed());
            bytes = page.json().len();
        }
        println!(
            "warm {name}: p50 {:.2} p95 {:.2} (n={SAMPLES}, {bytes} bytes)",
            percentile(&mut samples, 0.5),
            percentile(&mut samples, 0.95)
        );
    }
    reader.close().await;
    chain.drop().await;
}
