//! Boot housekeeping (body/boot.md §3.6, ≤ 100 ms): spill cleanup and the spill budget seed.
//!
//! Only directories found directly in `state/spill/` are ever removed, each by its exact path
//! (`remove_dir_all(<exact path>)`): never a glob, never `state/spill/` itself, never a symlink or
//! a plain file. Anything that cannot be read is kept and reported.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Spill directories of other sessions older than this (newest file mtime) are removed.
pub const SPILL_MAX_AGE_S: u64 = 14 * 24 * 3600;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Housekeeping {
    /// Bytes in `state/spill/<session>/` (seeds tools.md §4.4's 1 GiB per-session counter).
    pub spill_bytes: u64,
    pub warnings: Vec<String>,
}

/// `home` = `$INIT_HOME`; `session` = `INIT_SESSION` (may be empty: a new session, nothing kept).
pub fn housekeep(home: &Path, session: &str, now: SystemTime) -> Housekeeping {
    let mut hk = Housekeeping::default();
    let spill = home.join("state").join("spill");
    // A session id is one path component; anything else is treated as a new session.
    let session =
        if session.is_empty() || session == "." || session == ".." || session.contains(['/', '\0'])
        {
            if !session.is_empty() {
                hk.warn(format!(
                    "INIT_SESSION {session:?} is not a directory name; nothing kept"
                ));
            }
            ""
        } else {
            session
        };

    // Spill cleanup.
    match fs::read_dir(&spill) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => hk.warn(format!("cannot list {}: {e}", spill.display())),
        Ok(rd) => {
            for entry in rd {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        hk.warn(format!("cannot list {}: {e}", spill.display()));
                        break;
                    }
                };
                let name = entry.file_name();
                if !session.is_empty() && name == session {
                    continue;
                }
                // `file_type` does not follow symlinks: only real directories are candidates.
                match entry.file_type() {
                    Ok(t) if t.is_dir() => {}
                    Ok(_) => continue,
                    Err(e) => {
                        hk.warn(format!("cannot stat {}: {e}", entry.path().display()));
                        continue;
                    }
                }
                let dir = spill.join(&name); // the exact path of this one directory
                let newest = match newest_mtime(&dir) {
                    Ok(Some(t)) => t,
                    Ok(None) => match fs::symlink_metadata(&dir).and_then(|m| m.modified()) {
                        Ok(t) => t, // an empty directory: its own mtime
                        Err(e) => {
                            hk.warn(format!("cannot stat {}: {e}", dir.display()));
                            continue;
                        }
                    },
                    Err(e) => {
                        hk.warn(format!("kept {}: {e}", dir.display()));
                        continue;
                    }
                };
                let old = now
                    .duration_since(newest)
                    .is_ok_and(|age| age > Duration::from_secs(SPILL_MAX_AGE_S));
                if old && let Err(e) = fs::remove_dir_all(&dir) {
                    hk.warn(format!("cannot remove {}: {e}", dir.display()));
                }
            }
        }
    }

    // Spill budget: the current session's bytes so far (a new session starts at 0).
    if !session.is_empty() {
        let dir = spill.join(session);
        match fs::read_dir(&dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => hk.warn(format!("cannot list {}: {e}", dir.display())),
            Ok(rd) => {
                for entry in rd {
                    match entry.and_then(|e| e.metadata()) {
                        Ok(m) if m.is_file() => hk.spill_bytes += m.len(),
                        Ok(_) => {}
                        Err(e) => hk.warn(format!("cannot stat in {}: {e}", dir.display())),
                    }
                }
            }
        }
        crate::tools::bash::init_spill_total(&dir);
    }
    hk
}

impl Housekeeping {
    fn warn(&mut self, msg: String) {
        self.warnings.push(format!("housekeep: {msg}"));
    }
}

/// The newest mtime among the files below `dir` (symlinks not followed); `None` when it holds no
/// file at all. Any error means "unknown", and the caller keeps the directory.
fn newest_mtime(dir: &Path) -> io::Result<Option<SystemTime>> {
    let mut newest: Option<SystemTime> = None;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let t = entry.file_type()?;
        let m = if t.is_dir() {
            newest_mtime(&entry.path())?
        } else {
            Some(entry.metadata()?.modified()?)
        };
        newest = newest.max(m);
    }
    Ok(newest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);
    const DAY: Duration = Duration::from_secs(24 * 3600);

    struct Home(PathBuf);

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0); // exact path, under target/test-homes/wp13/
        }
    }

    impl Home {
        fn new(name: &str) -> Home {
            let p = std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute()) // G0: the slot's out/target (the export is read-only)
                .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
                .join("test-homes/wp13")
                .join(format!(
                    "{name}-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::SeqCst)
                ));
            if p.exists() {
                fs::remove_dir_all(&p).unwrap(); // a stale home of a crashed run, same pid
            }
            fs::create_dir_all(p.join("state/spill")).unwrap();
            Home(p)
        }

        fn spill(&self, rel: &str) -> PathBuf {
            self.0.join("state/spill").join(rel)
        }

        /// Writes `state/spill/<rel>` with `len` bytes and the given mtime.
        fn file(&self, rel: &str, len: usize, mtime: SystemTime) {
            let p = self.spill(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, vec![b'x'; len]).unwrap();
            File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(mtime)
                .unwrap();
        }
    }

    #[test]
    fn old_other_sessions_removed_fresh_and_current_kept() {
        let h = Home::new("hk");
        let now = SystemTime::now();
        let old = now - 20 * DAY;
        h.file("20260101-000000-aaaaaa/a.txt", 10, old);
        h.file("20260101-000000-aaaaaa/b.txt", 10, old - DAY);
        // One fresh file keeps the whole directory, even below a subdirectory.
        h.file("20260102-000000-bbbbbb/a.txt", 10, old);
        h.file("20260102-000000-bbbbbb/sub/c.txt", 10, now - DAY);
        h.file("20260103-000000-cccccc/a.txt", 10, now - 13 * DAY);
        // The current session is kept however old, and its bytes are counted.
        h.file("20260104-000000-dddddd/x.txt", 1000, old);
        h.file("20260104-000000-dddddd/y.txt", 234, now);
        // A plain file in spill/ is not a session directory: untouched.
        h.file("stray.txt", 5, old);
        let r = housekeep(&h.0, "20260104-000000-dddddd", now);
        assert_eq!(
            r,
            Housekeeping {
                spill_bytes: 1234,
                warnings: vec![]
            }
        );
        assert!(!h.spill("20260101-000000-aaaaaa").exists());
        assert!(h.spill("20260102-000000-bbbbbb/sub/c.txt").exists());
        assert!(h.spill("20260103-000000-cccccc/a.txt").exists());
        assert!(h.spill("20260104-000000-dddddd/x.txt").exists());
        assert!(h.spill("stray.txt").exists());
        assert!(h.0.join("state/spill").is_dir(), "never the parent");
    }

    #[test]
    fn empty_session_keeps_nothing_and_empty_dirs_use_their_own_mtime() {
        let h = Home::new("hknew");
        let now = SystemTime::now();
        h.file("s1/a.txt", 10, now);
        fs::create_dir_all(h.spill("empty")).unwrap();
        // 15 days later: the file and the empty directory's own mtime are both too old.
        let r = housekeep(&h.0, "", now + 15 * DAY);
        assert_eq!(r, Housekeeping::default());
        assert!(!h.spill("s1").exists());
        assert!(!h.spill("empty").exists());
        // An empty directory with a fresh mtime is kept.
        fs::create_dir_all(h.spill("empty2")).unwrap();
        let r = housekeep(&h.0, "", now + 13 * DAY);
        assert_eq!(r, Housekeeping::default());
        assert!(h.spill("empty2").exists());
    }

    #[test]
    fn symlinks_are_never_followed_or_removed() {
        let h = Home::new("hklink");
        let now = SystemTime::now();
        let outside = h.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("precious.txt"), "keep").unwrap();
        std::os::unix::fs::symlink(&outside, h.spill("link")).unwrap();
        // A link inside an old session directory: the directory goes, the target stays.
        h.file("old/a.txt", 1, now - 20 * DAY);
        std::os::unix::fs::symlink(&outside, h.spill("old/inner")).unwrap();
        let r = housekeep(&h.0, "cur", now + 20 * DAY);
        assert!(r.warnings.is_empty(), "{r:?}");
        assert!(fs::symlink_metadata(h.spill("link")).is_ok());
        assert!(!h.spill("old").exists());
        assert_eq!(
            fs::read_to_string(outside.join("precious.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn missing_spill_dir_and_bad_session_are_harmless() {
        let h = Home::new("hknone");
        fs::remove_dir_all(h.0.join("state/spill")).unwrap();
        assert_eq!(
            housekeep(&h.0, "s", SystemTime::now()),
            Housekeeping::default()
        );
        fs::create_dir_all(h.spill("x")).unwrap();
        let r = housekeep(&h.0, "../x", SystemTime::now());
        assert_eq!(r.spill_bytes, 0);
        assert_eq!(r.warnings.len(), 1, "{r:?}");
        assert!(h.spill("x").exists(), "fresh, kept");
    }

    #[test]
    fn fast_enough() {
        let h = Home::new("hkfast");
        let now = SystemTime::now();
        for i in 0..200 {
            h.file(&format!("s{i}/a.txt"), 10, now);
        }
        let t = std::time::Instant::now();
        let r = housekeep(&h.0, "s0", now);
        assert!(r.warnings.is_empty());
        assert!(
            t.elapsed() < Duration::from_millis(100),
            "{:?}",
            t.elapsed()
        );
    }
}
