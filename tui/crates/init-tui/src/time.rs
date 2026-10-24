//! Times without a time-zone crate (docs/tui.md §3.9): relative times, and UTC with a `Z` where an
//! absolute time is needed.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn unix_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parses an RFC 3339 timestamp (`2026-10-20T21:04:05Z`, optional fraction, `Z` or `±hh:mm`) to
/// Unix milliseconds.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b't' || b[10] == b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut i = 19;
    let mut ms = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let frac = &s[start..i];
        let digits: String = frac.chars().chain("000".chars()).take(3).collect();
        ms = digits.parse().ok()?;
    }
    let off = match b.get(i)? {
        b'Z' | b'z' => 0,
        c @ (b'+' | b'-') => {
            let oh = n(i + 1..i + 3)?;
            let om = n(i + 4..i + 6)?;
            let o = (oh * 60 + om) * 60_000;
            if *c == b'+' { o } else { -o }
        }
        _ => return None,
    };
    Some((days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se) * 1000 + ms - off)
}

/// `21:04:05Z`.
pub fn utc_hms(ms: i64) -> String {
    let s = ms.div_euclid(1000).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

/// `3 min ago`, `12 s ago`, `2 h 5 min ago`.
pub fn ago(ms: i64, now_ms: i64) -> String {
    format!("{} ago", span((now_ms - ms).max(0) as u64 / 1000))
}

/// `in 5 h 12 min`.
pub fn until(ms: i64, now_ms: i64) -> String {
    format!("in {}", span((ms - now_ms).max(0) as u64 / 1000))
}

/// A coarse span: `12 s`, `3 min`, `5 h 12 min`, `2 d 3 h`.
pub fn span(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min", secs / 60),
        3600..86_400 => format!("{} h {} min", secs / 3600, secs / 60 % 60),
        _ => format!("{} d {} h", secs / 86_400, secs / 3600 % 24),
    }
}

/// A tool duration: `0.3 s`, `12 s`, `3 min`.
pub fn dur(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 10.0 { format!("{s:.1} s") } else { span(d.as_secs()) }
}

/// An activity's elapsed time: `4.2 s`, `42 s`, `1 min 12 s`, `1 h 5 min`.
pub fn elapsed(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..10 => format!("{:.1} s", d.as_secs_f64()),
        10..60 => format!("{s} s"),
        60..3600 => format!("{} min {} s", s / 60, s % 60),
        _ => span(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-10-20T21:04:05Z"), Some(1_792_530_245_000));
        assert_eq!(parse_rfc3339("2026-10-20T21:04:05.250Z"), Some(1_792_530_245_250));
        assert_eq!(parse_rfc3339("2026-10-20T23:04:05+02:00"), Some(1_792_530_245_000));
        assert_eq!(parse_rfc3339("garbage"), None);
        assert_eq!(utc_hms(1_792_530_245_000), "21:04:05Z");
    }

    #[test]
    fn spans() {
        assert_eq!(span(12), "12 s");
        assert_eq!(span(180), "3 min");
        assert_eq!(span(5 * 3600 + 12 * 60), "5 h 12 min");
        assert_eq!(ago(0, 180_000), "3 min ago");
        assert_eq!(until(18_720_000, 0), "in 5 h 12 min");
        assert_eq!(dur(Duration::from_millis(300)), "0.3 s");
        assert_eq!(dur(Duration::from_secs(12)), "12 s");
        assert_eq!(elapsed(Duration::from_millis(4200)), "4.2 s");
        assert_eq!(elapsed(Duration::from_secs(72)), "1 min 12 s");
        assert_eq!(elapsed(Duration::from_secs(3900)), "1 h 5 min");
    }
}
