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

/// A decimal carrier as an integer coefficient at `scale` digits: `"12.3"`
/// at scale 2 is 1230. A carrier never has more fractional digits than its
/// certified scale; anything else is no decimal of this contract.
pub(super) fn coefficient(text: &str, scale: u8) -> Result<BigInt, ExecutionError> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let unsigned = whole.strip_prefix('-').unwrap_or(whole);
    if unsigned.is_empty() || !digits(unsigned) || !digits(fraction) {
        return Err(ExecutionError::SourceContract);
    }
    let missing = usize::from(scale)
        .checked_sub(fraction.len())
        .ok_or(ExecutionError::SourceContract)?;
    format!("{whole}{fraction}{}", "0".repeat(missing))
        .parse()
        .map_err(|_| ExecutionError::SourceContract)
}

/// The exact text of `coefficient` at `scale` digits.
fn carrier(coefficient: &BigInt, scale: u8) -> String {
    let text = coefficient.to_string();
    let (sign, digits) = text
        .strip_prefix('-')
        .map_or(("", text.as_str()), |v| ("-", v));
    let digits = format!("{:0>width$}", digits, width = usize::from(scale) + 1);
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let split = digits.len() - usize::from(scale);
    format!("{sign}{}.{}", &digits[..split], &digits[split..])
}

/// `10^precision`: the first magnitude a value of `precision` digits
/// cannot reach. Computed once per column, never per value.
pub(super) fn bound(precision: u8) -> BigInt {
    BigInt::from(10).pow(u32::from(precision))
}

/// Whether `value` stays below `bound`: the one rule every exact
/// precision check of this adapter applies (#261).
pub(super) fn within_bound(value: &BigInt, bound: &BigInt) -> bool {
    value.magnitude() < bound.magnitude()
}

/// Whether `value` has at most `precision` digits.
pub(super) fn within(value: &BigInt, precision: u8) -> bool {
    within_bound(value, &bound(precision))
}

/// The coefficient and scale of any decimal carrier, whatever its scale.
fn parsed(text: &str) -> Result<(BigInt, u8), ExecutionError> {
    let scale = text.split_once('.').map_or(0, |(_, f)| f.len());
    let scale = u8::try_from(scale).map_err(|_| ExecutionError::SourceContract)?;
    Ok((coefficient(text, scale)?, scale))
}

/// The exact order of two decimal carriers of any scales.
pub(super) fn compare(a: &str, b: &str) -> Result<std::cmp::Ordering, ExecutionError> {
    let ((a, sa), (b, sb)) = (parsed(a)?, parsed(b)?);
    let at = |v: BigInt, from: u8, to: u8| v * BigInt::from(10).pow(u32::from(to - from));
    let to = sa.max(sb);
    Ok(at(a, sa, to).cmp(&at(b, sb, to)))
}

/// `a ± b` or `a × b` of two decimal carriers of scales `sa` and `sb`,
/// exactly, as a carrier of `scale` digits: what DuckDB cannot represent
/// past 38 digits (#261). The result is not range-checked here: the
/// adapter's output conformance checks it against its certified type, so
/// a range failure is never an engine error.
///
/// # Errors
/// `SourceContract` for a malformed carrier or an impossible scale.
pub(super) fn combine(
    a: &str,
    b: &str,
    op: char,
    (sa, sb, scale): (u8, u8, u8),
) -> Result<String, ExecutionError> {
    let (a, b) = (coefficient(a, sa)?, coefficient(b, sb)?);
    let up = |v: BigInt, from: u8| -> Result<BigInt, ExecutionError> {
        let by = scale
            .checked_sub(from)
            .ok_or(ExecutionError::SourceContract)?;
        Ok(v * BigInt::from(10).pow(u32::from(by)))
    };
    let result = match op {
        '+' => up(a, sa)? + up(b, sb)?,
        '-' => up(a, sa)? - up(b, sb)?,
        '*' if u16::from(sa) + u16::from(sb) == u16::from(scale) => a * b,
        _ => return Err(ExecutionError::SourceContract),
    };
    Ok(carrier(&result, scale))
}

/// A key whose text order is the numeric order of the decimal carrier
/// `text`: a sign byte, then for a positive value its digit count and
/// digits, for a negative one their nines' complement so that larger
/// magnitudes sort first.
pub(super) fn sort_key(text: &str, scale: u8) -> Result<String, ExecutionError> {
    let value = coefficient(text, scale)?;
    let digits = value.magnitude().to_string();
    // At most 76 digits plus a scale of 76: three-digit lengths suffice.
    let length = format!("{:03}", digits.len());
    Ok(match value.sign() {
        Sign::Minus => {
            let complement = |s: &str| {
                s.bytes()
                    .map(|b| char::from(b'9' - (b - b'0')))
                    .collect::<String>()
            };
            format!("0{}{}", complement(&length), complement(&digits))
        }
        Sign::NoSign => "1".to_owned(),
        Sign::Plus => format!("2{length}{digits}"),
    })
}

/// The operator and the (left, right, result) scales of a combination.
type CombineOptions = (char, (u8, u8, u8));

/// `sef_exact_combine(a, b, 'precision:scale:+|-|*:left scale:right scale')`:
/// exact decimal sum, difference or product past DuckDB's 38 digits.
pub(super) struct Combine;

impl VArrowScalar for Combine {
    type State = ();
    fn invoke((): &(), input: RecordBatch) -> Result<ArrayRef, Box<dyn std::error::Error>> {
        let (a, b, options) = (
            strings(&input, 0)?,
            strings(&input, 1)?,
            strings(&input, 2)?,
        );
        // The options are a literal of the plan: parsed once per batch.
        let mut parsed_options: Option<(String, CombineOptions)> = None;
        let values = (0..input.num_rows())
            .map(|i| {
                if a.is_null(i) || b.is_null(i) || options.is_null(i) {
                    return Ok(None);
                }
                if let Some((text, (op, scales))) = &parsed_options
                    && text == options.value(i)
                {
                    return combine(a.value(i), b.value(i), *op, *scales).map(Some);
                }
                // `precision:scale:op:left scale:right scale`; the precision
                // is checked by the adapter on the output.
                let parts = options.value(i).split(':').collect::<Vec<_>>();
                let [_, scale, op, sa, sb] = parts.as_slice() else {
                    return Err(ExecutionError::Engine);
                };
                let number = |s: &str| s.parse::<u8>().map_err(|_| ExecutionError::Engine);
                let op = match *op {
                    "+" => '+',
                    "-" => '-',
                    "*" => '*',
                    _ => return Err(ExecutionError::Engine),
                };
                let scales = (number(sa)?, number(sb)?, number(scale)?);
                parsed_options = Some((options.value(i).to_owned(), (op, scales)));
                combine(a.value(i), b.value(i), op, scales).map(Some)
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

/// The base of the three parts a wide SUM input is split into (#261).
const PART: i64 = 1_000_000_000_000_000_000;

/// The three parts of the coefficient of `expr`, a DECIMAL of at most 38
/// digits, computed by DuckDB from its digits: units of 10^36, 10^18 and 1,
/// truncated toward zero so that every part has the value's sign and stays
/// below 10^18 in magnitude, summed by DuckDB in 128 bits. The fallback of
/// `split_sql` past 22 decimals (#261).
pub(super) fn part_sql(expr: &str) -> [String; 3] {
    let power = |digits: usize| format!("CAST(1{} AS HUGEINT)", "0".repeat(digits));
    // Below 10^38, the coefficient fits a HUGEINT exactly.
    let coefficient = format!("CAST(replace(CAST({expr} AS VARCHAR), '.', '') AS HUGEINT)");
    [
        format!("({coefficient}) // {}", power(36)),
        format!("(({coefficient}) % {}) // {}", power(36), power(18)),
        format!("({coefficient}) % {}", power(18)),
    ]
}

/// `expr`, a DECIMAL(`precision`,`scale`) of at most 38 digits, as a count
/// of units of 2^k and an exact remainder, both summed natively by DuckDB
/// without overflow; `None` past 22 decimals (see `part_sql`). With
/// `k` such that every value is below 2^(53+k), the count is a truncation
/// of the value's double, shrunk by 10^-15 so that it never passes the
/// value: below 2^53 it is exact, and so is its multiple of 2^k, which
/// DuckDB turns into a DECIMAL exactly. The remainder `expr - count * 2^k`
/// is exact decimal arithmetic, has the value's sign and stays below
/// 10^scale + 2*10^(precision-15) in coefficient: sums of up to 10^14 rows
/// per group stay far within 128 bits. A value below 2^k in magnitude, the
/// common case, is its own remainder and costs nothing more (#261).
pub(super) fn split_sql(expr: &str, precision: u8, scale: u8) -> Option<([String; 2], u32)> {
    if scale > 22 {
        return None;
    }
    let digits = u32::from(precision - scale);
    let bits = 128 - 10_u128.pow(digits).leading_zeros();
    let k = bits.saturating_sub(53);
    let unit = 1_u128 << k;
    let bound = |sign: &str| format!("CAST('{sign}{unit}' AS DECIMAL(38,{scale}))");
    let small = format!("({expr} > {} AND {expr} < {})", bound("-"), bound(""));
    let factor = 0.999_999_999_999_999 / f64::from(2_u32).powi(i32::try_from(k).ok()?);
    let count = format!("trunc(CAST({expr} AS DOUBLE) * CAST('{factor:e}' AS DOUBLE))");
    let multiple = format!(
        "CAST(CAST({count} * CAST('{unit}' AS DOUBLE) AS DECIMAL(38,0)) AS DECIMAL({precision},{scale}))"
    );
    Some((
        [
            format!("CASE WHEN {small} THEN 0 ELSE CAST({count} AS BIGINT) END"),
            format!("CASE WHEN {small} THEN {expr} ELSE {expr} - {multiple} END"),
        ],
        k,
    ))
}

/// The exact SUM from the sum of the counts of 2^k (`high`) and the sum of
/// the remainders (`low`, a DECIMAL at `scale`) of `split_sql`, as a carrier
/// of `scale` digits, or `None` past `precision` digits (#261).
pub(super) fn sum_split(
    high: &str,
    low: &str,
    precision: u8,
    scale: u8,
    k: u32,
) -> Result<Option<String>, ExecutionError> {
    let total =
        coefficient(high, 0)? * (BigInt::from(1) << k) * BigInt::from(10).pow(u32::from(scale))
            + coefficient(low, scale)?;
    Ok(within(&total, precision).then(|| carrier(&total, scale)))
}

/// `sef_exact_sum_split(high, low, 'precision:scale:k')`: the exact SUM of
/// `split_sql` as a carrier, `'!'` when it is out of its certified range,
/// NULL when there was nothing to sum.
pub(super) struct SumSplit;

impl VArrowScalar for SumSplit {
    type State = ();
    fn invoke((): &(), input: RecordBatch) -> Result<ArrayRef, Box<dyn std::error::Error>> {
        let (high, low, options) = (
            strings(&input, 0)?,
            strings(&input, 1)?,
            strings(&input, 2)?,
        );
        let values = (0..input.num_rows())
            .map(|i| {
                if high.is_null(i) || low.is_null(i) || options.is_null(i) {
                    return Ok(None);
                }
                let mut fields = options.value(i).split(':');
                let mut next = || fields.next().ok_or(ExecutionError::Engine);
                let (precision, scale, k) = (next()?, next()?, next()?);
                let sum = sum_split(
                    high.value(i),
                    low.value(i),
                    precision.parse().map_err(|_| ExecutionError::Engine)?,
                    scale.parse().map_err(|_| ExecutionError::Engine)?,
                    k.parse().map_err(|_| ExecutionError::Engine)?,
                )?;
                Ok(Some(sum.unwrap_or_else(|| "!".to_owned())))
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

/// The exact sum of three part sums, as a carrier of `scale` digits, or
/// `None` when it exceeds `precision` digits: the certified range of SUM
/// decided exactly, never by the engine's accumulator (#261).
pub(super) fn sum_parts(
    high: &str,
    middle: &str,
    low: &str,
    precision: u8,
    scale: u8,
) -> Result<Option<String>, ExecutionError> {
    let integer = |t: &str| coefficient(t, 0);
    let base = BigInt::from(PART);
    let total = integer(high)? * &base * &base + integer(middle)? * &base + integer(low)?;
    Ok(within(&total, precision).then(|| carrier(&total, scale)))
}

/// `sef_exact_sum_parts(high, middle, low, 'precision:scale')`: the exact
/// SUM as a carrier, `'!'` when it is out of its certified range, NULL when
/// there was nothing to sum.
pub(super) struct SumParts;

impl VArrowScalar for SumParts {
    type State = ();
    fn invoke((): &(), input: RecordBatch) -> Result<ArrayRef, Box<dyn std::error::Error>> {
        let parts = (
            strings(&input, 0)?,
            strings(&input, 1)?,
            strings(&input, 2)?,
            strings(&input, 3)?,
        );
        let values = (0..input.num_rows())
            .map(|i| {
                let (high, middle, low, options) = parts;
                if high.is_null(i) || middle.is_null(i) || low.is_null(i) || options.is_null(i) {
                    return Ok(None);
                }
                let (precision, scale) = options
                    .value(i)
                    .split_once(':')
                    .ok_or(ExecutionError::Engine)?;
                let number = |s: &str| s.parse::<u8>().map_err(|_| ExecutionError::Engine);
                let sum = sum_parts(
                    high.value(i),
                    middle.value(i),
                    low.value(i),
                    number(precision)?,
                    number(scale)?,
                )?;
                Ok(Some(sum.unwrap_or_else(|| "!".to_owned())))
            })
            .collect::<Result<Vec<_>, ExecutionError>>()?;
        Ok(Arc::new(StringArray::from(values)))
    }
    fn signatures() -> Vec<ArrowFunctionSignature> {
        vec![ArrowFunctionSignature::exact(
            vec![DataType::Utf8; 4],
            DataType::Utf8,
        )]
    }
}

/// `sef_exact_cmp(a, b)`: -1, 0 or 1 by the exact order of two decimal
/// carriers; NULL when either is NULL, as SQL comparisons are.
pub(super) struct Compare;

impl VArrowScalar for Compare {
    type State = ();
    fn invoke((): &(), input: RecordBatch) -> Result<ArrayRef, Box<dyn std::error::Error>> {
        let (a, b) = (strings(&input, 0)?, strings(&input, 1)?);
        let values = (0..input.num_rows())
            .map(|i| {
                if a.is_null(i) || b.is_null(i) {
                    return Ok(None);
                }
                compare(a.value(i), b.value(i)).map(|order| Some(order as i64))
            })
            .collect::<Result<Vec<_>, ExecutionError>>()?;
        Ok(Arc::new(Int64Array::from(values)))
    }
    fn signatures() -> Vec<ArrowFunctionSignature> {
        vec![ArrowFunctionSignature::exact(
            vec![DataType::Utf8; 2],
            DataType::Int64,
        )]
    }
}

/// `sef_exact_sort_key(text, 'scale')`: the order-preserving key of a wide
/// decimal carrier; NULL stays NULL so the query's NULLS policy applies.
pub(super) struct SortKey;

impl VArrowScalar for SortKey {
    type State = ();
    fn invoke((): &(), input: RecordBatch) -> Result<ArrayRef, Box<dyn std::error::Error>> {
        let (text, scale) = (strings(&input, 0)?, strings(&input, 1)?);
        let values = (0..input.num_rows())
            .map(|i| {
                if text.is_null(i) || scale.is_null(i) {
                    return Ok(None);
                }
                sort_key(
                    text.value(i),
                    scale.value(i).parse().map_err(|_| ExecutionError::Engine)?,
                )
                .map(Some)
            })
            .collect::<Result<Vec<_>, ExecutionError>>()?;
        Ok(Arc::new(StringArray::from(values)))
    }
    fn signatures() -> Vec<ArrowFunctionSignature> {
        vec![ArrowFunctionSignature::exact(
            vec![DataType::Utf8; 2],
            DataType::Utf8,
        )]
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)] // A failure must fail the arithmetic property/test.
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Part `index` (0, 1, 2: units of 10^36, 10^18, 1) of the coefficient of
    /// `text` at `scale` digits, truncated toward zero so that every part has
    /// the value's sign. Each part stays below 10^18 in magnitude, so DuckDB
    /// sums them in 128 bits for any feasible number of rows.
    fn part(text: &str, scale: u8, index: u8) -> Result<i64, ExecutionError> {
        let value = coefficient(text, scale)?;
        let base = BigInt::from(PART);
        let high = &value / (&base * &base);
        let rest = &value % (&base * &base);
        let part = match index {
            0 => high,
            1 => &rest / &base,
            2 => &rest % &base,
            _ => return Err(ExecutionError::Engine),
        };
        // A DECIMAL of at most 38 digits keeps every part within i64.
        i64::try_from(part).map_err(|_| ExecutionError::SourceContract)
    }

    /// Any integer of at most `digits` digits, as a `BigInt`, by its digits.
    fn up_to(digits: usize) -> impl Strategy<Value = BigInt> {
        (
            any::<bool>(),
            proptest::string::string_regex(&format!("[0-9]{{1,{digits}}}"))
                .expect("generated test value"),
        )
            .prop_map(|(negative, d)| {
                let value: BigInt = d.parse().expect("generated test value");
                if negative { -value } else { value }
            })
    }

    proptest! {
        /// Exact sums, differences and products of carriers equal integer
        /// arithmetic on their coefficients (#261).
        #[test]
        fn combine_is_integer_arithmetic(
            a in up_to(38), b in up_to(38), op in prop::sample::select(vec!['+', '-', '*']),
            sa in 0_u8..5, sb in 0_u8..5,
        ) {
            let scale = if op == '*' { sa + sb } else { sa.max(sb) };
            let up = |v: &BigInt, from: u8| v * BigInt::from(10).pow(u32::from(scale - from));
            let expected = match op {
                '+' => up(&a, sa) + up(&b, sb),
                '-' => up(&a, sa) - up(&b, sb),
                _ => &a * &b,
            };
            prop_assert_eq!(
                combine(&carrier(&a, sa), &carrier(&b, sb), op, (sa, sb, scale)),
                Ok(carrier(&expected, scale))
            );
        }

        /// Splitting into parts and summing them is exact integer addition,
        /// with the certified range decided on the exact total (#261).
        #[test]
        fn summing_parts_is_exact_addition(values in prop::collection::vec(up_to(38), 1..6), scale in 0_u8..4) {
            let texts: Vec<String> = values.iter().map(|v| carrier(v, scale)).collect();
            let sum_of = |index| -> BigInt {
                texts.iter().map(|t| BigInt::from(part(t, scale, index).expect("a carrier"))).sum()
            };
            let (high, middle, low) = (sum_of(0), sum_of(1), sum_of(2));
            let total: BigInt = values.iter().sum();
            let expected = within(&total, 38).then(|| carrier(&total, scale));
            prop_assert_eq!(
                sum_parts(&high.to_string(), &middle.to_string(), &low.to_string(), 38, scale),
                Ok(expected)
            );
        }

        /// DuckDB's own split of any DECIMAL of 19 to 38 digits, at any
        /// scale, is the exact one (#261).
        #[test]
        fn the_native_split_is_the_exact_one(
            (precision, scale, value) in (19_u8..=38)
                .prop_flat_map(|p| (Just(p), 0..=p))
                .prop_flat_map(|(p, s)| (Just(p), Just(s), up_to(usize::from(p))))
        ) {
            let text = carrier(&value, scale);
            let parts = part_sql(&format!("CAST('{text}' AS DECIMAL({precision},{scale}))"))
                .map(|part| format!("CAST({part} AS VARCHAR)"));
            let connection = duckdb::Connection::open_in_memory().expect("DuckDB");
            let native: [String; 3] = connection
                .query_row(&format!("SELECT {}", parts.join(", ")), [], |row| {
                    Ok([row.get(0)?, row.get(1)?, row.get(2)?])
                })
                .expect("the split runs");
            for (index, got) in (0_u8..).zip(native) {
                prop_assert_eq!(got, part(&text, scale, index).expect("a carrier").to_string());
            }
        }

        /// DuckDB's split of any DECIMAL of 19 to 38 digits and up to 22
        /// decimals, summed and recomposed, is the exact SUM with its exact
        /// range, extremes of either sign included (#261).
        #[test]
        fn the_native_sum_split_is_exact(
            (precision, scale, values, extremes) in (19_u8..=38)
                .prop_flat_map(|p| (Just(p), 0..=p.min(22)))
                .prop_flat_map(|(p, s)| (
                    Just(p), Just(s),
                    prop::collection::vec(up_to(usize::from(p)), 1..6),
                    prop::collection::vec(any::<bool>(), 0..3),
                ))
        ) {
            let largest: BigInt = BigInt::from(10).pow(u32::from(precision)) - 1;
            let values: Vec<BigInt> = values
                .into_iter()
                .chain(extremes.into_iter().map(|negative| if negative { -&largest } else { largest.clone() }))
                .collect();
            let rows = values
                .iter()
                .map(|v| format!("(CAST('{}' AS DECIMAL({precision},{scale})))", carrier(v, scale)))
                .collect::<Vec<_>>()
                .join(", ");
            let ([high, low], k) = split_sql("v", precision, scale).expect("up to 22 decimals");
            let connection = duckdb::Connection::open_in_memory().expect("DuckDB");
            let (high, low): (String, String) = connection
                .query_row(
                    &format!("SELECT CAST(SUM({high}) AS VARCHAR), CAST(SUM({low}) AS VARCHAR) FROM (VALUES {rows}) t(v)"),
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("the split runs");
            let total: BigInt = values.iter().sum();
            let expected = within(&total, 38).then(|| carrier(&total, scale));
            prop_assert_eq!(sum_split(&high, &low, 38, scale, k), Ok(expected));
        }

        /// `within` is the digit count, exactly at the edge.
        #[test]
        fn within_counts_digits(precision in 1_u8..=76) {
            let edge = BigInt::from(10).pow(u32::from(precision));
            prop_assert!(within(&(&edge - 1), precision));
            prop_assert!(within(&(1 - &edge), precision));
            prop_assert!(!within(&edge, precision));
            prop_assert!(!within(&-&edge, precision));
        }

        /// The sort key orders carriers exactly as their numeric values.
        #[test]
        fn the_sort_key_orders_as_the_numbers(a in up_to(76), b in up_to(76), scale in 0_u8..6) {
            let (ka, kb) = (
                sort_key(&carrier(&a, scale), scale).expect("generated test value"),
                sort_key(&carrier(&b, scale), scale).expect("generated test value"),
            );
            prop_assert_eq!(ka.cmp(&kb), a.cmp(&b));
        }
    }

    #[test]
    fn a_malformed_carrier_is_no_decimal_of_the_contract() {
        for text in ["", "-", "1.234", "1e3", "١٢", "1.2.3"] {
            assert_eq!(
                coefficient(text, 2),
                Err(ExecutionError::SourceContract),
                "{text}"
            );
        }
        assert_eq!(coefficient("-12.3", 2), Ok(BigInt::from(-1230)));
    }

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
