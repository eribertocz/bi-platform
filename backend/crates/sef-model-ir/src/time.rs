use crate::diagnostic::{Diagnostic, ErrorCode};
use crate::identity::StableId;

/// Syntactically checked declared zone. Membership and TZDB execution belong to adapters.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct IanaZoneId(String);

impl IanaZoneId {
    /// # Errors
    ///
    /// Returns `InvalidTemporalContract` if the declared zone identifier is
    /// empty or contains an invalid path segment. Does not check TZDB membership.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        if value.is_empty()
            || !value.split('/').all(|segment| {
                !segment.is_empty()
                    && segment != "."
                    && segment != ".."
                    && segment
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_+-".contains(&b))
            })
        {
            return Err(Diagnostic::error(
                ErrorCode::InvalidTemporalContract,
                None,
                "Malformed declared IANA timezone identifier",
            ));
        }
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub enum LocalTimeResolution {
    #[default]
    Reject,
    Earlier,
    Later,
    ShiftForward,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum TimestampSemantics {
    Instant,
    LocalDateTime {
        source_timezone: IanaZoneId,
        local_time_resolution: LocalTimeResolution,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum TimeRole {
    EventTime,
    SnapshotTime,
    ValidityStart,
    ValidityEnd,
    ProcessingTime,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum CalendarProfile {
    Gregorian,
    IsoWeek,
    Custom { id: StableId },
    Fiscal { id: StableId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum TimeGrain {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct TimeBucket {
    pub source: StableId,
    pub timezone: IanaZoneId,
    pub calendar: CalendarProfile,
    pub grain: TimeGrain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeSemantics {
    pub role: TimeRole,
    pub timezone: IanaZoneId,
    pub calendar: CalendarProfile,
    pub pinned_tzdb_version: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum NullPlacement {
    First,
    Last,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct OrderTerm {
    pub value: StableId,
    pub direction: SortDirection,
    pub nulls: NullPlacement,
}

/// Days since 1970-01-01 of an ISO `YYYY-MM-DD` date in the proleptic
/// Gregorian calendar; `None` for anything else (wrong shape, month 13,
/// February 29 of a common year…).
#[must_use]
pub fn iso_date_days(text: &str) -> Option<i32> {
    let mut parts = text.splitn(3, '-');
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if !(digits(year) && digits(month) && digits(day)) {
        return None;
    }
    let (year, month, day): (i64, i64, i64) =
        (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=days_in_month).contains(&day) {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i32::try_from(era * 146_097 + doe - 719_468).ok()
}
