//! The typing rule of exact scalar functions is sound (#77): for every
//! decimal input, the value the function's contract computes fits the type
//! `ScalarFunction::result` declares. Values are computed here with integer
//! arithmetic on coefficients, independently of any engine.
#![allow(clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use sef_model_ir::expressions::{ScalarExpr, ScalarExprKind};
use sef_model_ir::functions::ScalarFunction;
use sef_model_ir::identity::StableId;
use sef_model_ir::types::{DecimalType, Literal, ValueType, ZeroDivisionPolicy};

fn column(precision: u8, scale: u8) -> ScalarExpr {
    ScalarExpr {
        kind: ScalarExprKind::FieldRef(StableId::new("t/field/x").expect("id")),
        value_type: ValueType::Decimal(DecimalType::new(precision, scale).expect("type")),
        unit: None,
    }
}

fn places(digits: u8) -> ScalarExpr {
    ScalarExpr {
        kind: ScalarExprKind::Literal(Literal::SignedInteger(i128::from(digits))),
        value_type: ValueType::Integer {
            signed: true,
            bits: Some(64),
        },
        unit: None,
    }
}

fn decimal(value_type: &ValueType) -> (u8, u8) {
    match value_type {
        ValueType::Decimal(d) => (d.precision(), d.scale()),
        other => panic!("expected a decimal, got {other:?}"),
    }
}

/// `value` (a coefficient) fits `precision` digits.
fn fits(value: i128, precision: u8) -> bool {
    value.unsigned_abs() < 10_u128.pow(u32::from(precision))
}

/// A decimal type and a coefficient it holds.
fn typed_value() -> impl Strategy<Value = (u8, u8, i128)> {
    (1_u8..=9).prop_flat_map(|precision| {
        let bound = 10_i128.pow(u32::from(precision)) - 1;
        (Just(precision), 0..=precision, -bound..=bound)
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    /// ROUND (ties away from zero) and TRUNCATE (toward zero) at `d` digits.
    #[test]
    fn rounding_results_fit_their_type((p, s, v) in typed_value(), d in 0_u8..=10) {
        for function in [ScalarFunction::Round, ScalarFunction::Truncate] {
            let (value_type, _) = function.result(&[column(p, s), places(d)]).expect("typed");
            let (rp, rs) = decimal(&value_type);
            if d >= s {
                prop_assert_eq!((rp, rs), (p, s), "more digits than the input keeps the type");
                continue;
            }
            prop_assert_eq!(rs, d);
            let factor = 10_i128.pow(u32::from(s - d));
            let mut result = v / factor;
            if function == ScalarFunction::Round && (v % factor).abs() * 2 >= factor {
                result += v.signum();
            }
            prop_assert!(fits(result, rp), "{:?} of {} at DECIMAL({},{}) to {} digits: {} does not fit DECIMAL({},{})", function, v, p, s, d, result, rp, rs);
        }
    }

    /// FLOOR and CEIL to an integral value.
    #[test]
    fn integral_results_fit_their_type((p, s, v) in typed_value()) {
        for function in [ScalarFunction::Floor, ScalarFunction::Ceil] {
            let (value_type, _) = function.result(&[column(p, s)]).expect("typed");
            let (rp, rs) = decimal(&value_type);
            prop_assert_eq!(rs, 0);
            let factor = 10_i128.pow(u32::from(s));
            let result = if function == ScalarFunction::Floor {
                v.div_euclid(factor)
            } else {
                -(-v).div_euclid(factor)
            };
            prop_assert!(fits(result, rp), "{:?} of {} at DECIMAL({},{}): {} does not fit DECIMAL({},0)", function, v, p, s, result, rp);
        }
    }

    /// MOD keeps the sign of the dividend and stays below the divisor.
    #[test]
    fn remainders_fit_their_type((p1, s1, a) in typed_value(), (p2, s2, b) in typed_value()) {
        prop_assume!(b != 0);
        let function = ScalarFunction::Mod { zero: ZeroDivisionPolicy::Null };
        let (value_type, _) = function.result(&[column(p1, s1), column(p2, s2)]).expect("typed");
        let (rp, rs) = decimal(&value_type);
        let scale = s1.max(s2);
        prop_assert_eq!(rs, scale);
        let left = a * 10_i128.pow(u32::from(scale - s1));
        let right = b * 10_i128.pow(u32::from(scale - s2));
        let remainder = left % right;
        prop_assert!(remainder == 0 || remainder.signum() == left.signum());
        prop_assert!(fits(remainder, rp), "{} mod {} = {} does not fit DECIMAL({},{})", left, right, remainder, rp, rs);
    }
}

#[test]
fn digits_must_be_a_non_negative_constant() {
    let negative = ScalarExpr {
        kind: ScalarExprKind::Literal(Literal::SignedInteger(-1)),
        ..places(0)
    };
    assert!(
        ScalarFunction::Round
            .result(&[column(18, 2), negative])
            .is_err()
    );
    let computed = ScalarExpr {
        kind: ScalarExprKind::FieldRef(StableId::new("t/field/d").expect("id")),
        ..places(0)
    };
    assert!(
        ScalarFunction::Truncate
            .result(&[column(18, 2), computed])
            .is_err()
    );
}

#[test]
fn the_widest_decimal_stays_representable() {
    // DECIMAL(76, 0): FLOOR keeps the type; ROUND at more digits keeps it too.
    let widest = column(76, 0);
    assert_eq!(
        ScalarFunction::Floor
            .result(std::slice::from_ref(&widest))
            .expect("typed")
            .0,
        widest.value_type
    );
    assert_eq!(
        ScalarFunction::Round
            .result(&[widest.clone(), places(3)])
            .expect("typed")
            .0,
        widest.value_type
    );
    // DECIMAL(76, 2) rounded to 0 digits would need 75 integral digits: fits.
    let (precision, scale) = decimal(
        &ScalarFunction::Round
            .result(&[column(76, 2), places(0)])
            .expect("typed")
            .0,
    );
    assert_eq!((precision, scale), (75, 0));
}

// ---- Review of #92 --------------------------------------------------------------------

fn integer_column(signed: bool, bits: u16) -> ScalarExpr {
    ScalarExpr {
        kind: ScalarExprKind::FieldRef(StableId::new("t/field/i").expect("id")),
        value_type: ValueType::Integer {
            signed,
            bits: Some(bits),
        },
        unit: None,
    }
}

/// Decimal digits of the largest magnitude, computed independently.
fn magnitude_digits(signed: bool, bits: u16) -> u8 {
    let largest = if signed {
        1_u128 << (bits - 1)
    } else if bits == 128 {
        u128::MAX
    } else {
        (1_u128 << bits) - 1
    };
    u8::try_from(largest.to_string().len()).expect("digits")
}

#[test]
fn a_remainder_of_wide_integers_fits_its_type() {
    let modulo = ScalarFunction::Mod {
        zero: ZeroDivisionPolicy::Null,
    };
    // UINT64::MAX mod 20000000000000000000 is UINT64::MAX: 20 digits.
    let (value_type, _) = modulo
        .result(&[integer_column(false, 64), column(38, 0)])
        .expect("typed");
    assert_eq!(decimal(&value_type), (20, 0));
    // An INT128 against DECIMAL(38,0) keeps the 38 digits of the divisor.
    let (value_type, _) = modulo
        .result(&[integer_column(true, 128), column(38, 0)])
        .expect("typed");
    assert_eq!(decimal(&value_type), (38, 0));
    // An unbounded integer has no exact digits to mix with a decimal.
    let unbounded = ScalarExpr {
        value_type: ValueType::Integer {
            signed: true,
            bits: None,
        },
        ..integer_column(true, 64)
    };
    assert!(modulo.result(&[unbounded, column(10, 2)]).is_err());
}

proptest! {
    /// Every integer width and signedness against every decimal: the
    /// remainder's digits are bounded by both operands, and the type holds them.
    #[test]
    fn remainders_of_integers_and_decimals_fit(
        signed in any::<bool>(),
        bits in proptest::sample::select(vec![8_u16, 16, 32, 64, 128]),
        (p, s, _) in typed_value(),
    ) {
        let modulo = ScalarFunction::Mod { zero: ZeroDivisionPolicy::Null };
        let (value_type, _) = modulo.result(&[integer_column(signed, bits), column(p, s)]).expect("typed");
        let (rp, rs) = decimal(&value_type);
        prop_assert_eq!(rs, s);
        prop_assert!(rp >= magnitude_digits(signed, bits).min(p - s) + s);
    }
}

#[test]
fn greatest_and_least_only_take_ordered_types() {
    let same = |value_type: ValueType| {
        let value = ScalarExpr {
            kind: ScalarExprKind::Literal(Literal::Null(value_type.clone())),
            value_type,
            unit: None,
        };
        [value.clone(), value]
    };
    for value_type in [
        ValueType::Utf8,
        ValueType::Boolean,
        ValueType::Date,
        ValueType::Integer {
            signed: true,
            bits: Some(64),
        },
    ] {
        assert!(ScalarFunction::Greatest.result(&same(value_type)).is_ok());
    }
    for value_type in [
        ValueType::Float { bits: 64 },
        ValueType::OpaqueExtension {
            namespace: "probe".to_owned(),
            type_name: "unordered".to_owned(),
        },
        ValueType::List(Box::new(ValueType::Utf8)),
    ] {
        assert!(
            ScalarFunction::Least
                .result(&same(value_type.clone()))
                .is_err(),
            "{value_type:?}"
        );
    }
}

#[test]
fn digits_stay_within_the_decimal_range() {
    let at = |digits: i128| ScalarExpr {
        kind: ScalarExprKind::Literal(Literal::SignedInteger(digits)),
        ..places(0)
    };
    assert!(
        ScalarFunction::Round
            .result(&[column(18, 2), at(76)])
            .is_ok()
    );
    for digits in [77, 255, 256] {
        assert!(
            ScalarFunction::Round
                .result(&[column(18, 2), at(digits)])
                .is_err(),
            "{digits}"
        );
    }
}
