//! Small std-only helpers shared by the fakes and the scenario runner: RFC 3339 time without a
//! date crate (Hinnant's days-from-civil, as body/log-context.md §3.2 does), random hex from
//! `/dev/urandom`, atomic writes, JSON-lines appends and binary lookup.

use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch, wall clock.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `2026-10-19T12:36:03.302Z` for a millisecond timestamp.
pub fn rfc3339(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z",
        sod / 3600,
        sod % 3600 / 60,
        sod % 60
    )
}

/// The current wall-clock time as RFC 3339 with milliseconds.
pub fn ts() -> String {
    rfc3339(now_ms())
}

/// Parse RFC 3339 (`YYYY-MM-DDTHH:MM:SS[.fff](Z|±HH:MM)`) into epoch milliseconds.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || !(b[10] == b'T' || b[10] == b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (
        num(0..4)?,
        num(5..7)?,
        num(8..10)?,
        num(11..13)?,
        num(14..16)?,
        num(17..19)?,
    );
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
        if frac.is_empty() {
            return None;
        }
        let mut f = frac.chars().take(3).collect::<String>();
        while f.len() < 3 {
            f.push('0');
        }
        ms = f.parse().ok()?;
    }
    let off_min = match s.get(i..)? {
        "Z" | "z" => 0,
        o if o.len() == 6 && (o.starts_with('+') || o.starts_with('-')) && &o[3..4] == ":" => {
            let oh: i64 = o[1..3].parse().ok()?;
            let om: i64 = o[4..6].parse().ok()?;
            let v = oh * 60 + om;
            if o.starts_with('-') { -v } else { v }
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo, d);
    Some(((days * 86_400 + h * 3600 + mi * 60 + se) - off_min * 60) * 1000 + ms)
}

/// Parse a fixed UTC offset `±HH:MM` into minutes.
pub fn parse_offset(s: &str) -> Option<i64> {
    if s.len() != 6 || &s[3..4] != ":" {
        return None;
    }
    let sign = match &s[0..1] {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    Some(sign * (s[1..3].parse::<i64>().ok()? * 60 + s[4..6].parse::<i64>().ok()?))
}

/// `n` random bytes from `/dev/urandom`.
pub fn random_bytes(n: usize) -> io::Result<Vec<u8>> {
    let mut b = vec![0u8; n];
    File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b)
}

/// Lowercase hex.
pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

/// `2n` lowercase hex characters of randomness (never all zeros).
pub fn random_hex(n: usize) -> String {
    loop {
        let b = random_bytes(n).unwrap_or_else(|_| {
            // Fallback: hash of time and pid; only used if /dev/urandom is unreadable.
            let seed = format!("{}-{}-{:?}", now_ms(), std::process::id(), Instant::now());
            sha256(seed.as_bytes())[..n.min(32)].to_vec()
        });
        if b.iter().any(|&x| x != 0) {
            return hex(&b);
        }
    }
}

/// SHA-256 digest.
pub fn sha256(b: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(b).into()
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(b: &[u8]) -> String {
    hex(&sha256(b))
}

/// SHA-256 of a file, streamed.
pub fn sha256_file(p: &Path) -> io::Result<(String, u64)> {
    use sha2::Digest;
    let mut f = File::open(p)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut n_total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        n_total += n as u64;
        h.update(&buf[..n]);
    }
    Ok((hex(&h.finalize()), n_total))
}

/// Write `bytes` to `path` atomically: temp file in the same directory, optional `sync_all`, rename.
pub fn write_atomic(path: &Path, bytes: &[u8], sync: bool) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("f");
    let tmp = dir.join(format!(
        ".{name}.tmp.{}.{}",
        std::process::id(),
        random_hex(4)
    ));
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        if sync {
            f.sync_all()?;
        }
    }
    fs::rename(&tmp, path)
}

/// Append one line (`bytes` plus `\n` unless already present) with `O_APPEND`; `sync_data` when asked.
pub fn append_line(path: &Path, bytes: &[u8], sync: bool) -> io::Result<()> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    let mut buf = bytes.to_vec();
    if buf.last() != Some(&b'\n') {
        buf.push(b'\n');
    }
    f.write_all(&buf)?;
    if sync {
        f.sync_data()?;
    }
    Ok(())
}

/// Append a JSON value as one ICJ line (keys sorted; floats are written as-is if present).
pub fn append_json(path: &Path, v: &Value, sync: bool) -> io::Result<()> {
    let s = serde_json::to_vec(v).map_err(io::Error::other)?;
    append_line(path, &s, sync)
}

/// Read a JSON-lines file, skipping lines that do not parse (a torn tail).
pub fn read_jsonl(path: &Path) -> Vec<Value> {
    let Ok(text) = fs::read(path) else {
        return Vec::new();
    };
    text.split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_slice(l).ok())
        .collect()
}

/// Poll `f` every `step` until it returns `Some`, or `timeout` passes.
pub fn poll<T>(timeout: Duration, step: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(x) = f() {
            return Some(x);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(step);
    }
}

/// Locate a binary of this crate (or a sibling workspace binary) for tests and the runner:
/// `$INIT_<NAME>_BIN` (name upper-cased, `-` → `_`, without an `init-` prefix: `INIT_FAKEROOT_BIN`),
/// then the directory of the running executable and its parent (cargo puts test binaries in
/// `target/<profile>/deps/` and bins in `target/<profile>/`).
pub fn bin_path(name: &str) -> Option<PathBuf> {
    let short = name.strip_prefix("init-").unwrap_or(name);
    let var = format!("INIT_{}_BIN", short.to_ascii_uppercase().replace('-', "_"));
    if let Some(p) = std::env::var_os(&var) {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    for _ in 0..2 {
        let c = dir.join(name);
        if c.is_file() {
            return Some(c);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

/// The test-home root of this WP: `kernel/target/test-homes/wp3/` (BUILD_AGENT_BRIEF: every test
/// uses a temporary home under `target/test-homes/<wp>/`), or `$INIT_TEST_SCRATCH` when set.
pub fn test_root() -> PathBuf {
    match std::env::var_os("INIT_TEST_SCRATCH") {
        Some(p) => PathBuf::from(p),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp3"),
    }
}

/// A fresh directory `<test_root>/<name>-<8 hex>` (short, so socket paths stay under 104 bytes).
/// The caller deletes it by this exact path.
pub fn test_dir(name: &str) -> PathBuf {
    let root = test_root();
    let _ = fs::create_dir_all(&root);
    let root = root.canonicalize().unwrap_or(root);
    let d = root.join(format!("{name}-{}", random_hex(4)));
    let _ = fs::create_dir_all(&d);
    d
}

/// A session id `YYYYMMDD-HHMMSS-<6 hex>` (UTC), as kernel/supervise.md §3.4 chooses it.
pub fn new_session_id(now_ms: i64) -> String {
    let t = rfc3339(now_ms);
    format!(
        "{}{}{}-{}{}{}-{}",
        &t[0..4],
        &t[5..7],
        &t[8..10],
        &t[11..13],
        &t[14..16],
        &t[17..19],
        random_hex(3)
    )
}

/// True for a session id the kernel and body accept (`^[0-9A-Za-z._-]{1,64}$`).
pub fn is_session_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_round_trip() {
        for ms in [0i64, 1_760_877_363_302, 951_782_400_000, 4_102_444_800_999] {
            let s = rfc3339(ms);
            assert_eq!(parse_rfc3339(&s), Some(ms), "{s}");
        }
        assert_eq!(rfc3339(1_760_877_363_302), "2025-10-19T12:36:03.302Z");
        assert_eq!(
            parse_rfc3339("2026-10-20T00:00:00+07:00"),
            parse_rfc3339("2026-10-19T17:00:00Z")
        );
        assert_eq!(
            parse_rfc3339("2026-10-20T00:00:00.5Z").map(|x| x % 1000),
            Some(500)
        );
        assert!(parse_rfc3339("2026-13-01T00:00:00Z").is_none());
        assert!(parse_rfc3339("garbage").is_none());
        assert_eq!(parse_offset("+07:00"), Some(420));
        assert_eq!(parse_offset("-01:30"), Some(-90));
    }

    #[test]
    fn ids() {
        let h = random_hex(8);
        assert_eq!(h.len(), 16);
        assert!(h.bytes().all(|c| c.is_ascii_hexdigit()));
        let sid = new_session_id(1_760_877_363_302);
        assert!(
            sid.starts_with("20251019-123603-") && sid.len() == 22 && is_session_id(&sid),
            "{sid}"
        );
        assert!(!is_session_id("a b"));
    }
}
