//! `DATEDIFF` between instants (#77): boundaries of the UTC epoch, the
//! contract every adapter reproduces whatever its session zone.
#![allow(clippy::expect_used)]

use proptest::prelude::*;
use sef_model_ir::functions::DatePart;

const SECOND: i64 = 1_000_000;
const MINUTE: i64 = 60 * SECOND;
/// 1900-01-01T00:00:00Z, when `America/La_Paz` was -04:32:36.
const Y1900: i64 = -2_208_988_800 * SECOND;

fn minutes(start: i64, end: i64) -> i64 {
    DatePart::Minute
        .instant_boundaries(start, end)
        .expect("minutes")
}

fn seconds(start: i64, end: i64) -> i64 {
    DatePart::Second
        .instant_boundaries(start, end)
        .expect("seconds")
}

#[test]
fn normative_examples() {
    // A local clock would see one minute boundary here (19:27:44 → 19:28:04
    // in La Paz); the UTC epoch sees none.
    assert_eq!(minutes(Y1900 + 20 * SECOND, Y1900 + 40 * SECOND), 0);
    // One second elapsed, one boundary crossed.
    assert_eq!(minutes(59 * SECOND, 60 * SECOND), 1);
    // Almost a minute elapsed, no boundary crossed.
    assert_eq!(minutes(0, MINUTE - 1), 0);
    // Earlier ends count negatively, but not within the same unit.
    assert_eq!(minutes(60 * SECOND, 59 * SECOND), -1);
    assert_eq!(minutes(40 * SECOND, 20 * SECOND), 0);
    // Fractions of a second, also before 1970 (floor, not truncation).
    assert_eq!(seconds(-SECOND / 2, SECOND / 5), 1);
    assert_eq!(seconds(SECOND / 5, 9 * SECOND / 10), 0);
    assert_eq!(seconds(-3 * SECOND / 2, -SECOND), 1);
    assert_eq!(minutes(-1, 0), 1);
    // The whole range of instants, without overflow.
    assert_eq!(seconds(i64::MIN, i64::MAX), 18_446_744_073_709);
}

#[test]
fn hours_and_calendar_parts_are_not_counted_on_instants() {
    for part in [
        DatePart::Hour,
        DatePart::Day,
        DatePart::Week,
        DatePart::Month,
        DatePart::Quarter,
        DatePart::Year,
        DatePart::DayOfWeek,
        DatePart::DayOfYear,
    ] {
        assert_eq!(part.instant_boundaries(0, MINUTE), None, "{part:?}");
    }
}

fn part() -> impl Strategy<Value = (DatePart, i64)> {
    prop_oneof![
        Just((DatePart::Second, SECOND)),
        Just((DatePart::Minute, MINUTE))
    ]
}

/// Instants within about ±3,000 years of 1970, so shifts cannot overflow.
fn instant() -> impl Strategy<Value = i64> {
    -100_000_000_000_000_000..100_000_000_000_000_000_i64
}

proptest! {
    #[test]
    fn counts_are_antisymmetric_and_additive(
        (part, _) in part(),
        a in instant(),
        b in instant(),
        c in instant(),
    ) {
        let count = |x, y| part.instant_boundaries(x, y).expect("counted");
        prop_assert_eq!(count(a, b), -count(b, a));
        prop_assert_eq!(count(a, b) + count(b, c), count(a, c));
    }

    #[test]
    fn a_count_is_within_one_unit_of_the_elapsed_time(
        (part, unit) in part(),
        a in instant(),
        b in instant(),
    ) {
        let count = part.instant_boundaries(a, b).expect("counted");
        let gap = i128::from(count) * i128::from(unit) - (i128::from(b) - i128::from(a));
        prop_assert!(gap.abs() < i128::from(unit), "count {count} for {a} → {b}");
    }

    #[test]
    fn shifting_both_instants_by_whole_units_keeps_the_count(
        (part, unit) in part(),
        a in instant(),
        b in instant(),
        k in -1_000_000_000_i64..1_000_000_000,
    ) {
        let count = |x, y| part.instant_boundaries(x, y).expect("counted");
        prop_assert_eq!(count(a + k * unit, b + k * unit), count(a, b));
    }
}
