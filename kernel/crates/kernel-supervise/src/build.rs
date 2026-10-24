//! The build slot and the dependency cache (supervise.md §3.6.2).
//!
//! One fixed slot, `cache/build/slot/{src,out}`, so absolute paths are identical across builds
//! (builds are serialized by the caller). The candidate is exported into `src/` from lineage by
//! SHA, built offline in a `build` instance, and its binary staged into `bin/` as a new inode.
//! `cache/deps/<key>/target/` holds dependency artifacts compiled from a stub workspace that
//! contains no first-party code; a warm build starts from a copy-on-write clone of it.

use crate::git::{CodedError, Git, GitError};
use crate::instance::{ExitInfo, SpawnRequest, Spawner, run_to_completion};
use crate::spawnspec::{SandboxPaths, build_params};
use init_proto::root_link::InstanceKind;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use toml::{Table, Value};

/// `build.log` cap (§3.6.2).
pub const BUILD_LOG_CAP: u64 = 4 << 20;
/// Inner sandbox builds cannot inherit CI's jobserver or trust host-visible CPU
/// counts. Keep compiler concurrency bounded on laptops and quota-based runners.
pub const SANDBOX_CARGO_JOBS: &str = "2";
/// `deps_cache_keys` (§4.9).
pub const DEPS_KEYS: usize = 3;
/// Read caps for manifests in an export (K-F2 style: the content is agent-written).
const MANIFEST_CAP: usize = 1 << 20;
const LOCK_CAP: usize = 64 << 20;

/// The exact `cargo` arguments of §3.6.2's build command.
pub const BUILD_ARGS: [&str; 10] = [
    "build",
    "--frozen",
    "--bin",
    "init-body",
    "--profile",
    "dev",
    "--config",
    "source.crates-io.replace-with=\"vendored-sources\"",
    "--config",
    "source.vendored-sources.directory=\"vendor\"",
];

// ---- deps_key (§3.6.2) ----------------------------------------------------------------------

/// `hex(sha256(Cargo.lock bytes ‖ tree id of vendor/ ‖ blob id of Cargo.toml ‖ blob id of
/// .cargo/config.toml or "-" ‖ blob ids of ext/Cargo.toml and ext/Cargo.lock or "-" ‖
/// rustc -vV ‖ host triple ‖ "dev"))[..16]`, every part read from lineage by SHA, never from an
/// agent tree. Each part is followed by a NUL so no two part lists hash the same bytes.
pub fn deps_key(git: &Git, sha: &str, rustc_vv: &str, host: &str) -> Result<String, GitError> {
    use sha2::Digest;
    let lock = git.path_id(sha, "Cargo.lock")?.ok_or_else(|| GitError::Usage(format!("{sha} has no Cargo.lock")))?;
    let lock = git.read_blob(&lock)?;
    let id = |p: &str| -> Result<String, GitError> { Ok(git.path_id(sha, p)?.unwrap_or_else(|| "-".into())) };
    let parts = [id("vendor")?, id("Cargo.toml")?, id(".cargo/config.toml")?, id("ext/Cargo.toml")?, id("ext/Cargo.lock")?];
    let mut h = sha2::Sha256::new();
    h.update(&lock);
    h.update([0]);
    for p in parts.iter().map(String::as_str).chain([rustc_vv, host, "dev"]) {
        h.update(p.as_bytes());
        h.update([0]);
    }
    Ok(crate::fsx::hex(&h.finalize())[..16].to_string())
}

/// A deps key: 16 lowercase hex (the only names [`DepsCache`] turns into paths).
pub fn is_key(key: &str) -> bool {
    key.len() == 16 && key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// ---- cache/deps (§3.6.2) --------------------------------------------------------------------

/// `cache/deps/`: `<key>/target/` per completed key (never written again), populated through
/// `<key>.tmp/` and a rename, at most [`DEPS_KEYS`] kept (LRU in `lru.json`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepsCache {
    pub dir: PathBuf,
}

impl DepsCache {
    pub fn new(home: &Path) -> DepsCache {
        DepsCache { dir: home.join("cache/deps") }
    }

    fn key_dir(&self, key: &str) -> io::Result<PathBuf> {
        if is_key(key) { Ok(self.dir.join(key)) } else { Err(io::Error::new(io::ErrorKind::InvalidInput, format!("bad deps key {key:?}"))) }
    }

    /// `cache/deps/<key>/target/` when that key is complete.
    pub fn lookup(&self, key: &str) -> Option<PathBuf> {
        let t = self.key_dir(key).ok()?.join("target");
        let real_dir = |p: &Path| fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false);
        (real_dir(&self.dir.join(key)) && real_dir(&t)).then_some(t)
    }

    /// Hit: clone `cache/deps/<key>/target/` to `dst` (the slot's `out/target`; an empty
    /// directory there is replaced) with `sys::clone_path`, and mark the key used.
    pub fn clone_into(&self, key: &str, dst: &Path) -> io::Result<()> {
        let src = self.lookup(key).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("deps key {key} not cached")))?;
        if let Ok(m) = fs::symlink_metadata(dst) {
            if !m.is_dir() || fs::read_dir(dst)?.next().is_some() {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists and is not an empty dir", dst.display())));
            }
            fs::remove_dir(dst)?;
        }
        crate::sys::clone_path(&src, dst)?;
        self.touch(key)
    }

    /// Populate `<key>` from a finished stub build's target dir: [`DepsCache::stage`] then
    /// [`DepsCache::publish`]. `Ok(false)` when the key was already complete (it is never written
    /// again). Run the stub build in the build slot so its absolute paths match later builds.
    pub fn commit(&self, key: &str, built_target: &Path) -> io::Result<bool> {
        match self.stage(key, built_target)? {
            Some(digest) => self.publish(key, &digest),
            None => Ok(false),
        }
    }

    /// `<key>.tmp`: a staged entry, published only after the candidate's G0 passed.
    fn tmp_dir(&self, key: &str) -> io::Result<PathBuf> {
        self.key_dir(key).map(|_| self.dir.join(format!("{key}.tmp")))
    }

    /// Stage `<key>`: clone a finished stub build's target dir into `<key>.tmp/target` and return
    /// its [`tree_digest`], taken before any candidate code runs. `Ok(None)` when the key is already
    /// complete. Leftover `<key>.tmp` dirs of earlier, interrupted builds are deleted by exact path.
    pub fn stage(&self, key: &str, built_target: &Path) -> io::Result<Option<String>> {
        let fin = self.key_dir(key)?;
        if fs::symlink_metadata(&fin).is_ok() {
            return Ok(None);
        }
        fs::create_dir_all(&self.dir)?;
        for e in fs::read_dir(&self.dir)? {
            let name = e?.file_name().to_string_lossy().into_owned();
            if name.strip_suffix(".tmp").is_some_and(is_key) {
                remove_exact(&self.dir.join(&name))?;
            }
        }
        let tmp = self.tmp_dir(key)?;
        fs::create_dir(&tmp)?;
        let res = crate::sys::clone_path(built_target, &tmp.join("target")).and_then(|()| tree_digest(&tmp.join("target")));
        if res.is_err() {
            let _ = remove_exact(&tmp);
        }
        res.map(Some)
    }

    /// Publish a staged `<key>`: its tree must still have the digest [`DepsCache::stage`] returned
    /// (else it is deleted and this fails: something wrote into it while the candidate ran). Writes
    /// `manifest.json` with the digest, renames `<key>.tmp` → `<key>`, fsyncs `cache/deps/` and
    /// marks the key used. `Ok(false)` when the key was already complete.
    pub fn publish(&self, key: &str, digest: &str) -> io::Result<bool> {
        let fin = self.key_dir(key)?;
        let tmp = self.tmp_dir(key)?;
        if fs::symlink_metadata(&fin).is_ok() {
            remove_exact(&tmp)?;
            return Ok(false);
        }
        let res = (|| {
            let now = tree_digest(&tmp.join("target"))?;
            if now != digest {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("staged deps {key} changed after staging")));
            }
            write_new(&tmp.join(DEPS_MANIFEST), serde_json::json!({"v": 1, "key": key, "digest": digest}).to_string().as_bytes())?;
            fs::rename(&tmp, &fin)
        })();
        if let Err(e) = res {
            let _ = remove_exact(&tmp);
            return if fs::symlink_metadata(&fin).is_ok() { Ok(false) } else { Err(e) };
        }
        crate::fsx::sync_dir(&self.dir)?;
        self.touch(key)?;
        Ok(true)
    }

    /// Delete a staged `<key>.tmp` by exact path (the candidate failed, or the build was cut short).
    pub fn discard(&self, key: &str) -> io::Result<()> {
        remove_exact(&self.tmp_dir(key)?)
    }

    /// Whether the complete `<key>` still has the digest recorded in its `manifest.json`. A
    /// missing or unreadable manifest, or any changed, added or removed file, is `false`.
    pub fn verify(&self, key: &str) -> bool {
        let Some(t) = self.lookup(key) else { return false };
        let Ok(b) = crate::fsx::read_nofollow(&self.dir.join(key).join(DEPS_MANIFEST), 4096, false) else { return false };
        let m: serde_json::Value = serde_json::from_slice(&b).unwrap_or_default();
        match (m["v"].as_u64(), m["key"].as_str(), m["digest"].as_str()) {
            (Some(1), Some(k), Some(d)) if k == key => tree_digest(&t).is_ok_and(|now| now == d),
            _ => false,
        }
    }

    /// Delete a complete `<key>` by exact path (a corrupt entry) and drop it from `lru.json`.
    pub fn remove(&self, key: &str) -> io::Result<()> {
        remove_exact(&self.key_dir(key)?)?;
        let mut m = self.read_lru();
        if m.remove(key).is_some() {
            self.write_lru(&m)?;
        }
        Ok(())
    }

    /// Keep the `keep` most recently used complete keys; delete the rest by exact path.
    /// Returns the evicted keys.
    pub fn evict_lru(&self, keep: usize) -> io::Result<Vec<String>> {
        let mut keys = self.complete_keys()?;
        let lru = self.read_lru();
        let used = |k: &str| lru.get(k).copied().unwrap_or_else(|| self.mtime_ms(k));
        keys.sort_by_key(|k| std::cmp::Reverse(used(k)));
        let gone: Vec<String> = keys.split_off(keep.min(keys.len()));
        for k in &gone {
            remove_exact(&self.key_dir(k)?)?;
        }
        if !gone.is_empty() {
            let kept: BTreeMap<String, i64> = keys.iter().map(|k| (k.clone(), used(k))).collect();
            self.write_lru(&kept)?;
        }
        Ok(gone)
    }

    /// Complete keys: real directories named by 16 hex.
    fn complete_keys(&self) -> io::Result<Vec<String>> {
        let rd = match fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut v = Vec::new();
        for e in rd {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if is_key(&name) && e.file_type()?.is_dir() {
                v.push(name);
            }
        }
        Ok(v)
    }

    fn lru_path(&self) -> PathBuf {
        self.dir.join("lru.json")
    }

    fn read_lru(&self) -> BTreeMap<String, i64> {
        let Ok(b) = crate::fsx::read_nofollow(&self.lru_path(), 1 << 20, false) else { return BTreeMap::new() };
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap_or_default();
        v.get("keys")
            .and_then(|k| k.as_object())
            .map(|o| o.iter().filter_map(|(k, ms)| Some((k.clone(), ms.as_i64()?))).filter(|(k, _)| is_key(k)).collect())
            .unwrap_or_default()
    }

    fn write_lru(&self, keys: &BTreeMap<String, i64>) -> io::Result<()> {
        crate::fsx::write_json(&self.lru_path(), &serde_json::json!({"keys": keys, "v": 1}))
    }

    /// Mark `key` used now.
    fn touch(&self, key: &str) -> io::Result<()> {
        let mut m = self.read_lru();
        m.insert(key.to_string(), crate::clock::now_ms());
        self.write_lru(&m)
    }

    fn mtime_ms(&self, key: &str) -> i64 {
        fs::symlink_metadata(self.dir.join(key)).and_then(|m| m.modified()).map(crate::clock::to_ms).unwrap_or(0)
    }
}

/// A complete key's record: `{"v":1,"key":<key>,"digest":<tree_digest of target/>}`.
const DEPS_MANIFEST: &str = "manifest.json";

/// SHA-256 over a directory tree, as 64 lowercase hex: every entry in sorted relative-path order,
/// with its type, and for a regular file its length and the SHA-256 of its bytes, for a symlink
/// its target. Symlinks are never followed.
pub fn tree_digest(dir: &Path) -> io::Result<String> {
    use sha2::Digest;
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;
    let mut entries: Vec<(PathBuf, fs::FileType)> = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for e in fs::read_dir(dir.join(&rel))? {
            let e = e?;
            let (r, ft) = (rel.join(e.file_name()), e.file_type()?);
            if ft.is_dir() {
                stack.push(r.clone());
            }
            entries.push((r, ft));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    for (rel, ft) in entries {
        let p = dir.join(&rel);
        let kind: &[u8] = if ft.is_dir() { b"d" } else if ft.is_symlink() { b"l" } else if ft.is_file() { b"f" } else { b"o" };
        h.update(kind);
        h.update(rel.as_os_str().as_bytes());
        h.update([0]);
        if ft.is_symlink() {
            h.update(fs::read_link(&p)?.as_os_str().as_bytes());
            h.update([0]);
        } else if ft.is_file() {
            let mut f = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&p)?;
            let mut fh = sha2::Sha256::new();
            let mut len = 0u64;
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                fh.update(&buf[..n]);
                len += n as u64;
            }
            h.update(len.to_string().as_bytes());
            h.update([0]);
            h.update(fh.finalize());
        }
    }
    Ok(crate::fsx::hex(&h.finalize()))
}

/// The mtime every entry under an export's `vendor/` gets (2000-01-01T00:00:00Z). `git archive`
/// stamps every file with the commit's time, so without this each new commit would look newer
/// than the cached build-script outputs (`rerun-if-changed` compares mtimes) and rebuild them and
/// everything above them. The bytes are untouched; `deps_key` covers the vendor tree's content.
pub const VENDOR_MTIME_S: u64 = 946_684_800;

/// `SOURCE_DATE_EPOCH` of every slot build (the stub, the candidate build, G0): one fixed value,
/// not the commit's time. The `cc` crate (wasmtime's build script) declares
/// `rerun-if-env-changed=SOURCE_DATE_EPOCH`, so a per-commit value made every promote rebuild
/// wasmtime and everything above it, cache or not (MERGE-T2CACHE). A constant is as reproducible.
pub const SLOT_EPOCH_S: i64 = VENDOR_MTIME_S as i64;

/// Set the mtime (and atime) of every directory and regular file under `<src>/vendor` to
/// [`VENDOR_MTIME_S`]. Symlinks are neither followed nor changed. Returns how many entries were
/// set; no `vendor/` is 0.
pub fn pin_vendor_mtimes(src: &Path) -> io::Result<u64> {
    let root = src.join("vendor");
    match fs::symlink_metadata(&root) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Ok(0),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    }
    let t = std::time::UNIX_EPOCH + Duration::from_secs(VENDOR_MTIME_S);
    let times = fs::FileTimes::new().set_accessed(t).set_modified(t);
    let set = |p: &Path| -> io::Result<()> { OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(p)?.set_times(times) };
    let mut n = 0u64;
    let mut stack = vec![root.clone()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let e = e?;
            let ft = e.file_type()?;
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() {
                set(&e.path())?;
                n += 1;
            }
        }
        set(&d)?;
        n += 1;
    }
    Ok(n)
}

/// Delete, by exact path, every artifact of the first-party packages `names` from a stub build's
/// target dir: in each profile dir (one holding `.fingerprint/`, at most two levels down), the
/// entries of `.fingerprint/`, `deps/`, `build/`, `examples/` and `incremental/` named
/// `<n>-…` or `lib<n>-…` (`n` in both the `-` and `_` spellings), and the profile dir's own
/// `<n>`, `<n>.…` and `lib<n>.…`. The stub's empty `init-body` must never look fresh to a
/// candidate's build (cargo would skip compiling the candidate). Returns how many entries went.
pub fn prune_first_party(target: &Path, names: &[String]) -> io::Result<usize> {
    let mut forms: Vec<String> = Vec::new();
    for n in names {
        for f in [n.clone(), n.replace('-', "_")] {
            if !f.is_empty() && !forms.contains(&f) {
                forms.push(f);
            }
        }
    }
    let hashed = |name: &str| forms.iter().any(|f| name.starts_with(&format!("{f}-")) || name.starts_with(&format!("lib{f}-")));
    let top = |name: &str| forms.iter().any(|f| name == f || name.starts_with(&format!("{f}.")) || name.starts_with(&format!("lib{f}.")));
    let real_dir = |p: &Path| fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false);
    let mut profiles = Vec::new();
    let mut level = vec![target.to_path_buf()];
    for _ in 0..3 {
        let mut next = Vec::new();
        for d in &level {
            if real_dir(&d.join(".fingerprint")) {
                profiles.push(d.clone());
            }
            if let Ok(rd) = fs::read_dir(d) {
                for e in rd.flatten() {
                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        next.push(e.path());
                    }
                }
            }
        }
        level = next;
    }
    let mut gone = 0;
    for p in profiles {
        for sub in [".fingerprint", "deps", "build", "examples", "incremental"] {
            let Ok(rd) = fs::read_dir(p.join(sub)) else { continue };
            for e in rd.flatten() {
                if hashed(&e.file_name().to_string_lossy()) {
                    remove_exact(&e.path())?;
                    gone += 1;
                }
            }
        }
        for e in fs::read_dir(&p)?.flatten() {
            if top(&e.file_name().to_string_lossy()) {
                remove_exact(&e.path())?;
                gone += 1;
            }
        }
    }
    Ok(gone)
}

/// Remove a file or directory tree at exactly `p` (absent is fine; a symlink is removed, not
/// followed).
pub(crate) fn remove_exact(p: &Path) -> io::Result<()> {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(p),
        Ok(_) => fs::remove_file(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

// ---- the stub workspace (§3.6.2 "Miss") -----------------------------------------------------

/// Top-level manifest tables a stub keeps verbatim: dependency resolution must not change
/// (`--frozen` uses the candidate's `Cargo.lock`).
const STUB_TABLES: [&str; 10] = [
    "dependencies",
    "dev-dependencies",
    "dev_dependencies",
    "build-dependencies",
    "build_dependencies",
    "features",
    "profile",
    "workspace",
    "patch",
    "replace",
];
/// `[package]` keys a stub keeps; everything naming files or scripts is dropped.
const STUB_PACKAGE: [&str; 7] = ["name", "version", "edition", "rust-version", "resolver", "publish", "workspace"];
/// `-C` keys a stub's rustflags may carry: they change code generation, never which program
/// runs (no `linker`, `link-arg(s)`, `llvm-args`, plugins or codegen backends).
const SAFE_CODEGEN: [&str; 16] = [
    "opt-level",
    "debuginfo",
    "target-cpu",
    "target-feature",
    "codegen-units",
    "panic",
    "overflow-checks",
    "debug-assertions",
    "force-frame-pointers",
    "force-unwind-tables",
    "embed-bitcode",
    "lto",
    "strip",
    "split-debuginfo",
    "relocation-model",
    "symbol-mangling-version",
];

/// Build a stub workspace in `stub_dir` (created; must not exist or be empty) from an export in
/// `src_dir`: the candidate's `Cargo.toml` with every target replaced by one `src/main.rs`
/// (`fn main() {}`), `build = false`, the same dependency, `[features]`, `[profile.*]`,
/// `[workspace]` and `[patch]` tables; the same `Cargo.lock`; a clone of `vendor/`; and
/// `.cargo/config.toml` reduced to its source replacement, offline mode, profiles and safe
/// rustflags. Only vendored crates' code can run when it builds. When the export has
/// `ext/Cargo.toml` (the extension workspace), the stub gets `ext/` too: its workspace tables and
/// `Cargo.lock`, and one [`stub_member_manifest`] crate with an empty `src/lib.rs` per member
/// directory. Returns the stub's first-party package names (for [`prune_first_party`]).
pub fn make_stub_workspace(src_dir: &Path, stub_dir: &Path) -> io::Result<Vec<String>> {
    if fs::symlink_metadata(stub_dir).is_ok() && fs::read_dir(stub_dir)?.next().is_some() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} is not empty", stub_dir.display())));
    }
    fs::create_dir_all(stub_dir.join("src"))?;
    let manifest = parse_toml(&crate::fsx::read_nofollow(&src_dir.join("Cargo.toml"), MANIFEST_CAP, false)?)?;
    let stub = stub_manifest(&manifest)?;
    let mut names = vec![package_name(&stub)?];
    write_new(&stub_dir.join("Cargo.toml"), toml_text(&stub)?.as_bytes())?;
    write_new(&stub_dir.join("src/main.rs"), b"fn main() {}\n")?;
    let lock = crate::fsx::read_nofollow(&src_dir.join("Cargo.lock"), LOCK_CAP, false)?;
    write_new(&stub_dir.join("Cargo.lock"), &lock)?;
    match fs::symlink_metadata(src_dir.join("vendor")) {
        Ok(m) if m.is_dir() => crate::sys::clone_path(&src_dir.join("vendor"), &stub_dir.join("vendor"))?,
        Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, "vendor/ is not a directory")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir(stub_dir.join("vendor"))?,
        Err(e) => return Err(e),
    }
    let cfg = match fs::symlink_metadata(src_dir.join(".cargo")) {
        Ok(m) if m.is_dir() => match crate::fsx::read_nofollow(&src_dir.join(".cargo/config.toml"), MANIFEST_CAP, false) {
            Ok(b) => parse_toml(&b)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Table::new(),
            Err(e) => return Err(e),
        },
        Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, ".cargo is not a directory")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Table::new(),
        Err(e) => return Err(e),
    };
    fs::create_dir_all(stub_dir.join(".cargo"))?;
    write_new(&stub_dir.join(".cargo/config.toml"), toml_text(&stub_config(&cfg))?.as_bytes())?;
    let ext = src_dir.join("ext");
    if fs::symlink_metadata(&ext).map(|m| m.is_dir()).unwrap_or(false) && fs::symlink_metadata(ext.join("Cargo.toml")).map(|m| m.is_file()).unwrap_or(false) {
        make_ext_stub(&ext, &stub_dir.join("ext"), &mut names)?;
    }
    Ok(names)
}

/// The extension workspace's stub (see [`make_stub_workspace`]).
fn make_ext_stub(src: &Path, stub: &Path, names: &mut Vec<String>) -> io::Result<()> {
    let m = parse_toml(&crate::fsx::read_nofollow(&src.join("Cargo.toml"), MANIFEST_CAP, false)?)?;
    let ws: Table = m.iter().filter(|(k, _)| ["workspace", "profile", "patch", "replace"].contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
    fs::create_dir_all(stub)?;
    write_new(&stub.join("Cargo.toml"), toml_text(&ws)?.as_bytes())?;
    match crate::fsx::read_nofollow(&src.join("Cargo.lock"), LOCK_CAP, false) {
        Ok(b) => write_new(&stub.join("Cargo.lock"), &b)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut dirs: Vec<String> = Vec::new();
    for e in fs::read_dir(src)? {
        let e = e?;
        if e.file_type()?.is_dir() && fs::symlink_metadata(e.path().join("Cargo.toml")).map(|m| m.is_file()).unwrap_or(false) {
            dirs.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    dirs.sort();
    for d in dirs {
        let mm = parse_toml(&crate::fsx::read_nofollow(&src.join(&d).join("Cargo.toml"), MANIFEST_CAP, false)?)?;
        let s = stub_member_manifest(&mm)?;
        names.push(package_name(&s)?);
        fs::create_dir_all(stub.join(&d).join("src"))?;
        write_new(&stub.join(&d).join("Cargo.toml"), toml_text(&s)?.as_bytes())?;
        write_new(&stub.join(&d).join("src/lib.rs"), b"")?;
    }
    Ok(())
}

fn package_name(m: &Table) -> io::Result<String> {
    m.get("package").and_then(|p| p.get("name")).and_then(Value::as_str).map(str::to_string).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "[package] has no name"))
}

/// The stub `Cargo.toml` (see [`make_stub_workspace`]).
pub fn stub_manifest(m: &Table) -> io::Result<Table> {
    let mut out = stub_package(m)?;
    let bin = Table::from_iter([("name".to_string(), Value::from("init-body")), ("path".to_string(), Value::from("src/main.rs"))]);
    out.insert("bin".into(), Value::Array(vec![Value::Table(bin)]));
    Ok(out)
}

/// An extension member's stub `Cargo.toml`: as [`stub_manifest`], with one `[lib]` at
/// `src/lib.rs` (keeping only the member's `crate-type`) instead of the bin.
pub fn stub_member_manifest(m: &Table) -> io::Result<Table> {
    let mut out = stub_package(m)?;
    out.remove("workspace");
    let mut lib = Table::from_iter([("path".to_string(), Value::from("src/lib.rs"))]);
    if let Some(ct @ Value::Array(a)) = m.get("lib").and_then(|l| l.get("crate-type")) {
        if a.iter().all(|v| v.as_str().is_some_and(|s| ["cdylib", "rlib", "lib", "staticlib", "dylib"].contains(&s))) {
            lib.insert("crate-type".into(), ct.clone());
        }
    }
    out.insert("lib".into(), Value::Table(lib));
    Ok(out)
}

/// `[package]` cut to [`STUB_PACKAGE`] plus `build = false` and no auto targets, the
/// [`STUB_TABLES`] verbatim and the target-specific dependency tables.
fn stub_package(m: &Table) -> io::Result<Table> {
    let Some(Value::Table(pkg)) = m.get("package") else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Cargo.toml has no [package]"));
    };
    let mut p: Table = pkg.iter().filter(|(k, _)| STUB_PACKAGE.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
    p.insert("build".into(), Value::Boolean(false));
    for k in ["autobins", "autoexamples", "autotests", "autobenches"] {
        p.insert(k.into(), Value::Boolean(false));
    }
    let mut out = Table::new();
    out.insert("package".into(), Value::Table(p));
    for (k, v) in m {
        if STUB_TABLES.contains(&k.as_str()) {
            out.insert(k.clone(), v.clone());
        }
    }
    if let Some(Value::Table(t)) = m.get("target") {
        let deps = |x: &Value| -> Option<Value> {
            let Value::Table(x) = x else { return None };
            let kept: Table = x.iter().filter(|(k, _)| k.contains("dependencies")).map(|(k, v)| (k.clone(), v.clone())).collect();
            (!kept.is_empty()).then_some(Value::Table(kept))
        };
        let kept: Table = t.iter().filter_map(|(k, v)| Some((k.clone(), deps(v)?))).collect();
        if !kept.is_empty() {
            out.insert("target".into(), Value::Table(kept));
        }
    }
    Ok(out)
}

/// The stub `.cargo/config.toml`: forced vendored sources, `net.offline`, `[profile]` verbatim,
/// and `build.rustflags` / `target.<t>.rustflags` only when every flag is safe.
pub fn stub_config(c: &Table) -> Table {
    let mut out = Table::new();
    if let Some(Value::Table(b)) = c.get("build") {
        if let Some(rf) = b.get("rustflags").filter(|v| safe_rustflags(v)) {
            out.insert("build".into(), Value::Table(Table::from_iter([("rustflags".to_string(), rf.clone())])));
        }
    }
    if let Some(Value::Table(t)) = c.get("target") {
        let kept: Table = t
            .iter()
            .filter_map(|(k, v)| {
                let rf = v.as_table()?.get("rustflags").filter(|v| safe_rustflags(v))?;
                Some((k.clone(), Value::Table(Table::from_iter([("rustflags".to_string(), rf.clone())]))))
            })
            .collect();
        if !kept.is_empty() {
            out.insert("target".into(), Value::Table(kept));
        }
    }
    if let Some(p @ Value::Table(_)) = c.get("profile") {
        out.insert("profile".into(), p.clone());
    }
    let src = |k: &str, v: &str| Value::Table(Table::from_iter([(k.to_string(), Value::from(v))]));
    out.insert(
        "source".into(),
        Value::Table(Table::from_iter([
            ("crates-io".to_string(), src("replace-with", "vendored-sources")),
            ("vendored-sources".to_string(), src("directory", "vendor")),
        ])),
    );
    out.insert("net".into(), Value::Table(Table::from_iter([("offline".to_string(), Value::Boolean(true))])));
    out
}

/// True when every rustflag is `-C <safe key>[=v]`, `--cfg x` or a lint flag.
pub fn safe_rustflags(v: &Value) -> bool {
    let toks: Vec<&str> = match v {
        Value::String(s) => s.split_whitespace().collect(),
        Value::Array(a) => match a.iter().map(Value::as_str).collect::<Option<Vec<_>>>() {
            Some(t) => t,
            None => return false,
        },
        _ => return false,
    };
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        let (name, inline) = match t.split_once('=') {
            Some((n, v)) if t.starts_with("--") => (n, Some(v)),
            _ => (t, None),
        };
        let (flag, arg) = match name {
            "-C" | "--codegen" | "--cfg" | "-W" | "-A" | "-D" | "-F" | "--warn" | "--allow" | "--deny" | "--forbid" | "--cap-lints" => {
                let arg = match inline {
                    Some(a) => a,
                    None => {
                        i += 1;
                        match toks.get(i) {
                            Some(a) => *a,
                            None => return false,
                        }
                    }
                };
                (name, arg)
            }
            _ => match t.get(..2) {
                Some(f @ ("-C" | "-W" | "-A" | "-D" | "-F")) => (f, &t[2..]),
                _ => return false,
            },
        };
        if matches!(flag, "-C" | "--codegen") && !SAFE_CODEGEN.contains(&arg.split('=').next().unwrap_or("")) {
            return false;
        }
        i += 1;
    }
    true
}

fn parse_toml(b: &[u8]) -> io::Result<Table> {
    let s = std::str::from_utf8(b).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    toml::from_str::<Table>(s).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

fn toml_text(t: &Table) -> io::Result<String> {
    toml::to_string(t).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

/// Create a new file (never through a symlink) with `bytes`.
fn write_new(p: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut f = OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o644).open(p)?;
    f.write_all(bytes)
}

// ---- the build slot (§3.6.2) ----------------------------------------------------------------

/// Build timeouts (`build_timeout_cold_s` · `build_timeout_warm_s`, §4.9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildTimeouts {
    pub cold: Duration,
    pub warm: Duration,
}

impl Default for BuildTimeouts {
    fn default() -> Self {
        BuildTimeouts { cold: Duration::from_secs(3600), warm: Duration::from_secs(1800) }
    }
}

impl BuildTimeouts {
    /// `[supervise] build_timeout_cold_s` / `build_timeout_warm_s` from `kernel/config.toml`.
    pub fn from_cfg(c: &crate::config::SuperviseCfg) -> BuildTimeouts {
        BuildTimeouts { cold: Duration::from_secs(c.build_timeout_cold_s), warm: Duration::from_secs(c.build_timeout_warm_s) }
    }
}

/// How a build ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildOutcome {
    /// Exit 0 and `out/target/debug/init-body` is a regular file.
    pub ok: bool,
    pub warm: bool,
    pub exit: Option<ExitInfo>,
    pub timed_out: bool,
    /// G0's failure reason: `build_timeout`, `build_failed` or `spawn_failed`.
    pub reason: Option<&'static str>,
    pub message: String,
    pub wall_ms: u64,
}

/// The single build slot `cache/build/slot/{src,out}` (§3.1, §3.6.2). `home` must be canonical
/// (it becomes sandbox parameters).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildSlot {
    pub home: PathBuf,
    pub timeouts: BuildTimeouts,
    /// `build.log` cap (4 MiB).
    pub log_cap: u64,
}

impl BuildSlot {
    /// The slot with the default timeouts (3600 s / 1800 s). Every site that runs a build uses
    /// [`BuildSlot::configured`] instead, so `kernel/config.toml` applies.
    pub fn new(home: &Path) -> BuildSlot {
        BuildSlot { home: home.to_path_buf(), timeouts: BuildTimeouts::default(), log_cap: BUILD_LOG_CAP }
    }

    /// The slot with the configured build timeouts (install mode and G0).
    pub fn configured(home: &Path, c: &crate::config::SuperviseCfg) -> BuildSlot {
        BuildSlot { timeouts: BuildTimeouts::from_cfg(c), ..BuildSlot::new(home) }
    }

    /// `cache/build/slot`.
    pub fn dir(&self) -> PathBuf {
        self.home.join("cache/build/slot")
    }

    /// `slot/src`: the export (the build's `SRC` and cwd).
    pub fn src(&self) -> PathBuf {
        self.dir().join("src")
    }

    /// `slot/out`: the build's `OUT` (`target/ tmp/ home/ cargo-home/`).
    pub fn out(&self) -> PathBuf {
        self.dir().join("out")
    }

    /// `slot/out/target` (`CARGO_TARGET_DIR`).
    pub fn target(&self) -> PathBuf {
        self.out().join("target")
    }

    /// `slot/out/target/debug/init-body` (the dev profile's output).
    pub fn built_binary(&self) -> PathBuf {
        self.target().join("debug/init-body")
    }

    /// Delete `src/` and `out/` by exact path and recreate `src/` and
    /// `out/{target,tmp,home,cargo-home}` empty.
    pub fn reset(&self) -> io::Result<()> {
        remove_exact(&self.src())?;
        remove_exact(&self.out())?;
        fs::create_dir_all(self.src())?;
        for d in ["target", "tmp", "home", "cargo-home"] {
            fs::create_dir_all(self.out().join(d))?;
        }
        Ok(())
    }

    /// Export `sha` from lineage into `slot/src` (§3.6.2 "Export"; call [`BuildSlot::reset`]
    /// first).
    pub fn export(&self, git: &Git, sha: &str) -> Result<(), GitError> {
        git.export_tree(sha, &self.src())
    }

    /// The `build` instance for §3.6.2's build command: program `$TOOLCHAIN/bin/cargo`, args
    /// [`BUILD_ARGS`], exactly the env table (`SOURCE_DATE_EPOCH` = `commit_time`; the promote
    /// path passes [`SLOT_EPOCH_S`]), cwd `slot/src`, params `spawnspec::build_params`.
    pub fn build_request(&self, commit_time: i64, sp: &SandboxPaths) -> SpawnRequest {
        let s = |p: PathBuf| p.to_string_lossy().into_owned();
        let tool = sp.toolchain.join("bin");
        let mut env = BTreeMap::new();
        let path = if cfg!(target_os = "macos") {
            format!("{}:{}:/usr/bin:/bin", tool.display(), sp.devdir.join("usr/bin").display())
        } else {
            format!("{}:/usr/bin:/bin", tool.display())
        };
        env.insert("PATH".to_string(), path);
        env.insert("HOME".into(), s(self.out().join("home")));
        env.insert("TMPDIR".into(), format!("{}/", self.out().join("tmp").display()));
        env.insert("CARGO_HOME".into(), s(self.out().join("cargo-home")));
        env.insert("CARGO_TARGET_DIR".into(), s(self.target()));
        env.insert("CARGO_NET_OFFLINE".into(), "true".into());
        env.insert("CARGO_BUILD_JOBS".into(), SANDBOX_CARGO_JOBS.into());
        env.insert("CARGO_INCREMENTAL".into(), "0".into());
        env.insert("SOURCE_DATE_EPOCH".into(), commit_time.to_string());
        if cfg!(target_os = "macos") {
            if let Some(sdk) = &sp.sdkroot {
                env.insert("SDKROOT".into(), s(sdk.clone()));
            }
            if !sp.devdir.as_os_str().is_empty() {
                env.insert("DEVELOPER_DIR".into(), s(sp.devdir.clone()));
            }
        }
        SpawnRequest {
            kind: InstanceKind::Build,
            profile: "build".into(),
            params: build_params(&self.home, &self.src(), &self.out(), sp),
            program: tool.join("cargo"),
            args: BUILD_ARGS.iter().map(|a| a.to_string()).collect(),
            env,
            cwd: self.src(),
            lane: None,
            replaces: None,
        }
    }

    /// Run the build to completion: output (stdout+stderr) to `log_path` (truncated, capped at
    /// `log_cap`), killed at `timeouts.cold` / `timeouts.warm` (`reason: "build_timeout"`).
    pub fn run_build(&self, spawner: &dyn Spawner, req: &SpawnRequest, warm: bool, log_path: &Path) -> BuildOutcome {
        let timeout = if warm { self.timeouts.warm } else { self.timeouts.cold };
        let t0 = Instant::now();
        let fail = |reason, message: String, exit, timed_out| BuildOutcome {
            ok: false,
            warm,
            exit,
            timed_out,
            reason: Some(reason),
            message,
            wall_ms: t0.elapsed().as_millis() as u64,
        };
        if let Err(e) = log_path.parent().map_or(Ok(()), fs::create_dir_all).and_then(|()| fs::File::create(log_path).map(drop)) {
            return fail("spawn_failed", format!("build log {}: {e}", log_path.display()), None, false);
        }
        match run_to_completion(spawner, req, timeout, Some(log_path), self.log_cap) {
            Err(e) => fail("spawn_failed", e.to_string(), None, false),
            Ok((exit, true)) => fail("build_timeout", format!("no result within {} s", timeout.as_secs()), Some(exit), true),
            Ok((exit, false)) if !exit.success() => fail("build_failed", format!("cargo exited {:?} signal {:?}", exit.code, exit.signal), Some(exit), false),
            Ok((exit, false)) => match fs::symlink_metadata(self.built_binary()) {
                Ok(m) if m.is_file() => BuildOutcome {
                    ok: true,
                    warm,
                    exit: Some(exit),
                    timed_out: false,
                    reason: None,
                    message: String::new(),
                    wall_ms: t0.elapsed().as_millis() as u64,
                },
                _ => fail("build_failed", "no regular file at target/debug/init-body".into(), Some(exit), false),
            },
        }
    }

    /// Staging the binary (§3.6.2): copy `out/target/debug/init-body` (opened `O_NOFOLLOW`,
    /// a regular file) to `bin/.init-body.staged.tmp` as a new inode, `sync_all`, rename to
    /// `bin/init-body.staged`, fsync `bin/`. Returns its SHA-256. The binary is never modified
    /// in place (macOS code-signing rule).
    pub fn stage_binary(&self) -> io::Result<String> {
        let bin = self.home.join("bin");
        fs::create_dir_all(&bin)?;
        let mut src = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(self.built_binary())?;
        if !src.metadata()?.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "built init-body is not a regular file"));
        }
        let tmp = bin.join(".init-body.staged.tmp");
        remove_exact(&tmp)?;
        let res = (|| {
            let mut dst = OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o755).open(&tmp)?;
            io::copy(&mut src, &mut dst)?;
            dst.set_permissions(fs::Permissions::from_mode(0o755))?;
            dst.sync_all()?;
            drop(dst);
            let sha = crate::fsx::sha256_file(&tmp)?;
            fs::rename(&tmp, bin.join("init-body.staged"))?;
            crate::fsx::sync_dir(&bin)?;
            Ok(sha)
        })();
        if res.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        res
    }
}

/// Free space check before a build (§3.7 "Disk"): `E_DISK` when the `INIT_HOME` filesystem has
/// fewer than `min_free_bytes` available. Returns the free bytes.
pub fn disk_ok_for_build(home: &Path, min_free_bytes: u64) -> Result<u64, CodedError> {
    let free = crate::sys::free_bytes(home).map_err(|e| CodedError::new("E_INTERNAL", format!("statvfs {}: {e}", home.display())))?;
    if free < min_free_bytes {
        return Err(CodedError::new("E_DISK", format!("{free} bytes free, builds need {min_free_bytes}")));
    }
    Ok(free)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testfx::Fx;
    use crate::instance::LocalSpawner;

    fn sp() -> SandboxPaths {
        SandboxPaths { toolchain: "/tc".into(), devdir: "/dev/dir".into(), home_real: "/Users/x".into(), sdkroot: Some("/sdk".into()) }
    }

    const VV: &str = "rustc 1.97.1\nhost: aarch64-apple-darwin\n";
    const HOST: &str = "aarch64-apple-darwin";

    #[test]
    fn deps_key_is_stable_and_sensitive() {
        let fx = Fx::new("build-key");
        fx.write("Cargo.toml", b"[package]\nname = \"init-body\"\n");
        fx.write("Cargo.lock", b"version = 4\n");
        fx.write("vendor/a/lib.rs", b"//a\n");
        fx.write("src/main.rs", b"fn main() {}\n");
        let base = fx.commit("base");
        fx.write("src/main.rs", b"fn main() { println!(); }\n");
        let src_only = fx.commit("first-party change");
        let mut variants = Vec::new();
        let mut change = |f: &dyn Fn(&Fx)| {
            fx.ag(&["switch", "-q", "-c", &format!("v{}", variants.len()), &base]);
            f(&fx);
            variants.push(fx.commit("variant"));
        };
        change(&|fx| fx.write("Cargo.lock", b"version = 4\n# x\n"));
        change(&|fx| fx.write("vendor/a/lib.rs", b"//b\n"));
        change(&|fx| fx.write("Cargo.toml", b"[package]\nname = \"init-body\"\n# x\n"));
        change(&|fx| fx.write(".cargo/config.toml", b"[net]\noffline = true\n"));
        change(&|fx| fx.write("ext/Cargo.lock", b"version = 4\n"));
        change(&|fx| fx.write("ext/Cargo.toml", b"[workspace]\n"));
        fx.sync();
        let g = &fx.git;
        let k = deps_key(g, &base, VV, HOST).unwrap();
        assert!(is_key(&k), "{k}");
        assert_eq!(deps_key(g, &base, VV, HOST).unwrap(), k, "stable");
        assert_eq!(deps_key(g, &src_only, VV, HOST).unwrap(), k, "first-party code is not in the key");
        let mut seen = std::collections::BTreeSet::from([k.clone()]);
        for v in &variants {
            assert!(seen.insert(deps_key(g, v, VV, HOST).unwrap()), "variant {v} must change the key");
        }
        assert!(seen.insert(deps_key(g, &base, "rustc 1.98.0\n", HOST).unwrap()));
        assert!(seen.insert(deps_key(g, &base, VV, "x86_64-apple-darwin").unwrap()));
        fx.ag(&["switch", "-q", "-c", "nolock", &base]);
        fx.ag(&["rm", "-q", "Cargo.lock"]);
        let nolock = fx.commit("no lock");
        fx.sync();
        assert!(deps_key(g, &nolock, VV, HOST).is_err());
    }

    #[test]
    fn stub_manifest_drops_targets_and_keeps_resolution() {
        let m: Table = toml::from_str(
            r#"
            cargo-features = ["x"]
            [package]
            name = "init-body"
            version = "0.1.0"
            edition = "2024"
            build = "build.rs"
            links = "z"
            default-run = "init-body"
            [lib]
            path = "src/lib.rs"
            [[bin]]
            name = "init-body"
            path = "src/main.rs"
            [[bin]]
            name = "tool"
            path = "src/tool.rs"
            [[test]]
            name = "it"
            [dependencies]
            serde = { version = "=1.0.229", features = ["derive"] }
            [dev-dependencies]
            tempfile = "3"
            [build-dependencies]
            cc = "1"
            [target.'cfg(unix)'.dependencies]
            libc = "0.2"
            [features]
            fast = []
            [profile.dev.package."*"]
            opt-level = 2
            [workspace]
            exclude = ["ext"]
            [patch.crates-io]
            serde = { path = "vendor/serde" }
            [badges]
            maintenance = { status = "none" }
        "#,
        )
        .unwrap();
        let s = stub_manifest(&m).unwrap();
        let keys: Vec<&str> = s.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["bin", "build-dependencies", "dependencies", "dev-dependencies", "features", "package", "patch", "profile", "target", "workspace"]
        );
        let p = s["package"].as_table().unwrap();
        assert_eq!(p["build"], Value::Boolean(false));
        assert_eq!(p["name"].as_str(), Some("init-body"));
        assert!(!p.contains_key("links") && !p.contains_key("default-run"));
        assert_eq!(p["autobins"], Value::Boolean(false));
        let bins = s["bin"].as_array().unwrap();
        assert_eq!(bins.len(), 1);
        assert_eq!(bins[0]["path"].as_str(), Some("src/main.rs"));
        assert_eq!(s["dependencies"], m["dependencies"]);
        assert_eq!(s["profile"], m["profile"]);
        assert_eq!(s["target"]["cfg(unix)"]["dependencies"], m["target"]["cfg(unix)"]["dependencies"]);
        let virt: Table = toml::from_str("[workspace]\nmembers = []\n").unwrap();
        assert!(stub_manifest(&virt).is_err(), "a virtual manifest has no package");
    }

    #[test]
    fn stub_config_keeps_only_safe_keys() {
        let c: Table = toml::from_str(
            r#"
            [build]
            rustflags = ["-C", "target-cpu=native", "--cfg", "tokio_unstable", "-Dwarnings"]
            rustc-wrapper = "./wrap.sh"
            target-dir = "/elsewhere"
            [target.aarch64-apple-darwin]
            linker = "./ld.sh"
            rustflags = "-C link-arg=-fuse-ld=./evil"
            [target.x86_64-apple-darwin]
            rustflags = "-Copt-level=3"
            [env]
            CC = "./cc.sh"
            [alias]
            b = "build"
            [source.crates-io]
            replace-with = "mine"
            [source.mine]
            directory = "../../elsewhere"
            [profile.dev]
            debug = 1
            [net]
            git-fetch-with-cli = true
        "#,
        )
        .unwrap();
        let s = stub_config(&c);
        let keys: Vec<&str> = s.keys().map(String::as_str).collect();
        assert_eq!(keys, ["build", "net", "profile", "source", "target"]);
        assert_eq!(s["build"].as_table().unwrap().len(), 1);
        assert_eq!(s["build"]["rustflags"], c["build"]["rustflags"]);
        let t = s["target"].as_table().unwrap();
        assert_eq!(t.keys().collect::<Vec<_>>(), ["x86_64-apple-darwin"], "link-arg flags are refused");
        assert_eq!(s["source"]["vendored-sources"]["directory"].as_str(), Some("vendor"));
        assert_eq!(s["source"]["crates-io"]["replace-with"].as_str(), Some("vendored-sources"));
        assert_eq!(s["net"].as_table().unwrap().len(), 1);
        for bad in ["-C linker=./x", "-Clink-args=a", "-Z unstable-options", "--extern x=y", "-L ./lib", "-C", "-Cllvm-args=-load=./x.so"] {
            assert!(!safe_rustflags(&Value::from(bad)), "{bad}");
        }
        for ok in ["", "-C opt-level=3", "--cfg=foo -W unused --cap-lints warn", "-Ctarget-feature=+crt-static"] {
            assert!(safe_rustflags(&Value::from(ok)), "{ok}");
        }
    }

    #[test]
    fn make_stub_workspace_writes_the_stub() {
        let d = crate::testutil::test_dir("build-stub");
        let src = d.join("src");
        fs::create_dir_all(src.join("vendor/a")).unwrap();
        fs::create_dir_all(src.join(".cargo")).unwrap();
        fs::create_dir_all(src.join("src")).unwrap();
        fs::write(src.join("Cargo.toml"), "[package]\nname = \"init-body\"\nversion = \"0.1.0\"\nbuild = \"build.rs\"\n[dependencies]\n").unwrap();
        fs::write(src.join("Cargo.lock"), "version = 4\n").unwrap();
        fs::write(src.join("vendor/a/lib.rs"), "//\n").unwrap();
        fs::write(src.join(".cargo/config.toml"), "[build]\nrustc-wrapper = \"./w\"\n").unwrap();
        fs::write(src.join("build.rs"), "fn main() { /* first-party */ }\n").unwrap();
        fs::write(src.join("src/lib.rs"), "pub fn x() {}\n").unwrap();
        let stub = d.join("stub");
        make_stub_workspace(&src, &stub).unwrap();
        let mut files: Vec<String> = Vec::new();
        let mut stack = vec![stub.clone()];
        while let Some(p) = stack.pop() {
            for e in fs::read_dir(&p).unwrap() {
                let e = e.unwrap();
                if e.file_type().unwrap().is_dir() {
                    stack.push(e.path());
                } else {
                    files.push(e.path().strip_prefix(&stub).unwrap().to_string_lossy().into_owned());
                }
            }
        }
        files.sort();
        assert_eq!(files, [".cargo/config.toml", "Cargo.lock", "Cargo.toml", "src/main.rs", "vendor/a/lib.rs"]);
        let m: Table = toml::from_str(&fs::read_to_string(stub.join("Cargo.toml")).unwrap()).unwrap();
        assert_eq!(m["package"]["build"], Value::Boolean(false));
        assert!(!fs::read_to_string(stub.join(".cargo/config.toml")).unwrap().contains("wrapper"));
        assert_eq!(fs::read(stub.join("Cargo.lock")).unwrap(), b"version = 4\n");
        assert!(make_stub_workspace(&src, &stub).is_err(), "stub dir must be new");
        // A symlinked manifest is refused (K-F2).
        let src2 = d.join("src2");
        fs::create_dir_all(&src2).unwrap();
        std::os::unix::fs::symlink(src.join("Cargo.toml"), src2.join("Cargo.toml")).unwrap();
        assert!(make_stub_workspace(&src2, &d.join("stub2")).is_err());
        crate::testutil::remove_test_dir(&d);
    }

    #[test]
    fn deps_cache_commits_once_clones_and_evicts_lru() {
        let d = crate::testutil::test_dir("build-deps");
        let home = d.join("home");
        let built = d.join("built");
        fs::create_dir_all(built.join("debug/deps")).unwrap();
        fs::write(built.join("debug/deps/liba.rlib"), b"rlib").unwrap();
        let c = DepsCache::new(&home);
        let keys = ["0000000000000001", "0000000000000002", "0000000000000003", "0000000000000004"];
        assert_eq!(c.lookup(keys[0]), None);
        assert!(c.commit("../escape", &built).is_err());
        for k in keys {
            assert!(c.commit(k, &built).unwrap());
            std::thread::sleep(Duration::from_millis(3));
        }
        assert!(!c.commit(keys[0], &built).unwrap(), "a completed key is never written again");
        assert!(!c.dir.join(format!("{}.tmp", keys[0])).exists());
        // Use key 1 so key 2 is the least recently used.
        let slot_target = d.join("slot/out/target");
        fs::create_dir_all(&slot_target).unwrap();
        c.clone_into(keys[0], &slot_target).unwrap();
        assert_eq!(fs::read(slot_target.join("debug/deps/liba.rlib")).unwrap(), b"rlib");
        assert!(c.clone_into(keys[0], &slot_target).is_err(), "a non-empty target is not replaced");
        assert!(c.clone_into("00000000000000ff", &d.join("x")).is_err());
        // A stray entry that is not a key is never touched.
        fs::create_dir_all(c.dir.join("keepme")).unwrap();
        let gone = c.evict_lru(DEPS_KEYS).unwrap();
        assert_eq!(gone, [keys[1]]);
        assert!(!c.dir.join(keys[1]).exists());
        for k in [keys[0], keys[2], keys[3]] {
            assert!(c.lookup(k).is_some(), "{k} kept");
        }
        assert!(c.dir.join("keepme").is_dir());
        let mut gone = c.evict_lru(1).unwrap();
        gone.sort();
        assert_eq!(gone, [keys[2], keys[3]]);
        assert!(c.lookup(keys[0]).is_some(), "the most recently used key stays");
        crate::testutil::remove_test_dir(&d);
    }

    /// A staged entry that changes before publishing is refused and deleted; a complete entry
    /// that changes (content, a removed file, an added file, a lost manifest) fails `verify`.
    #[test]
    fn deps_cache_rejects_tampered_and_corrupt_entries() {
        let d = crate::testutil::test_dir("build-deps-verify");
        let built = d.join("built");
        fs::create_dir_all(built.join("debug/deps")).unwrap();
        fs::write(built.join("debug/deps/liba-1.rlib"), b"rlib a").unwrap();
        fs::write(built.join("debug/deps/libb-2.rlib"), b"rlib b").unwrap();
        let c = DepsCache::new(&d.join("home"));
        let k = "00000000000000aa";
        // Tampered between stage and publish (a candidate writing into <key>.tmp while it ran).
        let digest = c.stage(k, &built).unwrap().expect("staged");
        assert_eq!(digest, tree_digest(&built).unwrap(), "the digest is the stub's target");
        fs::write(c.dir.join(format!("{k}.tmp/target/debug/deps/liba-1.rlib")), b"evil").unwrap();
        assert!(c.publish(k, &digest).is_err());
        assert!(!c.dir.join(format!("{k}.tmp")).exists() && c.lookup(k).is_none(), "refused and deleted");
        // Discarded after a failed candidate.
        c.stage(k, &built).unwrap().unwrap();
        c.discard(k).unwrap();
        assert!(!c.dir.join(format!("{k}.tmp")).exists());
        // A leftover <key>.tmp of an interrupted build is cleared by the next stage.
        fs::create_dir_all(c.dir.join("00000000000000bb.tmp/target")).unwrap();
        let digest = c.stage(k, &built).unwrap().unwrap();
        assert!(!c.dir.join("00000000000000bb.tmp").exists());
        assert!(c.publish(k, &digest).unwrap());
        assert!(c.verify(k));
        assert_eq!(c.stage(k, &built).unwrap(), None, "a complete key is never written again");
        let entry = c.lookup(k).unwrap();
        let corrupt: [&dyn Fn(); 4] = [
            &|| fs::write(entry.join("debug/deps/libb-2.rlib"), b"rlib B").unwrap(),
            &|| fs::remove_file(entry.join("debug/deps/libb-2.rlib")).unwrap(),
            &|| fs::write(entry.join("debug/deps/libc-3.rlib"), b"planted").unwrap(),
            &|| fs::remove_file(c.dir.join(k).join(DEPS_MANIFEST)).unwrap(),
        ];
        for (i, f) in corrupt.iter().enumerate() {
            c.remove(k).unwrap();
            assert!(c.commit(k, &built).unwrap());
            assert!(c.verify(k), "fresh entry {i}");
            f();
            assert!(!c.verify(k), "corruption {i} detected");
        }
        c.remove(k).unwrap();
        assert!(c.lookup(k).is_none());
        crate::testutil::remove_test_dir(&d);
    }

    #[test]
    fn prune_first_party_removes_only_the_named_packages() {
        let d = crate::testutil::test_dir("build-prune");
        let t = d.join("target");
        let files = [
            "debug/.fingerprint/init-body-0123/bin-init-body",
            "debug/.fingerprint/serde-4567/lib-serde",
            "debug/.fingerprint/count-lines-89ab/lib-count_lines",
            "debug/deps/init_body-0123",
            "debug/deps/init_body-0123.d",
            "debug/deps/libserde-4567.rlib",
            "debug/deps/libcount_lines-89ab.rlib",
            "debug/build/init-body-cdef/out/x",
            "debug/build/serde-cdef/out/x",
            "debug/init-body",
            "debug/init-body.d",
            "debug/init-bodyx",
            "wasm32-wasip2/debug/.fingerprint/count-lines-1111/lib-count_lines",
            "wasm32-wasip2/debug/deps/count_lines-1111.wasm",
            "wasm32-wasip2/debug/deps/libwit_bindgen-2222.rlib",
            "wasm32-wasip2/debug/count_lines.wasm",
            "wasm32-wasip2/debug/libcount_lines.d",
        ];
        for f in files {
            fs::create_dir_all(t.join(f).parent().unwrap()).unwrap();
            fs::write(t.join(f), b"x").unwrap();
        }
        let gone = prune_first_party(&t, &["init-body".to_string(), "count-lines".to_string()]).unwrap();
        let left: Vec<&str> = files.iter().copied().filter(|f| t.join(f).exists()).collect();
        assert_eq!(
            left,
            [
                "debug/.fingerprint/serde-4567/lib-serde",
                "debug/deps/libserde-4567.rlib",
                "debug/build/serde-cdef/out/x",
                "debug/init-bodyx",
                "wasm32-wasip2/debug/deps/libwit_bindgen-2222.rlib",
            ]
        );
        assert_eq!(gone, 12);
        crate::testutil::remove_test_dir(&d);
    }

    #[test]
    fn pin_vendor_mtimes_sets_one_old_time() {
        let d = crate::testutil::test_dir("build-pin");
        assert_eq!(pin_vendor_mtimes(&d.join("nothing")).unwrap(), 0);
        fs::create_dir_all(d.join("vendor/a/src")).unwrap();
        fs::write(d.join("vendor/a/build.rs"), b"fn main() {}").unwrap();
        fs::write(d.join("vendor/a/src/lib.rs"), b"").unwrap();
        fs::write(d.join("main.rs"), b"").unwrap();
        std::os::unix::fs::symlink("/nonexistent", d.join("vendor/a/dangling")).unwrap();
        assert_eq!(pin_vendor_mtimes(&d).unwrap(), 5, "3 dirs and 2 files; the symlink is skipped");
        let want = std::time::UNIX_EPOCH + Duration::from_secs(VENDOR_MTIME_S);
        for p in ["vendor", "vendor/a", "vendor/a/src", "vendor/a/build.rs", "vendor/a/src/lib.rs"] {
            assert_eq!(fs::metadata(d.join(p)).unwrap().modified().unwrap(), want, "{p}");
        }
        assert_ne!(fs::metadata(d.join("main.rs")).unwrap().modified().unwrap(), want, "first-party files keep theirs");
        crate::testutil::remove_test_dir(&d);
    }

    #[test]
    fn make_stub_workspace_stubs_the_ext_members() {
        let d = crate::testutil::test_dir("build-stub-ext");
        let src = d.join("src");
        for (p, t) in [
            ("Cargo.toml", "[package]\nname = \"init-body\"\nversion = \"0.1.0\"\n[workspace]\nexclude = [\"ext\"]\n"),
            ("Cargo.lock", "version = 4\n"),
            ("ext/Cargo.toml", "[workspace]\nresolver = \"3\"\nmembers = [\"*\"]\n[workspace.dependencies]\nserde = \"1\"\n[profile.dev]\nopt-level = \"s\"\n[alias]\nx = \"y\"\n"),
            ("ext/Cargo.lock", "version = 4\n# ext\n"),
            ("ext/count-lines/Cargo.toml", "[package]\nname = \"count-lines\"\nversion = \"0.1.0\"\nedition.workspace = true\nbuild = \"build.rs\"\n[lib]\ncrate-type = [\"cdylib\"]\npath = \"src/other.rs\"\n[dependencies]\nserde.workspace = true\n"),
            ("ext/count-lines/build.rs", "fn main() {}\n"),
            ("ext/count-lines/src/lib.rs", "pub fn first_party() {}\n"),
            ("ext/notes/README", "not a crate\n"),
        ] {
            fs::create_dir_all(src.join(p).parent().unwrap()).unwrap();
            fs::write(src.join(p), t).unwrap();
        }
        let stub = d.join("stub");
        assert_eq!(make_stub_workspace(&src, &stub).unwrap(), ["init-body", "count-lines"]);
        let ws: Table = toml::from_str(&fs::read_to_string(stub.join("ext/Cargo.toml")).unwrap()).unwrap();
        assert_eq!(ws.keys().collect::<Vec<_>>(), ["profile", "workspace"], "no [alias]");
        assert_eq!(fs::read(stub.join("ext/Cargo.lock")).unwrap(), b"version = 4\n# ext\n");
        let m: Table = toml::from_str(&fs::read_to_string(stub.join("ext/count-lines/Cargo.toml")).unwrap()).unwrap();
        assert_eq!(m["package"]["build"], Value::Boolean(false));
        assert_eq!(m["lib"]["path"].as_str(), Some("src/lib.rs"));
        assert_eq!(m["lib"]["crate-type"], Value::Array(vec![Value::from("cdylib")]));
        assert_eq!(m["dependencies"]["serde"]["workspace"], Value::Boolean(true));
        assert!(!m.contains_key("bin") && !m.contains_key("workspace"));
        assert_eq!(fs::read(stub.join("ext/count-lines/src/lib.rs")).unwrap(), b"");
        assert!(!stub.join("ext/count-lines/build.rs").exists() && !stub.join("ext/notes").exists());
        crate::testutil::remove_test_dir(&d);
    }

    #[test]
    fn build_request_is_exactly_the_spec_command() {
        let slot = BuildSlot::new(Path::new("/h"));
        let r = slot.build_request(1_700_000_000, &sp());
        assert_eq!(r.kind, InstanceKind::Build);
        assert_eq!(r.profile, "build");
        assert_eq!(r.program, Path::new("/tc/bin/cargo"));
        assert_eq!(r.args, BUILD_ARGS);
        assert_eq!(r.cwd, Path::new("/h/cache/build/slot/src"));
        assert_eq!(r.params, build_params(Path::new("/h"), Path::new("/h/cache/build/slot/src"), Path::new("/h/cache/build/slot/out"), &sp()));
        let mut want = BTreeMap::from([
            ("HOME", "/h/cache/build/slot/out/home"),
            ("TMPDIR", "/h/cache/build/slot/out/tmp/"),
            ("CARGO_HOME", "/h/cache/build/slot/out/cargo-home"),
            ("CARGO_TARGET_DIR", "/h/cache/build/slot/out/target"),
            ("CARGO_NET_OFFLINE", "true"),
            ("CARGO_BUILD_JOBS", "2"),
            ("CARGO_INCREMENTAL", "0"),
            ("SOURCE_DATE_EPOCH", "1700000000"),
        ]);
        if cfg!(target_os = "macos") {
            want.insert("PATH", "/tc/bin:/dev/dir/usr/bin:/usr/bin:/bin");
            want.insert("SDKROOT", "/sdk");
            want.insert("DEVELOPER_DIR", "/dev/dir");
        } else {
            want.insert("PATH", "/tc/bin:/usr/bin:/bin");
        }
        let want: BTreeMap<String, String> = want.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        assert_eq!(r.env, want);
        let no_sdk = SandboxPaths { sdkroot: None, ..sp() };
        assert!(!slot.build_request(0, &no_sdk).env.contains_key("SDKROOT"));
    }

    #[test]
    fn build_requests_do_not_inherit_host_job_count() {
        // Set the hostile outer value only in a fresh test process; never
        // mutate this multithreaded test runner's environment.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "build::tests::build_request_is_exactly_the_spec_command", "--nocapture"])
            .env("CARGO_BUILD_JOBS", "128").output().unwrap();
        assert!(output.status.success(), "child environment qualification failed: {}", String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed; 0 failed;"), "the exact request test must actually execute");
    }

    #[test]
    fn disk_check_reports_e_disk() {
        assert!(disk_ok_for_build(Path::new("/"), 1).unwrap() > 0);
        assert_eq!(disk_ok_for_build(Path::new("/"), u64::MAX).unwrap_err().code, "E_DISK");
    }

    /// `$TOOLCHAIN` for the real build test: the toolchain running this test.
    fn toolchain() -> Option<PathBuf> {
        let cargo = std::env::var_os("CARGO").map(PathBuf::from)?;
        let tc = cargo.parent()?.parent()?.to_path_buf();
        tc.join("bin/rustc").is_file().then_some(tc)
    }

    #[test]
    fn real_offline_build_stages_the_binary() {
        let Some(tc) = toolchain() else {
            eprintln!("note: no toolchain found through $CARGO; real build test skipped");
            return;
        };
        let fx = Fx::new("build-real");
        fx.write(
            "Cargo.toml",
            b"[package]\nname = \"init-body\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"init-body\"\npath = \"src/main.rs\"\n\n[workspace]\n",
        );
        fx.write("Cargo.lock", b"# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"init-body\"\nversion = \"0.1.0\"\n");
        fx.write("vendor/.keep", b"");
        fx.write("src/main.rs", b"fn main() { println!(\"tiny body\"); }\n");
        let sha = fx.commit("tiny body");
        fx.set_main(&sha);
        let home = fx.home.clone();
        let sdk = std::process::Command::new("/usr/bin/xcrun").arg("--show-sdk-path").output().ok().filter(|o| o.status.success());
        let sp = SandboxPaths {
            toolchain: tc,
            devdir: if cfg!(target_os = "macos") { "/Library/Developer/CommandLineTools".into() } else { PathBuf::new() },
            home_real: "/nonexistent".into(),
            sdkroot: sdk.map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim())),
        };
        let mut slot = BuildSlot::new(&home);
        slot.timeouts = BuildTimeouts { cold: Duration::from_secs(60), warm: Duration::from_secs(60) };
        slot.reset().unwrap();
        slot.export(&fx.git, &sha).unwrap();
        let req = slot.build_request(fx.git.commit_time(&sha).unwrap(), &sp);
        let spawner = LocalSpawner::new();
        let log = home.join("state/kernel/gate/j-1/build.log");
        let out = slot.run_build(&spawner, &req, false, &log);
        assert!(out.ok, "{out:?}\n{}", fs::read_to_string(&log).unwrap_or_default());
        assert!(out.wall_ms < 60_000);
        let sha256 = slot.stage_binary().unwrap();
        let staged = home.join("bin/init-body.staged");
        assert_eq!(crate::fsx::sha256_file(&staged).unwrap(), sha256);
        assert!(!home.join("bin/.init-body.staged.tmp").exists());
        assert_eq!(fs::metadata(&staged).unwrap().permissions().mode() & 0o777, 0o755);
        use std::os::unix::fs::MetadataExt;
        assert_ne!(fs::metadata(&staged).unwrap().ino(), fs::metadata(slot.built_binary()).unwrap().ino(), "a new inode");
        let run = std::process::Command::new(&staged).env_clear().output().unwrap();
        assert_eq!(String::from_utf8_lossy(&run.stdout), "tiny body\n");
        // Populate the cache from this target, then a warm build from a clone of it.
        let cache = DepsCache::new(&home);
        let key = deps_key(&fx.git, &sha, "rustc -vV", "host").unwrap();
        assert!(cache.commit(&key, &slot.target()).unwrap());
        slot.reset().unwrap();
        slot.export(&fx.git, &sha).unwrap();
        cache.clone_into(&key, &slot.target()).unwrap();
        let warm = slot.run_build(&spawner, &req, true, &log);
        assert!(warm.ok && warm.warm, "{warm:?}");
        // A build that cannot finish in time is killed and reported as build_timeout.
        slot.timeouts.warm = Duration::from_millis(1);
        let mut slow = req.clone();
        slow.program = "/bin/sleep".into();
        slow.args = vec!["30".into()];
        let to = slot.run_build(&spawner, &slow, true, &log);
        assert_eq!((to.ok, to.timed_out, to.reason), (false, true, Some("build_timeout")));
    }

    /// The configured `build_timeout_cold_s` / `_warm_s` reach the slot and bound the build
    /// (a configured 3600 s used to be ignored for the 900 s default).
    #[test]
    fn configured_build_timeouts_are_honoured() {
        let cfg = crate::config::parse("[supervise]\nbuild_timeout_cold_s = 3600\nbuild_timeout_warm_s = 7\n").unwrap();
        let d = crate::testutil::test_dir("build-cfg-timeout");
        let slot = BuildSlot::configured(&d, &cfg.supervise);
        assert_eq!(slot.timeouts, BuildTimeouts { cold: Duration::from_secs(3600), warm: Duration::from_secs(7) });
        assert_eq!(BuildSlot::configured(&d, &crate::config::defaults().supervise).timeouts, BuildTimeouts::default());
        // A configured 1 s cold timeout kills a build that would take 30 s.
        let cfg = crate::config::parse("[supervise]\nbuild_timeout_cold_s = 1\n").unwrap();
        let slot = BuildSlot::configured(&d, &cfg.supervise);
        slot.reset().unwrap();
        let mut req = slot.build_request(0, &sp());
        req.program = "/bin/sleep".into();
        req.args = vec!["30".into()];
        let t = Instant::now();
        let out = slot.run_build(&LocalSpawner::new(), &req, false, &d.join("state/kernel/gate/j/build.log"));
        assert_eq!((out.ok, out.timed_out, out.reason), (false, true, Some("build_timeout")), "{out:?}");
        assert_eq!(out.message, "no result within 1 s");
        assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
        crate::testutil::remove_test_dir(&d);
    }

    #[test]
    fn failed_build_and_missing_binary_are_build_failed() {
        let d = crate::testutil::test_dir("build-fail");
        let slot = BuildSlot::new(&d);
        slot.reset().unwrap();
        let mut req = slot.build_request(0, &sp());
        req.program = "/bin/sh".into();
        req.args = vec!["-c".into(), "echo boom >&2; exit 101".into()];
        let log = d.join("state/kernel/gate/j/build.log");
        let out = slot.run_build(&LocalSpawner::new(), &req, false, &log);
        assert_eq!((out.ok, out.reason, out.exit.as_ref().and_then(|e| e.code)), (false, Some("build_failed"), Some(101)));
        assert!(fs::read_to_string(&log).unwrap().contains("boom"));
        req.args = vec!["-c".into(), "exit 0".into()];
        let out = slot.run_build(&LocalSpawner::new(), &req, false, &log);
        assert_eq!(out.reason, Some("build_failed"), "exit 0 without a binary");
        assert!(slot.stage_binary().is_err());
        req.program = "/nonexistent/cargo".into();
        assert_eq!(slot.run_build(&LocalSpawner::new(), &req, false, &log).reason, Some("spawn_failed"));
        crate::testutil::remove_test_dir(&d);
    }
}
