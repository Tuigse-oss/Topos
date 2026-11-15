//! Text the map shows: byte counts, digit grouping, and file times.

use chrono::{DateTime, Local, TimeZone};

/// Human-readable byte counts in the "917.6 Mb" style: binary multiples,
/// one decimal, exact bytes below 1 Kb.
#[must_use]
pub fn pretty_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["bytes", "Kb", "Mb", "Gb", "Tb", "Pb"];
    #[allow(clippy::cast_precision_loss)] // display only; one decimal shown
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Digits grouped in threes: `352972824576` → `352,972,824,576`.
#[must_use]
pub fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// An exact byte count: `142,036,959,232 bytes`.
#[must_use]
pub fn exact_bytes(bytes: u64) -> String {
    format!("{} bytes", group_digits(bytes))
}

/// A file time (seconds since the Unix epoch) in local time,
/// `06 Nov 2026 14:31:14`; `None` when out of range.
#[must_use]
pub fn local_time(seconds: i64) -> Option<String> {
    time_in(seconds, &Local)
}

/// [`local_time`] for any time zone.
#[must_use]
pub fn time_in<Tz: TimeZone>(seconds: i64, zone: &Tz) -> Option<String>
where
    Tz::Offset: std::fmt::Display,
{
    let utc = DateTime::from_timestamp(seconds, 0)?;
    Some(
        utc.with_timezone(zone)
            .format("%d %b %Y %H:%M:%S")
            .to_string(),
    )
}

/// A share of a whole as a percentage with one decimal, `2.7%`; `0.0%` for
/// an empty whole.
#[must_use]
pub fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        return String::from("0.0%");
    }
    #[allow(clippy::cast_precision_loss)] // display only
    let share = part as f64 * 100.0 / whole as f64;
    format!("{share:.1}%")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    #[test]
    fn pretty_bytes_uses_binary_units() {
        assert_eq!(pretty_bytes(0), "0 bytes");
        assert_eq!(pretty_bytes(1023), "1023 bytes");
        assert_eq!(pretty_bytes(1024), "1.0 Kb");
        assert_eq!(pretty_bytes(962_150_000), "917.6 Mb");
        assert_eq!(pretty_bytes(13_529_146_982), "12.6 Gb");
        assert_eq!(pretty_bytes(u64::MAX), "16384.0 Pb");
    }

    #[test]
    fn digits_group_in_threes() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(352_972_824_576), "352,972,824,576");
        assert_eq!(group_digits(u64::MAX), "18,446,744,073,709,551,615");
        assert_eq!(exact_bytes(1234), "1,234 bytes");
    }

    #[test]
    fn times_render_in_the_given_zone() {
        // 2026-11-06 22:31:14 UTC.
        let seconds = 1_794_004_274;
        let utc = FixedOffset::east_opt(0).unwrap();
        let pacific = FixedOffset::west_opt(8 * 3600).unwrap();
        assert_eq!(
            time_in(seconds, &utc).as_deref(),
            Some("06 Nov 2026 22:31:14")
        );
        assert_eq!(
            time_in(seconds, &pacific).as_deref(),
            Some("06 Nov 2026 14:31:14")
        );
        assert_eq!(time_in(0, &utc).as_deref(), Some("01 Jan 1970 00:00:00"));
        assert!(time_in(i64::MAX, &utc).is_none());
    }

    #[test]
    fn percent_has_one_decimal() {
        assert_eq!(percent(27, 1000), "2.7%");
        assert_eq!(percent(1, 3), "33.3%");
        assert_eq!(percent(5, 0), "0.0%");
    }
}
