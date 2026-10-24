//! The runtime layout checks (supervise.md §3.1: the 64-byte rule and K-F5) and the session id
//! (§3.4).

use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// `INIT_HOME` is at most 64 bytes, so `$INIT_HOME/probe/<12 hex>/run/kernel.sock` fits macOS's
/// 104-byte `sun_path` (§3.1).
pub const MAX_HOME_BYTES: usize = 64;

/// The top-level directories K-F5 checks.
pub const TOP_DIRS: [&str; 7] = ["bin", "state", "lineage", "cache", "vault", "run", "work"];

/// §3.1 and K-F5: `INIT_HOME` absolute, symlink-free, ≤ 64 bytes; every existing top-level
/// directory and `state/kernel/` a real directory owned by this uid. In test mode the length
/// rule is reported but not enforced (test homes live under the repo's `target/`, which is
/// longer; no lane socket is bound by these tests).
pub fn check(home: &Path, test_mode: bool) -> Result<(), String> {
    if !home.is_absolute() {
        return Err(format!("E_LAYOUT: INIT_HOME {} is not absolute", home.display()));
    }
    let canon = std::fs::canonicalize(home).map_err(|e| format!("E_LAYOUT: INIT_HOME {}: {e}", home.display()))?;
    if canon != home {
        return Err(format!("E_LAYOUT: INIT_HOME {} is not symlink-free (resolves to {})", home.display(), canon.display()));
    }
    let len = home.as_os_str().len();
    if len > MAX_HOME_BYTES && !test_mode {
        return Err(format!("E_LAYOUT: INIT_HOME is {len} bytes; at most {MAX_HOME_BYTES}"));
    }
    // SAFETY-free: getuid via std is not available; compare with the home's own owner.
    let uid = std::fs::metadata(home).map(|m| m.uid()).map_err(|e| e.to_string())?;
    for d in TOP_DIRS.iter().copied().chain(["state/kernel"]) {
        let p = home.join(d);
        match std::fs::symlink_metadata(&p) {
            Ok(md) => {
                if !md.file_type().is_dir() {
                    return Err(format!("E_LAYOUT: {d} is not a real directory"));
                }
                if md.uid() != uid {
                    return Err(format!("E_LAYOUT: {d} is owned by uid {}", md.uid()));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("E_LAYOUT: {d}: {e}")),
        }
    }
    Ok(())
}

/// `^[0-9A-Za-z._-]{1,64}$` (session and task ids).
pub fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// `state/session` per K-F2 (≤ 64 bytes after the newline is stripped), if valid.
pub fn read_session(home: &Path) -> Option<String> {
    let b = crate::fsx::read_nofollow(&home.join("state/session"), 66, false).ok()?;
    let s = String::from_utf8(b).ok()?;
    let s = s.strip_suffix('\n').unwrap_or(&s).to_string();
    valid_id(&s).then_some(s)
}

/// A new session id `YYYYMMDD-HHMMSS-<6 hex>` in UTC (§3.4).
pub fn new_session_id() -> String {
    format!("{}-{}", crate::clock::compact_utc(crate::clock::now_ms()), crate::sys::random_hex(3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_rules() {
        // LINUX: a name long enough that the home exceeds 64 bytes wherever the repo is checked out (a short checkout
        // path such as /w/work/init made this depend on the machine)
        let d = crate::testutil::test_dir(&format!("layout-{}", "x".repeat(64)));
        std::fs::create_dir_all(d.join("state/kernel")).unwrap();
        assert!(check(&d, true).is_ok());
        assert!(check(&d, false).unwrap_err().contains("bytes"), "repo test homes exceed 64 bytes");
        std::os::unix::fs::symlink(d.join("state"), d.join("vault")).unwrap();
        assert!(check(&d, true).unwrap_err().contains("vault"));
        assert!(check(Path::new("relative"), true).is_err());
        std::fs::write(d.join("state/session"), "20261019-120000-3fa9c1\n").unwrap();
        assert_eq!(read_session(&d).as_deref(), Some("20261019-120000-3fa9c1"));
        std::fs::write(d.join("state/session"), "../../etc\n").unwrap();
        assert_eq!(read_session(&d), None);
        let s = new_session_id();
        assert_eq!(s.len(), 22);
        assert!(valid_id(&s));
        crate::testutil::remove_test_dir(&d);
    }
}
