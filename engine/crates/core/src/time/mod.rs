//! Time conversions.
//!
//! It covers `SystemTime` to and from Unix milliseconds and seconds, the two Windows tick formats
//! the wire carries (100-ns ticks since 0001-01-01, and FILETIME: 100-ns ticks since 1601-01-01),
//! proleptic Gregorian calendar arithmetic, and parsing the RFC 3339 date-times the services send.
//!
//! Every conversion is total: a value outside `i64` saturates, and a time before 1970 gives a
//! negative count. A caller that needs a different rule (clamping to the epoch, failing) says so at
//! the call site.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 100-ns ticks since 0001-01-01 at the Unix epoch.
pub const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;

/// FILETIME (100-ns ticks since 1601-01-01) at the Unix epoch.
pub const UNIX_EPOCH_FILETIME: i64 = 116_444_736_000_000_000;

/// Unix milliseconds of FILETIME zero (1601-01-01T00:00:00Z).
pub const FILETIME_ZERO_UNIX_MS: i64 = -(UNIX_EPOCH_FILETIME / 10_000);

/// Milliseconds since the Unix epoch, negative before it.
pub fn unix_ms(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
    }
}

/// Seconds since the Unix epoch, negative before it.
pub fn unix_secs(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// The time `ms` milliseconds after (before, if negative) the Unix epoch.
///
/// # Panics
///
/// If the time is outside what `SystemTime` represents (hundreds of millions of years away).
pub fn system_time(ms: i64) -> SystemTime {
    checked_system_time(ms).expect("a millisecond count within SystemTime's range")
}

/// The time `ms` milliseconds after (before, if negative) the Unix epoch, if `SystemTime` holds it.
pub fn checked_system_time(ms: i64) -> Option<SystemTime> {
    let offset = Duration::from_millis(ms.unsigned_abs());
    if ms >= 0 { UNIX_EPOCH.checked_add(offset) } else { UNIX_EPOCH.checked_sub(offset) }
}

/// Whole milliseconds of `d`, saturating.
pub fn duration_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Unix milliseconds as 100-ns ticks since 0001-01-01, saturating.
pub fn ticks_from_unix_ms(ms: i64) -> i64 {
    UNIX_EPOCH_TICKS.saturating_add(ms.saturating_mul(10_000))
}

/// 100-ns ticks since 0001-01-01 as Unix milliseconds, rounded down.
pub fn unix_ms_from_ticks(ticks: i64) -> i64 {
    ticks.saturating_sub(UNIX_EPOCH_TICKS).div_euclid(10_000)
}

#[cfg(feature = "test-support")]
/// Unix milliseconds as FILETIME.
pub fn filetime_from_unix_ms(ms: i64) -> i64 {
    (ms - FILETIME_ZERO_UNIX_MS) * 10_000
}

#[cfg(feature = "test-support")]
/// FILETIME as Unix milliseconds, rounded down.
pub fn unix_ms_from_filetime(filetime: i64) -> i64 {
    filetime.div_euclid(10_000) + FILETIME_ZERO_UNIX_MS
}

/// `t` as FILETIME at 100-ns precision; a time before the Unix epoch counts as the epoch.
pub fn filetime(t: SystemTime) -> i64 {
    let since_epoch = t.duration_since(UNIX_EPOCH).map_or(0, |d| (d.as_nanos() / 100) as i64);
    since_epoch + UNIX_EPOCH_FILETIME
}

/// Whether `year` is a Gregorian leap year.
pub fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// The days in `month` (1–12) of `year`; any other month counts 31.
pub fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's `days_from_civil`).
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The proleptic Gregorian `(year, month, day)` of `days` since 1970-01-01 (H. Hinnant's
/// `civil_from_days`).
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Parses a date-time with an offset (`YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)`) to milliseconds
/// since the Unix epoch (fractions below a millisecond are dropped).
///
/// `None` for anything else, a date-time without an offset included.
pub fn parse_date_time(value: &str) -> Option<i64> {
    let b = value.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't') || b[13] != b':' || b[16] != b':' {
        return None;
    }

    let num = |a: usize, z: usize| -> Option<i64> {
        let s = value.get(a..z)?;
        s.bytes().all(|c| c.is_ascii_digit()).then(|| s.parse().ok()).flatten()
    };

    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }

    let mut rest = &value[19..];
    let mut millis = 0i64;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        for (i, c) in fraction[..digits].bytes().take(3).enumerate() {
            millis += i64::from(c - b'0') * 10i64.pow(2 - i as u32);
        }
        rest = &fraction[digits..];
    }

    let offset_secs = match rest {
        "Z" | "z" => 0,
        _ => {
            let ob = rest.as_bytes();
            if ob.len() != 6 || !matches!(ob[0], b'+' | b'-') || ob[3] != b':' {
                return None;
            }

            let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit()).then(|| s.parse::<i64>().ok()).flatten();
            let (oh, om) = (digits(&rest[1..3])?, digits(&rest[4..6])?);
            if oh > 14 || om > 59 {
                return None;
            }

            let secs = oh * 3600 + om * 60;
            if ob[0] == b'+' { secs } else { -secs }
        }
    };

    let days = days_from_civil(year, month, day);
    let local = days * 86_400 + hour * 3600 + minute * 60 + second;
    Some((local - offset_secs) * 1000 + millis)
}

#[cfg(test)]
mod tests;
