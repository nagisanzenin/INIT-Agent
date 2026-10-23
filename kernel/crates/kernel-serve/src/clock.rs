//! RFC 3339 UTC timestamps with milliseconds (`2026-10-19T12:36:00.981Z`) without a date crate
//! (Howard Hinnant's civil-from-days), and UTC day numbers for the daily quotas.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    }
}

/// The current time as RFC 3339 UTC with milliseconds.
pub fn now_rfc3339() -> String {
    format_ms(now_ms())
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// ms since the epoch → `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn format_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z", sod / 3600, (sod / 60) % 60, sod % 60)
}

/// The UTC day number (days since 1970-01-01) of `ms`.
pub fn utc_day(ms: i64) -> i64 {
    ms.div_euclid(86_400_000)
}

/// `YYYY-MM-DD` of a UTC day number.
pub fn day_string(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// RFC 3339 of the start of a UTC day.
pub fn day_start(day: i64) -> String {
    format_ms(day * 86_400_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(format_ms(0), "1970-01-01T00:00:00.000Z");
        // 2026-10-19T12:36:00.981Z
        let ms = 1_792_413_360_981;
        assert_eq!(format_ms(ms), "2026-10-19T12:36:00.981Z");
        assert_eq!(day_string(utc_day(ms)), "2026-10-19");
        assert_eq!(day_start(utc_day(ms) + 1), "2026-10-20T00:00:00.000Z");
        assert_eq!(format_ms(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }
}
