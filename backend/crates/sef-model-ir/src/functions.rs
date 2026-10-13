//! Scalar functions of CSMIR (`OSSIE_SQL_2026` REQUIRED, #77).
//!
//! Each variant states its exact, engine-independent semantics: it is the
//! contract every adapter implements, so two conformant engines return the
//! same value. Where engines disagree (NULL handling of `CONCAT` and
//! `GREATEST`, negative lengths, zero divisors), the choice is written here
//! and the adapter must lower to it. Every function is NULL-strict: a NULL
//! argument yields NULL.
//!
//! [`ScalarFunction::result`] is the single typing rule: converters use it
//! to type what they build and validation uses it to check what it is given.

use crate::expressions::ScalarExpr;
use crate::time::TimestampSemantics;
use crate::types::{DecimalType, Literal, ValueType, ZeroDivisionPolicy};
use crate::units::Unit;

/// The widest decimal precision CSMIR represents.
const MAX_PRECISION: u8 = 76;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScalarFunction {
    // ---- Text: positions and lengths count Unicode scalar values -------------
    /// Two or more strings joined in order. `CONCAT` and `||` alike: a NULL
    /// argument yields NULL (never an empty string).
    Concat,
    /// Number of Unicode scalar values (not bytes).
    Length,
    /// Unicode simple lowercase mapping, one scalar value to one.
    Lower,
    /// Unicode simple uppercase mapping, one scalar value to one.
    Upper,
    /// Removes leading and trailing characters: U+0020 SPACE by default
    /// (ANSI), or the single character given as second argument.
    Trim,
    /// As [`Self::Trim`], leading characters only.
    LTrim,
    /// As [`Self::Trim`], trailing characters only.
    RTrim,
    /// The first `n` characters; the whole string when `n` exceeds its
    /// length; the empty string when `n <= 0`.
    Left,
    /// The last `n` characters, with the same bounds as [`Self::Left`].
    Right,
    /// `SUBSTRING(s FROM start FOR length)` (ANSI): the characters at
    /// 1-based positions `p` with `start <= p < start + length` and
    /// `1 <= p <= LENGTH(s)`. A negative `length` is an error.
    Substring,
    /// Every non-overlapping occurrence of the second argument, scanned
    /// left to right, replaced by the third. An empty search string leaves
    /// the input unchanged.
    Replace,
    /// The `n`-th part (1-based) of the string split by the delimiter; a
    /// negative `n` counts from the end; past the last part, the empty
    /// string; `n = 0` is an error. An empty delimiter makes the whole
    /// string the only part.
    SplitPart,
    /// 1-based position of the first occurrence of the first argument in
    /// the second; 0 when absent; 1 for an empty search string.
    Position,
    /// Whether the second argument occurs in the first (true when empty).
    Contains,
    /// Whether the first argument starts with the second (true when empty).
    StartsWith,
    /// Whether the first argument ends with the second (true when empty).
    EndsWith,
    /// SQL `LIKE`: `%` matches any sequence, `_` exactly one character,
    /// compared by scalar value; no escape character.
    Like,
    /// [`Self::Like`] after the simple lowercase mapping of both sides.
    ILike,

    // ---- Exact math: integers and decimals never pass through floats ---------
    /// Absolute value, same type; a result out of range is an error.
    Abs,
    /// -1, 0 or 1 as a 64-bit integer.
    Sign,
    /// Toward negative infinity, to an integral value (scale 0).
    Floor,
    /// Toward positive infinity, to an integral value (scale 0).
    Ceil,
    /// To the digits given by a constant second argument (0 when absent),
    /// ties away from zero.
    Round,
    /// To the digits given by a constant second argument (0 when absent),
    /// toward zero.
    Truncate,
    /// `a - b * TRUNC(a / b)`: the sign of the dividend. A zero divisor
    /// follows the declared policy, as exact division does.
    Mod { zero: ZeroDivisionPolicy },
    /// The largest argument in its type's order: numbers by value, text by
    /// Unicode scalar value (binary, no collation), `false < true`, dates
    /// and times chronologically. NULL when any argument is NULL. Types
    /// without a fixed order (floats, lists, structures, extensions) are
    /// refused.
    Greatest,
    /// The smallest argument, in the same order as [`Self::Greatest`].
    Least,

    // ---- Dates: proleptic Gregorian calendar, ISO weeks starting Monday -------
    /// The `part` of a date or a local date-time as a 64-bit integer. `Week`
    /// is the ISO week number (1–53), `DayOfWeek` the ISO weekday (Monday 1
    /// … Sunday 7) and `Second` whole seconds. An instant has no calendar
    /// fields by itself: converters first bucket it to its local date in
    /// the business zone.
    Extract { part: DatePart },
    /// Adds an integer number of `part`s. Calendar parts move the date and
    /// clamp to the last day of a shorter month (January 31 plus one month
    /// is the last day of February); a week is 7 days; clock parts are
    /// exact durations, the only ones an instant admits. A result out of
    /// range is an error.
    DateAdd { part: DatePart },
    /// The number of `part` boundaries crossed from the first argument to
    /// the second, negative when the second is earlier: months count
    /// `12 * year + month`, weeks count ISO Mondays, days count dates, and
    /// clock parts count whole units of the wall clock of a local
    /// date-time. Instants admit only minutes and seconds, counted on the
    /// UTC epoch and never on a local clock
    /// ([`DatePart::instant_boundaries`]). The count is zero when both
    /// values fall within one unit, even if the second is earlier.
    DateDiff { part: DatePart },
    /// An ISO 8601 calendar date (`YYYY-MM-DD`) read from text; anything
    /// else is an error, never a guess.
    ToDate,
}

/// A calendar or clock field of a date or date-time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatePart {
    Year,
    Quarter,
    Month,
    Week,
    Day,
    DayOfWeek,
    DayOfYear,
    Hour,
    Minute,
    Second,
}

impl DatePart {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Year => "year",
            Self::Quarter => "quarter",
            Self::Month => "month",
            Self::Week => "week",
            Self::Day => "day",
            Self::DayOfWeek => "day-of-week",
            Self::DayOfYear => "day-of-year",
            Self::Hour => "hour",
            Self::Minute => "minute",
            Self::Second => "second",
        }
    }

    /// Hours, minutes and seconds: fields of the clock, not of the date.
    #[must_use]
    pub const fn is_clock(self) -> bool {
        matches!(self, Self::Hour | Self::Minute | Self::Second)
    }

    /// Fields that name a position (weekday, day of year) and cannot be
    /// added or counted.
    const fn is_position(self) -> bool {
        matches!(self, Self::DayOfWeek | Self::DayOfYear)
    }

    /// `DATEDIFF(part, start, end)` between two instants in microseconds
    /// since 1970-01-01T00:00:00Z: the boundaries of the UTC epoch crossed,
    /// `floor(end / unit) - floor(start / unit)`, rounding toward negative
    /// infinity before 1970 as well. Neither the session zone nor the
    /// business zone takes part: a historical offset with seconds
    /// (`America/La_Paz` was -04:32:36 in 1900) would move local minute
    /// boundaries. `None` for the parts an instant cannot be counted in:
    /// hours, because offsets such as +05:30 make UTC hours differ from
    /// the local ones, and calendar parts, which need a zone.
    #[must_use]
    pub const fn instant_boundaries(self, start: i64, end: i64) -> Option<i64> {
        let unit = match self {
            Self::Second => 1_000_000,
            Self::Minute => 60_000_000,
            _ => return None,
        };
        // Both quotients lie within ±i64::MAX / 10^6: no overflow.
        Some(end.div_euclid(unit) - start.div_euclid(unit))
    }
}

/// Why arguments do not fit a function.
pub type TypingError = &'static str;

fn text(argument: &ScalarExpr) -> bool {
    argument.value_type == ValueType::Utf8
}

fn integer(argument: &ScalarExpr) -> bool {
    matches!(argument.value_type, ValueType::Integer { .. })
}

fn instant(argument: &ScalarExpr) -> bool {
    matches!(
        argument.value_type,
        ValueType::Timestamp {
            timezone_semantics: TimestampSemantics::Instant
        }
    )
}

/// Whether `argument` has the field `part`: dates only calendar parts;
/// local date-times every part; instants only clock parts, and only when
/// `arithmetic` (adding or counting exact durations).
fn temporal(argument: &ScalarExpr, part: DatePart, arithmetic: bool) -> bool {
    match &argument.value_type {
        ValueType::Date => !part.is_clock(),
        ValueType::Timestamp {
            timezone_semantics: TimestampSemantics::Instant,
        } => arithmetic && part.is_clock(),
        ValueType::Timestamp { .. } => true,
        _ => false,
    }
}

fn exact(argument: &ScalarExpr) -> bool {
    matches!(
        argument.value_type,
        ValueType::Integer { .. } | ValueType::Decimal(_)
    )
}

const fn int64() -> ValueType {
    ValueType::Integer {
        signed: true,
        bits: Some(64),
    }
}

/// A constant single-character string, as `TRIM` takes.
fn single_character(argument: &ScalarExpr) -> bool {
    matches!(&argument.kind, crate::expressions::ScalarExprKind::Literal(Literal::Utf8(text))
        if text.chars().count() == 1)
}

/// The digits of `ROUND`/`TRUNCATE`: an integer literal in 0..=76.
fn digits(argument: Option<&ScalarExpr>) -> Result<u8, TypingError> {
    const RANGE: TypingError = "digits must be between 0 and 76";
    let Some(argument) = argument else {
        return Ok(0);
    };
    let digits = match &argument.kind {
        crate::expressions::ScalarExprKind::Literal(Literal::SignedInteger(value)) => {
            u8::try_from(*value).map_err(|_| RANGE)?
        }
        crate::expressions::ScalarExprKind::Literal(Literal::UnsignedInteger(value)) => {
            u8::try_from(*value).map_err(|_| RANGE)?
        }
        _ => return Err("the digits of ROUND and TRUNCATE must be an integer constant"),
    };
    if digits > MAX_PRECISION {
        return Err(RANGE);
    }
    Ok(digits)
}

/// A decimal type of exactly these digits; never a narrower one.
fn decimal(precision: u8, scale: u8) -> Result<ValueType, TypingError> {
    if precision > MAX_PRECISION {
        return Err("the result does not fit a decimal type");
    }
    DecimalType::new(precision.max(1), scale)
        .map(ValueType::Decimal)
        .map_err(|_| "the result does not fit a decimal type")
}

/// The decimal digits of the largest magnitude an integer type holds: the
/// most negative value of a signed type, the maximum of an unsigned one.
/// `None` for unbounded integers.
fn integer_digits(signed: bool, bits: Option<u16>) -> Option<u8> {
    let bits = u32::from(bits?);
    if bits == 0 || bits > 128 {
        return None;
    }
    let largest: u128 = if signed {
        1_u128 << (bits - 1)
    } else if bits == 128 {
        u128::MAX
    } else {
        (1_u128 << bits) - 1
    };
    let mut digits = 0;
    let mut rest = largest;
    while rest > 0 {
        digits += 1;
        rest /= 10;
    }
    Some(digits)
}

/// Types whose order the contract of `GREATEST` and `LEAST` fixes.
fn ordered(value_type: &ValueType) -> bool {
    matches!(
        value_type,
        ValueType::Integer { .. }
            | ValueType::Decimal(_)
            | ValueType::Utf8
            | ValueType::Boolean
            | ValueType::Date
            | ValueType::Time
            | ValueType::Timestamp { .. }
    )
}

/// The integer and fractional digits of an exact type.
fn digits_of(value_type: &ValueType) -> Option<(u8, u8)> {
    match value_type {
        ValueType::Integer { signed, bits } => integer_digits(*signed, *bits).map(|d| (d, 0)),
        ValueType::Decimal(decimal) => {
            Some((decimal.precision() - decimal.scale(), decimal.scale()))
        }
        _ => None,
    }
}

impl ScalarFunction {
    /// The stable name in canonical encoding and diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Concat => "concat",
            Self::Length => "length",
            Self::Lower => "lower",
            Self::Upper => "upper",
            Self::Trim => "trim",
            Self::LTrim => "ltrim",
            Self::RTrim => "rtrim",
            Self::Left => "left",
            Self::Right => "right",
            Self::Substring => "substring",
            Self::Replace => "replace",
            Self::SplitPart => "split-part",
            Self::Position => "position",
            Self::Contains => "contains",
            Self::StartsWith => "starts-with",
            Self::EndsWith => "ends-with",
            Self::Like => "like",
            Self::ILike => "ilike",
            Self::Abs => "abs",
            Self::Sign => "sign",
            Self::Floor => "floor",
            Self::Ceil => "ceil",
            Self::Round => "round",
            Self::Truncate => "truncate",
            Self::Mod { .. } => "mod",
            Self::Greatest => "greatest",
            Self::Least => "least",
            Self::Extract { .. } => "extract",
            Self::DateAdd { .. } => "date-add",
            Self::DateDiff { .. } => "date-diff",
            Self::ToDate => "to-date",
        }
    }

    /// The type and unit of a call with these arguments.
    ///
    /// # Errors
    /// Why the arguments do not fit: wrong count, wrong types, a missing
    /// constant, or a result no CSMIR type holds.
    pub fn result(
        self,
        arguments: &[ScalarExpr],
    ) -> Result<(ValueType, Option<Unit>), TypingError> {
        match self {
            Self::Concat => match arguments {
                [_, _, ..] if arguments.iter().all(text) => Ok((ValueType::Utf8, None)),
                _ => Err("CONCAT takes two or more strings"),
            },
            Self::Length => match arguments {
                [value] if text(value) => Ok((int64(), None)),
                _ => Err("LENGTH takes one string"),
            },
            Self::Lower | Self::Upper => match arguments {
                [value] if text(value) => Ok((ValueType::Utf8, None)),
                _ => Err("LOWER and UPPER take one string"),
            },
            Self::Trim | Self::LTrim | Self::RTrim => match arguments {
                [value] if text(value) => Ok((ValueType::Utf8, None)),
                [value, character] if text(value) && single_character(character) => {
                    Ok((ValueType::Utf8, None))
                }
                _ => Err("TRIM takes a string and, optionally, one constant character"),
            },
            Self::Left | Self::Right => match arguments {
                [value, count] if text(value) && integer(count) => Ok((ValueType::Utf8, None)),
                _ => Err("LEFT and RIGHT take a string and an integer"),
            },
            Self::Substring => match arguments {
                [value, start, length] if text(value) && integer(start) && integer(length) => {
                    Ok((ValueType::Utf8, None))
                }
                _ => Err("SUBSTRING takes a string, an integer start and an integer length"),
            },
            Self::Replace => match arguments {
                [_, _, _] if arguments.iter().all(text) => Ok((ValueType::Utf8, None)),
                _ => Err("REPLACE takes three strings"),
            },
            Self::SplitPart => match arguments {
                [value, delimiter, part] if text(value) && text(delimiter) && integer(part) => {
                    Ok((ValueType::Utf8, None))
                }
                _ => Err("SPLIT_PART takes two strings and an integer"),
            },
            Self::Position => match arguments {
                [_, _] if arguments.iter().all(text) => Ok((int64(), None)),
                _ => Err("POSITION takes two strings"),
            },
            Self::Contains | Self::StartsWith | Self::EndsWith | Self::Like | Self::ILike => {
                match arguments {
                    [_, _] if arguments.iter().all(text) => Ok((ValueType::Boolean, None)),
                    _ => Err("this text predicate takes two strings"),
                }
            }
            Self::Abs => match arguments {
                [value] if exact(value) => Ok((value.value_type.clone(), value.unit.clone())),
                _ => Err("ABS takes one integer or decimal"),
            },
            Self::Sign => match arguments {
                [value] if exact(value) => Ok((int64(), None)),
                _ => Err("SIGN takes one integer or decimal"),
            },
            Self::Floor | Self::Ceil => match arguments {
                [value] => match &value.value_type {
                    ValueType::Integer { .. } => Ok((value.value_type.clone(), value.unit.clone())),
                    ValueType::Decimal(d) if d.scale() == 0 => {
                        Ok((value.value_type.clone(), value.unit.clone()))
                    }
                    ValueType::Decimal(d) => {
                        // 9.5 rounds up to 10: one more integral digit.
                        let integral = d.precision() - d.scale();
                        Ok((decimal(integral.saturating_add(1), 0)?, value.unit.clone()))
                    }
                    _ => Err("FLOOR and CEIL take one integer or decimal"),
                },
                _ => Err("FLOOR and CEIL take one integer or decimal"),
            },
            Self::Round | Self::Truncate => self.rounded(arguments),
            Self::Mod { .. } => match arguments {
                [dividend, divisor] => Self::modulo(dividend, divisor),
                _ => Err("MOD takes two integers or decimals"),
            },
            Self::Extract { .. } | Self::DateAdd { .. } | Self::DateDiff { .. } | Self::ToDate => {
                self.dated(arguments)
            }
            Self::Greatest | Self::Least => match arguments {
                [first, _, ..]
                    if ordered(&first.value_type)
                        && arguments.iter().all(|argument| {
                            argument.value_type == first.value_type && argument.unit == first.unit
                        }) =>
                {
                    Ok((first.value_type.clone(), first.unit.clone()))
                }
                _ => Err(
                    "GREATEST and LEAST take two or more values of one ordered type (numbers, \
                     text, booleans, dates or times) and one unit",
                ),
            },
        }
    }

    fn dated(self, arguments: &[ScalarExpr]) -> Result<(ValueType, Option<Unit>), TypingError> {
        match self {
            Self::Extract { part } => match arguments {
                [value] if temporal(value, part, false) => Ok((int64(), None)),
                [value] if instant(value) => {
                    Err("an instant has no calendar fields: bucket it to its local date first")
                }
                _ => Err("EXTRACT takes a date (calendar parts) or a local date-time"),
            },
            Self::DateAdd { part } => match arguments {
                [count, value]
                    if integer(count) && !part.is_position() && temporal(value, part, true) =>
                {
                    Ok((value.value_type.clone(), None))
                }
                _ => Err("DATEADD takes a part, an integer and a date or date-time that admits it"),
            },
            Self::DateDiff { part } => match arguments {
                [start, end]
                    if start.value_type == end.value_type
                        && !part.is_position()
                        && temporal(start, part, true)
                        && (!instant(start) || part.instant_boundaries(0, 0).is_some()) =>
                {
                    Ok((int64(), None))
                }
                _ => Err(
                    "DATEDIFF takes a part and two values of one date or date-time type that admits it",
                ),
            },
            Self::ToDate => match arguments {
                [value] if text(value) => Ok((ValueType::Date, None)),
                _ => Err("TO_DATE takes one ISO 8601 date string"),
            },
            _ => Err("not a date function"),
        }
    }

    fn rounded(self, arguments: &[ScalarExpr]) -> Result<(ValueType, Option<Unit>), TypingError> {
        let (value, places) = match arguments {
            [value] => (value, None),
            [value, places] if integer(places) => (value, Some(places)),
            _ => return Err("ROUND and TRUNCATE take a number and optional integer digits"),
        };
        let places = digits(places)?;
        let unit = value.unit.clone();
        match &value.value_type {
            ValueType::Integer { .. } => Ok((value.value_type.clone(), unit)),
            ValueType::Decimal(d) if places >= d.scale() => Ok((value.value_type.clone(), unit)),
            ValueType::Decimal(d) => {
                let integral = d.precision() - d.scale();
                // Rounding can carry into a new integral digit (9.99 → 10.0);
                // truncation cannot.
                let carry = u8::from(self == Self::Round);
                Ok((decimal(integral + places + carry, places)?, unit))
            }
            _ => Err("ROUND and TRUNCATE take an integer or decimal"),
        }
    }

    /// `|result| < |divisor|` and `|result| <= |dividend|`, with the finer
    /// scale of the two.
    fn modulo(
        dividend: &ScalarExpr,
        divisor: &ScalarExpr,
    ) -> Result<(ValueType, Option<Unit>), TypingError> {
        if divisor.unit.is_some() && divisor.unit != dividend.unit {
            return Err("the divisor of MOD has no unit or the dividend's");
        }
        let unit = dividend.unit.clone();
        if integer(dividend) && integer(divisor) {
            return Ok((dividend.value_type.clone(), unit));
        }
        let (Some((left_integral, left_scale)), Some((right_integral, right_scale))) = (
            digits_of(&dividend.value_type),
            digits_of(&divisor.value_type),
        ) else {
            return Err(
                "MOD takes two integers or decimals; an unbounded integer mixes with a decimal                  only after a CAST",
            );
        };
        let scale = left_scale.max(right_scale);
        Ok((
            decimal(left_integral.min(right_integral) + scale, scale)?,
            unit,
        ))
    }
}
