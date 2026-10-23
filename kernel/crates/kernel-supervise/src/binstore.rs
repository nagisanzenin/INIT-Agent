//! Atomic renames in `bin/` (supervise.md §3.6.5). `bin/init-body` is never missing: every step
//! is a `rename(2)` that atomically replaces its target, and binaries are never modified in place
//! (macOS caches code signatures per file).

use crate::fsx;
use std::fs;
use std::io;
use std::path::Path;

/// Remove `p` if present (exact path).
fn rm(p: &Path) {
    let _ = fs::remove_file(p);
}

/// Keep the outgoing `init-body` as `init-body.prev` (hard link: same inode, no copy).
fn keep_prev(bin: &Path) -> io::Result<()> {
    if !bin.join("init-body").exists() {
        return Ok(());
    }
    let tmp = bin.join(".init-body.prev.tmp");
    rm(&tmp);
    fs::hard_link(bin.join("init-body"), &tmp)?;
    fs::rename(&tmp, bin.join("init-body.prev"))
}

/// The handoff commit (§3.6.5 `commit_bin`): `.prev` ← current, `init-body` ← `.staged`.
pub fn commit_bin(bin: &Path) -> io::Result<()> {
    keep_prev(bin)?;
    crate::fault::point("supervise.commit.after_prev_link");
    fs::rename(bin.join("init-body.staged"), bin.join("init-body"))?; // commit point
    crate::fault::point("supervise.commit.after_staged_rename");
    fsx::sync_dir(bin)
}

/// Rollback and probation installs (§3.6.5): `hard_link(<from> → .init-body.next.tmp)`, keep
/// the outgoing binary as `.prev`, then rename `.next.tmp` over `init-body`.
pub fn install_from(bin: &Path, from_file: &str) -> io::Result<()> {
    let next = bin.join(".init-body.next.tmp");
    rm(&next);
    fs::hard_link(bin.join(from_file), &next)?;
    keep_prev(bin)?;
    fs::rename(&next, bin.join("init-body"))?;
    fsx::sync_dir(bin)
}

/// The LKG install (§3.6.5): `init-body.lkg` ← the current inode.
pub fn install_lkg(bin: &Path) -> io::Result<()> {
    let tmp = bin.join(".init-body.lkg.tmp");
    rm(&tmp);
    fs::hard_link(bin.join("init-body"), &tmp)?;
    crate::fault::point("supervise.lkg.after_bin_link");
    fs::rename(&tmp, bin.join("init-body.lkg"))?;
    fsx::sync_dir(bin)
}

/// Write `bytes` as a new inode named `name` (store.get restores, §3.9 step 6).
pub fn write_new_inode(bin: &Path, name: &str, src: &mut dyn io::Read) -> io::Result<String> {
    use io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = bin.join(format!(".{name}.restore.tmp"));
    rm(&tmp);
    let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o755).custom_flags(libc::O_NOFOLLOW).open(&tmp)?;
    io::copy(src, &mut f)?;
    f.flush()?;
    f.sync_all()?;
    drop(f);
    let sha = fsx::sha256_file(&tmp)?;
    fs::rename(&tmp, bin.join(name))?;
    fsx::sync_dir(bin)?;
    Ok(sha)
}

/// Cold-start repair (§3.9 step 7). Returns what was done.
pub fn repair(bin: &Path) -> io::Result<Option<&'static str>> {
    if bin.join("init-body").exists() {
        return Ok(None);
    }
    if bin.join("init-body.prev").exists() {
        fs::rename(bin.join("init-body.prev"), bin.join("init-body"))?;
        fsx::sync_dir(bin)?;
        return Ok(Some("restored init-body from init-body.prev"));
    }
    if bin.join("init-body.lkg").exists() {
        let tmp = bin.join(".init-body.repair.tmp");
        rm(&tmp);
        fs::hard_link(bin.join("init-body.lkg"), &tmp)?;
        fs::rename(&tmp, bin.join("init-body"))?;
        fsx::sync_dir(bin)?;
        return Ok(Some("restored init-body from init-body.lkg"));
    }
    Ok(None)
}

/// SHA-256 of `bin/<name>`, if present.
pub fn sha_of(bin: &Path, name: &str) -> Option<String> {
    fsx::sha256_file(&bin.join(name)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn renames_keep_init_body_present_and_inodes_distinct() {
        let d = crate::testutil::test_dir("binstore");
        let bin = d.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("init-body"), b"X").unwrap();
        install_lkg(&bin).unwrap();
        assert_eq!(fs::metadata(bin.join("init-body")).unwrap().ino(), fs::metadata(bin.join("init-body.lkg")).unwrap().ino());
        fs::write(bin.join("init-body.staged"), b"Y").unwrap();
        commit_bin(&bin).unwrap();
        assert_eq!(fs::read(bin.join("init-body")).unwrap(), b"Y");
        assert_eq!(fs::read(bin.join("init-body.prev")).unwrap(), b"X");
        assert!(!bin.join("init-body.staged").exists());
        // Rollback to LKG.
        install_from(&bin, "init-body.lkg").unwrap();
        assert_eq!(fs::read(bin.join("init-body")).unwrap(), b"X");
        assert_eq!(fs::read(bin.join("init-body.prev")).unwrap(), b"Y");
        // SV-22: init-body deleted, .prev present → restored before spawn.
        fs::remove_file(bin.join("init-body")).unwrap();
        assert!(repair(&bin).unwrap().is_some());
        assert_eq!(fs::read(bin.join("init-body")).unwrap(), b"Y");
        fs::remove_file(bin.join("init-body")).unwrap();
        assert!(repair(&bin).unwrap().unwrap().contains("lkg"));
        assert_eq!(fs::read(bin.join("init-body")).unwrap(), b"X");
        let sha = write_new_inode(&bin, "init-body", &mut &b"Z"[..]).unwrap();
        assert_eq!(sha, fsx::sha256_bytes(b"Z"));
        crate::testutil::remove_test_dir(&d);
    }
}
