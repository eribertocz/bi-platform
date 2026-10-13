//! Exact operations DuckDB cannot perform under the semantic contract.
//! Arrow callbacks keep FFI inside duckdb-rs; this crate contains no unsafe code.

use super::ExecutionError;
use chrono::{DateTime, NaiveDate, Utc};
use duckdb::arrow::array::{Array, ArrayRef, Date32Array, Int64Array, StringArray};
use duckdb::arrow::datatypes::DataType;
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::vscalar::{ArrowFunctionSignature, VArrowScalar};
use num_bigint::{BigInt, Sign};
use sef_model_ir::types::RoundingMode;
use std::sync::Arc;

pub(super) struct Ratio;

fn strings(input: &RecordBatch, i: usize) -> Result<&StringArray, ExecutionError> {
    input
        .column(i)
        .as_any()
        .downcast_ref()
        .ok_or(ExecutionError::SourceContract)
}

/// Decimal strings are exact carriers, never floats. Result has `scale` digits.
pub(super) fn divide(
    a: &str,
    b: &str,
    scale: u8,
    rounding: RoundingMode,
) -> Result<String, ExecutionError> {
    let parse = |s: &str| -> Result<(BigInt, usize), ExecutionError> {
        let (whole, fraction) = s.split_once('.').unwrap_or((s, ""));
        Ok((
            format!("{whole}{fraction}")
                .parse()
                .map_err(|_| ExecutionError::Engine)?,
            fraction.len(),
        ))
    };
    let (a, sa) = parse(a)?;
    let (b, sb) = parse(b)?;
    if b == BigInt::from(0) {
        return Err(ExecutionError::NumericOverflow);
    }
    let numerator = a * BigInt::from(10)
        .pow(u32::try_from(sb + usize::from(scale)).map_err(|_| ExecutionError::NumericOverflow)?);
    let denominator =
        b * BigInt::from(10).pow(u32::try_from(sa).map_err(|_| ExecutionError::NumericOverflow)?);
    let mut q = &numerator / &denominator;
    let rem = &numerator % &denominator;
    let abs = |v: &BigInt| {
        if v.sign() == Sign::Minus {
            -v
        } else {
            v.clone()
        }
    };
    let negative = numerator.sign() != denominator.sign();
    let nonzero = rem != BigInt::from(0);
    let twice = abs(&rem) * 2;
    let den = abs(&denominator);
    let increment = match rounding {
        RoundingMode::TowardZero => false,
        RoundingMode::Floor => negative && nonzero,
        RoundingMode::Ceiling => !negative && nonzero,
        RoundingMode::HalfUp => twice >= den,
        RoundingMode::HalfEven => twice > den || (twice == den && &q % 2 != BigInt::from(0)),
    };
    if increment {
        q += if negative { -1 } else { 1 };
    }
    let text = q.to_string();
    let (sign, digits) = text
        .strip_prefix('-')
        .map_or(("", text.as_str()), |v| ("-", v));
    let digits = format!("{:0>width$}", digits, width = usize::from(scale) + 1);
    if scale == 0 {
        return Ok(format!("{sign}{digits}"));
    }
    let split = digits.len() - usize::from(scale);
    Ok(format!("{sign}{}.{}", &digits[..split], &digits[split..]))
}

impl VArrowScalar for Ratio {
    type State = ();
    fn invoke((): &(), input: RecordBatch) -> Result<ArrayRef, Box<dyn std::error::Error>> {
        let (a, b, options) = (
            strings(&input, 0)?,
            strings(&input, 1)?,
            strings(&input, 2)?,
        );
        let values = (0..input.num_rows())
            .map(|i| {
                if a.is_null(i) || b.is_null(i) || options.is_null(i) {
                    return Ok(None);
                }
                let (scale, mode) = options
                    .value(i)
                    .split_once(':')
                    .ok_or(ExecutionError::Engine)?;
                let rounding = match mode {
                    "half-even" => RoundingMode::HalfEven,
                    "half-up" => RoundingMode::HalfUp,
                    "floor" => RoundingMode::Floor,
                    "ceiling" => RoundingMode::Ceiling,
                    "toward-zero" => RoundingMode::TowardZero,
                    _ => return Err(ExecutionError::Engine),
                };
                divide(
                    a.value(i),
                    b.value(i),
                    scale.parse().map_err(|_| ExecutionError::Engine)?,
                    rounding,
                )
                .map(Some)
            })
            .collect::<Result<Vec<_>, ExecutionError>>()?;
        Ok(Arc::new(StringArray::from(values)))
    }
    fn signatures() -> Vec<ArrowFunctionSignature> {
        vec![ArrowFunctionSignature::exact(
            vec![DataType::Utf8; 3],
            DataType::Utf8,
        )]
    }
}

pub(super) struct LocalDate;
impl VArrowScalar for LocalDate {
    type State = ();
    fn invoke((): &(), input: RecordBatch) -> Result<ArrayRef, Box<dyn std::error::Error>> {
        let epochs = input
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or(ExecutionError::SourceContract)?;
        let zones = strings(&input, 1)?;
        let values = (0..input.num_rows())
            .map(|i| {
                if epochs.is_null(i) || zones.is_null(i) {
                    return Ok(None);
                }
                let zone: chrono_tz::Tz = zones
                    .value(i)
                    .parse()
                    .map_err(|_| ExecutionError::SourceContract)?;
                let date = DateTime::<Utc>::from_timestamp_micros(epochs.value(i))
                    .ok_or(ExecutionError::NumericOverflow)?
                    .with_timezone(&zone)
                    .date_naive();
                let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).ok_or(ExecutionError::Engine)?;
                i32::try_from((date - epoch).num_days())
                    .map(Some)
                    .map_err(|_| ExecutionError::NumericOverflow)
            })
            .collect::<Result<Vec<_>, ExecutionError>>()?;
        Ok(Arc::new(Date32Array::from(values)))
    }
    fn signatures() -> Vec<ArrowFunctionSignature> {
        vec![ArrowFunctionSignature::exact(
            vec![DataType::Int64, DataType::Utf8],
            DataType::Date32,
        )]
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)] // A failure must fail the arithmetic property/test.
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rounding_respects_sign_ties_and_scales() {
        for (mode, positive, negative) in [
            (RoundingMode::HalfEven, "2", "-2"),
            (RoundingMode::HalfUp, "3", "-3"),
            (RoundingMode::Floor, "2", "-3"),
            (RoundingMode::Ceiling, "3", "-2"),
            (RoundingMode::TowardZero, "2", "-2"),
        ] {
            assert_eq!(
                divide("5", "2", 0, mode).expect("valid exact arithmetic fixture"),
                positive
            );
            assert_eq!(
                divide("5", "-2", 0, mode).expect("valid exact arithmetic fixture"),
                negative
            );
        }
        assert_eq!(
            divide("7", "2", 0, RoundingMode::HalfEven).expect("valid exact arithmetic fixture"),
            "4"
        );
        assert_eq!(
            divide("0.001", "0.02", 4, RoundingMode::HalfEven)
                .expect("valid exact arithmetic fixture"),
            "0.0500"
        );
        assert_eq!(
            divide("9007199254740993", "1", 2, RoundingMode::HalfEven)
                .expect("valid exact arithmetic fixture"),
            "9007199254740993.00"
        );
        assert_eq!(
            divide("1", "0", 2, RoundingMode::HalfEven),
            Err(ExecutionError::NumericOverflow)
        );
    }

    proptest! {
        #[test]
        fn truncating_ratio_has_bounded_remainder(a in any::<i64>(), b in 1_i64..=i64::MAX, scale in 0_u8..=12) {
            let result = divide(&a.to_string(), &b.to_string(), scale, RoundingMode::TowardZero).expect("valid exact arithmetic fixture");
            let coefficient: BigInt = result.replace('.', "").parse().expect("valid exact arithmetic fixture");
            let numerator = BigInt::from(a) * BigInt::from(10).pow(u32::from(scale));
            let remainder = numerator - &coefficient * BigInt::from(b);
            prop_assert!(remainder >= BigInt::from(-b) && remainder < BigInt::from(b));
            prop_assert!(coefficient == BigInt::from(0) || coefficient.sign() == BigInt::from(a).sign());
        }
    }
}
