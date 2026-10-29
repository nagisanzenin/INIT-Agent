//! How the Root prints a path inside a command the owner runs (src/util.rs `sh_word` and `agent_cmd_for`), for the
//! texts tests expect. A checkout under the passwd home, or one whose path holds a space (the main checkout,
//! "INIT Agent"), prints the launcher as `~/'…'`, so a test that builds its expected text from `display()` fails there
//! and passes in a /private/tmp worktree.
#![allow(dead_code)] // each test crate uses part of it

use std::path::{Path, PathBuf};

/// `util::sh_word`: `s` as one shell word, single-quoted only when it must be.
pub fn sh_word(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"/._+-,:@%=".contains(&c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `sys::passwd_home`: the home directory the passwd database gives this user (not $HOME).
pub fn passwd_home() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0 as libc::c_char; 4096];
    // SAFETY: a zeroed passwd is a valid out-buffer.
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every buffer outlives the call, `buf.len()` is its size, and getuid cannot fail.
    let r = unsafe { libc::getpwuid_r(libc::getuid(), &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) };
    if r != 0 || out.is_null() || pw.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir points into `buf`, NUL-terminated, and `buf` is still alive.
    let dir = unsafe { std::ffi::CStr::from_ptr(pw.pw_dir) };
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
}

/// `util::agent_cmd_for`: the launcher in `bin_dir` as the Root prints it, with `args` after it.
pub fn agent(bin_dir: &Path, args: &str) -> String {
    let exe = bin_dir.join("init-agent");
    let shown = match passwd_home().and_then(|h| exe.strip_prefix(&h).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", sh_word(&rest.to_string_lossy())),
        None => sh_word(&exe.to_string_lossy()),
    };
    if args.is_empty() { shown } else { format!("{shown} {args}") }
}
