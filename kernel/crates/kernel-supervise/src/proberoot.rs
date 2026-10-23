//! The probe root's filesystem half (supervise.md §3.6.3 steps 1 and 5; D8): creating
//! `probe/<id>/` before the probe starts, copying `keep_on_stop` entries and the agent-visible
//! outputs after it ended, reading its last `boot` event (K-F2), and deleting the root by exact
//! path. Spawning, waiting and stopping are the caller's ([`crate::instance::Spawner`]).
//!
//! The root holds real directories and copy-on-write clones only: no symlink to `kernel/`,
//! `lineage/` or `bin/` (D8), so every sandbox parameter is canonical.

use crate::git::Git;
use crate::instance::ExitInfo;
use crate::spawnspec::{BodyLane, SandboxPaths, body_params};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Agent-visible copies of stdout and stderr keep their last 1 MiB (§3.6.3 step 5).
pub const OUTPUT_TAIL: usize = 1 << 20;
/// The session log is read from its last 1 MiB (§3.6.3 step 5).
const SESSION_TAIL: usize = 1 << 20;
/// `ready` and `state/session` are small.
const SMALL_CAP: usize = 64 << 10;

/// Where the probe's session comes from (§3.6.3 `ProbeOpts.session`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionSrc {
    /// G2: a clone of the live log `live_log`, which becomes `state/sessions/<sid>.jsonl`.
    Copy { live_log: PathBuf, sid: String },
    /// G3, G4: no session; the body starts a new one.
    Fresh,
}

/// Why `keep_on_stop` failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeepError {
    /// `ProbeFail::KeepPath`: a source outside the root (or through a symlink), or a destination
    /// that is not a new path under `cache/gate/<job>/`.
    KeepPath(String),
    /// The copy itself failed.
    Io(String),
}

impl std::fmt::Display for KeepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeepError::KeepPath(m) => write!(f, "keep path: {m}"),
            KeepError::Io(m) => write!(f, "keep io: {m}"),
        }
    }
}

impl std::error::Error for KeepError {}

/// `INIT_HOME/probe/<id>/`.
#[derive(Debug, PartialEq, Eq)]
pub struct ProbeRoot {
    home: PathBuf,
    id: String,
    root: PathBuf,
}

impl ProbeRoot {
    /// §3.6.3 step 1: create `probe/<id>/` (`id`: 12 lowercase hex; the root must be new) with
    ///
    /// | path | content |
    /// |---|---|
    /// | `body/` | [`Git::clone_for_root`] at `sha` |
    /// | `bin/init-body` · `bin/ext/` | CoW clones of `binary_src` and `ext_set_dir` (else empty) |
    /// | `kernel/` | a CoW clone of `INIT_HOME/kernel` |
    /// | `state/sessions/<sid>.jsonl` · `state/session` | `Copy`: a clone of the live log · `<sid>`; `Fresh`: absent |
    /// | `state/kernel/` · `state/run/` · `work/{tmp,home,cargo-home}/` · `run/` | empty |
    /// | `extra_files` | `(source, path relative to the root)`, cloned last, never over an existing path |
    ///
    /// On any failure the partial root is deleted by exact path.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        home: &Path,
        id: &str,
        git: &Git,
        sha: &str,
        binary_src: &Path,
        ext_set_dir: Option<&Path>,
        session: &SessionSrc,
        extra_files: &[(PathBuf, PathBuf)],
    ) -> io::Result<ProbeRoot> {
        if !is_probe_id(id) {
            return Err(invalid(format!("probe id {id:?} is not 12 lowercase hex")));
        }
        let probe_dir = home.join("probe");
        crate::fsx::mkdir_mode(&probe_dir, 0o700)?;
        let root = probe_dir.join(id);
        fs::create_dir(&root)?;
        let pr = ProbeRoot { home: home.to_path_buf(), id: id.to_string(), root };
        match pr.fill(git, sha, binary_src, ext_set_dir, session, extra_files) {
            Ok(()) => Ok(pr),
            Err(e) => {
                let _ = fs::remove_dir_all(&pr.root);
                Err(e)
            }
        }
    }

    fn fill(
        &self,
        git: &Git,
        sha: &str,
        binary_src: &Path,
        ext_set_dir: Option<&Path>,
        session: &SessionSrc,
        extra_files: &[(PathBuf, PathBuf)],
    ) -> io::Result<()> {
        let r = &self.root;
        git.clone_for_root(&r.join("body"), sha).map_err(io::Error::other)?;
        fs::create_dir(r.join("bin"))?;
        clone_checked(binary_src, &r.join("bin/init-body"), Kind::File)?;
        match ext_set_dir {
            Some(d) => clone_checked(d, &r.join("bin/ext"), Kind::Dir)?,
            None => fs::create_dir(r.join("bin/ext"))?,
        }
        clone_checked(&self.home.join("kernel"), &r.join("kernel"), Kind::Dir)?;
        for d in ["state/sessions", "state/kernel", "state/run", "work/tmp", "work/home", "work/cargo-home"] {
            fs::create_dir_all(r.join(d))?;
        }
        crate::fsx::mkdir_mode(&r.join("run"), 0o700)?;
        if let SessionSrc::Copy { live_log, sid } = session {
            if !is_sid(sid) {
                return Err(invalid(format!("session id {sid:?}")));
            }
            clone_checked(live_log, &r.join(format!("state/sessions/{sid}.jsonl")), Kind::File)?;
            crate::fsx::write_atomic(&r.join("state/session"), format!("{sid}\n").as_bytes())?;
        }
        for (src, rel) in extra_files {
            let dest = self.new_path_inside(rel)?;
            if fs::symlink_metadata(src)?.file_type().is_symlink() {
                return Err(invalid(format!("extra file source {} is a symlink", src.display())));
            }
            fs::create_dir_all(dest.parent().unwrap_or(r))?;
            crate::sys::clone_path(src, &dest)?;
        }
        for p in ["body", "bin", "bin/init-body", "bin/ext", "kernel", "state", "state/kernel", "state/run", "work", "run"] {
            if fs::symlink_metadata(r.join(p))?.file_type().is_symlink() {
                return Err(invalid(format!("{p} is a symlink")));
            }
        }
        Ok(())
    }

    /// The root `INIT_HOME/probe/<id>`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The probe id (12 hex).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// `BodyLane::Probe(id)`.
    pub fn lane(&self) -> BodyLane {
        BodyLane::Probe(self.id.clone())
    }

    /// The `body` profile's parameters for this probe (sandbox.md §3.3.3).
    pub fn params(&self, sp: &SandboxPaths) -> BTreeMap<String, String> {
        body_params(&self.home, &self.lane(), sp)
    }

    /// `<root>/stdout.jsonl`: where the caller writes the probe's stdout frames (cap 4 MiB).
    pub fn stdout_path(&self) -> PathBuf {
        self.root.join("stdout.jsonl")
    }

    /// `<root>/state/kernel/boot.json`: the probe boot record (written by the caller).
    pub fn boot_json_path(&self) -> PathBuf {
        self.root.join("state/kernel/boot.json")
    }

    /// `<root>/state/run/ready` (written by the probe body).
    pub fn ready_path(&self) -> PathBuf {
        self.root.join("state/run/ready")
    }

    /// A path inside the root that does not exist yet, reached without crossing a symlink.
    fn new_path_inside(&self, rel: &Path) -> io::Result<PathBuf> {
        let p = walk_inside(&self.root, rel, false).map_err(invalid)?;
        if fs::symlink_metadata(&p).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists", rel.display())));
        }
        Ok(p)
    }

    /// `keep_on_stop` (§3.6.3): after the instance ended and before the root is deleted, copy
    /// each `(path relative to the root, destination)` with `sys::clone_path` (symlinks copied as
    /// links). The source must resolve inside the root without following a symlink; the
    /// destination (absolute, or relative to `cache/gate/<job>/`) must be a new path under
    /// `INIT_HOME/cache/gate/<job>/`. Everything is checked before anything is copied.
    pub fn keep_on_stop(&self, entries: &[(PathBuf, PathBuf)], home: &Path, job: &str) -> Result<Vec<PathBuf>, KeepError> {
        if !is_name(job) {
            return Err(KeepError::KeepPath(format!("job {job:?}")));
        }
        let gate = home.join("cache/gate").join(job);
        let mut plan: Vec<(PathBuf, PathBuf)> = Vec::new();
        for (rel, dest) in entries {
            let src = walk_inside(&self.root, rel, true).map_err(KeepError::KeepPath)?;
            let dest = if dest.is_absolute() { dest.clone() } else { gate.join(dest) };
            let tail = dest.strip_prefix(&gate).map_err(|_| KeepError::KeepPath(format!("{} is not under {}", dest.display(), gate.display())))?;
            let dest = walk_inside(&gate, tail, false).map_err(KeepError::KeepPath)?;
            if fs::symlink_metadata(&dest).is_ok() || plan.iter().any(|(_, d)| d.starts_with(&dest) || dest.starts_with(d)) {
                return Err(KeepError::KeepPath(format!("{} is not a new path", dest.display())));
            }
            plan.push((src, dest));
        }
        let mut kept = Vec::new();
        for (src, dest) in plan {
            let io = |e: io::Error| KeepError::Io(format!("{} → {}: {e}", src.display(), dest.display()));
            fs::create_dir_all(dest.parent().unwrap_or(&gate)).map_err(io)?;
            crate::sys::clone_path(&src, &dest).map_err(io)?;
            kept.push(dest);
        }
        Ok(kept)
    }

    /// §3.6.3 step 5 (`purpose = Probe`): replace `state/kernel/probe/<sha12>/` with
    /// `stdout.jsonl` (last 1 MiB of `stdout_path`), `stderr.log` (last 1 MiB of `stderr_tail`),
    /// `ready` and `boot.json` (when present), and `exit.json`. Agent-written inputs are read
    /// per K-F2. Returns the directory.
    pub fn copy_outputs_for_agent(&self, home: &Path, sha12: &str, stdout_path: &Path, stderr_tail: &[u8], exit: Option<&ExitInfo>) -> io::Result<PathBuf> {
        if sha12.len() != 12 || !is_probe_id(sha12) {
            return Err(invalid(format!("sha12 {sha12:?}")));
        }
        let dir = home.join("state/kernel/probe").join(sha12);
        remove_exact(&dir)?;
        fs::create_dir_all(&dir)?;
        let stdout = optional(crate::fsx::read_nofollow(stdout_path, OUTPUT_TAIL, true))?.unwrap_or_default();
        crate::fsx::write_atomic(&dir.join("stdout.jsonl"), &stdout)?;
        let err = &stderr_tail[stderr_tail.len().saturating_sub(OUTPUT_TAIL)..];
        crate::fsx::write_atomic(&dir.join("stderr.log"), err)?;
        if let Some(b) = optional(crate::fsx::read_nofollow(&self.ready_path(), SMALL_CAP, false))? {
            crate::fsx::write_atomic(&dir.join("ready"), &b)?;
        }
        if let Some(b) = optional(crate::fsx::read_nofollow(&self.boot_json_path(), OUTPUT_TAIL, false))? {
            crate::fsx::write_atomic(&dir.join("boot.json"), &b)?;
        }
        let ex = serde_json::json!({
            "at": exit.map(|e| e.at.clone()),
            "code": exit.and_then(|e| e.code),
            "signal": exit.and_then(|e| e.signal),
            "v": 1,
        });
        crate::fsx::write_json(&dir.join("exit.json"), &ex)?;
        Ok(dir)
    }

    /// §3.6.3 step 5: the last complete line of the probe's session log (`state/session` names
    /// it) whose `"type"` is `"boot"`, reading at most the last 1 MiB, per K-F2.
    pub fn last_boot_event(&self) -> Option<serde_json::Value> {
        let sid_raw = crate::fsx::read_nofollow(&self.root.join("state/session"), SMALL_CAP, false).ok()?;
        let sid = String::from_utf8(sid_raw).ok()?.trim().to_string();
        if !is_sid(&sid) {
            return None;
        }
        let path = self.root.join(format!("state/sessions/{sid}.jsonl"));
        let len = fs::symlink_metadata(&path).ok()?.len();
        let bytes = crate::fsx::read_nofollow(&path, SESSION_TAIL, true).ok()?;
        let mut body = &bytes[..];
        if len > SESSION_TAIL as u64 {
            let first = body.iter().position(|b| *b == b'\n')?;
            body = &body[first + 1..]; // the first line may be cut
        }
        let end = body.iter().rposition(|b| *b == b'\n')?;
        body[..end]
            .split(|b| *b == b'\n')
            .rev()
            .filter_map(|l| serde_json::from_slice::<serde_json::Value>(l).ok())
            .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("boot"))
    }

    /// §3.6.3 step 5: delete the root by exact path; refuses unless it is `home/probe/<id>`.
    pub fn delete(self, home: &Path) -> io::Result<()> {
        if self.root != home.join("probe").join(&self.id) || !is_probe_id(&self.id) {
            return Err(invalid(format!("{} is not {}/probe/{}", self.root.display(), home.display(), self.id)));
        }
        remove_exact(&self.root)
    }
}

/// A probe id: 12 lowercase hex.
pub fn is_probe_id(id: &str) -> bool {
    id.len() == 12 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A session id usable as a file name: `[A-Za-z0-9._-]`, not starting with `.`, ≤ 128 bytes.
fn is_sid(s: &str) -> bool {
    is_name(s) && s.len() <= 128
}

/// One safe path component.
fn is_name(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('.') && s.len() <= 255 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `base.join(rel)` where `rel` is relative with normal components only and every existing
/// intermediate component is a real directory (lstat, never followed). With `must_exist` the
/// final component must exist (any type: a symlink is returned as itself).
fn walk_inside(base: &Path, rel: &Path, must_exist: bool) -> Result<PathBuf, String> {
    let comps: Vec<&std::ffi::OsStr> = rel
        .components()
        .filter(|c| *c != Component::CurDir)
        .map(|c| match c {
            Component::Normal(n) => Ok(n),
            _ => Err(format!("{} is not a plain relative path", rel.display())),
        })
        .collect::<Result<_, _>>()?;
    let Some((last, dirs)) = comps.split_last() else {
        return Err(format!("{:?} names no entry", rel.display()));
    };
    let mut cur = base.to_path_buf();
    for c in dirs {
        cur.push(c);
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(format!("{} crosses a non-directory or symlink", rel.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound && !must_exist => {}
            Err(e) => return Err(format!("{}: {e}", rel.display())),
        }
    }
    cur.push(last);
    if must_exist {
        fs::symlink_metadata(&cur).map_err(|e| format!("{}: {e}", rel.display()))?;
    }
    Ok(cur)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
}

/// Clone `src` (which must itself be a regular file or a real directory) to the new path `dst`
/// and check the clone is the same kind (a source swapped for a symlink would clone as one).
fn clone_checked(src: &Path, dst: &Path, kind: Kind) -> io::Result<()> {
    let ok = |m: &fs::Metadata| if kind == Kind::File { m.is_file() } else { m.is_dir() };
    if !ok(&fs::symlink_metadata(src)?) {
        return Err(invalid(format!("{} is not a regular {}", src.display(), if kind == Kind::File { "file" } else { "directory" })));
    }
    crate::sys::clone_path(src, dst)?;
    if !ok(&fs::symlink_metadata(dst)?) {
        return Err(invalid(format!("clone of {} changed kind", src.display())));
    }
    Ok(())
}

/// Remove `p` by exact path. A tree holding read-only directories gets them made owner-writable
/// first, never following a link, and is then removed again (X0d). Every probe root's `kernel/`
/// clones the imported pool mirror `kernel/pool/<id>/` (directories 0555, files 0444), and plain
/// `remove_dir_all` stopped at its first EACCES, which leaked about 1.4 GB per G4 screen phase.
fn remove_exact(p: &Path) -> io::Result<()> {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => match fs::remove_dir_all(p) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                make_dirs_writable(p)?;
                fs::remove_dir_all(p)
            }
            r => r,
        },
        Ok(_) => fs::remove_file(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Give `dir` and every directory under it owner `rwx` (unlinking needs write and search on the
/// parent; a file's own mode does not matter). Entries are typed by `lstat`, so a symlink is never
/// followed, and the mode is set with `AT_SYMLINK_NOFOLLOW`, so an entry swapped for a link after
/// the check is not followed either.
fn make_dirs_writable(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let m = match fs::symlink_metadata(&d) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if !m.is_dir() {
            continue;
        }
        let mode = m.permissions().mode() & 0o7777;
        if mode & 0o700 != 0o700 {
            crate::sys::chmod_nofollow(&d, mode | 0o700)?;
        }
        for e in fs::read_dir(&d)? {
            let e = e?;
            if e.file_type()?.is_dir() {
                stack.push(e.path());
            }
        }
    }
    Ok(())
}

/// `NotFound` → `None`.
fn optional<T>(r: io::Result<T>) -> io::Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn invalid(m: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, m.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testfx::Fx;
    use std::os::unix::fs::{PermissionsExt, symlink};

    /// A test home with lineage at one commit, `kernel/`, a staged binary, a kept-component
    /// set and a live session log.
    fn setup(name: &str) -> (Fx, String) {
        let fx = Fx::new(name);
        fx.write("src/main.rs", b"fn main() {}\n");
        let sha = fx.commit("body");
        fx.set_main(&sha);
        let h = &fx.home;
        for (p, c) in [
            ("kernel/bin/init-kernel", &b"#!kernel"[..]),
            ("kernel/config.toml", b"[supervise]\n"),
            ("bin/init-body.staged", b"\xcf\xfa\xed\xfe binary"),
            ("bin/ext.d/abc123/tool.wasm", b"\0asm"),
            ("state/sessions/s-1.jsonl", b"{\"type\":\"boot\",\"n\":1}\n{\"type\":\"user\"}\n"),
            ("extra/task/prompt.txt", b"do the task\n"),
        ] {
            fs::create_dir_all(h.join(p).parent().unwrap()).unwrap();
            fs::write(h.join(p), c).unwrap();
        }
        (fx, sha)
    }

    fn create(fx: &Fx, sha: &str, id: &str, session: &SessionSrc, extra: &[(PathBuf, PathBuf)]) -> io::Result<ProbeRoot> {
        let h = &fx.home;
        ProbeRoot::create(h, id, &fx.git, sha, &h.join("bin/init-body.staged"), Some(&h.join("bin/ext.d/abc123")), session, extra)
    }

    fn copy_session(fx: &Fx) -> SessionSrc {
        SessionSrc::Copy { live_log: fx.home.join("state/sessions/s-1.jsonl"), sid: "s-1".into() }
    }

    #[test]
    fn create_lays_out_the_root_without_symlinks() {
        let (fx, sha) = setup("probe-create");
        let h = &fx.home;
        let extra = [(h.join("extra/task"), PathBuf::from("work/task"))];
        let pr = create(&fx, &sha, "0123456789ab", &copy_session(&fx), &extra).unwrap();
        let r = pr.root().to_path_buf();
        assert_eq!(r, h.join("probe/0123456789ab"));
        assert_eq!(fs::read_to_string(r.join("body/.git/HEAD")).unwrap().trim(), sha);
        assert_eq!(fs::read(r.join("body/src/main.rs")).unwrap(), b"fn main() {}\n");
        assert_eq!(fs::read(r.join("bin/init-body")).unwrap(), fs::read(h.join("bin/init-body.staged")).unwrap());
        assert_eq!(fs::read(r.join("bin/ext/tool.wasm")).unwrap(), b"\0asm");
        assert_eq!(fs::read(r.join("kernel/config.toml")).unwrap(), b"[supervise]\n");
        assert_eq!(fs::read(r.join("state/sessions/s-1.jsonl")).unwrap(), fs::read(h.join("state/sessions/s-1.jsonl")).unwrap());
        assert_eq!(fs::read_to_string(r.join("state/session")).unwrap(), "s-1\n");
        assert_eq!(fs::read(r.join("work/task/prompt.txt")).unwrap(), b"do the task\n");
        for d in ["state/kernel", "state/run", "work/tmp", "work/home", "work/cargo-home", "run"] {
            assert!(fs::symlink_metadata(r.join(d)).unwrap().is_dir(), "{d}");
        }
        assert_eq!(fs::metadata(r.join("run")).unwrap().permissions().mode() & 0o777, 0o700);
        for p in ["body", "bin", "bin/init-body", "bin/ext", "kernel", "kernel/bin/init-kernel", "state", "work", "run"] {
            assert!(!fs::symlink_metadata(r.join(p)).unwrap().file_type().is_symlink(), "{p}");
        }
        let sp = SandboxPaths::default();
        let p = pr.params(&sp);
        assert_eq!(pr.lane(), BodyLane::Probe("0123456789ab".into()));
        assert_eq!(Path::new(&p["BODY"]), r.join("body"));
        assert_eq!(Path::new(&p["KERNEL"]), r.join("kernel"));
        assert_eq!(Path::new(&p["BIN"]), r.join("bin"));
        assert_eq!(Path::new(&p["LINEAGE"]), h.join("lineage"));
        assert_eq!(p["LINK_KERNEL"], p["KERNEL"]);
        // The same id twice is refused; the first root is untouched.
        assert!(create(&fx, &sha, "0123456789ab", &SessionSrc::Fresh, &[]).is_err());
        assert!(r.join("body").is_dir());
        pr.delete(h).unwrap();
        assert!(!r.exists());
        // Fresh: no session at all.
        let pr = create(&fx, &sha, "0123456789ac", &SessionSrc::Fresh, &[]).unwrap();
        assert!(!pr.root().join("state/session").exists());
        assert_eq!(fs::read_dir(pr.root().join("state/sessions")).unwrap().count(), 0);
        pr.delete(h).unwrap();
    }

    #[test]
    fn create_refuses_bad_inputs_and_cleans_up() {
        let (fx, sha) = setup("probe-refuse");
        let h = &fx.home;
        symlink(h.join("state/sessions/s-1.jsonl"), h.join("state/sessions/link.jsonl")).unwrap();
        type Case<'a> = (&'a str, SessionSrc, Vec<(PathBuf, PathBuf)>);
        let cases: Vec<Case> = vec![
            ("bad-id", SessionSrc::Fresh, vec![]),
            ("aaaaaaaaaaa1", SessionSrc::Copy { live_log: h.join("state/sessions/link.jsonl"), sid: "s-1".into() }, vec![]),
            ("aaaaaaaaaaa2", SessionSrc::Copy { live_log: h.join("state/sessions/s-1.jsonl"), sid: "../x".into() }, vec![]),
            ("aaaaaaaaaaa3", SessionSrc::Fresh, vec![(h.join("extra/task"), PathBuf::from("/abs"))]),
            ("aaaaaaaaaaa4", SessionSrc::Fresh, vec![(h.join("extra/task"), PathBuf::from("work/../../x"))]),
            ("aaaaaaaaaaa5", SessionSrc::Fresh, vec![(h.join("extra/task"), PathBuf::from("bin/init-body"))]),
            ("aaaaaaaaaaa6", SessionSrc::Fresh, vec![(h.join("state/sessions/link.jsonl"), PathBuf::from("work/x"))]),
        ];
        for (id, session, extra) in cases {
            assert!(create(&fx, &sha, id, &session, &extra).is_err(), "{id}");
            assert!(!h.join("probe").join(id).exists(), "{id}: partial root removed");
        }
        let link = h.join("bin/link-body");
        symlink(h.join("bin/init-body.staged"), &link).unwrap();
        let r = ProbeRoot::create(h, "aaaaaaaaaaa7", &fx.git, &sha, &link, None, &SessionSrc::Fresh, &[]);
        assert!(r.is_err(), "a symlinked binary is refused");
        assert!(create(&fx, &"0".repeat(40), "aaaaaaaaaaa8", &SessionSrc::Fresh, &[]).is_err(), "unknown sha");
        assert!(!h.join("probe/aaaaaaaaaaa8").exists());
    }

    #[test]
    fn keep_on_stop_copies_inside_and_refuses_keep_paths() {
        let (fx, sha) = setup("probe-keep");
        let h = &fx.home;
        let pr = create(&fx, &sha, "00000000000b", &SessionSrc::Fresh, &[]).unwrap();
        let r = pr.root().to_path_buf();
        fs::create_dir_all(r.join("work/task/out")).unwrap();
        fs::write(r.join("work/task/out/result.txt"), b"42\n").unwrap();
        symlink("result.txt", r.join("work/task/out/rel-link")).unwrap();
        symlink(h.join("kernel"), r.join("work/escape")).unwrap();
        let gate = h.join("cache/gate/j-000007");
        let kept = pr
            .keep_on_stop(
                &[(PathBuf::from("work/task"), PathBuf::from("task")), (PathBuf::from("work/task/out/result.txt"), gate.join("files/result.txt"))],
                h,
                "j-000007",
            )
            .unwrap();
        assert_eq!(kept, [gate.join("task"), gate.join("files/result.txt")]);
        assert_eq!(fs::read(gate.join("task/out/result.txt")).unwrap(), b"42\n");
        assert_eq!(fs::read_link(gate.join("task/out/rel-link")).unwrap(), Path::new("result.txt"), "links copied as links");
        assert_eq!(fs::read(gate.join("files/result.txt")).unwrap(), b"42\n");
        let refused = |rel: &str, dest: PathBuf, job: &str| {
            let r = pr.keep_on_stop(&[(PathBuf::from(rel), dest.clone())], h, job);
            assert!(matches!(r, Err(KeepError::KeepPath(_))), "{rel} -> {}: {r:?}", dest.display());
        };
        refused("../0123", PathBuf::from("a"), "j-000007");
        refused("/etc/hosts", PathBuf::from("a"), "j-000007");
        refused("work/escape/config.toml", PathBuf::from("a"), "j-000007");
        refused("work/missing", PathBuf::from("a"), "j-000007");
        refused("", PathBuf::from("a"), "j-000007");
        refused("work/task", h.join("cache/gate/other/x"), "j-000007");
        refused("work/task", h.join("state/kernel/x"), "j-000007");
        refused("work/task", PathBuf::from("../other/x"), "j-000007");
        refused("work/task", PathBuf::from("task"), "j-000007");
        refused("work/task", gate.clone(), "j-000007");
        refused("work/task", PathBuf::from("x"), "../j");
        // Two entries racing for one destination are refused before anything is copied.
        let r = pr.keep_on_stop(&[(PathBuf::from("work/task"), PathBuf::from("dup")), (PathBuf::from("work/task"), PathBuf::from("dup/inner"))], h, "j-000007");
        assert!(matches!(r, Err(KeepError::KeepPath(_))));
        assert!(!gate.join("dup").exists());
        // The escaping link itself (not its target) may be kept: it is copied as a link.
        let kept = pr.keep_on_stop(&[(PathBuf::from("work/escape"), PathBuf::from("escape-link"))], h, "j-000007").unwrap();
        assert!(fs::symlink_metadata(&kept[0]).unwrap().file_type().is_symlink());
        pr.delete(h).unwrap();
    }

    fn mode(p: &Path) -> u32 {
        fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn set_mode(p: &Path, m: u32) {
        fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
    }

    /// RF-05 (X0d, RELEASE_PLAN probe-root leak). The imported pool mirror is read-only (pool.py
    /// `set_modes`: directories 0555, files 0444), and every probe root clones it with `kernel/`.
    /// `delete` makes the clone writable and removes it. The home's own mirror and a read-only
    /// directory reached through a symlink in the root keep their modes. A root that still cannot
    /// be removed (an immutable file) surfaces its error: `delete` fails, and the probe's
    /// `delete_root` logs it, alerts and returns it for `ProbeOutcome.delete_error`.
    // This fixture asserts BSD immutable-file flags and unprivileged macOS deletion semantics.
    #[cfg(target_os = "macos")]
    #[test]
    fn rf05_read_only_pool_clone_is_deleted_and_errors_surface() {
        let (fx, sha) = setup("probe-rf05");
        let h = &fx.home;
        let pool = h.join("kernel/pool/t-001");
        fs::create_dir_all(pool.join("ws/src")).unwrap();
        fs::write(pool.join("task.json"), b"{}\n").unwrap();
        fs::write(pool.join("ws/src/lib.rs"), b"// task\n").unwrap();
        let outside = fx.dir.join("outside-ro");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), b"keep\n").unwrap();
        for f in [pool.join("task.json"), pool.join("ws/src/lib.rs"), outside.join("keep.txt")] {
            set_mode(&f, 0o444);
        }
        for d in [pool.join("ws/src"), pool.join("ws"), pool.clone(), h.join("kernel/pool"), outside.clone()] {
            set_mode(&d, 0o555);
        }
        let pr = create(&fx, &sha, "00000000000d", &SessionSrc::Fresh, &[]).unwrap();
        let r = pr.root().to_path_buf();
        assert_eq!(mode(&r.join("kernel/pool/t-001/ws")), 0o555, "the clone keeps the mirror's modes");
        symlink(&outside, r.join("work/ro-link")).unwrap();
        // The leak: plain remove_dir_all stops at the first read-only directory.
        assert_eq!(fs::remove_dir_all(&r).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert!(r.join("kernel/pool/t-001/ws/src/lib.rs").exists(), "the read-only clone survived");
        pr.delete(h).unwrap();
        assert!(!r.exists(), "the whole root is gone");
        assert_eq!((mode(&pool), mode(&pool.join("ws/src")), mode(&pool.join("task.json"))), (0o555, 0o555, 0o444), "the home's mirror is untouched");
        assert_eq!((mode(&outside), fs::read(outside.join("keep.txt")).unwrap()), (0o555, b"keep\n".to_vec()), "a link is never followed");

        // A root that cannot be removed: an immutable file (chflags uchg) in the read-only clone.
        let pr = create(&fx, &sha, "00000000000e", &SessionSrc::Fresh, &[]).unwrap();
        let r = pr.root().to_path_buf();
        let stuck = r.join("kernel/pool/t-001/task.json");
        let c = std::ffi::CString::new(stuck.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid C string (test only).
        assert_eq!(unsafe { libc::chflags(c.as_ptr(), libc::UF_IMMUTABLE as _) }, 0);
        let err = crate::probe::delete_root(pr, h).expect("the delete error is surfaced");
        assert!(err.contains("probe/00000000000e") && err.contains("not deleted"), "{err}");
        let alerts = fs::read_to_string(h.join("state/kernel/alerts.jsonl")).unwrap();
        assert!(alerts.contains("\"code\":\"probe_root_leak\"") && alerts.contains("\"source\":\"probe\""), "{alerts}");
        assert!(stuck.exists());
        // Clean up by exact path.
        // SAFETY: as above.
        assert_eq!(unsafe { libc::chflags(c.as_ptr(), 0) }, 0);
        remove_exact(&r).unwrap();
        assert!(!r.exists());
        for d in [h.join("kernel/pool"), outside.clone()] {
            make_dirs_writable(&d).unwrap();
        }
        crate::testutil::remove_test_dir(&fx.dir);
    }

    #[test]
    fn outputs_for_the_agent_and_last_boot_event() {
        let (fx, sha) = setup("probe-out");
        let h = &fx.home;
        let pr = create(&fx, &sha, "00000000000c", &copy_session(&fx), &[]).unwrap();
        let r = pr.root().to_path_buf();
        // Session log: two boot events, then a torn line.
        let log = r.join("state/sessions/s-1.jsonl");
        let mut text = fs::read_to_string(&log).unwrap();
        text.push_str("{\"type\":\"boot\",\"n\":2}\n{\"type\":\"turn\"}\n{\"type\":\"boot\",\"n\":3");
        fs::write(&log, &text).unwrap();
        assert_eq!(pr.last_boot_event().unwrap()["n"], 2);
        // Over 1 MiB: only the tail is read, and the cut first line is skipped.
        let mut big = "{\"type\":\"boot\",\"n\":0}\n".to_string();
        big.push_str(&"{\"type\":\"pad\",\"x\":\"..............................\"}\n".repeat(40_000));
        big.push_str("{\"type\":\"boot\",\"n\":9}\n{\"type\":\"turn\"}\n");
        assert!(big.len() > SESSION_TAIL);
        fs::write(&log, &big).unwrap();
        assert_eq!(pr.last_boot_event().unwrap()["n"], 9);
        fs::write(&log, "{\"type\":\"turn\"}\n").unwrap();
        assert_eq!(pr.last_boot_event(), None);
        // A symlinked log is not followed (K-F2).
        fs::remove_file(&log).unwrap();
        symlink(h.join("state/sessions/s-1.jsonl"), &log).unwrap();
        assert_eq!(pr.last_boot_event(), None);
        // Outputs.
        let stdout = pr.stdout_path();
        let mut out = vec![b'a'; OUTPUT_TAIL];
        out.extend_from_slice(b"{\"t\":\"last\"}\n");
        fs::write(&stdout, &out).unwrap();
        fs::write(pr.ready_path(), b"{\"boot_id\":\"b\",\"v\":1}\n").unwrap();
        fs::write(pr.boot_json_path(), b"{\"mode\":\"probe\",\"v\":1}\n").unwrap();
        let mut err = vec![b'e'; OUTPUT_TAIL + 10];
        err.extend_from_slice(b"panic!");
        let exit = ExitInfo { code: Some(70), signal: None, at: "2026-10-20T00:00:00.000Z".into() };
        let dir = pr.copy_outputs_for_agent(h, "4b1e0c2d9a7f", &stdout, &err, Some(&exit)).unwrap();
        assert_eq!(dir, h.join("state/kernel/probe/4b1e0c2d9a7f"));
        let so = fs::read(dir.join("stdout.jsonl")).unwrap();
        assert_eq!(so.len(), OUTPUT_TAIL);
        assert!(so.ends_with(b"{\"t\":\"last\"}\n"));
        let se = fs::read(dir.join("stderr.log")).unwrap();
        assert_eq!(se.len(), OUTPUT_TAIL);
        assert!(se.ends_with(b"panic!"));
        assert_eq!(fs::read(dir.join("ready")).unwrap(), b"{\"boot_id\":\"b\",\"v\":1}\n");
        assert_eq!(fs::read(dir.join("boot.json")).unwrap(), b"{\"mode\":\"probe\",\"v\":1}\n");
        let ex: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("exit.json")).unwrap()).unwrap();
        assert_eq!(ex, serde_json::json!({"at": "2026-10-20T00:00:00.000Z", "code": 70, "signal": null, "v": 1}));
        // A second copy replaces the directory: no stale `ready`.
        fs::remove_file(pr.ready_path()).unwrap();
        let dir = pr.copy_outputs_for_agent(h, "4b1e0c2d9a7f", &stdout, b"", None).unwrap();
        assert!(!dir.join("ready").exists());
        let ex: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("exit.json")).unwrap()).unwrap();
        assert_eq!(ex["code"], serde_json::Value::Null);
        assert!(pr.copy_outputs_for_agent(h, "../x", &stdout, b"", None).is_err());
        // delete refuses another home, then removes the root by exact path.
        let other = fx.dir.join("other-home");
        let pr = match pr.delete(&other) {
            Err(_) => ProbeRoot { home: h.clone(), id: "00000000000c".into(), root: r.clone() },
            Ok(()) => panic!("delete under another home must be refused"),
        };
        assert!(r.exists());
        pr.delete(h).unwrap();
        assert!(!r.exists());
    }
}
