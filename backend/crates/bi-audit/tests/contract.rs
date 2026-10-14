use bi_audit::{
    Action, ActorType, Checkpoint, Error, Event, GENESIS, MetadataRedacted, QueryShape, ResultCode,
    Source, Verifier, event_hash,
};
use proptest::prelude::*;

fn event(id: &str) -> Event {
    Event {
        event_id: id.to_owned(),
        occurred_at: "2026-10-11T12:00:00Z".parse().unwrap(),
        actor_id: "user-1".into(),
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

#[test]
fn v1_serialization_and_hash_match_persisted_golden_vector() {
    // SHA-256 independently computed with Python hashlib. This fixture protects
    // existing histories from accidental framing/serialization changes.
    let payload = r#"{"event_id":"event-1","occurred_at":"2026-10-11T12:00:00Z","actor_id":"user-1","actor_type":"user","session_id":"session-1","action":"query","resource_type":"dataset","resource_id":"sales","result":"success","request_id":"request-1","trace_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","policy_decision_id":null,"source":"http","metadata_redacted":{"query_shape":null},"schema_version":1}"#;
    assert_eq!(event("event-1").canonical_json().unwrap(), payload);
    assert_eq!(
        event_hash(1, &GENESIS, payload),
        [
            0x84, 0x0d, 0xb5, 0xce, 0xb9, 0xe5, 0x5d, 0x47, 0x26, 0x8f, 0x3b, 0x2f, 0xfc, 0xe2,
            0x75, 0x21, 0x95, 0x17, 0x29, 0xce, 0xfb, 0xe3, 0xa3, 0xe9, 0x19, 0x62, 0x43, 0x24,
            0x83, 0xa3, 0x7a, 0xbf,
        ]
    );
}

#[test]
fn all_fields_are_required_and_version_is_checked() {
    let value = serde_json::to_value(event("event-1")).unwrap();
    for key in value.as_object().unwrap().keys() {
        let mut invalid = value.clone();
        invalid.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<Event>(invalid).is_err(),
            "missing {key}"
        );
    }
    let mut unsupported = event("event-1");
    unsupported.schema_version = 2;
    assert_eq!(unsupported.canonical_json(), Err(Error::InvalidEvent));
    let mut missing = event("event-1");
    missing.actor_id.clear();
    assert_eq!(missing.validate(), Err(Error::InvalidEvent));
}

#[test]
fn every_field_is_covered_by_the_hash() {
    let original = event("event-1").canonical_json().unwrap();
    let hash = event_hash(1, &GENESIS, &original);
    let value: serde_json::Value = serde_json::from_str(&original).unwrap();
    for key in value.as_object().unwrap().keys() {
        let mut modified = value.clone();
        modified[key] = serde_json::json!("changed");
        assert_eq!(
            Verifier::default().push(1, &modified.to_string(), &GENESIS, &hash),
            Err(Error::HashMismatch),
            "changed {key}"
        );
    }
}

#[test]
fn metadata_rejects_secrets_pii_and_arbitrary_text() {
    for key in [
        "password",
        "authorization",
        "cookie",
        "email",
        "name",
        "sql",
        "filter_values",
        "error",
    ] {
        let mut value = serde_json::to_value(event("event-1")).unwrap();
        value["metadata_redacted"][key] = serde_json::json!("private@example.test");
        assert!(serde_json::from_value::<Event>(value).is_err());
    }
    let mut value = serde_json::to_value(event("event-1")).unwrap();
    value["metadata_redacted"]["query_shape"] =
        serde_json::json!({"dimensions": "secret", "measures": 1, "filters": 1, "limit": 10});
    assert!(serde_json::from_value::<Event>(value).is_err());
}

#[test]
fn framing_prevents_sequence_and_link_substitution() {
    let payload = event("event-1").canonical_json().unwrap();
    assert_ne!(
        event_hash(1, &GENESIS, &payload),
        event_hash(2, &GENESIS, &payload)
    );
    assert_ne!(
        event_hash(1, &GENESIS, &payload),
        event_hash(1, &[1; 32], &payload)
    );
    assert_eq!(
        Verifier::default().push(2, &payload, &GENESIS, &event_hash(2, &GENESIS, &payload)),
        Err(Error::SequenceMismatch)
    );
    assert_eq!(
        Verifier::default().push(1, &payload, &[1; 32], &event_hash(1, &[1; 32], &payload)),
        Err(Error::PreviousHashMismatch)
    );
}

#[test]
fn a_recomputed_hash_over_invalid_content_advances_the_chain_but_is_counted() {
    // Cryptographic integrity and content validity are independent: a row
    // can be exactly what was stored (correct hash) while its content is
    // not a well-formed Event (written before this contract existed, or by
    // a path that skipped validation). The chain still advances past it, so
    // a later cryptographic break is not hidden behind an earlier defect.
    for payload in [
        "{}".to_owned(),
        format!(" {}", event("event-1").canonical_json().unwrap()),
    ] {
        let mut verifier = Verifier::default();
        let hash = event_hash(1, &GENESIS, &payload);
        assert_eq!(verifier.push(1, &payload, &GENESIS, &hash), Ok(()));
        assert_eq!(verifier.invalid_content_count(), 1);
        assert_eq!(verifier.first_invalid_content_sequence(), Some(1));
        assert_eq!(verifier.checkpoint(), Checkpoint { sequence: 1, hash });
        assert_eq!(
            verifier.finish(Checkpoint { sequence: 1, hash }),
            Ok(Checkpoint { sequence: 1, hash })
        );
    }
    assert_eq!(
        Verifier::default().finish(Checkpoint::default()),
        Ok(Checkpoint::default())
    );
}

#[test]
fn a_later_cryptographic_break_is_detected_past_earlier_invalid_content() {
    let canonical = event("event-1").canonical_json().unwrap();
    let mut rows = Vec::new();
    let mut previous = GENESIS;
    // #1 valid content, #2 invalid content (correct hash), #3 valid content
    // linked to #2.
    for payload in [canonical.clone(), "{}".to_owned(), canonical] {
        let sequence = u64::try_from(rows.len()).unwrap() + 1;
        let hash = event_hash(sequence, &previous, &payload);
        rows.push((sequence, payload, previous, hash));
        previous = hash;
    }
    let mut verifier = Verifier::default();
    for (sequence, payload, previous, hash) in &rows {
        assert_eq!(verifier.push(*sequence, payload, previous, hash), Ok(()));
    }
    assert_eq!(verifier.invalid_content_count(), 1);
    assert_eq!(verifier.first_invalid_content_sequence(), Some(2));
    let head = Checkpoint {
        sequence: 3,
        hash: previous,
    };
    assert_eq!(verifier.finish(head), Ok(head));

    // Now corrupt #3's stored bytes without recomputing its hash: the chain
    // breaks exactly there, independently of #2's content defect.
    let mut tampered = rows.clone();
    tampered[2].1 = "{}".to_owned();
    let mut verifier = Verifier::default();
    let mut stopped_at = None;
    for (sequence, payload, previous, hash) in &tampered {
        if verifier.push(*sequence, payload, previous, hash).is_err() {
            stopped_at = Some(*sequence);
            break;
        }
    }
    assert_eq!(stopped_at, Some(3));
    // The content defect at #2 was still recorded before the break at #3.
    assert_eq!(verifier.invalid_content_count(), 1);
    assert_eq!(verifier.first_invalid_content_sequence(), Some(2));
}

proptest! {
    #[test]
    fn valid_histories_verify_and_deletion_or_insertion_is_detected(ids in prop::collection::vec("[a-z][a-z0-9]{0,15}", 1..50), removed in any::<usize>()) {
        let mut previous = GENESIS;
        let rows: Vec<_> = ids.iter().enumerate().map(|(index, id)| {
            let sequence = u64::try_from(index).unwrap() + 1;
            let payload = event(id).canonical_json().unwrap();
            let hash = event_hash(sequence, &previous, &payload);
            let row = (sequence, payload, previous, hash);
            previous = hash;
            row
        }).collect();
        let head = Checkpoint { sequence: u64::try_from(rows.len()).unwrap(), hash: previous };
        let mut valid = Verifier::default();
        for (sequence, payload, previous, hash) in &rows {
            prop_assert_eq!(valid.push(*sequence, payload, previous, hash), Ok(()));
        }
        prop_assert_eq!(valid.finish(head), Ok(head));
        let removed = removed % rows.len();
        let mut broken = Verifier::default();
        let mut failed = false;
        for (index, (sequence, payload, previous, hash)) in rows.iter().enumerate() {
            if index == removed { continue; }
            if broken.push(*sequence, payload, previous, hash).is_err() { failed = true; break; }
        }
        prop_assert!(failed || broken.finish(head).is_err());
        let mut injected = Verifier::default();
        let row = &rows[0];
        prop_assert_eq!(injected.push(row.0, &row.1, &row.2, &row.3), Ok(()));
        prop_assert!(injected.push(row.0, &row.1, &row.2, &row.3).is_err());
    }

    #[test]
    fn query_metadata_contains_only_numeric_shape(dimensions in any::<u32>(), measures in any::<u32>(), filters in any::<u32>(), limit in any::<u32>()) {
        let metadata = MetadataRedacted { query_shape: Some(QueryShape { dimensions, measures, filters, limit }) };
        let value = serde_json::to_value(metadata).unwrap();
        prop_assert!(value["query_shape"].as_object().unwrap().values().all(serde_json::Value::is_u64));
    }
}
