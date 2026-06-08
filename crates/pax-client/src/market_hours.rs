//! US equity regular-trading-hours (RTH) check — dependency-free.
//!
//! RTH = Mon–Fri, 09:30–16:00 America/New_York. The Eastern offset is derived from the
//! US DST rule (EDT = UTC−4 from the 2nd Sunday of March to the 1st Sunday of November;
//! EST = UTC−5 otherwise). Holidays are NOT modelled — on a market holiday orders would
//! simply rest/reject and the master won't change, so the practical impact is nil.

use std::time::{SystemTime, UNIX_EPOCH};

/// True if *now* (system clock, in UTC) falls within US equity regular trading hours.
pub fn is_us_equity_rth_now() -> bool {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    is_rth_at(secs)
}

/// True if *now* falls on a US-Eastern weekend (Saturday or Sunday). Used to exclude
/// weekends from disconnect-alert timing (markets are closed, so a disconnect is benign).
pub fn is_weekend_et_now() -> bool {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let offset_hours: i64 = if is_us_dst(y, m, d) { -4 } else { -5 };
    let east_days = (secs + offset_hours * 3600).div_euclid(86_400);
    let weekday = (east_days.rem_euclid(7) + 4).rem_euclid(7); // 0 = Sunday
    weekday == 0 || weekday == 6
}

/// True if the given UNIX timestamp (UTC seconds) is within US equity RTH.
pub fn is_rth_at(utc_secs: i64) -> bool {
    // Pick the Eastern offset from the UTC calendar date (the 1-hour ambiguity at the 02:00
    // DST switch is irrelevant to the 09:30–16:00 window).
    let (y, m, d) = civil_from_days(utc_secs.div_euclid(86_400));
    let offset_hours: i64 = if is_us_dst(y, m, d) { -4 } else { -5 };

    let east_secs = utc_secs + offset_hours * 3600;
    let east_days = east_secs.div_euclid(86_400);
    let sod = east_secs.rem_euclid(86_400); // seconds into the Eastern day

    // 0 = Sunday. Unix day 0 (1970-01-01) was a Thursday → +4 phase shift.
    let weekday = (east_days.rem_euclid(7) + 4).rem_euclid(7);
    if weekday == 0 || weekday == 6 {
        return false; // weekend
    }
    let minutes = sod / 60;
    (570i64..960).contains(&minutes) // 09:30 (inclusive) .. 16:00 (exclusive)
}

/// Is the US in daylight-saving time on this (Eastern-ish) calendar date?
fn is_us_dst(y: i64, m: u32, d: u32) -> bool {
    match m {
        1 | 2 | 12 => false,
        4..=10 => true,
        3 => d >= nth_sunday(y, 3, 2),  // DST begins 2nd Sunday of March
        11 => d < nth_sunday(y, 11, 1), // DST ends 1st Sunday of November
        _ => false,
    }
}

/// Day-of-month of the `n`-th Sunday of `(year, month)` (n = 1 → first).
fn nth_sunday(y: i64, m: u32, n: u32) -> u32 {
    let first_wd = (days_from_civil(y, m, 1).rem_euclid(7) + 4).rem_euclid(7) as u32; // 0 = Sun
    let first_sunday = 1 + ((7 - first_wd) % 7);
    first_sunday + (n - 1) * 7
}

// Howard Hinnant's proleptic-Gregorian date algorithms (public domain).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { (m - 3) as i64 } else { (m + 9) as i64 };
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a UTC timestamp for a civil UTC date/time.
    fn utc(y: i64, m: u32, d: u32, hh: i64, mm: i64) -> i64 {
        days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60
    }

    #[test]
    fn winter_est_window() {
        // 2024-01-03 is a Wednesday. EST = UTC−5, so 10:00 ET = 15:00 UTC.
        assert!(is_rth_at(utc(2024, 1, 3, 15, 0)), "10:00 ET should be RTH");
        assert!(!is_rth_at(utc(2024, 1, 3, 14, 0)), "09:00 ET is before the open");
        assert!(!is_rth_at(utc(2024, 1, 3, 21, 30)), "16:30 ET is after the close");
        assert!(is_rth_at(utc(2024, 1, 3, 14, 30)), "09:30 ET is the open (inclusive)");
        assert!(!is_rth_at(utc(2024, 1, 3, 21, 0)), "16:00 ET is the close (exclusive)");
    }

    #[test]
    fn summer_edt_window() {
        // 2024-07-03 is a Wednesday. EDT = UTC−4, so 10:00 ET = 14:00 UTC.
        assert!(is_rth_at(utc(2024, 7, 3, 14, 0)), "10:00 ET should be RTH in summer");
        assert!(!is_rth_at(utc(2024, 7, 3, 13, 0)), "09:00 ET is before the open");
    }

    #[test]
    fn weekend_is_closed() {
        // 2024-01-06 is a Saturday, 2024-01-07 a Sunday — never RTH.
        assert!(!is_rth_at(utc(2024, 1, 6, 16, 0)));
        assert!(!is_rth_at(utc(2024, 1, 7, 16, 0)));
    }
}
