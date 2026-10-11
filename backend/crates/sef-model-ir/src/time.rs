use crate::diagnostic::{Diagnostic, ErrorCode};
use crate::identity::StableId;

/// Syntactically checked declared zone. Membership and TZDB execution belong to adapters.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct IanaZoneId(String);

impl IanaZoneId {
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
