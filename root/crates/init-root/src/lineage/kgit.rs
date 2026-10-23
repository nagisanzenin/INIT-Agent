//! `INIT_HOME/lineage/kernel.git` (root/root.md §3.8.1, D7): a bare repository only the Root writes and
//! the zone can only read. Refs: `refs/upstream/<version>` (release source bundles, create-only),
//! `refs/heads/main` (the current tuple's kernel source, CAS), `refs/heads/lkg`,
//! `refs/init/karchive/<sha>` and `refs/init/rolled-back/<sha>` (create-only).
//! The swap (§3.8) and install (§4.3) call these; `fetch_agent` takes the sandboxed upload-pack.
#![allow(dead_code)] // called by install and the swap (install.rs, sup.rs: WP21)

use super::mirror::run;
use crate::install::{self, Paths};
use std::path::Path;
use std::process::Command;

/// `$RK`: the hardened Root git prefix (§3.11) on `INIT_HOME/lineage/kernel.git`, objects checked.
fn rk(p: &Paths, git: &str, args: &[&str]) -> Result<String, String> {
    let mut c: Command = install::git(p, git);
    c.env("GIT_ASKPASS", "/usr/bin/false")
        .args(["-c", "protocol.allow=never", "-c", "protocol.file.allow=always", "-c", "transfer.fsckObjects=true"])
        .arg(format!("--git-dir={}", p.i("lineage/kernel.git").display()))
        .args(args);
    run(c, b"").map(|s| s.trim().to_string())
}

fn oid(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// The commit a ref names, if it exists.
pub fn get(p: &Paths, git: &str, r: &str) -> Option<String> {
    rk(p, git, &["rev-parse", "-q", "--verify", &format!("{r}^{{commit}}")]).ok().filter(|s| oid(s))
}

/// `update-ref` with compare-and-swap: `old` None means the ref must not exist (create-only).
pub fn update(p: &Paths, git: &str, r: &str, new: &str, old: Option<&str>) -> Result<(), String> {
    if !oid(new) || old.is_some_and(|o| !oid(o)) {
        return Err(format!("{r}: not an object id"));
    }
    rk(p, git, &["update-ref", r, new, old.unwrap_or("")]).map(drop)
}

/// A maintainer source bundle (§3.8.1): exactly one ref, `refs/heads/release`, imported create-only as
/// `refs/upstream/<version>`. The same commit again is a no-op; another commit is refused.
pub fn import(p: &Paths, git: &str, bundle: &Path, version: &str) -> Result<String, String> {
    let ok = (1..=64).contains(&version.len()) && !version.contains("..") && !version.starts_with('.')
        && version.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-+".contains(&c));
    if !ok {
        return Err(format!("kernel source version {version:?} is not a plain version string"));
    }
    let b = bundle.to_str().ok_or("bundle path is not UTF-8")?;
    let heads = rk(p, git, &["bundle", "list-heads", b])?;
    let sha = match heads.lines().map(|l| l.split_once(' ')).collect::<Vec<_>>().as_slice() {
        [Some((sha, "refs/heads/release"))] if oid(sha) => sha.to_string(),
        _ => return Err(format!("{}: a kernel source bundle holds exactly one ref, refs/heads/release", bundle.display())),
    };
    let r = format!("refs/upstream/{version}");
    match get(p, git, &r) {
        Some(e) if e == sha => return Ok(sha),
        Some(e) => return Err(format!("{r} exists at {e}; the bundle has {sha}: refused (create-only)")),
        None => {}
    }
    rk(p, git, &["fetch", "--no-tags", "--no-write-fetch-head", b, &format!("refs/heads/release:{r}")])?;
    Ok(sha)
}

/// Install (§4.3 step 6): every `INIT_HOME/kernel/src/<version>.bundle`; with exactly one bundle and no
/// `main` yet, `main` is created at it (the installed kernel's source), and so is `lkg` (build-deviations §5
/// L4: both name the imported release). Returns (version, commit) pairs.
pub fn import_release(p: &Paths, git: &str) -> Result<Vec<(String, String)>, String> {
    let mut b: Vec<_> = std::fs::read_dir(p.i("kernel/src")).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|x| x.extension().is_some_and(|e| e == "bundle")).collect();
    b.sort();
    let mut out = Vec::new();
    for f in &b {
        let v = f.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        out.push((v.clone(), import(p, git, f, &v)?));
    }
    if let [(_, sha)] = out.as_slice()
        && get(p, git, "refs/heads/main").is_none()
    {
        update(p, git, "refs/heads/main", sha, None)?;
        if get(p, git, "refs/heads/lkg").is_none() {
            update(p, git, "refs/heads/lkg", sha, None)?;
        }
    }
    Ok(out)
}

/// An agent kernel source (§3.8.1, phase 4): exactly `sha` from `repo` (a bare repo under INIT_HOME)
/// through `upload_pack` (the Root's `init-root sandboxed-upload-pack`, never a plain git on zone data)
/// into the create-only `refs/init/karchive/<sha>`; it must descend from `main`. Moving `main` is the
/// caller's CAS `update` once the install succeeds.
pub fn fetch_agent(p: &Paths, git: &str, upload_pack: &str, repo: &Path, sha: &str) -> Result<(), String> {
    if !oid(sha) || !repo.is_absolute() || !repo.starts_with(&p.ih) || repo.components().any(|c| c.as_os_str() == "..") {
        return Err("fetch_agent: a full commit id and a bare repo under INIT_HOME".into());
    }
    let karchive = format!("refs/init/karchive/{sha}");
    if get(p, git, &karchive).is_some() {
        return Err(format!("{karchive} exists (create-only)"));
    }
    let r = repo.to_str().ok_or("repo path is not UTF-8")?;
    rk(p, git, &["fetch", "--no-tags", "--no-write-fetch-head", "--no-recurse-submodules",
        &format!("--upload-pack={upload_pack}"), r, &format!("+{sha}:{karchive}")])?;
    super::env_fault("root.kgit.after_fetch").map_err(|e| e.to_string())?;
    match get(p, git, "refs/heads/main") {
        Some(main) => rk(p, git, &["merge-base", "--is-ancestor", &main, sha])
            .map(drop).map_err(|_| format!("{sha} does not descend from main {main}")),
        None => Ok(()),
    }
}

/// Rollback (§3.8.1): `main` moves back to `to` (CAS), and the source it leaves is kept under the
/// create-only `refs/init/rolled-back/<sha>`.
pub fn rollback(p: &Paths, git: &str, to: &str) -> Result<(), String> {
    let cur = get(p, git, "refs/heads/main").ok_or("refs/heads/main is missing")?;
    if cur == to {
        return Ok(());
    }
    let kept = format!("refs/init/rolled-back/{cur}");
    if get(p, git, &kept).is_none() {
        update(p, git, &kept, &cur, None)?;
    }
    update(p, git, "refs/heads/main", to, Some(&cur))
}
