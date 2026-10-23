//! `write` (tools.md §3.3): create or replace a file atomically (temp + rename, no fsync),
//! writing through symlinks and keeping mode bits.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};

use super::args::{check_fields, req_str};
use super::{
    Limits, PathCx, Ran, ToolCx, ToolError, commas, io_error, is_directory, resolve, text_result,
};

#[derive(Clone, Debug)]
pub struct WriteArgs {
    pub path: String,
    pub abs: PathBuf,
    pub content: String,
}

pub fn parse(m: &Map<String, Value>, cx: &PathCx) -> Result<WriteArgs, ToolError> {
    check_fields(m, &["content", "path"], "")?;
    let path = req_str(m, "path", "")?;
    let content = req_str(m, "content", "")?;
    let abs = resolve(&path, cx)?;
    Ok(WriteArgs { path, abs, content })
}

/// Lines in `s`: newlines, plus one for a last line without `\n`.
pub fn count_lines(s: &[u8]) -> u64 {
    let nl = s.iter().filter(|&&c| c == b'\n').count() as u64;
    nl + (!s.is_empty() && *s.last().unwrap() != b'\n') as u64
}

static SEQ: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
thread_local! {
    /// Test fault: the next `atomic_write` on this thread fails at `rename`.
    pub static FAIL_RENAME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The target a write goes to: through a symlink (kept), dangling links resolved by hand.
pub fn write_target(abs: &Path) -> PathBuf {
    match std::fs::symlink_metadata(abs) {
        Ok(m) if m.file_type().is_symlink() => match std::fs::canonicalize(abs) {
            Ok(t) => t,
            Err(_) => match std::fs::read_link(abs) {
                Ok(l) => abs.parent().map(|p| p.join(&l)).unwrap_or(l),
                Err(_) => abs.to_path_buf(),
            },
        },
        _ => abs.to_path_buf(),
    }
}

/// Temp file in the target's directory, then `rename` (atomic within one filesystem). The temp
/// file is removed on every error. Mode bits of an existing target are kept.
pub fn atomic_write(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = target.parent().unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let tmp = dir.join(format!(
        ".{name}.init-tmp.{}.{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    let res = (|| {
        f.write_all(bytes)?;
        drop(f);
        if let Ok(old) = std::fs::metadata(target) {
            std::fs::set_permissions(&tmp, old.permissions())?;
        }
        #[cfg(test)]
        if FAIL_RENAME.with(|c| c.replace(false)) {
            return Err(io::Error::other("injected rename failure"));
        }
        std::fs::rename(&tmp, target)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

pub fn run(a: &WriteArgs, cx: &ToolCx, lim: &Limits) -> Result<Ran, ToolError> {
    let len = a.content.len();
    if len > lim.write_bytes {
        return Err(ToolError::new(
            "too_large",
            format!(
                "content is {} bytes; write takes at most {} MiB",
                commas(len as u64),
                lim.write_bytes >> 20
            ),
        )
        .hint(
            "write the file in parts (write the first part, then append with bash: cat >> or edit)",
        ));
    }
    if std::fs::metadata(&a.abs)
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        return Err(is_directory(&a.path));
    }
    if let Some(parent) = a.abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error(&a.path, &e))?;
    }
    let target = write_target(&a.abs);
    let old = std::fs::metadata(&target).ok().map(|m| m.len());
    atomic_write(&target, a.content.as_bytes()).map_err(|e| io_error(&a.path, &e))?;
    let lines = count_lines(a.content.as_bytes());
    let tail = match old {
        None => "(new file)".to_string(),
        Some(o) => format!("(replaced {} bytes)", commas(o)),
    };
    Ok(Ran {
        result: text_result(
            cx.call_id,
            format!(
                "Wrote {} bytes ({lines} lines) to {} {tail}.",
                commas(len as u64),
                a.path
            ),
        ),
        summary: format!("wrote {} bytes", commas(len as u64)),
    })
}
