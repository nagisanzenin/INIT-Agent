//! Small shared helpers: ICJ (body/log-context.md §3.8.1), hex, /dev/urandom, the Root clock and
//! RFC 3339, atomic writes (R-F3, kernel/supervise.md §3.12), secrets (R-F4), the Root log.

use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// ICJ v1: sorted keys (serde_json without `preserve_order`), integers only, no whitespace.
pub fn icj(v: &Value) -> io::Result<String> {
    fn float(v: &Value) -> bool {
        match v {
            Value::Number(n) => n.is_f64(),
            Value::Array(a) => a.iter().any(float),
            Value::Object(o) => o.values().any(float),
            _ => false,
        }
    }
    if float(v) {
        return Err(bad("ICJ: floats are not allowed"));
    }
    serde_json::to_string(v).map_err(|e| bad(e.to_string()))
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != 2 * N || !s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

pub fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest;
    hex(&sha2::Sha256::digest(b))
}

pub fn random<const N: usize>() -> io::Result<[u8; N]> {
    let mut b = [0u8; N];
    File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b)
}

/// The Root clock in ms since the Unix epoch: the wall clock, or the fake clock (§3.16, test builds).
pub fn now_ms() -> i64 {
    #[cfg(feature = "test-fixtures")]
    if let Some(t) = crate::testfix::fake_now() {
        return t;
    }
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    d.map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// RFC 3339 UTC with milliseconds (`2026-10-20T08:01:02.345Z`); civil-from-days (H. Hinnant).
pub fn rfc3339(ms: i64) -> String {
    let (days, r) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    let (h, mi, s) = (r / 3_600_000, r / 60_000 % 60, r / 1000 % 60);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{:03}Z", r % 1000)
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fff…](Z|±HH:MM)`.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b).filter(|t| t.bytes().all(|c| c.is_ascii_digit()))?.parse::<i64>().ok();
    let sep = |i: usize, c: &[u8]| s.as_bytes().get(i).is_some_and(|x| c.contains(x));
    if !(sep(4, b"-") && sep(7, b"-") && sep(10, b"Tt") && sep(13, b":") && sep(16, b":")) {
        return None;
    }
    let (y, m, d, h, mi, sec) = (n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?, n(17, 19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    let mut ms = 0;
    if sep(19, b".") {
        let digits = s[20..].bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        ms = format!("{:0<3}", &s[20..20 + digits.min(3)]).parse::<i64>().ok()?;
        i = 20 + digits;
    }
    let off = match &s[i..] {
        "Z" | "z" => 0,
        t if t.len() == 6 && sep(i + 3, b":") && (t.starts_with('+') || t.starts_with('-')) => {
            let v = n(i + 1, i + 3)? * 60 + n(i + 4, i + 6)?;
            if t.starts_with('-') { -v } else { v }
        }
        _ => return None,
    };
    let yy = if m <= 2 { y - 1 } else { y };
    let (era, yoe) = (yy.div_euclid(400), yy.rem_euclid(400));
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some(((days * 24 + h) * 60 + mi - off) * 60_000 + sec * 1000 + ms)
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// R-F3: temp file (`O_EXCL|O_NOFOLLOW`), fsync, rename, fsync the directory.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| bad("no parent"))?;
    let name = path.file_name().ok_or_else(|| bad("no file name"))?.to_string_lossy();
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.tmp.{}.{seq}", std::process::id()));
    let res = (|| {
        let mut o = OpenOptions::new();
        o.write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(mode);
        let mut f = o.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res?;
    File::open(dir)?.sync_all()
}

/// `write_atomic` for a file the Root copies whole (release binaries): `fs::copy` to a temp name, which is
/// an APFS clone on macOS (`fclonefileat`), so a 150 MB debug kernel costs no second copy; then the mode,
/// `sync_all`, a rename over `path` (a new inode, R-F3) and a directory fsync.
pub fn copy_atomic(src: &Path, path: &Path, mode: u32) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| bad("no parent"))?;
    let name = path.file_name().ok_or_else(|| bad("no file name"))?.to_string_lossy();
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.tmp.{}.{seq}", std::process::id()));
    if fs::symlink_metadata(&tmp).is_ok() {
        return Err(bad(format!("{} exists", tmp.display())));
    }
    let res = (|| {
        fs::copy(src, &tmp)?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
        File::open(&tmp)?.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res?;
    File::open(dir)?.sync_all()
}

/// R-F4: a secret is a regular 0600 file owned by us, opened with `O_NOFOLLOW`.
pub fn read_secret(path: &Path) -> io::Result<Vec<u8>> {
    let mut f = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)?;
    let m = f.metadata()?;
    if !m.is_file() || m.uid() != crate::sys::euid() || m.mode() & 0o077 != 0 {
        return Err(bad(format!("insecure secret file {}", path.display())));
    }
    let mut v = Vec::new();
    f.read_to_end(&mut v)?;
    Ok(v)
}

/// One ICJ line appended to a 0600 JSONL file (logs, alerts, queues), synced.
pub fn append_line(path: &Path, v: &Value) -> io::Result<()> {
    let mut o = OpenOptions::new();
    o.create(true).append(true).mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let mut f = o.open(path)?;
    f.write_all(format!("{}\n", icj(v)?).as_bytes())?;
    f.sync_data()
}

/// `logs/root.jsonl`: best effort, never fails the caller.
pub fn log(root_home: &Path, level: &str, msg: &str) {
    let v = json!({"v":1,"ts":rfc3339(now_ms()),"level":level,"msg":msg});
    let _ = append_line(&root_home.join("logs/root.jsonl"), &v);
}

/// ROOT-MSG (v0.5.1): a word for a POSIX shell, quoted only when it needs it.
pub fn sh_word(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"/._+-,:@%=".contains(&c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// ROOT-MSG (v0.5.1): the command an owner types in a terminal to run `init-root <args>` of the install at `rh`.
/// `init-root` is not on the owner's PATH (only `init-agent` is), so a bare `init-root …` fails with "command not
/// found". The default install (`~/.init-root` in the passwd home, which a new terminal's `~` is) prints as
/// `~/.init-root/bin/init-root <args>`, so alert text names no owner path (root/root.md §4.6); any other ROOT_HOME
/// prints its absolute binary with `INIT_ROOT_HOME=` in front, as install.sh does, because `init-root` finds its
/// ROOT_HOME from that variable, else `~/.init-root`.
pub fn root_cmd_for(rh: &Path, passwd_home: Option<&Path>, args: &str) -> String {
    let exe = if passwd_home.is_some_and(|h| h.join(".init-root") == rh) {
        "~/.init-root/bin/init-root".to_string()
    } else {
        let rhs = rh.to_string_lossy();
        format!("INIT_ROOT_HOME={} {}", sh_word(&rhs), sh_word(&rh.join("bin/init-root").to_string_lossy()))
    };
    if args.is_empty() { exe } else { format!("{exe} {args}") }
}

/// [`root_cmd_for`] with this account's passwd home.
pub fn root_cmd(rh: &Path, args: &str) -> String {
    root_cmd_for(rh, crate::sys::passwd_home().as_deref(), args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_round_trip() {
        for ms in [0, 1_760_947_262_345, 951_782_400_000, -86_400_000, 4_102_444_799_999] {
            assert_eq!(parse_rfc3339(&rfc3339(ms)), Some(ms), "{ms}");
        }
        assert_eq!(rfc3339(1_760_947_262_345), "2025-10-20T08:01:02.345Z");
        assert_eq!(parse_rfc3339("2026-10-20T08:00:00+07:00"), parse_rfc3339("2026-10-20T01:00:00Z"));
        assert_eq!(parse_rfc3339("2026-10-20T08:00:00.5Z").map(|t| t % 1000), Some(500));
        for bad in ["2026-10-20", "2026-13-01T00:00:00Z", "2026-10-20T08:00:00", "x026-10-20T08:00:00Z"] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn icj_sorted_and_integer_only() {
        let v = json!({"b":1,"a":{"d":"x/y\u{7f}","c":[true,null]}});
        assert_eq!(icj(&v).unwrap(), "{\"a\":{\"c\":[true,null],\"d\":\"x/y\u{7f}\"},\"b\":1}");
        assert!(icj(&json!({"a":[1.5]})).is_err());
    }

    /// ROOT-MSG: the printed command runs as printed: `~/…` for the default install, else the absolute binary with its
    /// ROOT_HOME, quoted when the path needs it.
    #[test]
    fn root_cmd_runs_as_printed() {
        let h = Path::new("/Users/o");
        assert_eq!(root_cmd_for(Path::new("/Users/o/.init-root"), Some(h), "resume"), "~/.init-root/bin/init-root resume");
        assert_eq!(root_cmd_for(Path::new("/Users/o/.init-root"), Some(h), ""), "~/.init-root/bin/init-root");
        assert_eq!(root_cmd_for(Path::new("/private/tmp/i5/r"), Some(h), "start --foreground"),
            "INIT_ROOT_HOME=/private/tmp/i5/r /private/tmp/i5/r/bin/init-root start --foreground");
        assert_eq!(root_cmd_for(Path::new("/Users/o/.init-root"), None, "stop"),
            "INIT_ROOT_HOME=/Users/o/.init-root /Users/o/.init-root/bin/init-root stop", "no passwd entry: the absolute form");
        assert_eq!(root_cmd_for(Path::new("/Users/o/My Root"), Some(h), "status"),
            "INIT_ROOT_HOME='/Users/o/My Root' '/Users/o/My Root/bin/init-root' status");
        assert_eq!(sh_word("it's"), "'it'\\''s'");
        assert_eq!(sh_word(""), "''");
    }

    #[test]
    fn hex_round_trip() {
        let b = [0u8, 1, 0xab, 0xff];
        assert_eq!(unhex::<4>(&hex(&b)), Some(b));
        assert_eq!(unhex::<4>("00AB01ff"), None);
    }
}
