//! UTC civil time without a date crate: Unix ms ↔ RFC 3339, dates, weekdays.

use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock Unix milliseconds.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Days since 1970-01-01 → (year, month 1..=12, day 1..=31). Howard Hinnant's algorithm.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
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

/// (year, month, day) → days since 1970-01-01.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 0 = Monday … 6 = Sunday, for days since the epoch (1970-01-01 was a Thursday).
pub fn weekday(days: i64) -> u32 {
    (days + 3).rem_euclid(7) as u32
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn rfc3339_ms(ms: u64) -> String {
    let days = (ms / 86_400_000) as i64;
    let rem = ms % 86_400_000;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}

/// `YYYY-MM-DDTHH:MM:SSZ` (whole seconds).
pub fn rfc3339_s(ms: u64) -> String {
    let s = rfc3339_ms(ms);
    format!("{}Z", &s[..19])
}

/// Parse RFC 3339 (`Z` or `±HH:MM`, optional fraction) into Unix ms.
pub fn parse_rfc3339(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || !(b[10] == b'T' || b[10] == b't' || b[10] == b' ')
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = s.get(r)?;
        if !t.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        t.parse().ok()
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut i = 19;
    let mut frac_ms = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let digits = &s[start..i];
        if digits.is_empty() {
            return None;
        }
        let mut ms_digits: String = digits.chars().take(3).collect();
        while ms_digits.len() < 3 {
            ms_digits.push('0');
        }
        frac_ms = ms_digits.parse().ok()?;
    }
    let offset_min = match b.get(i) {
        Some(b'Z') | Some(b'z') if i + 1 == b.len() => 0,
        Some(&c @ (b'+' | b'-')) if i + 6 == b.len() && b[i + 3] == b':' => {
            let oh = num(i + 1..i + 3)?;
            let om = num(i + 4..i + 6)?;
            let v = oh * 60 + om;
            if c == b'+' { v } else { -v }
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo as u32, d as u32);
    let ms = ((days * 86_400 + h * 3600 + mi * 60 + se - offset_min * 60) * 1000) + frac_ms;
    u64::try_from(ms).ok()
}

/// Parse a fixed offset `±HH:MM` into minutes.
pub fn parse_offset(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 6 || b[3] != b':' {
        return None;
    }
    let sign = match b[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let h: i64 = s.get(1..3)?.parse().ok()?;
    let m: i64 = s.get(4..6)?.parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(sign * (h * 60 + m))
}

/// `YYYY-MM-DD` of `ms` shifted by `offset_min`.
pub fn date_at_offset(ms: u64, offset_min: i64) -> String {
    let local = ms as i64 + offset_min * 60_000;
    let (y, m, d) = civil_from_days(local.div_euclid(86_400_000));
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!(rfc3339_ms(0), "1970-01-01T00:00:00.000Z");
        let t = parse_rfc3339("2026-10-19T13:24:07.123Z").unwrap();
        assert_eq!(rfc3339_ms(t), "2026-10-19T13:24:07.123Z");
        assert_eq!(
            parse_rfc3339("2026-10-19T17:00:00Z"),
            parse_rfc3339("2026-10-20T01:00:00+08:00")
        );
        assert_eq!(rfc3339_s(t), "2026-10-19T13:24:07Z");
        assert_eq!(parse_rfc3339("2026-13-19T17:00:00Z"), None);
        assert_eq!(parse_rfc3339("nonsense"), None);
        for days in [-1000i64, 0, 59, 60, 365, 20_000, 20_745] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        // 2026-10-19 is a Monday.
        assert_eq!(weekday(days_from_civil(2026, 10, 19)), 0);
        assert_eq!(weekday(days_from_civil(2026, 10, 24)), 5);
        assert_eq!(date_at_offset(t, 8 * 60), "2026-10-19");
        assert_eq!(
            date_at_offset(parse_rfc3339("2026-10-19T17:00:00Z").unwrap(), 480),
            "2026-10-20"
        );
        assert_eq!(parse_offset("+08:00"), Some(480));
        assert_eq!(parse_offset("-05:30"), Some(-330));
    }
}
