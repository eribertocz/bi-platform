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
