//! Small file helpers: no-follow reads with a cap (supervise.md K-F2: agent-written files are read
//! with `O_NOFOLLOW`, regular files only, bounded), atomic writes, UTC time.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Opens a regular file without following a final symlink; `Ok(None)` if it does not exist.
pub fn open_nofollow(path: &Path) -> io::Result<Option<File>> {
    match OpenOptions::new().read(true).custom_flags(libc_o_nofollow()).open(path) {
        Ok(f) => {
            if f.metadata()?.is_file() {
                Ok(Some(f))
            } else {
                Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not a regular file", path.display())))
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// `O_NOFOLLOW`.
fn libc_o_nofollow() -> i32 {
    libc::O_NOFOLLOW
}

/// Reads at most `cap` bytes of a regular file (no-follow); `Ok(None)` if absent.
pub fn read_capped(path: &Path, cap: u64) -> io::Result<Option<Vec<u8>>> {
    let Some(f) = open_nofollow(path)? else { return Ok(None) };
    let mut b = Vec::new();
    f.take(cap).read_to_end(&mut b)?;
    Ok(Some(b))
}

/// Reads bytes `[from, from + cap)` of a regular file (no-follow); `Ok(None)` if absent.
pub fn read_from(path: &Path, from: u64, cap: u64) -> io::Result<Option<Vec<u8>>> {
    let Some(mut f) = open_nofollow(path)? else { return Ok(None) };
    f.seek(SeekFrom::Start(from))?;
    let mut b = Vec::new();
    f.take(cap).read_to_end(&mut b)?;
    Ok(Some(b))
}

/// `(len, inode)` of a regular file, or `None`.
pub fn identity(path: &Path) -> Option<(u64, u64)> {
    let m = fs::symlink_metadata(path).ok()?;
    m.is_file().then(|| (m.len(), m.ino()))
}

/// Writes `bytes` to `path` atomically (temp file in the same directory, fsync, rename).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent"))?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("f"),
        std::process::id()
    ));
    {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).mode(0o644).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_data()?;
    }
    fs::rename(&tmp, path)
}

/// Unix seconds now.
pub fn now_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// RFC 3339 UTC with milliseconds for a Unix-ms instant (`2026-10-20T06:37:32.926Z`).
pub fn rfc3339_ms(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z", rem / 3600, (rem / 60) % 60, rem % 60, ms % 1000)
}

/// RFC 3339 now.
pub fn now_rfc3339() -> String {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    rfc3339_ms(ms)
}

/// Unix ms of an RFC 3339 UTC timestamp (`YYYY-MM-DDTHH:MM:SS[.fff]Z`); `None` otherwise.
pub fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || !s.ends_with('Z') {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let frac = match s.get(19..s.len() - 1) {
        Some("") => 0,
        Some(f) if f.starts_with('.') && f.len() > 1 => {
            let digits = &f[1..f.len().min(4)];
            let v: i64 = digits.parse().ok()?;
            v * 10i64.pow(3 - digits.len() as u32)
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo, d);
    let ms = ((days * 86_400 + h * 3600 + mi * 60 + se) * 1000) + frac;
    u64::try_from(ms).ok()
}

/// The UTC day (`YYYY-MM-DD`) of Unix seconds.
pub fn utc_day(secs: u64) -> String {
    let (y, m, d) = civil((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_round_trip() {
        let ms = parse_rfc3339_ms("2026-10-20T06:37:32.926Z").unwrap();
        assert_eq!(rfc3339_ms(ms), "2026-10-20T06:37:32.926Z");
        assert_eq!(parse_rfc3339_ms("2026-10-20T06:37:32Z").unwrap() % 1000, 0);
        assert_eq!(utc_day(ms / 1000), "2026-10-20");
        assert!(parse_rfc3339_ms("2026-10-20 06:37:32Z").is_none());
    }
}
