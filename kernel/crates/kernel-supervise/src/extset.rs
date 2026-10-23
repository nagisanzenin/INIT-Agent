//! Kept-component sets (supervise.md §3.6.5 "Kept T1 components move with the binary", §3.6.8).
//!
//! A set is published once as `bin/ext.d/<set>/` (`<set>` = the first 12 hex of SHA-256 over the
//! sorted `name\tsha256\n` lines; every file a new inode; the directory is never modified
//! afterwards). `bin/ext`, `bin/ext.prev` and `bin/ext.lkg` are symlinks to `ext.d/<set>`,
//! replaced atomically (`.<link>.tmp` + rename) and followed by a directory fsync.
//!
//! WP7b uses it for data-only T1 promotions (publish, swap `bin/ext`, `reload{kind:"kept"}`), the
//! LKG commit (`bin/ext.lkg`, `store.put` of each component, `ext_set`/`components` in the
//! report) and rollback (`bin/ext` follows `bin/ext.lkg`).

use crate::fsx;
use serde_json::{json, Value};
use std::io;
use std::path::Path;

/// A published set: its id and `(name, sha256 hex)` per component, sorted by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtSet {
    pub id: String,
    pub components: Vec<(String, String)>,
}

/// The `<name>.wasm` files of `dir` with their SHA-256, sorted by name.
pub fn components_of(dir: &Path) -> io::Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".wasm") else { continue };
        if !e.file_type()?.is_file() || !crate::layout::valid_id(stem) {
            continue;
        }
        out.push((stem.to_string(), fsx::sha256_file(&e.path())?));
    }
    out.sort();
    Ok(out)
}

/// The set id of `components` (12 hex).
pub fn set_id(components: &[(String, String)]) -> String {
    let lines: String = components.iter().map(|(n, s)| format!("{n}\t{s}\n")).collect();
    fsx::sha256_bytes(lines.as_bytes())[..12].to_string()
}

/// Publish the components the gate built in `built` as `bin/ext.d/<set>/` (reused when it already
/// exists). `None` when `built` holds no component.
pub fn publish(bin: &Path, built: &Path) -> io::Result<Option<ExtSet>> {
    let comps = match components_of(built) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if comps.is_empty() {
        return Ok(None);
    }
    let id = set_id(&comps);
    let d = bin.join("ext.d");
    let dst = d.join(&id);
    if !dst.is_dir() {
        std::fs::create_dir_all(&d)?;
        let tmp = d.join(format!(".{id}.tmp"));
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp)?; // exact path: a half-published copy of this set
        }
        std::fs::create_dir(&tmp)?;
        for (name, sha) in &comps {
            let mut src = std::fs::File::open(built.join(format!("{name}.wasm")))?;
            let got = crate::binstore::write_new_inode(&tmp, &format!("{name}.wasm"), &mut src)?;
            if &got != sha {
                return Err(io::Error::other(format!("{name}.wasm changed while publishing")));
            }
        }
        fsx::sync_dir(&tmp)?;
        std::fs::rename(&tmp, &dst)?;
        fsx::sync_dir(&d)?;
    }
    Ok(Some(ExtSet { id, components: comps }))
}

/// The set a `bin/<link>` symlink points at, if it points into `ext.d/`.
pub fn target(bin: &Path, link: &str) -> Option<String> {
    let t = std::fs::read_link(bin.join(link)).ok()?;
    let set = t.to_string_lossy().strip_prefix("ext.d/")?.to_string();
    (set.len() == 12 && set.bytes().all(|b| b.is_ascii_hexdigit())).then_some(set)
}

/// Point `bin/<link>` at `ext.d/<set>` atomically.
pub fn point(bin: &Path, link: &str, set: &str) -> io::Result<()> {
    let tmp = bin.join(format!(".{link}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(format!("ext.d/{set}"), &tmp)?;
    std::fs::rename(&tmp, bin.join(link))?;
    fsx::sync_dir(bin)
}

/// Swap `bin/<link>` to `set`, keeping the outgoing target as `bin/<link>.prev` (the order of
/// `commit_bin`), then delete sets no link references.
pub fn swap(bin: &Path, link: &str, set: &str) -> io::Result<()> {
    if let Some(old) = target(bin, link) {
        if old == set {
            return Ok(());
        }
        point(bin, &format!("{link}.prev"), &old)?;
    }
    point(bin, link, set)?;
    gc(bin);
    Ok(())
}

/// `bin/<link>` follows `bin/<from>` (rollback: `ext` ← `ext.lkg`). No-op when `from` is unset.
pub fn follow(bin: &Path, link: &str, from: &str) -> io::Result<()> {
    match target(bin, from) {
        Some(set) if target(bin, link).as_deref() != Some(set.as_str()) => point(bin, link, &set),
        _ => Ok(()),
    }
}

/// `ext_set` and `components` for a `promotion` report, from what `bin/<link>` points at.
pub fn report(bin: &Path, link: &str) -> (Value, Value) {
    let Some(set) = target(bin, link) else { return (Value::Null, json!([])) };
    let comps = components_of(&bin.join("ext.d").join(&set)).unwrap_or_default();
    (json!(set), Value::Array(comps.into_iter().map(|(n, s)| json!({"name": n, "sha256": s})).collect()))
}

/// Delete (by exact path) the sets none of `ext`, `ext.prev`, `ext.staged`, `ext.lkg` names.
fn gc(bin: &Path) {
    let keep: Vec<String> = ["ext", "ext.prev", "ext.staged", "ext.lkg"].iter().filter_map(|l| target(bin, l)).collect();
    let Ok(rd) = std::fs::read_dir(bin.join("ext.d")) else { return };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.len() == 12 && n.bytes().all(|b| b.is_ascii_hexdigit()) && !keep.contains(&n) {
            let _ = std::fs::remove_dir_all(bin.join("ext.d").join(&n));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_swap_follow_report_gc() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp7b");
        let d = base.join(format!("extset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (bin, built) = (d.join("bin"), d.join("built"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&built).unwrap();
        assert_eq!(publish(&bin, &built).unwrap(), None, "no component, no set");
        std::fs::write(built.join("semver-sort.wasm"), b"\0asm one").unwrap();
        std::fs::write(built.join("notes.txt"), b"ignored").unwrap();
        let a = publish(&bin, &built).unwrap().unwrap();
        assert_eq!(a.components.len(), 1);
        assert_eq!(a.id, set_id(&a.components));
        assert!(bin.join("ext.d").join(&a.id).join("semver-sort.wasm").is_file());
        swap(&bin, "ext", &a.id).unwrap();
        point(&bin, "ext.lkg", &a.id).unwrap();
        std::fs::write(built.join("semver-sort.wasm"), b"\0asm two").unwrap();
        let b = publish(&bin, &built).unwrap().unwrap();
        assert_ne!(a.id, b.id);
        swap(&bin, "ext", &b.id).unwrap();
        assert_eq!(target(&bin, "ext").as_deref(), Some(b.id.as_str()));
        assert_eq!(target(&bin, "ext.prev").as_deref(), Some(a.id.as_str()));
        let (set, comps) = report(&bin, "ext");
        assert_eq!(set, json!(b.id));
        assert_eq!(comps[0]["name"], "semver-sort");
        follow(&bin, "ext", "ext.lkg").unwrap();
        assert_eq!(target(&bin, "ext").as_deref(), Some(a.id.as_str()));
        // A set nothing references is deleted at the next swap.
        std::fs::write(built.join("semver-sort.wasm"), b"\0asm three").unwrap();
        let c = publish(&bin, &built).unwrap().unwrap();
        point(&bin, "ext.prev", &a.id).unwrap();
        swap(&bin, "ext", &c.id).unwrap();
        assert!(!bin.join("ext.d").join(&b.id).exists());
        assert!(bin.join("ext.d").join(&a.id).exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
