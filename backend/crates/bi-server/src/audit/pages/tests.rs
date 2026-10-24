//! Pages without PostgreSQL: request validation, the cursor's
//! authentication, the byte budget, and walks over a model of the chain.

use std::io::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bi_audit::{ActorType, MetadataRedacted, ResultCode, Source};
use hmac::{Hmac, KeyInit as _, Mac as _};
use proptest::prelude::*;

use super::cursor::layout;
use super::encode::{self, Bounded};
use super::*;

const KEY: [u8; 32] = [7; 32];
const NOW: u64 = 1_800_000_000_000;

fn event(sequence: u64, actor: &str, action: Action) -> Event {
    Event {
        event_id: format!("event-{sequence}"),
        occurred_at: "2026-10-15T12:00:00Z".parse().unwrap(),
        actor_id: actor.into(),
        actor_type: ActorType::User,
        session_id: None,
        action,
        resource_type: "session".into(),
        resource_id: "own".into(),
        result: ResultCode::Success,
        request_id: "request-1".into(),
        trace_id: "a".repeat(32),
        policy_decision_id: None,
        source: Source::Http,
        metadata_redacted: MetadataRedacted::default(),
        schema_version: bi_audit::SCHEMA_VERSION,
    }
}

fn row(sequence: u64) -> (u64, String) {
    (
        sequence,
        event(sequence, "actor-a", Action::Login)
            .canonical_json()
            .unwrap(),
    )
}

fn query(actor: Option<&str>, action: Option<Action>, limit: u32) -> PageQuery {
    PageQuery::new(
        Filters {
            actor: actor.map(|actor| actor.parse().unwrap()),
            action,
            ..Filters::default()
        },
        Limit::new(limit).unwrap(),
    )
    .unwrap()
}

fn position(last: u64, upper: u64) -> Position {
    Position {
        last,
        upper,
        expires_ms: NOW + 1000,
    }
}

/// `body` with a valid tag under `key`: what only a key holder can make.
fn signed(key: &[u8; 32], body: &[u8]) -> String {
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key).unwrap();
    mac.update(layout::TAG_DOMAIN);
    mac.update(body);
    let mut token = body.to_vec();
    token.extend_from_slice(&mac.finalize().into_bytes());
    URL_SAFE_NO_PAD.encode(token)
}

#[test]
fn limits_outside_one_to_two_hundred_are_refused() {
    assert_eq!(Limit::default().get(), 50);
    assert_eq!(Limit::new(1).unwrap().get(), 1);
    assert_eq!(Limit::new(200).unwrap().get(), 200);
    for limit in [0, 201, 65_536 + 50, u32::MAX] {
        assert_eq!(Limit::new(limit), Err(PageError::InvalidLimit), "{limit}");
    }
}

#[test]
fn actors_are_bounded_and_shaped_like_stored_ones() {
    assert!("analyst-1:team_a.b".parse::<Actor>().is_ok());
    assert!("a".repeat(MAX_ACTOR_BYTES).parse::<Actor>().is_ok());
    for actor in [
        String::new(),
        "a".repeat(MAX_ACTOR_BYTES + 1),
        "a b".into(),
        "año".into(),
        "x' OR '1'='1".into(),
        "a%".into(),
        "a\u{0}".into(),
    ] {
        assert_eq!(
            actor.parse::<Actor>(),
            Err(PageError::InvalidActor),
            "{actor:?}"
        );
    }
    let actor: Actor = "secret-person".parse().unwrap();
    assert_eq!(format!("{actor:?}"), "Actor(<redacted>)");
    let query = query(Some("secret-person"), None, 5);
    assert!(!format!("{query:?}").contains("secret-person"));
}

#[test]
fn actions_are_the_closed_wire_set_of_the_contract() {
    for action in [Action::Login, Action::Logout, Action::Query, Action::Denial] {
        let wire = serde_json::to_value(action).unwrap();
        assert_eq!(wire, action_wire(action));
        assert_eq!(action_from_wire(action_wire(action)), Ok(action));
    }
    for action in ["Query", "QUERY", "query ", "select", "", "query;", "1"] {
        assert_eq!(
            action_from_wire(action),
            Err(PageError::InvalidAction),
            "{action:?}"
        );
    }
}

#[test]
fn instants_are_utc_and_ranges_are_ordered() {
    let at = utc_from_wire("2026-10-15T12:00:00.000000001Z").unwrap();
    assert_eq!(at.timestamp_subsec_nanos(), 1);
    assert!(utc_from_wire("2026-10-15T12:00:00Z").is_ok());
    for instant in [
        "2026-10-15T12:00:00+02:00",
        "2026-10-15T12:00:00",
        "2026-10-15",
        "2026-13-01T00:00:00Z",
        "2026-02-30T00:00:00Z",
        "yesterday",
        "2026-10-15T12:00:00z",
        &format!("2026-10-15T12:00:00.{}Z", "0".repeat(60)),
    ] {
        assert_eq!(
            utc_from_wire(instant),
            Err(PageError::InvalidInstant),
            "{instant}"
        );
    }
    let early = utc_from_wire("2026-10-15T11:00:00Z").unwrap();
    let late = utc_from_wire("2026-10-15T12:00:00Z").unwrap();
    let range = |from, to| {
        PageQuery::new(
            Filters {
                from,
                to,
                ..Filters::default()
            },
            Limit::default(),
        )
    };
    assert!(range(Some(early), Some(late)).is_ok());
    assert!(range(Some(early), None).is_ok());
    assert!(range(None, Some(late)).is_ok());
    assert_eq!(range(Some(late), Some(early)), Err(PageError::InvalidRange));
    assert_eq!(range(Some(late), Some(late)), Err(PageError::InvalidRange));
    let far = DateTime::<Utc>::from_timestamp(253_402_300_800, 0).unwrap(); // Year 10000.
    assert_eq!(range(Some(far), None), Err(PageError::InvalidInstant));
    let before_zero = DateTime::<Utc>::from_timestamp(-62_167_219_201, 0).unwrap();
    assert_eq!(
        range(None, Some(before_zero)),
        Err(PageError::InvalidInstant)
    );
}

#[test]
fn the_profile_is_v1_or_tighter() {
    assert_eq!(Profile::V1.deadline(), Duration::from_millis(5000));
    assert_eq!(Profile::V1.page_bytes(), 4_194_304);
    assert!(Profile::tighter(Duration::from_millis(5000), 4_194_304).is_some());
    assert!(Profile::tighter(Duration::from_millis(1), 1).is_some());
    for (deadline, bytes) in [
        (Duration::ZERO, 10),
        (Duration::from_millis(5001), 10),
        (Duration::from_millis(10), 0),
        (Duration::from_millis(10), 4_194_305),
    ] {
        assert!(Profile::tighter(deadline, bytes).is_none());
    }
}

#[test]
fn error_codes_are_a_closed_distinct_vocabulary() {
    let codes: std::collections::BTreeSet<_> = PageError::ALL.iter().map(|e| e.code()).collect();
    assert_eq!(codes.len(), PageError::ALL.len());
    for error in PageError::ALL {
        assert!(error.code().starts_with("audit_page_"));
        assert!(
            error
                .code()
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'_')
        );
        assert_eq!(error.to_string(), error.code());
    }
}

#[test]
fn the_key_is_never_printed_and_never_falls_back() {
    let key = CursorKey::fixed(KEY);
    assert_eq!(format!("{key:?}"), "CursorKey(<redacted>)");
    assert!(CursorKey::from_entropy(|_| Err(getrandom::Error::UNSUPPORTED)).is_none());
    assert!(CursorKey::generate().is_some());
}

#[test]
fn a_cursor_opens_for_its_scope_until_it_expires() {
    let key = CursorKey::fixed(KEY);
    let scope = query(Some("actor-a"), Some(Action::Query), 3).scope();
    let token = key.seal(position(3, 7), &scope);
    assert!(token.len() <= MAX_CURSOR_BYTES);
    assert_eq!(token.len(), 119);
    assert!(
        token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
    assert_eq!(key.open(&token, &scope, NOW), Ok(position(3, 7)));
    // The last millisecond before expiry, then expiry itself and later.
    assert_eq!(key.open(&token, &scope, NOW + 999), Ok(position(3, 7)));
    assert_eq!(
        key.open(&token, &scope, NOW + 1000),
        Err(PageError::CursorExpired)
    );
    assert_eq!(
        key.open(&token, &scope, u64::MAX),
        Err(PageError::CursorExpired)
    );
}

#[test]
fn a_cursor_for_other_filters_or_limit_is_refused() {
    let key = CursorKey::fixed(KEY);
    let issued = query(Some("actor-a"), Some(Action::Query), 3);
    let token = key.seal(position(3, 7), &issued.scope());
    let at = |instant| Some(utc_from_wire(instant).unwrap());
    for other in [
        query(Some("actor-a"), Some(Action::Query), 4),
        query(Some("actor-b"), Some(Action::Query), 3),
        query(None, Some(Action::Query), 3),
        query(Some("actor-a"), Some(Action::Login), 3),
        query(Some("actor-a"), None, 3),
        PageQuery::new(
            Filters {
                actor: Some("actor-a".parse().unwrap()),
                action: Some(Action::Query),
                from: at("2026-10-15T12:00:00Z"),
                to: None,
            },
            Limit::new(3).unwrap(),
        )
        .unwrap(),
        PageQuery::new(
            Filters {
                actor: Some("actor-a".parse().unwrap()),
                action: Some(Action::Query),
                from: None,
                to: at("2026-10-15T12:00:00Z"),
            },
            Limit::new(3).unwrap(),
        )
        .unwrap(),
    ] {
        assert_eq!(
            key.open(&token, &other.scope(), NOW),
            Err(PageError::CursorMismatch),
            "{other:?}"
        );
    }
}

#[test]
fn a_cursor_under_another_key_or_malformed_is_refused() {
    let key = CursorKey::fixed(KEY);
    let scope = query(None, None, 3).scope();
    let token = key.seal(position(3, 7), &scope);
    let other = CursorKey::fixed([8; 32]);
    assert_eq!(
        other.open(&token, &scope, NOW),
        Err(PageError::CursorInvalid)
    );
    for malformed in [
        String::new(),
        "not base64!".into(),
        format!("{token}="),
        format!("{token}A"),
        token[..token.len() - 1].to_owned(),
        token[..20].to_owned(),
        "A".repeat(MAX_CURSOR_BYTES + 1),
        URL_SAFE_NO_PAD.encode([1_u8]),
    ] {
        assert_eq!(
            key.open(&malformed, &scope, NOW),
            Err(PageError::CursorInvalid),
            "{malformed}"
        );
    }
}

#[test]
fn an_unknown_version_is_refused_even_when_signed() {
    let key = CursorKey::fixed(KEY);
    let scope = query(None, None, 3).scope();
    let bytes = URL_SAFE_NO_PAD
        .decode(key.seal(position(3, 7), &scope))
        .unwrap();
    for version in [0_u8, 2, 255] {
        let mut body = bytes[..layout::TAG_AT].to_vec();
        body[layout::VERSION_AT] = version;
        assert_eq!(
            key.open(&signed(&KEY, &body), &scope, NOW),
            Err(PageError::CursorVersion),
            "{version}"
        );
    }
    // A longer v2 layout is refused by version, not by length.
    let mut longer = vec![2_u8; layout::TOKEN + 16];
    longer[0] = 2;
    assert_eq!(
        key.open(&URL_SAFE_NO_PAD.encode(&longer), &scope, NOW),
        Err(PageError::CursorVersion)
    );
}

#[test]
fn a_signed_cursor_past_its_bound_is_refused() {
    // Only a key holder could make it; the repository never issues one.
    let key = CursorKey::fixed(KEY);
    let scope = query(None, None, 3).scope();
    let bytes = URL_SAFE_NO_PAD
        .decode(key.seal(position(3, 7), &scope))
        .unwrap();
    let mut body = bytes[..layout::TAG_AT].to_vec();
    body[layout::LAST_AT..layout::LAST_AT + 8].copy_from_slice(&7_u64.to_be_bytes());
    assert_eq!(
        key.open(&signed(&KEY, &body), &scope, NOW),
        Err(PageError::CursorInvalid)
    );
}

proptest! {
    /// Any change to any byte of a token, its tag included, is refused.
    #[test]
    fn every_edited_byte_of_a_cursor_is_refused(
        last in 0_u64..1000,
        span in 1_u64..1000,
        at in 0_usize..89,
        mask in 1_u8..=255,
    ) {
        let key = CursorKey::fixed(KEY);
        let scope = query(None, Some(Action::Query), 10).scope();
        let mut bytes = URL_SAFE_NO_PAD
            .decode(key.seal(position(last, last + span), &scope))
            .unwrap();
        bytes[at] ^= mask;
        let opened = key.open(&URL_SAFE_NO_PAD.encode(&bytes), &scope, NOW);
        let expected = if at == layout::VERSION_AT {
            PageError::CursorVersion
        } else {
            PageError::CursorInvalid
        };
        prop_assert_eq!(opened, Err(expected));
    }

    /// Two queries share a fingerprint scope only when they are equal.
    #[test]
    fn distinct_queries_have_distinct_scopes(
        a in query_strategy(),
        b in query_strategy(),
    ) {
        prop_assert_eq!(a == b, a.scope() == b.scope());
    }
}

fn query_strategy() -> impl Strategy<Value = PageQuery> {
    (
        proptest::option::of("[a-c]{1,3}"),
        proptest::option::of(0_usize..4),
        proptest::option::of(0_i64..4),
        proptest::option::of(0_u32..3),
        1_u32..=4,
    )
        .prop_map(|(actor, action, from, nanos, limit)| {
            let actions = [Action::Login, Action::Logout, Action::Query, Action::Denial];
            let from = from.map(|secs| {
                DateTime::<Utc>::from_timestamp(1_800_000_000 + secs, nanos.unwrap_or(0)).unwrap()
            });
            PageQuery::new(
                Filters {
                    actor: actor.map(|actor| actor.parse().unwrap()),
                    action: action.map(|i| actions[i]),
                    from,
                    to: None,
                },
                Limit::new(limit).unwrap(),
            )
            .unwrap()
        })
}

#[test]
fn a_page_reads_one_row_ahead_to_know_whether_more_remain() {
    let (events, last) = assemble(Vec::new(), 3).unwrap();
    assert_eq!(events, Vec::<Event>::new());
    assert_eq!(last, None, "an empty page has no next cursor");
    let (events, last) = assemble((1..=3).map(row).collect(), 3).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(last, None, "a full final page has no next cursor");
    let (events, last) = assemble((1..=4).map(row).collect(), 3).unwrap();
    assert_eq!(
        events
            .iter()
            .map(|e| e.event_id.as_str())
            .collect::<Vec<_>>(),
        ["event-1", "event-2", "event-3"]
    );
    assert_eq!(last, Some(3));
}

#[test]
fn an_invalid_or_incompatible_stored_event_fails_the_page() {
    let valid = row(1).1;
    let mut newer: serde_json::Value = serde_json::from_str(&valid).unwrap();
    newer["schema_version"] = 2.into();
    let mut extra = newer.clone();
    extra["schema_version"] = 1.into();
    extra["email"] = "person@example.test".into();
    let mut bad_trace = extra.clone();
    bad_trace.as_object_mut().unwrap().remove("email");
    bad_trace["trace_id"] = "0".repeat(32).into();
    for payload in [
        "not json".to_owned(),
        "{}".to_owned(),
        newer.to_string(),
        extra.to_string(),
        bad_trace.to_string(),
    ] {
        let rows = vec![(1, valid.clone()), (2, payload.clone())];
        assert_eq!(
            assemble(rows, 5).unwrap_err(),
            PageError::InvalidStoredEvent,
            "{payload}"
        );
    }
    // The look-ahead row is never parsed: it is not part of the page.
    let rows = vec![(1, valid), (2, "not json".to_owned())];
    assert_eq!(assemble(rows, 1).unwrap().0.len(), 1);
}

fn encoded_len(events: &[Event], cursor: Option<&str>) -> usize {
    encode::page(events, cursor, usize::MAX).unwrap().len()
}

#[test]
fn the_page_budget_fits_exactly_and_refuses_one_byte_more() {
    let events: Vec<Event> = (1..=3)
        .map(|s| event(s, "actor-a", Action::Query))
        .collect();
    let token = CursorKey::fixed(KEY).seal(position(3, 7), &query(None, None, 3).scope());
    let exact = encoded_len(&events, Some(&token));
    let json = encode::page(&events, Some(&token), exact).unwrap();
    assert_eq!(json.len(), exact);
    // The oracle: the same body serialized without any bound.
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["next_cursor"], token.as_str());
    assert_eq!(value["events"].as_array().unwrap().len(), 3);
    assert_eq!(
        serde_json::from_value::<Vec<Event>>(value["events"].clone()).unwrap(),
        events
    );
    assert_eq!(
        encode::page(&events, Some(&token), exact - 1),
        Err(PageError::BudgetExceeded)
    );
    // The token counts: the same events without it fit in fewer bytes.
    assert!(encoded_len(&events, None) < exact);
    assert_eq!(
        encode::page(&[], None, usize::MAX).unwrap(),
        r#"{"events":[],"next_cursor":null}"#
    );
}

#[test]
fn the_writer_holds_exactly_the_v1_budget_and_never_more() {
    let limit = Profile::V1.page_bytes();
    let mut out = Bounded::new(limit);
    let chunk = vec![b'x'; 1024];
    for _ in 0..limit / 1024 {
        out.write_all(&chunk).unwrap();
    }
    assert!(out.write_all(b"y").is_err(), "one byte past 4 MiB");
    let mut at_once = Bounded::new(limit);
    assert!(at_once.write_all(&vec![b'x'; limit + 1]).is_err());
    assert!(at_once.write_all(&vec![b'x'; limit]).is_ok());
}

proptest! {
    /// Any budget either holds the whole page, byte for byte what an
    /// unbounded encoder writes, or refuses it.
    #[test]
    fn a_page_fits_its_budget_whole_or_not_at_all(
        count in 0_u64..6,
        with_cursor in any::<bool>(),
        slack in -64_i64..64,
    ) {
        let events: Vec<Event> = (1..=count).map(|s| event(s, "actor-a", Action::Login)).collect();
        let token = CursorKey::fixed(KEY).seal(position(1, 9), &query(None, None, 3).scope());
        let cursor = with_cursor.then_some(token.as_str());
        let full = encode::page(&events, cursor, usize::MAX).unwrap();
        let budget = usize::try_from((i64::try_from(full.len()).unwrap() + slack).max(0)).unwrap();
        match encode::page(&events, cursor, budget) {
            Ok(json) => {
                prop_assert!(budget >= full.len());
                prop_assert_eq!(json, full);
            }
            Err(error) => {
                prop_assert!(budget < full.len());
                prop_assert_eq!(error, PageError::BudgetExceeded);
            }
        }
    }
}

#[test]
fn the_statements_bind_every_value_and_never_skip_rows_by_offset() {
    for sql in [SQL_ALL, SQL_ACTOR, SQL_ACTION, SQL_ACTOR_ACTION] {
        assert!(!sql.to_ascii_uppercase().contains("OFFSET"), "{sql}");
        assert!(sql.ends_with("ORDER BY sequence LIMIT $3"), "{sql}");
        assert!(sql.contains("sequence > $1 AND sequence <= $2"), "{sql}");
        assert!(!sql.contains("FOR UPDATE") && !sql.contains("FOR SHARE"));
    }
    // The index expressions of migration 0003, verbatim.
    assert!(SQL_ACTOR.contains("WHERE (payload::jsonb)->>'actor_id' = $6 AND"));
    assert!(SQL_ACTION.contains("WHERE (payload::jsonb)->>'action' = $6 AND"));
    assert!(SQL_ACTOR_ACTION.contains("'actor_id' = $6 AND (payload::jsonb)->>'action' = $7"));
    assert!(!SQL_ALL.contains("$6"));
}

/// A chain in memory, read as the statements do: after `last`, at or below
/// `upper`, matching the filters, in order, `limit + 1` rows at most.
struct Model {
    events: Vec<(u64, String, Action)>,
}

impl Model {
    fn append(&mut self, actor: &str, action: Action) {
        let sequence = self.events.len() as u64 + 1;
        self.events.push((sequence, actor.to_owned(), action));
    }

    fn matches(query: &PageQuery, actor: &str, action: Action) -> bool {
        query.filters.actor.as_ref().is_none_or(|a| a.0 == actor)
            && query.filters.action.is_none_or(|a| a == action)
    }

    fn rows(&self, query: &PageQuery, last: u64, upper: u64) -> Vec<(u64, String)> {
        self.events
            .iter()
            .filter(|(s, actor, action)| {
                *s > last && *s <= upper && Self::matches(query, actor, *action)
            })
            .take(usize::from(query.limit.get()) + 1)
            .map(|(s, actor, action)| (*s, event(*s, actor, *action).canonical_json().unwrap()))
            .collect()
    }

    /// One page as the repository serves it, without PostgreSQL.
    fn page(
        &self,
        key: &CursorKey,
        query: &PageQuery,
        cursor: Option<&str>,
    ) -> Result<(Vec<u64>, Option<String>), PageError> {
        let scope = query.scope();
        let (last, upper, expires_ms) = match cursor {
            None => (0, self.events.len() as u64, NOW + 900_000),
            Some(token) => {
                let p = key.open(token, &scope, NOW)?;
                (p.last, p.upper, p.expires_ms)
            }
        };
        let (events, next) = assemble(
            self.rows(query, last, upper),
            usize::from(query.limit.get()),
        )?;
        let sequences = events
            .iter()
            .map(|e| e.event_id["event-".len()..].parse().unwrap())
            .collect();
        let token = next.map(|last| {
            key.seal(
                Position {
                    last,
                    upper,
                    expires_ms,
                },
                &scope,
            )
        });
        Ok((sequences, token))
    }
}

proptest! {
    /// Any walk visits each event it selects exactly once, in order, up to
    /// the head captured by its first page, whatever is appended between
    /// pages; empty and final pages carry no cursor.
    #[test]
    fn any_walk_visits_each_selected_event_once_in_order(
        initial in proptest::collection::vec((0_usize..3, 0_usize..4), 0..40),
        appended in proptest::collection::vec((0_usize..3, 0_usize..4), 0..10),
        actor in proptest::option::of(0_usize..3),
        action in proptest::option::of(0_usize..4),
        limit in 1_u32..=8,
    ) {
        let actors = ["actor-a", "actor-b", "actor-c"];
        let actions = [Action::Login, Action::Logout, Action::Query, Action::Denial];
        let mut model = Model { events: Vec::new() };
        for (a, b) in &initial {
            model.append(actors[*a], actions[*b]);
        }
        let query = query(actor.map(|a| actors[a]), action.map(|b| actions[b]), limit);
        let expected: Vec<u64> = model
            .events
            .iter()
            .filter(|(_, a, b)| Model::matches(&query, a, *b))
            .map(|(s, _, _)| *s)
            .collect();
        let key = CursorKey::fixed(KEY);
        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pending = appended.iter();
        loop {
            let (page, next) = model.page(&key, &query, cursor.as_deref()).unwrap();
            prop_assert!(page.len() <= usize::try_from(limit).unwrap());
            if next.is_some() {
                prop_assert_eq!(page.len(), usize::try_from(limit).unwrap());
            }
            seen.extend(page);
            let Some(next) = next else { break };
            cursor = Some(next);
            // Appended between pages, under a captured bound.
            if let Some((a, b)) = pending.next() {
                model.append(actors[*a], actions[*b]);
            }
        }
        prop_assert_eq!(seen, expected);
    }
}
