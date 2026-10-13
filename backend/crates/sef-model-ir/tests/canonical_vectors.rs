use std::str::FromStr;

use proptest::prelude::*;
use sef_model_ir::canonical::{
    Canonical, CanonicalError, CanonicalValue as C, Domain, Fingerprint, MAX_CANONICAL_DEPTH,
    MAX_CANONICAL_NODES, MAX_CANONICAL_PAYLOAD_BYTES, fingerprint, fingerprint_bytes,
};
use sef_model_ir::identity::{ObjectIdentity, SemanticVersion, StableId};
use sef_model_ir::model::{KeyBinding, ObjectMetadata};
use sef_model_ir::relationships::ContractEvidenceState;
use sef_model_ir::types::{DecimalType, DecimalValue};
use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                char::from(DIGITS[usize::from(byte >> 4)]),
                char::from(DIGITS[usize::from(byte & 0xf)]),
            ]
        })
        .collect()
}

// Bytes were derived from RFC 8949, digests independently with .NET SHA256.
fn typed_value_vectors() -> [(C, &'static str, &'static str); 15] {
    [
        (
            C::Null,
            "8100",
            "9037f92bc91fdcc764a89d9144c341d887ffc83351cc8cf0530555c89920a47d",
        ),
        (
            C::Bool(true),
            "8201f5",
            "18f2627553e5578fa1d8ae10bbb73bee3353bf1287c5072bd95cb32e87dea928",
        ),
        (
            C::Signed(0),
            "820200",
            "95397da2338a6e41e75d2a34be802255e05cb6e7d79088fcd6a0d9f99b344681",
        ),
        (
            C::Unsigned(0),
            "820300",
            "c1446d769d6d013454d8705a7e9fee7a3102f17248e22fa611b6ac49baf00df0",
        ),
        (
            C::Signed(-24),
            "820237",
            "23ac1804f253cf5325a174ff777429343aaa19fe6245b528b154090e5f75df4a",
        ),
        (
            C::Signed(i64::MIN),
            "82023b7fffffffffffffff",
            "24c0428e50aca7d4cf79e757e1b12fd680b4bda6b0f31467cba6182aab5d4ee5",
        ),
        (
            C::Unsigned(u64::MAX),
            "82031bffffffffffffffff",
            "91cc259cc671bf9cc777986f9d733b87e4a4477c849371434fa985f0199c0400",
        ),
        (
            C::Bytes(vec![0, 1]),
            "8205420001",
            "0c2e0bbcbb8f07f1f9b9c25d6de59f5f94d18ad93e8afc21a4c00f7d44f2dd18",
        ),
        (
            C::text("\u{e9}"),
            "820662c3a9",
            "6ddcc935b353f6db0eae874b43f9d92acff124713da2da3b8453ec98679b9c8a",
        ),
        (
            C::sequence([0_i64, 1]),
            "820782820200820201",
            "2c881897b5391bb0411d48121893360732492c0f257117a22068083e2c6f7936",
        ),
        (
            C::set([0_i64, 1, 0]),
            "820882820201820200",
            "6b3647c2343afa7c95c6e2360a6951c2df9141c23fad3625216950261c14b1e1",
        ),
        (
            C::Map(vec![
                ("aa".into(), C::Signed(2)),
                ("b".into(), C::Signed(1)),
            ]),
            "8209a26162820201626161820202",
            "5162ad841658fcda07b2405003c23035e98f1eeff6ef2c04d9872a3682f3f5ec",
        ),
        (
            C::Absent,
            "810b",
            "e92f5db43e501345e6b4b255bc299df49d038356f6244f5f51ecf58697f89de4",
        ),
        (
            C::Present(Box::new(C::Null)),
            "820c8100",
            "a573f6611f38841de51c63e58cb9f90d6865d310a1a5f0523521d8db7408c96a",
        ),
        (
            C::record("csmir.Empty", Vec::<(String, C)>::new()),
            "830a6b63736d69722e456d707479a0",
            "dc570052840e7123aa6def574579e13061a78fafed865b1b8fda8565255a05e3",
        ),
    ]
}

fn numeric_value_vectors() -> [(C, &'static str, &'static str); 5] {
    [
        (
            C::Float(1.5),
            "8204f93e00",
            "dd82f0824a7dcf6140fa9b7a9089a0f68deaa7a462c737802ed8a70fef3e23cb",
        ),
        (
            C::Float(65_504.0),
            "8204f97bff",
            "9eb7761de66fff41d76bb2f4000d5410dccf460edb332e38bb3af38585922051",
        ),
        (
            C::Float(100_000.0),
            "8204fa47c35000",
            "ec98687d96decfe24623bd2cac53f78678867b2e98530d861125e9a337eca7f2",
        ),
        (
            C::Float(1.1),
            "8204fb3ff199999999999a",
            "be04a81aec5439e8d0f6c9d61801dc1230d1e4276e50e384ee161a39fec8be04",
        ),
        (
            C::record(
                "csmir.Decimal",
                [
                    ("coefficient", C::text("1234")),
                    ("precision", C::Unsigned(4)),
                    ("scale", C::Unsigned(2)),
                ],
            ),
            "830a6d63736d69722e446563696d616ca3657363616c6582030269707265636973696f6e8203046b636f656666696369656e7482066431323334",
            "a37b7f3e31290afc81791bf9fb5c75cc614865a40956e25d1d293372a87f6b15",
        ),
    ]
}

#[test]
fn typed_value_golden_bytes_and_independent_sha256() -> Result<(), CanonicalError> {
    for (value, expected_bytes, expected_hash) in typed_value_vectors()
        .into_iter()
        .chain(numeric_value_vectors())
    {
        let bytes = value.encode()?;
        assert_eq!(hex(&bytes), expected_bytes, "{value:?}");
        assert_eq!(hex(&Sha256::digest(bytes)), expected_hash);
    }
    Ok(())
}

#[test]
fn domain_envelope_golden_vectors() -> Result<(), CanonicalError> {
    for (domain, expected_bytes, expected_hash) in [
        (
            Domain::Csmir,
            "86767365662d63616e6f6e6963616c2d76616c75652d7631737365662d6463626f722d7368613235362d76316543534d495263302e3163302e318100",
            "ba2e3af3f16f5cd246769aaa2e05f89234f6db6cea414cb110c2dfd0a2260e33",
        ),
        (
            Domain::Sqir,
            "86767365662d63616e6f6e6963616c2d76616c75652d7631737365662d6463626f722d7368613235362d7631645351495263302e3163302e318100",
            "fa8825bcc4f502bf9c7cf6c1736cd72026ba111627a1ce120eeb57327ac2ef57",
        ),
        (
            Domain::Seir,
            "86767365662d63616e6f6e6963616c2d76616c75652d7631737365662d6463626f722d7368613235362d7631645345495263302e3163302e318100",
            "3ff60aa698adbceca20b5c3c8d186a7e8bf9c3d53b636817c02ef42067dcc510",
        ),
    ] {
        assert_eq!(hex(&fingerprint_bytes(domain, &C::Null)?), expected_bytes);
        let expected = format!("sef-sha256-v1:{expected_hash}");
        let actual = fingerprint(domain, &C::Null)?;
        assert_eq!(actual.to_string(), expected);
        assert_eq!(Fingerprint::from_str(&expected)?, actual);
    }
    Ok(())
}

#[test]
fn rfc8949_preferred_integer_and_float_widths() -> Result<(), CanonicalError> {
    for (number, expected) in [
        (23, "820317"),
        (24, "82031818"),
        (255, "820318ff"),
        (256, "8203190100"),
        (65_535, "820319ffff"),
        (65_536, "82031a00010000"),
        (4_294_967_296, "82031b0000000100000000"),
    ] {
        assert_eq!(hex(&C::Unsigned(number).encode()?), expected);
    }
    assert_eq!(hex(&C::Float(0.0).encode()?), "8204f90000");
    assert_eq!(hex(&C::Float(-0.0).encode()?), "8204f90000");
    assert_eq!(
        hex(&C::Float(5.960_464_477_539_063e-8).encode()?),
        "8204f90001"
    );
    Ok(())
}

#[test]
fn absence_null_empty_and_union_cases_are_distinct() -> Result<(), CanonicalError> {
    let values = [
        C::Null,
        C::Absent,
        C::Present(Box::new(C::Null)),
        C::Sequence(vec![]),
        C::Set(vec![]),
        C::Map(vec![]),
        C::text(""),
        C::Bytes(vec![]),
        C::record("csmir.A", Vec::<(String, C)>::new()),
        C::record("csmir.B", Vec::<(String, C)>::new()),
        C::Signed(1),
        C::Unsigned(1),
        C::Float(1.0),
    ];
    let mut bytes = values
        .iter()
        .map(C::encode)
        .collect::<Result<Vec<_>, _>>()?;
    let count = bytes.len();
    bytes.sort();
    bytes.dedup();
    assert_eq!(bytes.len(), count);
    assert_ne!(
        C::Map(vec![]).encode()?,
        C::Map(vec![("x".into(), C::Absent)]).encode()?
    );
    Ok(())
}

#[test]
fn unicode_and_case_are_exact_without_normalization() -> Result<(), CanonicalError> {
    assert_ne!(C::text("\u{e9}").encode()?, C::text("e\u{301}").encode()?);
    assert_ne!(C::text("Revenue").encode()?, C::text("revenue").encode()?);
    assert_eq!(hex(&C::text("\u{1f600}").encode()?), "820664f09f9880");
    Ok(())
}

#[test]
fn malformed_values_fail_with_structured_errors() {
    assert_eq!(
        C::Float(f64::NAN).encode(),
        Err(CanonicalError::NonFiniteFloat)
    );
    assert_eq!(
        C::Float(f64::INFINITY).encode(),
        Err(CanonicalError::NonFiniteFloat)
    );
    assert_eq!(
        C::Float(f64::NEG_INFINITY).encode(),
        Err(CanonicalError::NonFiniteFloat)
    );
    assert_eq!(
        C::Map(vec![("x".into(), C::Null), ("x".into(), C::Bool(true))]).encode(),
        Err(CanonicalError::DuplicateMapKey("x".into()))
    );
    for tag in [
        "",
        ".type",
        "type.",
        "type..case",
        "type/case",
        "-type",
        "t\u{e9}pe",
    ] {
        assert_eq!(
            C::record(tag, Vec::<(String, C)>::new()).encode(),
            Err(CanonicalError::InvalidVariantTag(tag.into()))
        );
    }
    for bad in ["", "sha256:abc", "sef-sha256-v1:ABC", "sef-sha256-v1:"] {
        assert_eq!(
            Fingerprint::from_str(bad),
            Err(CanonicalError::InvalidFingerprint)
        );
    }
    let uppercase = format!("sef-sha256-v1:{}", "A".repeat(64));
    assert_eq!(
        Fingerprint::from_str(&uppercase),
        Err(CanonicalError::InvalidFingerprint)
    );
}

#[test]
fn explicit_domain_tags_accept_the_csmir_catalog() -> Result<(), CanonicalError> {
    for tag in [
        "namespace",
        "compiled-semantic-model",
        "conformance.exact-domain",
        "csmir.Decimal",
    ] {
        let value = C::record(tag, Vec::<(String, C)>::new());
        assert_ne!(value.encode()?, [] as [u8; 0]);
    }
    Ok(())
}

#[test]
fn exact_decimal_projection_matches_the_frozen_golden() -> Result<(), Box<dyn std::error::Error>> {
    let decimal_type = DecimalType::new(4, 2)?;
    let decimal = DecimalValue::new("+001234", decimal_type)?;
    let bytes = decimal.canonical_value().encode()?;
    assert_eq!(
        hex(&bytes),
        "830a6d63736d69722e446563696d616ca3657363616c6582030269707265636973696f6e8203046b636f656666696369656e7482066431323334"
    );
    assert_eq!(
        hex(&Sha256::digest(bytes)),
        "a37b7f3e31290afc81791bf9fb5c75cc614865a40956e25d1d293372a87f6b15"
    );

    let normalized = DecimalValue::new("1234", decimal_type)?;
    assert_eq!(decimal.canonical_value(), normalized.canonical_value());
    assert_eq!(
        DecimalValue::new("-000", decimal_type)?.canonical_value(),
        DecimalValue::new("0", decimal_type)?.canonical_value()
    );
    assert_ne!(
        DecimalValue::new("120", DecimalType::new(3, 2)?)?.canonical_value(),
        DecimalValue::new("12", DecimalType::new(2, 1)?)?.canonical_value()
    );
    assert_ne!(
        DecimalValue::new("12", DecimalType::new(2, 1)?)?.canonical_value(),
        DecimalValue::new("12", DecimalType::new(3, 1)?)?.canonical_value()
    );
    let maximum = "9".repeat(76);
    let wide = DecimalValue::new(&maximum, DecimalType::new(76, 0)?)?;
    assert_eq!(wide.coefficient(), maximum);
    assert_ne!(wide.canonical_value().encode()?, [] as [u8; 0]);
    Ok(())
}

#[test]
fn identity_projection_has_independent_golden_bytes() -> Result<(), Box<dyn std::error::Error>> {
    // Definite CBOR arrays/text/map assembled independently; SHA256 checked with .NET.
    let identity = ObjectIdentity {
        stable_id: StableId::new("test/metric/revenue")?,
        semantic_version: SemanticVersion::new("1.0.0")?,
    };
    let bytes = identity.canonical_value().encode()?;
    assert_eq!(
        hex(&bytes),
        "830a6f6f626a6563742d6964656e74697479a269737461626c655f6964820673746573742f6d65747269632f726576656e75657073656d616e7469635f76657273696f6e820665312e302e30"
    );
    assert_eq!(
        hex(&Sha256::digest(bytes)),
        "ddddc912e07b7c853f93720b2e3f1c5e10e31bf0ea2e74b9242ebef49901ec97"
    );
    Ok(())
}

#[test]
fn presentation_and_evidence_are_excluded_but_semantic_identity_is_included()
-> Result<(), Box<dyn std::error::Error>> {
    let mut metadata = ObjectMetadata::new(ObjectIdentity {
        stable_id: StableId::new("test/metric/revenue")?,
        semantic_version: SemanticVersion::new("1.0.0")?,
    });
    let original = fingerprint(Domain::Csmir, &metadata)?;
    metadata.label = Some("Ingresos\u{1f4b0}".into());
    metadata.description = Some("Presentation text".into());
    assert_eq!(fingerprint(Domain::Csmir, &metadata)?, original);
    metadata.identity.semantic_version = SemanticVersion::new("1.1.0")?;
    assert_ne!(fingerprint(Domain::Csmir, &metadata)?, original);
    metadata.identity.semantic_version = SemanticVersion::new("1.0.0")?;
    metadata.identity.stable_id = StableId::new("test/metric/net_revenue")?;
    assert_ne!(fingerprint(Domain::Csmir, &metadata)?, original);

    let mut binding = KeyBinding {
        entity_key: StableId::new("test/key/customer")?,
        fields: vec![
            StableId::new("test/field/country")?,
            StableId::new("test/field/customer_id")?,
        ],
        unique: true,
        evidence: ContractEvidenceState::DeclaredUnverified,
    };
    let declared = fingerprint(Domain::Csmir, &binding)?;
    binding.evidence = ContractEvidenceState::Verified {
        evidence_id: "proof-123".into(),
        observed_at: "2026-09-28T00:00:00Z".into(),
        source_snapshot: "snapshot-456".into(),
    };
    assert_eq!(fingerprint(Domain::Csmir, &binding)?, declared);
    binding.fields.reverse();
    assert_ne!(fingerprint(Domain::Csmir, &binding)?, declared);
    binding.fields.reverse();
    binding.unique = false;
    assert_ne!(fingerprint(Domain::Csmir, &binding)?, declared);
    Ok(())
}

#[test]
fn canonicalization_resource_limits_are_enforced() {
    let mut nested = C::Null;
    for _ in 0..=MAX_CANONICAL_DEPTH {
        nested = C::Present(Box::new(nested));
    }
    assert_eq!(nested.encode(), Err(CanonicalError::ResourceLimit("depth")));
    assert_eq!(
        C::Sequence(vec![C::Null; MAX_CANONICAL_NODES]).encode(),
        Err(CanonicalError::ResourceLimit("nodes"))
    );
    assert_eq!(
        C::Bytes(vec![0; MAX_CANONICAL_PAYLOAD_BYTES + 1]).encode(),
        Err(CanonicalError::ResourceLimit("payload_bytes"))
    );
}

proptest! {
    #[test]
    fn set_order_and_exact_duplicates_do_not_change_fingerprint(values in proptest::collection::vec(any::<i64>(), 0..80)) {
        let mut reversed = values.clone();
        reversed.reverse();
        reversed.extend_from_slice(&values);
        prop_assert_eq!(
            fingerprint(Domain::Csmir, &C::set(values)),
            fingerprint(Domain::Csmir, &C::set(reversed))
        );
    }

    #[test]
    fn map_insertion_order_is_irrelevant(entries in proptest::collection::btree_map("[a-z]{1,8}", any::<i64>(), 0..40)) {
        let values: Vec<_> = entries.into_iter().map(|(key, value)| (key, value.canonical_value())).collect();
        let mut reversed = values.clone();
        reversed.reverse();
        prop_assert_eq!(C::Map(values).encode(), C::Map(reversed).encode());
    }

    #[test]
    fn meaningful_sequence_order_changes_fingerprint(left in any::<i64>(), right in any::<i64>()) {
        prop_assume!(left != right);
        prop_assert_ne!(
            fingerprint(Domain::Csmir, &C::sequence([left, right])),
            fingerprint(Domain::Csmir, &C::sequence([right, left]))
        );
    }

    #[test]
    fn integer_types_and_domains_do_not_alias(value in 0_i64..i64::MAX) {
        let signed = C::Signed(value);
        let unsigned = C::Unsigned(value.cast_unsigned());
        prop_assert_ne!(signed.encode(), unsigned.encode());
        prop_assert_ne!(fingerprint(Domain::Csmir, &signed), fingerprint(Domain::Sqir, &signed));
        prop_assert_ne!(fingerprint(Domain::Sqir, &signed), fingerprint(Domain::Seir, &signed));
    }
}

#[test]
fn identifiers_and_text_encode_alike() -> Result<(), CanonicalError> {
    // The distinction is for readers of the tree; fingerprints ignore it.
    assert_eq!(
        C::Id("corpus/metric/revenue".to_owned()).encode()?,
        C::text("corpus/metric/revenue").encode()?
    );
    Ok(())
}
