//! The public context fingerprint: the HMAC itself, what it separates, the
//! instance without entropy, and explain on real row policies.

use std::collections::BTreeMap;

use hmac::{Hmac, KeyInit, Mac};
use proptest::prelude::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::*;
use crate::AppState;
use crate::auth::{DataScope, Principal, Session, User};
use crate::oidc::FederatedIdentity;
use crate::semantic::Outcome;
use crate::semantic::prepare::tests::composition::{
    ROW_POLICY, analytics, metric, model, project, request, state,
};
use sef_model_ir::types::Literal;

const KEY: [u8; 32] = [7; 32];

fn context(slots: &[(&str, Literal)]) -> SemanticContext {
    SemanticContext {
        slots: slots
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn scope<'a>(project: &'a str, principal: &'a str) -> Scope<'a> {
    Scope { project, principal }
}

/// What `main` published before #204: a plain SHA-256 of the values.
fn published_before(slots: &[(&str, Literal)]) -> String {
    let mut hasher = Sha256::new();
    for (name, value) in slots {
        for part in [(*name).to_owned(), format!("{value:?}")] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
    }
    format!("sha256:{}", hex(&hasher.finalize()))
}

fn norte() -> Literal {
    Literal::Utf8("Norte".to_owned())
}

/// RFC 4231, test cases 1 and 2: the HMAC this module uses is the standard.
#[test]
fn the_mac_is_hmac_sha256() {
    for (key, data, expected) in [
        (
            vec![0x0b; 20],
            b"Hi There".to_vec(),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
        ),
        (
            b"Jefe".to_vec(),
            b"what do ya want for nothing?".to_vec(),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
        ),
    ] {
        let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
        mac.update(&data);
        assert_eq!(hex(&mac.finalize().into_bytes()), expected);
    }
}

/// The construction, pinned: domain, canonical encoding and prefix. A
/// change here changes every published fingerprint, so it is deliberate.
#[test]
fn the_fingerprint_is_pinned() {
    let key = ContextKey::fixed(KEY);
    let value = key
        .fingerprint(&scope("p", "ana"), &context(&[("region", norte())]))
        .unwrap();
    assert_eq!(value, PINNED);
    // It is the HMAC of the domain and the canonical record, nothing else.
    let encoded = C::Map(vec![
        ("project".to_owned(), C::text("p")),
        ("principal".to_owned(), C::text("ana")),
        (
            "slots".to_owned(),
            C::Map(vec![("region".to_owned(), norte().canonical_value())]),
        ),
    ])
    .encode()
    .unwrap();
    let mut mac = Hmac::<Sha256>::new_from_slice(&KEY).unwrap();
    mac.update(b"bi-explain-context-v1");
    mac.update(&encoded);
    assert_eq!(
        value,
        format!("ctx-hmac-sha256-v1:{}", hex(&mac.finalize().into_bytes()))
    );
    // Without the domain it would be another value.
    let mut bare = Hmac::<Sha256>::new_from_slice(&KEY).unwrap();
    bare.update(&encoded);
    assert_ne!(
        value,
        format!("ctx-hmac-sha256-v1:{}", hex(&bare.finalize().into_bytes()))
    );
}

const PINNED: &str =
    "ctx-hmac-sha256-v1:1b0ac92492f50573124c8e457ea2c340e4fbd1c159c0cd182b5c9c164645ec2f";

/// The value a dictionary of `{Norte, Sur}` recovered from the old SHA is
/// not what is published now, and nothing of it is.
#[test]
fn the_old_plain_hash_is_never_published() {
    let key = ContextKey::fixed(KEY);
    for region in ["Norte", "Sur"] {
        let slots = [("region", Literal::Utf8(region.to_owned()))];
        let value = key
            .fingerprint(&scope("p", "ana"), &context(&slots))
            .unwrap();
        let published = value.strip_prefix(PREFIX).unwrap();
        assert!(!value.contains(region));
        // Whoever lacks the key can only hash guesses: with the old
        // construction, or plainly over the very record the HMAC covers.
        // No guess reaches the published value, the right one included.
        for guess in ["Norte", "Sur"] {
            let guessed = [("region", Literal::Utf8(guess.to_owned()))];
            let old = published_before(&guessed);
            assert_ne!(published, old.strip_prefix("sha256:").unwrap());
            let record = C::Map(vec![
                ("project".to_owned(), C::text("p")),
                ("principal".to_owned(), C::text("ana")),
                (
                    "slots".to_owned(),
                    C::Map(vec![(
                        "region".to_owned(),
                        Literal::Utf8(guess.to_owned()).canonical_value(),
                    )]),
                ),
            ])
            .encode()
            .unwrap();
            for keyless in [
                Sha256::new().chain_update(&record).finalize(),
                Sha256::new()
                    .chain_update(b"bi-explain-context-v1")
                    .chain_update(&record)
                    .finalize(),
            ] {
                assert_ne!(published, hex(&keyless), "{guess}");
            }
        }
    }
}

#[test]
fn without_entropy_there_is_no_key_and_no_fingerprint() {
    let key = ContextKey::from_entropy(|_| Err(getrandom::Error::UNSUPPORTED));
    assert!(!key.is_available());
    assert_eq!(
        key.fingerprint(&scope("p", "ana"), &context(&[("region", norte())])),
        None
    );
    assert_eq!(format!("{key:?}"), "ContextKey(unavailable)");
    // A key from entropy is available, and never printed.
    let real = ContextKey::from_entropy(|bytes| {
        bytes.fill(0xab);
        Ok(())
    });
    assert_eq!(format!("{real:?}"), "ContextKey(available)");
    assert!(ContextKey::generate().is_available());
}

fn literal() -> impl Strategy<Value = Literal> {
    prop_oneof![
        "[a-zA-Z0-9 ñ]{0,8}".prop_map(Literal::Utf8),
        any::<bool>().prop_map(Literal::Boolean),
        any::<i64>().prop_map(|n| Literal::SignedInteger(n.into())),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Equal inputs give equal fingerprints; changing the project, the
    /// principal, a value, its type, a slot's name or the key changes it.
    #[test]
    fn the_fingerprint_separates_everything_it_covers(
        value in literal(),
        other in literal(),
        name in "[a-z]{1,6}",
        project in "[a-z/]{1,8}",
        principal in "[a-z]{1,8}",
        key in any::<[u8; 32]>(),
        other_key in any::<[u8; 32]>(),
    ) {
        let slots = [(name.as_str(), value.clone())];
        let k = ContextKey::fixed(key);
        let at = |k: &ContextKey, project: &str, principal: &str, slots: &[(&str, Literal)]| {
            k.fingerprint(&scope(project, principal), &context(slots)).unwrap()
        };
        let base = at(&k, &project, &principal, &slots);
        prop_assert_eq!(&base, &at(&k, &project, &principal, &slots));
        prop_assert!(base.starts_with(PREFIX));
        prop_assert_ne!(&base, &at(&k, &format!("{project}x"), &principal, &slots));
        prop_assert_ne!(&base, &at(&k, &project, &format!("{principal}x"), &slots));
        prop_assert_ne!(&base, &at(&k, &project, &principal, &[(&format!("{name}x"), value.clone())]));
        prop_assert_ne!(&base, &at(&k, &project, &principal, &[]));
        if other != value {
            prop_assert_ne!(&base, &at(&k, &project, &principal, &[(name.as_str(), other)]));
        }
        if other_key != key {
            prop_assert_ne!(&base, &at(&ContextKey::fixed(other_key), &project, &principal, &slots));
        }
    }
}

/// The same digits as text and as an integer are different values.
#[test]
fn a_value_of_another_type_is_another_context() {
    let key = ContextKey::fixed(KEY);
    let text = context(&[("n", Literal::Utf8("1".to_owned()))]);
    let integer = context(&[("n", Literal::SignedInteger(1))]);
    let at = |c| key.fingerprint(&scope("p", "ana"), c).unwrap();
    assert_ne!(at(&text), at(&integer));
    // The order slots were bound in does not matter: the record is canonical.
    let mut forward = SemanticContext::default();
    forward.slots.insert("a".to_owned(), Literal::Boolean(true));
    forward.slots.insert("b".to_owned(), norte());
    let mut backward = SemanticContext::default();
    backward.slots.insert("b".to_owned(), norte());
    backward
        .slots
        .insert("a".to_owned(), Literal::Boolean(true));
    assert_eq!(at(&forward), at(&backward));
}

/// A principal verified by the provider, with its row-policy attributes.
fn principal(user: &str, region: &str) -> Principal {
    Principal::federated(
        Session {
            user: User {
                id: user.to_owned(),
                display_name: user.to_owned(),
            },
            data_scope: DataScope::Synthetic,
            expires_at: String::new(),
        },
        FederatedIdentity {
            version: 1,
            issuer: "https://issuer.invalid".to_owned(),
            subject: user.to_owned(),
            groups: Vec::new(),
            attributes: BTreeMap::from([("region".to_owned(), json!(region))]),
        },
    )
}

const MEMBERS: &str = "version: 1\nusers:\n  ana: [viewer]\n  beto: [viewer]\n";

/// Explain over a real row policy: what it publishes of each context.
async fn explained(state: &AppState, who: &Principal) -> Value {
    let query = json!({ "selections": [metric("t", "importe_total")] });
    let Outcome::Explained { explanation, .. } =
        crate::semantic::run(state, who, request(&query)).await
    else {
        panic!("explained");
    };
    serde_json::to_value(&explanation).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn explain_publishes_only_the_protected_fingerprint() {
    let p = project(&model(ROW_POLICY, ""), MEMBERS);
    let state = AppState {
        context_key: ContextKey::fixed(KEY),
        ..state(p.path(), analytics().await)
    };
    let ana_norte = explained(&state, &principal("ana", "Norte")).await;
    let text = ana_norte.to_string();
    let receipt = &ana_norte["receipt"];
    let old = published_before(&[("region", norte())]);
    // Neither the value nor the old hash, in any field or in the ID.
    assert!(!text.contains("Norte"), "{text}");
    assert!(!text.contains(&old[7..]), "{text}");
    assert!(receipt["context"].as_str().unwrap().starts_with(PREFIX));
    // Same project, principal and values: the same fingerprint and receipt.
    let again = explained(&state, &principal("ana", "Norte")).await;
    assert_eq!(again["receipt"], *receipt);
    // Another value, or another principal with the same value: another one,
    // and another receipt ID.
    let ana_sur = explained(&state, &principal("ana", "Sur")).await;
    let beto_norte = explained(&state, &principal("beto", "Norte")).await;
    for other in [&ana_sur, &beto_norte] {
        assert_ne!(other["receipt"]["context"], receipt["context"]);
        assert_ne!(other["receipt"]["id"], receipt["id"]);
        // Only the context, and the ID that covers it, differ.
        for part in ["query", "model", "plan", "policies"] {
            assert_eq!(other["receipt"][part], receipt[part], "{part}");
        }
    }
    // Nor does the decision carry the value: the same principal decides the
    // same for Norte and Sur.
    assert_eq!(ana_sur["receipt"]["decision"], receipt["decision"]);
    // Another key: another fingerprint for the same everything.
    let restarted = AppState {
        context_key: ContextKey::fixed([8; 32]),
        ..state.clone()
    };
    let after = explained(&restarted, &principal("ana", "Norte")).await;
    assert_ne!(after["receipt"]["context"], receipt["context"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_key_explain_is_unavailable_before_reading_anything() {
    let p = project(&model(ROW_POLICY, ""), MEMBERS);
    let state = AppState {
        context_key: ContextKey::from_entropy(|_| Err(getrandom::Error::UNSUPPORTED)),
        // No engine either: had it read anything, it would say so instead.
        ..state(p.path(), crate::Analytics::disabled())
    };
    let query = json!({ "selections": [metric("t", "importe_total")] });
    let outcome = crate::semantic::run(&state, &principal("ana", "Norte"), request(&query)).await;
    let Outcome::Failed(status, code, _) = outcome else {
        panic!("unavailable");
    };
    assert_eq!(
        (status, code),
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "explain_unavailable"
        )
    );
}
