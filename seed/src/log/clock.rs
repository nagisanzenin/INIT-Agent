//! Event ids, timestamps and session ids (log-context.md §3.2). std only: no RNG or date crate.

use std::hash::{BuildHasher, RandomState};
use std::time::{SystemTime, UNIX_EPOCH};

/// Random 64-bit event ids from a randomly keyed SipHash over (nanos, pid, counter).
pub struct IdGen {
    keys: RandomState,
    counter: u64,
    pid: u32,
}

impl Default for IdGen {
    fn default() -> Self {
        Self::new()
    }
}

impl IdGen {
    pub fn new() -> Self {
        IdGen {
            keys: RandomState::new(),
            counter: 0,
            pid: std::process::id(),
        }
    }

    /// Next id that is non-zero and not `taken`.
    pub fn next(&mut self, taken: &dyn Fn(u64) -> bool) -> u64 {
        loop {
            self.counter += 1;
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let id = self.keys.hash_one((nanos, self.pid, self.counter));
            if id != 0 && !taken(id) {
                return id;
            }
        }
    }

    /// `n` random hex digits (for session ids).
    pub fn hex(&mut self, n: usize) -> String {
        let id = self.next(&|_| false);
        let s = format!("{id:016x}");
        s[..n.min(16)].to_string()
    }
}

pub fn id_hex(id: u64) -> String {
    format!("{id:016x}")
}

/// Parses a 16-lowercase-hex, non-zero id.
pub fn parse_id(s: &str) -> Option<u64> {
    if s.len() != 16
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    match u64::from_str_radix(s, 16) {
        Ok(0) | Err(_) => None,
        Ok(v) => Some(v),
    }
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's `civil_from_days`).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Broken-down UTC time from Unix milliseconds.
pub fn utc_parts(ms: u64) -> (i64, u32, u32, u32, u32, u32, u32) {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400) as u32;
    let (y, m, d) = civil_from_days(days);
    (
        y,
        m,
        d,
        sod / 3600,
        (sod / 60) % 60,
        sod % 60,
        (ms % 1000) as u32,
    )
}

/// `2026-10-19T20:30:15.123Z`
pub fn rfc3339_ms(ms: u64) -> String {
    let (y, mo, d, h, mi, s, frac) = utc_parts(ms);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{frac:03}Z")
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn now_rfc3339_ms() -> String {
    rfc3339_ms(now_ms())
}

/// `YYYYMMDD-HHMMSS-<6 hex>` (UTC), the session id form (log-context.md §3.2).
pub fn new_session_id(ids: &mut IdGen) -> String {
    let (y, mo, d, h, mi, s, _) = utc_parts(now_ms());
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}-{}", ids.hex(6))
}

/// `^[0-9A-Za-z._-]{1,64}$`
pub fn valid_session_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_known_points() {
        assert_eq!(rfc3339_ms(0), "1970-01-01T00:00:00.000Z");
        // 2026-10-19T20:30:15.123Z
        assert_eq!(rfc3339_ms(1_792_441_815_123), "2026-10-19T20:30:15.123Z");
        // Leap day and end of year.
        assert_eq!(rfc3339_ms(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(rfc3339_ms(1_735_689_599_999), "2024-12-31T23:59:59.999Z");
    }

    #[test]
    fn ids() {
        let mut g = IdGen::new();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..10_000 {
            let id = g.next(&|x| seen.contains(&x));
            assert_ne!(id, 0);
            assert!(seen.insert(id));
            assert_eq!(parse_id(&id_hex(id)), Some(id));
        }
        assert_eq!(parse_id("0000000000000000"), None);
        assert_eq!(parse_id("ABCDEF0123456789"), None);
        assert_eq!(parse_id("abc"), None);
        let sid = new_session_id(&mut g);
        assert_eq!(sid.len(), 22, "{sid}");
        assert!(valid_session_id(&sid));
        assert!(!valid_session_id("a/b"));
        assert!(!valid_session_id(""));
    }
}
