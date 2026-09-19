//! `NovaTimestamp` — NovaDB's timestamp type.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds in a day, a second, and a minute.
const MS_PER_DAY: i64 = 86_400_000;
const MS_PER_HOUR: i64 = 3_600_000;
const MS_PER_MINUTE: i64 = 60_000;
const MS_PER_SECOND: i64 = 1_000;

/// A timestamp measured in milliseconds since the Unix epoch.
///
/// Using a signed value allows pre-1970 instants. `NovaTimestamp` is a `Copy`
/// type; equality and ordering compare raw epoch milliseconds, so timestamps
/// sort chronologically.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NovaTimestamp {
    millis: i64,
}

impl NovaTimestamp {
    /// The current wall-clock time as a `NovaTimestamp`.
    ///
    /// If the system clock is before the Unix epoch, this returns the epoch
    /// itself (seconds-resolution clocks never trigger this path in practice).
    #[must_use]
    pub fn now() -> Self {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        Self { millis }
    }

    /// Builds a timestamp from raw unix epoch milliseconds.
    #[must_use]
    pub const fn from_millis(millis: i64) -> Self {
        Self { millis }
    }

    /// The raw unix epoch milliseconds.
    #[must_use]
    pub const fn millis(&self) -> i64 {
        self.millis
    }

    /// Formats this timestamp as an ISO-8601 UTC string:
    /// `YYYY-MM-DDTHH:MM:SS.mmmZ`.
    ///
    /// This is a deterministic, allocation-per-call conversion using Howard
    /// Hinnant's civil-from-days algorithm; NovaDB deliberately does not depend
    /// on a full date-time crate for this.
    #[must_use]
    pub fn to_iso8601(&self) -> String {
        // Division with negative dividends must round toward negative infinity
        // so that a correct day + time-of-day decomposition is produced.
        let days = self.millis.div_euclid(MS_PER_DAY);
        let rem = self.millis.rem_euclid(MS_PER_DAY);

        let (year, month, day) = civil_from_days(days);
        let hour = rem / MS_PER_HOUR;
        let minute = rem / MS_PER_MINUTE % 60;
        let second = rem / MS_PER_SECOND % 60;
        let millis = rem % MS_PER_SECOND;

        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
    }
}

/// Converts a count of days since 1970-01-01 into a proleptic-Gregorian
/// `(year, month, day)` triple. Works for negative day counts (pre-1970).
///
/// Algorithm: Howard Hinnant, "chrono-Compatible Low-Level Date Algorithms".
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if month <= 2 { year + 1 } else { year }, month, day)
}

impl fmt::Display for NovaTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_iso8601())
    }
}

impl fmt::Debug for NovaTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NovaTimestamp({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iso(millis: i64) -> String {
        NovaTimestamp::from_millis(millis).to_iso8601()
    }

    #[test]
    fn known_instants_format_exactly() {
        // 1970-01-01T00:00:00.000Z
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        // 1970-01-02T00:00:00.000Z
        assert_eq!(iso(MS_PER_DAY), "1970-01-02T00:00:00.000Z");
        // 2000-01-01T00:00:00.000Z
        assert_eq!(iso(946_684_800_000), "2000-01-01T00:00:00.000Z");
        // 2005-01-01T00:00:00.000Z
        assert_eq!(iso(1_104_537_600_000), "2005-01-01T00:00:00.000Z");
        // 2023-11-14T22:13:20.000Z
        assert_eq!(iso(1_700_000_000_000), "2023-11-14T22:13:20.000Z");
        // Sub-second component survives.
        assert_eq!(iso(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
        // Rounding in seconds field.
        assert_eq!(iso(1_700_000_059_999), "2023-11-14T22:14:19.999Z");
    }

    #[test]
    fn pre_epoch_instants_format_exactly() {
        // One millisecond before the epoch.
        assert_eq!(iso(-1), "1969-12-31T23:59:59.999Z");
        // 1969-01-01T00:00:00.000Z is day -365.
        assert_eq!(iso(-365 * MS_PER_DAY), "1969-01-01T00:00:00.000Z");
        // A full day before the epoch.
        assert_eq!(iso(-MS_PER_DAY), "1969-12-31T00:00:00.000Z");
    }

    #[test]
    fn now_is_recent_and_sane() {
        let now = NovaTimestamp::now();
        let lower = NovaTimestamp::from_millis(now.millis() - MS_PER_DAY);
        let upper = NovaTimestamp::from_millis(now.millis() + MS_PER_DAY);
        assert!(
            now > lower && now < upper,
            "clock produced a wild timestamp"
        );
    }

    #[test]
    fn ordering_follows_millis() {
        let a = NovaTimestamp::from_millis(1000);
        let b = NovaTimestamp::from_millis(2000);
        assert!(a < b);
        assert_eq!(a.min(b), a);
        assert_eq!(b.max(a), b);
    }

    #[test]
    fn display_matches_iso8601() {
        let ts = NovaTimestamp::from_millis(1_700_000_000_123);
        assert_eq!(ts.to_string(), ts.to_iso8601());
    }

    #[test]
    fn debug_prints_the_timestamp() {
        let ts = NovaTimestamp::from_millis(1_700_000_000_123);
        assert_eq!(format!("{ts:?}"), "NovaTimestamp(2023-11-14T22:13:20.123Z)");
    }
}
