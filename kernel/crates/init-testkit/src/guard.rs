//! The live/disposable guard (testing.md §3.1a, v03-decisions D1).
//!
//! A disposable scenario, every sweep test and every scenario that starts the real `init-root`
//! may run only when **both** hold:
//! 1. `INIT_TEST_ENV=disposable` is set, and
//! 2. a root-owned sentinel file exists at the fixed path ([`SENTINEL`]: `/var/db/init-test-disposable`
//!    on macOS, `/etc/init-test-disposable` on Linux), is a regular file (not a symlink) owned by
//!    uid 0, not writable by group or others, and its content (trimmed) is the decimal uid allowed
//!    to run disposable tests, which must equal `getuid()`.
//!
//! The sentinel is created by whoever provisions the environment, never by a test, so a disposable
//! scenario cannot start in the owner's account by accident. The path is fixed: no API here takes
//! another path, so no test can point the guard at a file it wrote itself.

use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;

/// The sentinel path for this platform.
#[cfg(target_os = "macos")]
pub const SENTINEL: &str = "/var/db/init-test-disposable";
#[cfg(not(target_os = "macos"))]
pub const SENTINEL: &str = "/etc/init-test-disposable";

/// The environment variable that must say `disposable`.
pub const ENV_VAR: &str = "INIT_TEST_ENV";

/// The message a refused scenario reports (`skipped: …`, never `passed`).
pub const NEEDS_DISPOSABLE: &str = "needs a disposable environment";

/// What the sentinel file looked like (metadata only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentinelInfo {
    pub is_regular: bool,
    pub owner_uid: u32,
    pub mode: u32,
    pub content: String,
}

/// The guard's verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Disposable,
    /// Not disposable, with the reason (for the skip message).
    Live(String),
}

impl Verdict {
    pub fn is_disposable(&self) -> bool {
        matches!(self, Verdict::Disposable)
    }

    /// `skipped: needs a disposable environment (<reason>)`.
    pub fn skip_message(&self) -> String {
        match self {
            Verdict::Disposable => String::new(),
            Verdict::Live(r) => format!("skipped: {NEEDS_DISPOSABLE} ({r})"),
        }
    }
}

/// The pure decision, unit-tested without touching the real sentinel.
pub fn decide(env_value: Option<&str>, sentinel: Option<&SentinelInfo>, uid: u32) -> Verdict {
    if env_value != Some("disposable") {
        return Verdict::Live(format!("{ENV_VAR} is not `disposable`"));
    }
    let Some(s) = sentinel else {
        return Verdict::Live(format!("no sentinel at {SENTINEL}"));
    };
    if !s.is_regular {
        return Verdict::Live(format!("{SENTINEL} is not a regular file"));
    }
    if s.owner_uid != 0 {
        return Verdict::Live(format!("{SENTINEL} is not owned by root"));
    }
    if s.mode & 0o022 != 0 {
        return Verdict::Live(format!("{SENTINEL} is writable by group or others"));
    }
    match s.content.trim().parse::<u32>() {
        Ok(allowed) if allowed == uid => Verdict::Disposable,
        Ok(allowed) => Verdict::Live(format!(
            "{SENTINEL} allows uid {allowed}, this is uid {uid}"
        )),
        Err(_) => Verdict::Live(format!("{SENTINEL} does not hold a uid")),
    }
}

/// Read the sentinel's metadata and content (at most 64 bytes) without following a symlink.
pub fn read_sentinel() -> Option<SentinelInfo> {
    let p = Path::new(SENTINEL);
    let md = fs::symlink_metadata(p).ok()?;
    let ft = md.file_type();
    let is_regular = ft.is_file() && !ft.is_symlink() && !ft.is_socket() && !ft.is_fifo();
    let content = if is_regular && md.len() <= 64 {
        fs::read_to_string(p).unwrap_or_default()
    } else {
        String::new()
    };
    Some(SentinelInfo {
        is_regular,
        owner_uid: md.uid(),
        mode: md.mode(),
        content,
    })
}

/// The verdict for this process, from the real environment and the real sentinel.
pub fn check() -> Verdict {
    let env = std::env::var(ENV_VAR).ok();
    check_with_env(env.as_deref())
}

/// The verdict with an explicit `INIT_TEST_ENV` value (the runner passes the scenario process's
/// view); the sentinel is always the real, fixed one.
pub fn check_with_env(env_value: Option<&str>) -> Verdict {
    decide(env_value, read_sentinel().as_ref(), crate::sys::getuid())
}

/// For fake modes that would leave processes only a sweep can remove (`escape_setsid`, the
/// `ignore_term` kernel's `setsid` child): refuse with exit 64 unless disposable.
pub fn require_disposable_or_exit(what: &str) {
    let v = check();
    if !v.is_disposable() {
        eprintln!("{what}: refused: {}", v.skip_message());
        std::process::exit(64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(uid: u32, mode: u32, content: &str) -> SentinelInfo {
        SentinelInfo {
            is_regular: true,
            owner_uid: uid,
            mode,
            content: content.into(),
        }
    }

    #[test]
    fn decision_table() {
        let ok = s(0, 0o100644, "501\n");
        assert_eq!(
            decide(Some("disposable"), Some(&ok), 501),
            Verdict::Disposable
        );
        assert!(!decide(None, Some(&ok), 501).is_disposable());
        assert!(!decide(Some("live"), Some(&ok), 501).is_disposable());
        assert!(!decide(Some("disposable"), None, 501).is_disposable());
        assert!(
            !decide(Some("disposable"), Some(&ok), 502).is_disposable(),
            "another uid"
        );
        assert!(
            !decide(Some("disposable"), Some(&s(501, 0o100644, "501")), 501).is_disposable(),
            "owned by the tester"
        );
        assert!(
            !decide(Some("disposable"), Some(&s(0, 0o100666, "501")), 501).is_disposable(),
            "world-writable"
        );
        assert!(!decide(Some("disposable"), Some(&s(0, 0o100644, "x")), 501).is_disposable());
        let mut link = ok.clone();
        link.is_regular = false;
        assert!(!decide(Some("disposable"), Some(&link), 501).is_disposable());
        assert!(
            decide(None, None, 0)
                .skip_message()
                .starts_with("skipped: needs a disposable environment")
        );
    }

    /// SW-6 at the guard level: this build host is the owner's live account, so the real guard
    /// must refuse even when the environment variable claims otherwise (no root-owned sentinel
    /// exists here; if one does, it must name another uid or this machine is disposable by intent).
    #[test]
    fn live_account_is_refused() {
        if read_sentinel().is_some() {
            eprintln!(
                "note: {SENTINEL} exists on this machine; skipping the live-account assertion"
            );
            return;
        }
        assert!(!check_with_env(Some("disposable")).is_disposable());
        assert!(!check_with_env(None).is_disposable());
    }
}
