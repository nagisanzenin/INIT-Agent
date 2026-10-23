//! RFC 3339 UTC timestamps with milliseconds (`2026-10-19T12:36:00.981Z`, supervise.md §4.3)
//! without a date crate: Howard Hinnant's civil-from-days.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    to_ms(SystemTime::now())
}

/// `SystemTime` → ms since the epoch (negative before 1970).
pub fn to_ms(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    }
}

/// The current time as RFC 3339 UTC with milliseconds.
pub fn now_rfc3339() -> String {
    format_ms(now_ms())
}

/// `SystemTime` as RFC 3339.
pub fn rfc3339(t: SystemTime) -> String {
    format_ms(to_ms(t))
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

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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

/// `YYYYMMDD-HHMMSS` in UTC (session ids, §3.4).
pub fn compact_utc(ms: i64) -> String {
    let s = format_ms(ms);
    format!("{}{}{}-{}{}{}", &s[0..4], &s[5..7], &s[8..10], &s[11..13], &s[14..16], &s[17..19])
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.fff…]Z` (UTC only) to ms since the epoch.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b't') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)? as u32, num(8..10)? as u32);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let rest = &s[19..];
    let (frac, tz) = match rest.strip_prefix('.') {
        Some(r) => {
            let n = r.bytes().take_while(u8::is_ascii_digit).count();
            if n == 0 {
                return None;
            }
            let digits = &r[..n];
            let ms = format!("{:0<3}", &digits[..n.min(3)]).parse::<i64>().ok()?;
            (ms, &r[n..])
        }
        None => (0, rest),
    };
    if tz != "Z" && tz != "z" {
        return None;
    }
    Some((days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se) * 1000 + frac)
}

/// Age of an RFC 3339 timestamp relative to now (zero when in the future or unparsable → None).
pub fn age(ts: &str) -> Option<Duration> {
    let t = parse_rfc3339(ts)?;
    Some(Duration::from_millis(now_ms().saturating_sub(t).max(0) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_instants() {
        assert_eq!(format_ms(0), "1970-01-01T00:00:00.000Z");
        // 2026-10-19T12:36:00.981Z
        let t = parse_rfc3339("2026-10-19T12:36:00.981Z").unwrap();
        assert_eq!(format_ms(t), "2026-10-19T12:36:00.981Z");
        assert_eq!(compact_utc(t), "20261019-123600");
        assert_eq!(parse_rfc3339("2024-02-29T23:59:59Z").map(format_ms).unwrap(), "2024-02-29T23:59:59.000Z");
        assert_eq!(parse_rfc3339("2026-10-19T12:36:00.9Z").unwrap() % 1000, 900);
        assert!(parse_rfc3339("2026-10-19 12:36:00Z").is_none());
        assert!(parse_rfc3339("2026-10-19T12:36:00+01:00").is_none());
        for ms in [-86_400_001i64, 951_782_400_000, 4_102_444_800_123] {
            assert_eq!(parse_rfc3339(&format_ms(ms)), Some(ms));
        }
    }
}
