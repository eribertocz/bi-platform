//! Integer literals hold the full 128-bit range of their types (review of
//! #73) without changing any existing fingerprint: a value that fits 64
//! bits keeps its original encoding, and every value has exactly one form.
#![allow(clippy::expect_used)]

use sef_model_ir::canonical::{Canonical, CanonicalValue as C};
use sef_model_ir::types::{Literal, ValueType};

fn bytes(value: &impl Canonical) -> Vec<u8> {
    value.canonical_value().encode().expect("encoding")
}

#[test]
fn a_64_bit_value_keeps_its_original_encoding() {
    for value in [0_i64, -24, 5, i64::MIN, i64::MAX] {
        let original = C::record("literal.signed-integer", [("value", C::Signed(value))]);
        assert_eq!(
            bytes(&Literal::SignedInteger(value.into())),
            original.encode().expect("encoding"),
            "{value}"
        );
    }
    for value in [0_u64, 1, u64::MAX] {
        let original = C::record("literal.unsigned-integer", [("value", C::Unsigned(value))]);
        assert_eq!(
            bytes(&Literal::UnsignedInteger(value.into())),
            original.encode().expect("encoding"),
            "{value}"
        );
    }
}

#[test]
fn wider_values_have_their_own_single_form() {
    let wide = [
        Literal::SignedInteger(i128::from(i64::MAX) + 1),
        Literal::SignedInteger(i128::from(i64::MIN) - 1),
        Literal::SignedInteger(i128::MAX),
        Literal::SignedInteger(i128::MIN),
        Literal::UnsignedInteger(u128::from(u64::MAX) + 1),
        Literal::UnsignedInteger(u128::MAX),
    ];
    let mut encodings: Vec<Vec<u8>> = wide.iter().map(bytes).collect();
    // Signed 2^63 and unsigned 2^63 are different literals.
    encodings.push(bytes(&Literal::UnsignedInteger(1_u128 << 63)));
    let count = encodings.len();
    encodings.sort();
    encodings.dedup();
    assert_eq!(encodings.len(), count, "distinct literals, distinct bytes");
    assert_eq!(
        Literal::SignedInteger(i128::MAX).value_type(),
        ValueType::Integer {
            signed: true,
            bits: Some(128)
        }
    );
    assert_eq!(
        Literal::UnsignedInteger(7).value_type(),
        ValueType::Integer {
            signed: false,
            bits: Some(64)
        }
    );
}
