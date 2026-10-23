//! Input hardening for path arguments (kernel/initctl.md §7): every path in args is resolved by
//! the kernel without following symlinks and checked to stay under the lane's allowed roots.
//!
//! - relative paths are taken from the lane's home (`INIT_HOME`, or the probe root); absolute
//!   paths must lie inside it;
//! - `..` components are refused (`E_ARGS`), so the check is lexical and exact;
//! - every component below the home is `lstat`ed: a symlink or a missing component is
//!   `E_NOT_FOUND` (the `O_NOFOLLOW` rule, IC-18);
//! - the result must be under one of the allowed roots (`E_ARGS`).

use crate::backend::LaneSpec;
use init_proto::ErrorBody;
use init_proto::errors::ErrorCode;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

/// What the resolved path must be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    Dir,
    File,
}

/// Resolve `arg` for `lane` (see the module docs).
pub fn resolve(lane: &LaneSpec, field: &str, arg: &str, want: Want) -> Result<PathBuf, ErrorBody> {
    let p = Path::new(arg);
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(ErrorBody::new(ErrorCode::Args, format!("`{field}` must not contain `..`")));
    }
    let rel: PathBuf = if p.is_absolute() {
        match p.strip_prefix(&lane.home) {
            Ok(r) => r.to_path_buf(),
            Err(_) => return Err(ErrorBody::new(ErrorCode::Args, format!("`{field}` must be inside {}", lane.home.display()))),
        }
    } else {
        p.to_path_buf()
    };
    let rel: PathBuf = rel.components().filter(|c| matches!(c, Component::Normal(_))).collect();
    let full = lane.home.join(&rel);
    if !lane.allowed.iter().any(|a| full.starts_with(a)) {
        let roots: Vec<String> = lane.allowed.iter().map(|a| a.strip_prefix(&lane.home).unwrap_or(a).display().to_string()).collect();
        return Err(ErrorBody::new(ErrorCode::Args, format!("`{field}` must be under {}", roots.join(" or "))));
    }
    let mut cur = lane.home.clone();
    for c in rel.components() {
        cur.push(c);
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(ErrorBody::new(ErrorCode::NotFound, format!("`{field}`: {} is a symlink (paths are not followed)", cur.display())));
            }
            Ok(_) => {}
            Err(_) => return Err(ErrorBody::new(ErrorCode::NotFound, format!("`{field}`: {} not found", cur.display()))),
        }
    }
    let m = fs::symlink_metadata(&full).map_err(|_| ErrorBody::new(ErrorCode::NotFound, format!("`{field}` not found")))?;
    match want {
        Want::Dir if !m.is_dir() => Err(ErrorBody::new(ErrorCode::Args, format!("`{field}` must be a directory"))),
        Want::File if !m.is_file() => Err(ErrorBody::new(ErrorCode::Args, format!("`{field}` must be a regular file"))),
        _ => Ok(full),
    }
}

/// Read a resolved regular file with `O_NOFOLLOW`, at most `max` bytes, as UTF-8.
pub fn read_text(path: &Path, field: &str, max: usize) -> Result<String, ErrorBody> {
    let f: File = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| ErrorBody::new(ErrorCode::NotFound, format!("`{field}`: cannot open without following links: {e}")))?;
    let m = f.metadata().map_err(|e| ErrorBody::new(ErrorCode::NotFound, format!("`{field}`: {e}")))?;
    if !m.is_file() {
        return Err(ErrorBody::new(ErrorCode::Args, format!("`{field}` must be a regular file")));
    }
    if m.len() > max as u64 {
        return Err(ErrorBody::new(ErrorCode::TooLarge, format!("`{field}` is {} bytes; the cap is {max}", m.len())));
    }
    let mut buf = Vec::with_capacity(m.len() as usize);
    f.take(max as u64 + 1).read_to_end(&mut buf).map_err(|e| ErrorBody::new(ErrorCode::NotFound, format!("`{field}`: {e}")))?;
    if buf.len() > max {
        return Err(ErrorBody::new(ErrorCode::TooLarge, format!("`{field}` is over {max} bytes")));
    }
    String::from_utf8(buf).map_err(|_| ErrorBody::new(ErrorCode::Args, format!("`{field}` is not UTF-8")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_rules() {
        let h = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/wp7/unit-paths-{}", std::process::id()));
        let _ = fs::remove_dir_all(&h);
        fs::create_dir_all(h.join("work/wt")).unwrap();
        fs::create_dir_all(h.join("vault")).unwrap();
        let h = h.canonicalize().unwrap();
        fs::write(h.join("work/p.diff"), "diff\n").unwrap();
        std::os::unix::fs::symlink(h.join("work/p.diff"), h.join("work/link.diff")).unwrap();
        std::os::unix::fs::symlink(h.join("vault"), h.join("work/vault")).unwrap();
        let lane = LaneSpec::live(&h);
        assert_eq!(resolve(&lane, "repo", "work/wt", Want::Dir).unwrap(), h.join("work/wt"));
        let abs = h.join("work/p.diff");
        assert_eq!(resolve(&lane, "patch", abs.to_str().unwrap(), Want::File).unwrap(), abs);
        let e = |a: &str, w| resolve(&lane, "x", a, w).unwrap_err().code;
        assert_eq!(e("work/link.diff", Want::File), ErrorCode::NotFound, "IC-18: symlink");
        assert_eq!(e("work/vault", Want::Dir), ErrorCode::NotFound, "symlinked dir");
        assert_eq!(e("work/missing", Want::Dir), ErrorCode::NotFound);
        assert_eq!(e("vault", Want::Dir), ErrorCode::Args, "outside body/ and work/");
        assert_eq!(e("work/../vault", Want::Dir), ErrorCode::Args);
        assert_eq!(e("/etc", Want::Dir), ErrorCode::Args);
        assert_eq!(e("work/wt", Want::File), ErrorCode::Args);
        assert_eq!(read_text(&abs, "patch", 10).unwrap(), "diff\n");
        assert_eq!(read_text(&abs, "patch", 3).unwrap_err().code, ErrorCode::TooLarge);
        assert_eq!(read_text(&h.join("work/link.diff"), "patch", 10).unwrap_err().code, ErrorCode::NotFound);
        fs::remove_dir_all(&h).unwrap();
    }
}
