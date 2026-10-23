//! The hardened kernel git prefix `$G` (supervise.md §3.11.2), the lineage operations of
//! §3.11.4, and the fetch, candidate checks and export of the T2 pipeline (§3.6.1, §3.6.2).
//!
//! Every invocation starts from an empty environment plus exactly the variables and `-c` flags of
//! §3.11.2, runs with its cwd in the empty `cache/git-home`, and names its repository with
//! `--git-dir` (kernel-owned bare repos only) or `-C` on a clone the kernel just created (probe
//! and member roots, before any agent code runs: K-F3, D8). Each child is bounded by a timeout
//! (120 s by default) and killed through the handle this module holds, never by pid lookup.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Every `$G` command's bound (§3.11.2).
pub const GIT_TIMEOUT: Duration = Duration::from_secs(120);
/// `git --version` must be at least this (§3.9 step 3: the upload-pack lazy-fetch fix).
pub const MIN_VERSION: (u32, u32, u32) = (2, 39, 4);
/// `lineage/body.git/info/attributes` (§3.6.2): overrides any in-tree `.gitattributes`.
pub const LINEAGE_ATTRIBUTES: &str = "* -export-subst -export-ignore -filter -ident -text -eol -working-tree-encoding\n";
/// The agent identity every candidate commit must carry (§3.6.1 identities check).
pub const AGENT_NAME: &str = "INIT Agent";
/// See [`AGENT_NAME`].
pub const AGENT_EMAIL: &str = "agent@init.local";

/// The `-c` flags of `$G` (§3.11.2), in the doc's order.
pub const HARDENED_CONFIG: [&str; 12] = [
    "core.hooksPath=/dev/null",
    "core.fsmonitor=false",
    "core.attributesFile=/dev/null",
    "protocol.allow=never",
    "protocol.file.allow=always",
    "transfer.fsckObjects=true",
    "maintenance.auto=false",
    "gc.auto=0",
    "core.logAllRefUpdates=true",
    "user.useConfigOnly=true",
    "user.name=init-kernel",
    "user.email=kernel@init.invalid",
];

/// The fixed part of `$G`'s environment (§3.11.2); `PATH` and `HOME` are added per instance.
const HARDENED_ENV: [(&str, &str); 7] = [
    ("LC_ALL", "C"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_ATTR_NOSYSTEM", "1"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_NO_REPLACE_OBJECTS", "1"),
    ("GIT_NO_LAZY_FETCH", "1"),
];

/// stdout captured per command; more fails the command with [`GitError::TooLarge`].
const STDOUT_CAP: usize = 64 << 20;
/// stderr kept per command (its tail).
const STDERR_TAIL: usize = 8 << 10;
/// A symlink blob longer than this is not a path (`PATH_MAX`).
const MAX_LINK_TARGET: u64 = 4096;
/// Symlink hops while resolving one target inside the tree (Linux's `MAXSYMLINKS`).
const MAX_LINK_HOPS: u32 = 40;

/// What a successful git command printed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitOut {
    pub stdout: Vec<u8>,
    /// The last 8 KiB of stderr.
    pub stderr: Vec<u8>,
}

impl GitOut {
    /// stdout as text, trimmed.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_string()
    }
}

/// Why a git command failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitError {
    /// git (or `tar`) could not be started, or a pipe failed.
    Io(String),
    /// Non-zero exit (`code: None` = killed by a signal), with the stderr tail.
    Failed { code: Option<i32>, stderr: String },
    /// A ref compare-and-swap lost: `<old>` was stale, or a create-only ref exists (§3.11.4).
    Cas { stderr: String },
    /// The timeout expired; the child was killed by its handle.
    Timeout { stderr: String },
    /// stdout exceeded the 64 MiB capture cap.
    TooLarge,
    /// Arguments refused before running git.
    Usage(String),
}

impl GitError {
    /// The exit code, when git exited.
    pub fn code(&self) -> Option<i32> {
        match self {
            GitError::Failed { code, .. } => *code,
            GitError::Cas { .. } => Some(128),
            _ => None,
        }
    }
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::Io(m) => write!(f, "git: {m}"),
            GitError::Failed { code, stderr } => write!(f, "git exited {code:?}: {stderr}"),
            GitError::Cas { stderr } => write!(f, "git ref compare-and-swap failed: {stderr}"),
            GitError::Timeout { stderr } => write!(f, "git timed out: {stderr}"),
            GitError::TooLarge => write!(f, "git output too large"),
            GitError::Usage(m) => write!(f, "git usage: {m}"),
        }
    }
}

impl std::error::Error for GitError {}

/// A kernel error code with a message (`E_*`, kernel/initctl.md §4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodedError {
    pub code: &'static str,
    pub message: String,
}

impl CodedError {
    pub fn new(code: &'static str, message: impl Into<String>) -> CodedError {
        CodedError { code, message: message.into() }
    }
}

impl std::fmt::Display for CodedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CodedError {}

/// A failed §3.6.1 check: `E_NOT_DESCENDANT`, `E_TOO_LARGE`, `E_FOREIGN_COMMIT`, `E_TREE`, or
/// `E_INTERNAL` when git itself failed.
pub type CandidateError = CodedError;

/// The limits of the §3.6.1 checks table (configurable only so tests stay small).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateLimits {
    /// `rev-list --count main..SHA` ≤ this (`E_TOO_LARGE`).
    pub max_commits: u64,
    /// `ls-tree -r` entries ≤ this (`E_TREE`).
    pub max_entries: usize,
    /// Sum of blob sizes ≤ this (`E_TREE`).
    pub max_blob_bytes: u64,
}

impl Default for CandidateLimits {
    fn default() -> Self {
        CandidateLimits { max_commits: 200, max_entries: 50_000, max_blob_bytes: 512 << 20 }
    }
}

/// `$G`: a git binary, the `INIT_HOME` it serves and the kernel-owned bare repo it operates on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Git {
    /// Absolute path of the git binary (`[supervise] git`).
    pub git: PathBuf,
    /// `INIT_HOME` (canonical).
    pub home: PathBuf,
    /// The `--git-dir` (a kernel-owned bare repo under `lineage/`).
    pub git_dir: PathBuf,
}

impl Git {
    /// `$G` on `lineage/body.git`.
    pub fn body(git_bin: &Path, home: &Path) -> Git {
        Git::with_dir(git_bin, home, &home.join("lineage/body.git"))
    }

    /// `$G` with another kernel-owned `--git-dir` (`seed.git`, `kernel-cand.git`).
    pub fn with_dir(git_bin: &Path, home: &Path, git_dir: &Path) -> Git {
        Git { git: git_bin.to_path_buf(), home: home.to_path_buf(), git_dir: git_dir.to_path_buf() }
    }

    /// `HOME` for kernel git invocations: `cache/git-home` (empty).
    pub fn git_home(&self) -> PathBuf {
        self.home.join("cache/git-home")
    }

    /// The prefix without `--git-dir`: cleared env, §3.11.2's variables and `-c` flags, cwd in
    /// the (created if missing) git home.
    fn base(&self) -> Command {
        let gh = self.git_home();
        let _ = std::fs::create_dir_all(&gh);
        let mut c = Command::new(&self.git);
        c.env_clear().env("PATH", "/usr/bin:/bin").env("HOME", &gh);
        for (k, v) in HARDENED_ENV {
            c.env(k, v);
        }
        for kv in HARDENED_CONFIG {
            c.arg("-c").arg(kv);
        }
        c.current_dir(&gh);
        c
    }

    /// The hardened prefix **without** `--git-dir` (install's agent clone and seed import,
    /// before any agent code exists; §3.11.4).
    pub fn plain<S: AsRef<OsStr>>(&self, args: &[S]) -> Command {
        let mut c = self.base();
        c.args(args);
        c
    }

    /// `$G <args>` (§3.11.2) as a ready-to-run command.
    pub fn command<S: AsRef<OsStr>>(&self, args: &[S]) -> Command {
        let mut c = self.base();
        let mut gd = OsString::from("--git-dir=");
        gd.push(&self.git_dir);
        c.arg(gd).args(args);
        c
    }

    /// Run `$G <args>` with optional stdin, capturing stdout (≤ 64 MiB) and the stderr tail.
    pub fn run<S: AsRef<OsStr>>(&self, args: &[S], stdin: Option<&[u8]>, timeout: Duration) -> Result<GitOut, GitError> {
        exec(self.command(args), stdin, timeout)
    }

    /// [`Git::run`] with the default 120 s bound and no stdin.
    fn g<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<GitOut, GitError> {
        self.run(args, None, GIT_TIMEOUT)
    }

    // ---- refs (§3.11.1, §3.11.4) ------------------------------------------------------------

    /// `rev-parse --verify --quiet --end-of-options <rev>^{commit}` (§3.6.1). `None` when `rev`
    /// names no commit (or git failed).
    pub fn rev_parse_commit(&self, rev: &str) -> Option<String> {
        let spec = format!("{rev}^{{commit}}");
        let s = self.g(&["rev-parse", "--verify", "--quiet", "--end-of-options", &spec]).ok()?.text();
        is_oid(&s).then_some(s)
    }

    /// `update-ref [-m <msg>] <ref> <new> <old>`: a compare-and-swap (§3.11.4). `old` must be
    /// given: `Some("")` = create-only, `Some(<oid>)` = must currently be `<oid>`; `None` is
    /// refused ([`GitError::Usage`]). A lost race is [`GitError::Cas`].
    pub fn update_ref(&self, refname: &str, new: &str, old: Option<&str>, msg: Option<&str>) -> Result<(), GitError> {
        let Some(old) = old else {
            return Err(GitError::Usage("update_ref needs an explicit old value (\"\" = create-only)".into()));
        };
        check_refname(refname)?;
        if !is_oid(new) || !(old.is_empty() || is_oid(old)) {
            return Err(GitError::Usage(format!("update_ref {refname}: new/old must be object ids")));
        }
        let mut args = vec!["update-ref"];
        if let Some(m) = msg {
            args.extend(["-m", m]);
        }
        args.extend([refname, new, old]);
        self.g(&args).map(|_| ()).map_err(cas)
    }

    /// `update-ref -d <ref> <old>` (CAS: a stale `old` is [`GitError::Cas`]).
    pub fn delete_ref(&self, refname: &str, old: &str) -> Result<(), GitError> {
        check_refname(refname)?;
        if !is_oid(old) {
            return Err(GitError::Usage(format!("delete_ref {refname}: old must be an object id")));
        }
        self.g(&["update-ref", "-d", refname, old]).map(|_| ()).map_err(cas)
    }

    /// `merge-base --is-ancestor <a> <b>`: exit 0 → true, 1 → false, else an error.
    pub fn is_ancestor(&self, a: &str, b: &str) -> Result<bool, GitError> {
        no_option(a)?;
        no_option(b)?;
        match self.g(&["merge-base", "--is-ancestor", a, b]) {
            Ok(_) => Ok(true),
            Err(GitError::Failed { code: Some(1), .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// `rev-list --count <range>`.
    pub fn rev_list_count(&self, range: &str) -> Result<u64, GitError> {
        no_option(range)?;
        let t = self.g(&["rev-list", "--count", range])?.text();
        t.parse().map_err(|_| GitError::Failed { code: Some(0), stderr: format!("rev-list --count printed {t:?}") })
    }

    /// The committer time of `sha` in Unix seconds (`SOURCE_DATE_EPOCH`, §3.6.2), parsed from
    /// the raw commit object so no pretty-format configuration applies.
    pub fn commit_time(&self, sha: &str) -> Result<i64, GitError> {
        if !is_oid(sha) {
            return Err(GitError::Usage(format!("commit_time: {sha:?} is not an object id")));
        }
        let raw = self.g(&["cat-file", "commit", sha])?.stdout;
        let text = String::from_utf8_lossy(&raw);
        let line = text.lines().take_while(|l| !l.is_empty()).find(|l| l.starts_with("committer "));
        let secs = line.and_then(|l| l.rsplit(' ').nth(1)).and_then(|s| s.parse::<i64>().ok());
        secs.ok_or_else(|| GitError::Failed { code: Some(0), stderr: format!("no committer time in {sha}") })
    }

    /// The object id of `<sha>:<path>` (a blob or tree in lineage), `None` when the path is
    /// absent (`rev-parse --verify --quiet --end-of-options <sha>:<path>`).
    pub fn path_id(&self, sha: &str, path: &str) -> Result<Option<String>, GitError> {
        if !is_oid(sha) {
            return Err(GitError::Usage(format!("path_id: {sha:?} is not an object id")));
        }
        match self.g(&["rev-parse", "--verify", "--quiet", "--end-of-options", &format!("{sha}:{path}")]) {
            Ok(o) => Ok(Some(o.text()).filter(|s| is_oid(s))),
            Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The bytes of blob `oid` (`cat-file blob`; ≤ 64 MiB).
    pub fn read_blob(&self, oid: &str) -> Result<Vec<u8>, GitError> {
        if !is_oid(oid) {
            return Err(GitError::Usage(format!("read_blob: {oid:?} is not an object id")));
        }
        Ok(self.g(&["cat-file", "blob", oid])?.stdout)
    }

    /// `git check-ref-format <refname>`.
    pub fn check_ref_format(&self, refname: &str) -> bool {
        if refname.starts_with('-') {
            return false;
        }
        let mut c = self.base();
        c.args(["check-ref-format", refname]);
        exec(c, None, GIT_TIMEOUT).is_ok()
    }

    /// §3.6.1: `ID` matches `^[a-z0-9][a-z0-9._-]{0,62}$` and `refs/heads/evo/$ID` passes
    /// `check-ref-format` (else the caller answers `E_ARGS`).
    pub fn validate_evo_id(&self, id: &str) -> bool {
        evo_id_syntax_ok(id) && self.check_ref_format(&format!("refs/heads/evo/{id}"))
    }

    // ---- install (§3.11.4) ------------------------------------------------------------------

    /// install: create: `git init --bare <path>`, write `info/attributes` (§3.6.2), then
    /// `$G config core.logAllRefUpdates true`. `HEAD` names `refs/heads/main`.
    pub fn init_bare(&self, path: &Path) -> Result<(), GitError> {
        let mut c = self.base();
        c.args(["init", "--bare", "-q", "--template=", "--initial-branch=main"]).arg(path);
        exec(c, None, GIT_TIMEOUT)?;
        let info = path.join("info");
        std::fs::create_dir_all(&info).map_err(|e| GitError::Io(e.to_string()))?;
        crate::fsx::write_atomic(&info.join("attributes"), LINEAGE_ATTRIBUTES.as_bytes()).map_err(|e| GitError::Io(e.to_string()))?;
        Git::with_dir(&self.git, &self.home, path).g(&["config", "core.logAllRefUpdates", "true"])?;
        Ok(())
    }

    // ---- promote: fetch, resolve, archive (§3.6.1) ------------------------------------------

    /// `$G fetch --no-tags --no-write-fetch-head --no-recurse-submodules
    /// --upload-pack=<upload_pack> <repo> +<src>:<dst>`, then resolve `<dst>^{commit}` and
    /// archive it as `refs/init/archive/<sha>` (create-only, skipped if it exists). Returns the
    /// SHA, the only thing the pipeline uses afterwards.
    ///
    /// `upload_pack` is run by git through `sh -c` (git's contract), so a path with spaces must
    /// be quoted by the caller. Production passes
    /// `<INIT_HOME>/kernel/bin/init-kernel sandboxed-upload-pack`.
    pub fn fetch(&self, from_repo: &Path, src_ref: &str, dst_ref: &str, upload_pack: &str) -> Result<String, GitError> {
        check_refname(src_ref)?;
        self.fetch_src(from_repo, src_ref, dst_ref, upload_pack)
    }

    /// Fetch what `gate --quick` names (initctl.md §4.5): `HEAD` (the repo's checked-out commit),
    /// a full commit id (a clean `HEAD` or a dirty-tree snapshot that no ref names), a full ref
    /// name, or a branch name. Fetching by id needs protocol v2, git's default since 2.26, whose
    /// upload-pack serves any object it has. Returns the fetched commit.
    pub fn fetch_rev(&self, from_repo: &Path, rev: &str, dst_ref: &str, upload_pack: &str) -> Result<String, GitError> {
        let src = quick_rev_src(rev)?;
        let sha = self.fetch_src(from_repo, &src, dst_ref, upload_pack)?;
        if is_object_id(&src) && sha != src {
            return Err(GitError::Failed { code: None, stderr: format!("fetched {sha} for {src}") });
        }
        Ok(sha)
    }

    fn fetch_src(&self, from_repo: &Path, src_ref: &str, dst_ref: &str, upload_pack: &str) -> Result<String, GitError> {
        check_refname(dst_ref)?;
        if !from_repo.is_absolute() {
            return Err(GitError::Usage(format!("fetch: {} is not absolute", from_repo.display())));
        }
        let mut up = OsString::from("--upload-pack=");
        up.push(upload_pack);
        let mut c = self.command(&["fetch", "--no-tags", "--no-write-fetch-head", "--no-recurse-submodules"]);
        c.arg(up).arg(from_repo).arg(format!("+{src_ref}:{dst_ref}"));
        exec(c, None, GIT_TIMEOUT)?;
        let sha = self
            .rev_parse_commit(dst_ref)
            .ok_or_else(|| GitError::Failed { code: None, stderr: format!("{dst_ref} names no commit after the fetch") })?;
        let job = dst_ref.strip_prefix("refs/init/candidates/").unwrap_or(dst_ref);
        let id = src_ref.strip_prefix("refs/heads/").unwrap_or(src_ref);
        self.archive_ref(&sha, &format!("init: candidate {job} {id}"))?;
        Ok(sha)
    }

    /// `update-ref -m <msg> refs/init/archive/<sha> <sha> ""` (create-only). `Ok(false)` when
    /// it already existed.
    pub fn archive_ref(&self, sha: &str, msg: &str) -> Result<bool, GitError> {
        let r = format!("refs/init/archive/{sha}");
        if self.rev_parse_commit(&r).is_some() {
            return Ok(false);
        }
        match self.update_ref(&r, sha, Some(""), Some(msg)) {
            Ok(()) => Ok(true),
            Err(GitError::Cas { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    // ---- promote: checks (§3.6.1 table) -----------------------------------------------------

    /// The §3.6.1 checks on a fetched commit, in the table's order, with the default limits.
    pub fn check_candidate(&self, base_ref: &str, sha: &str, upstream_refs_prefix: &str) -> Result<(), CandidateError> {
        self.check_candidate_with(base_ref, sha, upstream_refs_prefix, &CandidateLimits::default())
    }

    /// [`Git::check_candidate`] with explicit limits.
    pub fn check_candidate_with(&self, base_ref: &str, sha: &str, upstream_refs_prefix: &str, lim: &CandidateLimits) -> Result<(), CandidateError> {
        if !is_oid(sha) {
            return Err(CodedError::new("E_INTERNAL", format!("candidate {sha:?} is not an object id")));
        }
        if !self.is_ancestor(base_ref, sha).map_err(internal)? {
            return Err(CodedError::new("E_NOT_DESCENDANT", format!("{sha} does not descend from {base_ref}: rebase onto origin/main")));
        }
        let range = format!("{base_ref}..{sha}");
        let n = self.rev_list_count(&range).map_err(internal)?;
        if n > lim.max_commits {
            return Err(CodedError::new("E_TOO_LARGE", format!("{n} commits in {range} (limit {})", lim.max_commits)));
        }
        self.check_identities(&range, upstream_refs_prefix)?;
        self.check_tree(sha, lim)
    }

    /// Identities: each commit in `range` is by `INIT Agent <agent@init.local>` (author and
    /// committer), or is an ancestor of some ref under `upstream_prefix`. The ancestry test is
    /// `for-each-ref --contains <c>`, equivalent to `merge-base --is-ancestor <c> <ref>` per ref.
    fn check_identities(&self, range: &str, upstream_prefix: &str) -> Result<(), CandidateError> {
        let out = self.g(&["log", "--no-use-mailmap", "--format=%H%x00%an%x00%ae%x00%cn%x00%ce", range]).map_err(internal)?;
        for line in String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.is_empty()) {
            let f: Vec<&str> = line.split('\0').collect();
            if f.len() != 5 {
                return Err(CodedError::new("E_INTERNAL", format!("unexpected log line {line:?}")));
            }
            if f[1] == AGENT_NAME && f[2] == AGENT_EMAIL && f[3] == AGENT_NAME && f[4] == AGENT_EMAIL {
                continue;
            }
            let hit = self
                .g(&["for-each-ref", "--count=1", "--contains", f[0], "--format=%(refname)", upstream_prefix])
                .map_err(internal)?;
            if hit.text().is_empty() {
                return Err(CodedError::new(
                    "E_FOREIGN_COMMIT",
                    format!("{} is by {} <{}> / {} <{}>, not {AGENT_NAME} <{AGENT_EMAIL}>, and not upstream", f[0], f[1], f[2], f[3], f[4]),
                ));
            }
        }
        Ok(())
    }

    /// Tree shape: no `160000`; every `120000` target relative and inside the tree after
    /// normalization (resolved through the tree's own symlinks; a loop is refused);
    /// ≤ `max_entries` entries; blob bytes ≤ `max_blob_bytes`.
    fn check_tree(&self, sha: &str, lim: &CandidateLimits) -> Result<(), CandidateError> {
        let out = match self.g(&["ls-tree", "-r", "-z", "--full-tree", "-l", sha]) {
            Ok(o) => o,
            Err(GitError::TooLarge) => return Err(CodedError::new("E_TREE", "tree listing too large")),
            Err(e) => return Err(internal(e)),
        };
        let mut entries = 0usize;
        let mut bytes = 0u64;
        let mut links: Vec<(Vec<u8>, String)> = Vec::new();
        for rec in out.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            entries += 1;
            if entries > lim.max_entries {
                return Err(CodedError::new("E_TREE", format!("more than {} entries", lim.max_entries)));
            }
            let tab = rec.iter().position(|b| *b == b'\t').ok_or_else(|| CodedError::new("E_INTERNAL", "ls-tree record without a tab"))?;
            let meta = String::from_utf8_lossy(&rec[..tab]);
            let path = rec[tab + 1..].to_vec();
            let m: Vec<&str> = meta.split_whitespace().collect();
            let (mode, oid, size) = match m.as_slice() {
                [mode, _ty, oid, size] => (*mode, *oid, *size),
                _ => return Err(CodedError::new("E_INTERNAL", format!("unexpected ls-tree record {meta:?}"))),
            };
            let shown = String::from_utf8_lossy(&path).into_owned();
            if mode == "160000" {
                return Err(CodedError::new("E_TREE", format!("submodule at {shown}")));
            }
            let size: u64 = size.parse().map_err(|_| CodedError::new("E_INTERNAL", format!("bad size in {meta:?}")))?;
            bytes = bytes.saturating_add(size);
            if mode == "120000" {
                if size == 0 || size > MAX_LINK_TARGET {
                    return Err(CodedError::new("E_TREE", format!("symlink {shown} has a {size}-byte target")));
                }
                links.push((path, oid.to_string()));
            }
        }
        if bytes > lim.max_blob_bytes {
            return Err(CodedError::new("E_TREE", format!("blob bytes {bytes} exceed {}", lim.max_blob_bytes)));
        }
        self.check_links(&links)
    }

    /// Read every symlink target in one `cat-file --batch` and resolve each inside the tree.
    fn check_links(&self, links: &[(Vec<u8>, String)]) -> Result<(), CandidateError> {
        if links.is_empty() {
            return Ok(());
        }
        let input: String = links.iter().map(|(_, oid)| format!("{oid}\n")).collect();
        let out = self.run(&["cat-file", "--batch"], Some(input.as_bytes()), GIT_TIMEOUT).map_err(internal)?;
        let targets = parse_batch(&out.stdout, links.len()).ok_or_else(|| CodedError::new("E_INTERNAL", "unexpected cat-file --batch output"))?;
        let map: HashMap<Vec<u8>, Vec<u8>> = links.iter().map(|(p, _)| p.clone()).zip(targets).collect();
        for (path, target) in &map {
            let mut dir: Vec<Vec<u8>> = path.split(|b| *b == b'/').map(<[u8]>::to_vec).collect();
            dir.pop();
            let mut hops = 0;
            if resolve_inside(&map, dir, target, &mut hops).is_none() {
                return Err(CodedError::new(
                    "E_TREE",
                    format!("symlink {} -> {} leaves the tree", String::from_utf8_lossy(path), String::from_utf8_lossy(target)),
                ));
            }
        }
        Ok(())
    }

    // ---- export (§3.6.2, §3.11.4) -----------------------------------------------------------

    /// `$G archive --format=tar <sha> | /usr/bin/tar -x -f - -C <dir>`: two children joined by
    /// a pipe (no shell), both bounded by the 120 s timeout; `dir` is created.
    pub fn export_tree(&self, sha: &str, dir: &Path) -> Result<(), GitError> {
        if !is_oid(sha) {
            return Err(GitError::Usage(format!("export_tree: {sha:?} is not an object id")));
        }
        std::fs::create_dir_all(dir).map_err(|e| GitError::Io(e.to_string()))?;
        let mut gc = self.command(&["archive", "--format=tar", sha]);
        gc.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut git = gc.spawn().map_err(|e| GitError::Io(format!("spawn git archive: {e}")))?;
        let pipe = git.stdout.take().expect("piped stdout");
        let mut tc = Command::new("/usr/bin/tar");
        tc.args(["-x", "-f", "-", "-C"]).arg(dir);
        tc.env_clear().env("PATH", "/usr/bin:/bin").env("LC_ALL", "C").current_dir(dir);
        tc.stdin(Stdio::from(pipe)).stdout(Stdio::null()).stderr(Stdio::piped());
        let tar = tc.spawn();
        drop(tc); // our copy of the pipe's read end: git must see EPIPE if tar dies
        let mut tar = match tar {
            Ok(t) => t,
            Err(e) => {
                let _ = git.kill();
                let _ = git.wait();
                return Err(GitError::Io(format!("spawn tar: {e}")));
            }
        };
        let git_err = pump(git.stderr.take(), STDERR_TAIL, true);
        let tar_err = pump(tar.stderr.take(), STDERR_TAIL, true);
        let deadline = Instant::now() + GIT_TIMEOUT;
        let gs = wait_until(&mut git, deadline);
        let ts = wait_until(&mut tar, deadline);
        let (gerr, _) = collect(git_err);
        let (terr, _) = collect(tar_err);
        let both = || format!("git: {} tar: {}", lossy(&gerr), lossy(&terr));
        match (gs, ts) {
            (Some(g), Some(t)) if g.success() && t.success() => Ok(()),
            (None, _) | (_, None) => Err(GitError::Timeout { stderr: both() }),
            (Some(g), Some(t)) => Err(GitError::Failed { code: if g.success() { t.code() } else { g.code() }, stderr: both() }),
        }
    }

    // ---- probe and member roots (§3.6.3 step 1, D8) -----------------------------------------

    /// `git clone --shared --no-checkout <lineage/body.git> <dest>` (prefix without
    /// `--git-dir`), then `git -C <dest> -c core.hooksPath=/dev/null checkout --detach <sha>`.
    /// Only for a root the kernel is creating, before any agent code runs against it (K-F3).
    pub fn clone_for_root(&self, dest_body: &Path, sha: &str) -> Result<(), GitError> {
        if !is_oid(sha) {
            return Err(GitError::Usage(format!("clone_for_root: {sha:?} is not an object id")));
        }
        if std::fs::symlink_metadata(dest_body).is_ok() {
            return Err(GitError::Usage(format!("clone_for_root: {} exists", dest_body.display())));
        }
        let mut c = self.base();
        c.args(["clone", "--shared", "--no-checkout"]).arg(&self.git_dir).arg(dest_body);
        exec(c, None, GIT_TIMEOUT)?;
        let mut c = self.base();
        c.arg("-C").arg(dest_body).args(["-c", "core.hooksPath=/dev/null", "checkout", "--detach", sha]);
        exec(c, None, GIT_TIMEOUT)?;
        Ok(())
    }
}

/// §3.9 step 3: run `<git_bin> --version` and require ≥ 2.39.4.
pub fn version_ok(git_bin: &Path) -> Result<(u32, u32, u32), String> {
    let mut c = Command::new(git_bin);
    c.arg("--version").env_clear().env("PATH", "/usr/bin:/bin").env("LC_ALL", "C");
    let out = exec(c, None, Duration::from_secs(30)).map_err(|e| e.to_string())?;
    let text = out.text();
    let v = parse_version(&text).ok_or_else(|| format!("cannot parse {text:?}"))?;
    if v < MIN_VERSION {
        return Err(format!("git {}.{}.{} is older than {}.{}.{}", v.0, v.1, v.2, MIN_VERSION.0, MIN_VERSION.1, MIN_VERSION.2));
    }
    Ok(v)
}

/// `git version 2.39.5 (Apple Git-154)` → (2, 39, 5).
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let v = s.strip_prefix("git version ")?.split_whitespace().next()?;
    let mut it = v.split('.').map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<u32>().ok());
    let major = it.next()??;
    let minor = it.next()??;
    let patch = it.next().flatten().unwrap_or(0);
    Some((major, minor, patch))
}

/// A full object id: 40 (SHA-1) or 64 (SHA-256) lowercase hex.
pub fn is_oid(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `^[a-z0-9][a-z0-9._-]{0,62}$` (§3.6.1).
pub fn evo_id_syntax_ok(id: &str) -> bool {
    let b = id.as_bytes();
    let first = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    !b.is_empty() && b.len() <= 63 && first(b[0]) && b[1..].iter().all(|c| first(*c) || matches!(c, b'.' | b'_' | b'-'))
}

/// Run a command to completion: stdin fed from a thread, stdout (capped) and the stderr tail
/// drained on threads, the child polled until `timeout`, then killed by its handle.
pub fn exec(mut cmd: Command, stdin: Option<&[u8]>, timeout: Duration) -> Result<GitOut, GitError> {
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| GitError::Io(format!("spawn {}: {e}", Path::new(cmd.get_program()).display())))?;
    if let (Some(mut w), Some(bytes)) = (child.stdin.take(), stdin) {
        let bytes = bytes.to_vec();
        // Detached: it ends when the child closes its stdin (EPIPE) or reads everything.
        std::thread::spawn(move || {
            let _ = w.write_all(&bytes);
        });
    }
    let out = pump(child.stdout.take(), STDOUT_CAP, false);
    let err = pump(child.stderr.take(), STDERR_TAIL, true);
    let status = wait_until(&mut child, Instant::now() + timeout);
    let (stdout, overflow) = collect(out);
    let (stderr, _) = collect(err);
    match status {
        None => Err(GitError::Timeout { stderr: lossy(&stderr) }),
        Some(s) if !s.success() => Err(GitError::Failed { code: s.code(), stderr: lossy(&stderr) }),
        Some(_) if overflow => Err(GitError::TooLarge),
        Some(_) => Ok(GitOut { stdout, stderr }),
    }
}

/// Poll `child` until it exits or `deadline` passes; on expiry kill it through its handle and
/// reap it. `None` = timed out.
fn wait_until(child: &mut Child, deadline: Instant) -> Option<ExitStatus> {
    let mut nap = Duration::from_millis(1);
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return Some(s),
            Ok(None) => {}
            Err(_) => break,
        }
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        std::thread::sleep(nap.min(deadline - now));
        nap = (nap * 2).min(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

type Pumped = mpsc::Receiver<(Vec<u8>, bool)>;

/// Drain a pipe on a thread: the first `cap` bytes (overflow flagged) or, with `tail`, the last.
fn pump<R: Read + Send + 'static>(r: Option<R>, cap: usize, tail: bool) -> Pumped {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut over = false;
        if let Some(mut r) = r {
            let mut chunk = [0u8; 16 << 10];
            while let Ok(n) = r.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                if tail {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > 2 * cap {
                        buf.drain(..buf.len() - cap);
                    }
                } else if buf.len() + n > cap {
                    over = true;
                } else {
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
        }
        if tail && buf.len() > cap {
            buf.drain(..buf.len() - cap);
        }
        let _ = tx.send((buf, over));
    });
    rx
}

/// A pump's result; a grandchild still holding the pipe after the child ended is not waited
/// for longer than 5 s.
fn collect(rx: Pumped) -> (Vec<u8>, bool) {
    rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default()
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim().to_string()
}

fn internal(e: GitError) -> CodedError {
    CodedError::new("E_INTERNAL", e.to_string())
}

/// Classify a failed `update-ref` whose lock or old value lost the race.
fn cas(e: GitError) -> GitError {
    match e {
        GitError::Failed { stderr, .. } if stderr.contains("cannot lock ref") => GitError::Cas { stderr },
        e => e,
    }
}

fn check_refname(r: &str) -> Result<(), GitError> {
    if r.starts_with("refs/") && !r.contains("..") && !r.bytes().any(|b| b.is_ascii_control() || b == b' ') {
        Ok(())
    } else {
        Err(GitError::Usage(format!("bad ref name {r:?}")))
    }
}

/// A full commit id (SHA-1 or SHA-256 hex, lower case).
fn is_object_id(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The refspec source for a `gate --quick` rev: `HEAD`, a full commit id, a full ref name, or a
/// branch (`refs/heads/<rev>`). Nothing that could change the refspec's meaning (`:`, `*`, `^`, …).
fn quick_rev_src(rev: &str) -> Result<String, GitError> {
    if rev == "HEAD" || is_object_id(rev) {
        return Ok(rev.to_string());
    }
    if rev.is_empty() || rev.starts_with('-') || rev.bytes().any(|b| b"+:*?[\\^~".contains(&b)) {
        return Err(GitError::Usage(format!("bad revision {rev:?}")));
    }
    let r = if rev.starts_with("refs/") { rev.to_string() } else { format!("refs/heads/{rev}") };
    check_refname(&r)?;
    Ok(r)
}

fn no_option(a: &str) -> Result<(), GitError> {
    if a.is_empty() || a.starts_with('-') { Err(GitError::Usage(format!("bad revision {a:?}"))) } else { Ok(()) }
}

/// Parse `cat-file --batch` output for `n` requested objects: their contents, in order.
fn parse_batch(mut out: &[u8], n: usize) -> Option<Vec<Vec<u8>>> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let nl = out.iter().position(|b| *b == b'\n')?;
        let header = std::str::from_utf8(&out[..nl]).ok()?;
        let size: usize = header.split(' ').nth(2)?.parse().ok()?;
        let body = out.get(nl + 1..nl + 1 + size)?;
        v.push(body.to_vec());
        out = out.get(nl + 2 + size..)?;
    }
    Some(v)
}

/// Resolve `target` (a symlink's bytes) from directory `cur` through the tree's own symlinks
/// (`links`: path → target). `None` when it is absolute, empty, contains NUL, climbs above the
/// root, or loops past [`MAX_LINK_HOPS`].
fn resolve_inside(links: &HashMap<Vec<u8>, Vec<u8>>, mut cur: Vec<Vec<u8>>, target: &[u8], hops: &mut u32) -> Option<Vec<Vec<u8>>> {
    if target.is_empty() || target[0] == b'/' || target.contains(&0) {
        return None;
    }
    for comp in target.split(|b| *b == b'/') {
        match comp {
            b"" | b"." => {}
            b".." => {
                cur.pop()?;
            }
            c => {
                cur.push(c.to_vec());
                if let Some(t) = links.get(&cur.join(&b'/')) {
                    *hops += 1;
                    if *hops > MAX_LINK_HOPS {
                        return None;
                    }
                    cur.pop();
                    cur = resolve_inside(links, cur, t, hops)?;
                }
            }
        }
    }
    Some(cur)
}

/// Shared fixtures for this module's, `build`'s and `proberoot`'s tests: a test home with a
/// lineage repo and an "agent" working repo, driven by a plain (non-hardened) git whose
/// discovery stops at the test root (`GIT_CEILING_DIRECTORIES`).
#[cfg(test)]
pub(crate) mod testfx {
    use super::*;

    pub const GIT_BIN: &str = "/usr/bin/git";
    pub const DATE: &str = "1700000000 +0000";

    pub struct Fx {
        pub dir: PathBuf,
        #[allow(dead_code)] // used by the build and proberoot tests
        pub home: PathBuf,
        pub agent: PathBuf,
        pub git: Git,
    }

    impl Fx {
        /// A test home with `lineage/body.git` (via [`Git::init_bare`]) and an empty agent repo.
        pub fn new(name: &str) -> Fx {
            let dir = crate::testutil::test_dir(name);
            let home = dir.join("home");
            std::fs::create_dir_all(home.join("lineage")).unwrap();
            std::fs::create_dir_all(home.join("cache")).unwrap();
            let git = Git::body(Path::new(GIT_BIN), &home);
            git.init_bare(&git.git_dir).unwrap();
            let agent = dir.join("agent");
            std::fs::create_dir_all(&agent).unwrap();
            let fx = Fx { dir, home, agent, git };
            fx.ag(&["init", "-q", "--initial-branch=main"]);
            fx
        }

        /// Run the fixture git in the agent repo as `name <email>`.
        pub fn ag_as(&self, name: &str, email: &str, args: &[&str]) -> String {
            let out = Command::new(GIT_BIN)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", &self.dir)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CEILING_DIRECTORIES", crate::testutil::test_root().canonicalize().unwrap())
                .env("GIT_AUTHOR_NAME", name)
                .env("GIT_AUTHOR_EMAIL", email)
                .env("GIT_COMMITTER_NAME", name)
                .env("GIT_COMMITTER_EMAIL", email)
                .env("GIT_AUTHOR_DATE", DATE)
                .env("GIT_COMMITTER_DATE", DATE)
                .arg("-C")
                .arg(&self.agent)
                .args(["-c", "core.symlinks=true", "-c", "core.hooksPath=/dev/null"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }

        /// [`Fx::ag_as`] as the agent identity.
        pub fn ag(&self, args: &[&str]) -> String {
            self.ag_as(AGENT_NAME, AGENT_EMAIL, args)
        }

        /// Write a file in the agent work tree.
        pub fn write(&self, rel: &str, content: &[u8]) {
            let p = self.agent.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }

        /// A symlink in the agent work tree.
        pub fn link(&self, rel: &str, target: &str) {
            let p = self.agent.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(target, p).unwrap();
        }

        /// `add -A` and commit as `name <email>`; returns the new HEAD.
        pub fn commit_as(&self, name: &str, email: &str, msg: &str) -> String {
            self.ag_as(name, email, &["add", "-A"]);
            self.ag_as(name, email, &["commit", "-q", "--allow-empty", "-m", msg]);
            self.ag(&["rev-parse", "HEAD"])
        }

        /// Commit as the agent.
        pub fn commit(&self, msg: &str) -> String {
            self.commit_as(AGENT_NAME, AGENT_EMAIL, msg)
        }

        /// Copy every agent branch into lineage as `refs/agent/<branch>` (plain fetch).
        pub fn sync(&self) {
            let a = self.agent.to_string_lossy().into_owned();
            self.git.run(&["fetch", "-q", "--no-tags", "--no-write-fetch-head", &a, "+refs/heads/*:refs/agent/*"], None, GIT_TIMEOUT).unwrap();
        }

        /// Set lineage `refs/heads/main` to `sha` (unconditional, test setup only).
        pub fn set_main(&self, sha: &str) {
            self.sync();
            self.git.run(&["update-ref", "refs/heads/main", sha], None, GIT_TIMEOUT).unwrap();
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            crate::testutil::remove_test_dir(&self.dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testfx::*;
    use super::*;

    #[test]
    fn prefix_is_exactly_the_hardened_one() {
        let g = Git::body(Path::new("/usr/bin/git"), Path::new("/h"));
        let c = g.command(&["status"]);
        let envs: Vec<(String, String)> =
            c.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.unwrap().to_string_lossy().into_owned())).collect();
        let mut want: Vec<(String, String)> = HARDENED_ENV.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        want.push(("PATH".into(), "/usr/bin:/bin".into()));
        want.push(("HOME".into(), "/h/cache/git-home".into()));
        let (mut a, mut b) = (envs.clone(), want);
        a.sort();
        b.sort();
        assert_eq!(a, b);
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let mut want = Vec::new();
        for kv in HARDENED_CONFIG {
            want.push("-c".to_string());
            want.push(kv.to_string());
        }
        want.push("--git-dir=/h/lineage/body.git".into());
        want.push("status".into());
        assert_eq!(args, want);
        assert_eq!(c.get_current_dir(), Some(Path::new("/h/cache/git-home")));
    }

    #[test]
    fn versions_and_names() {
        assert_eq!(parse_version("git version 2.39.5 (Apple Git-154)"), Some((2, 39, 5)));
        assert_eq!(parse_version("git version 2.45.0.windows.1"), Some((2, 45, 0)));
        assert_eq!(parse_version("git version 2.40"), Some((2, 40, 0)));
        assert_eq!(parse_version("hello"), None);
        assert!(version_ok(Path::new(GIT_BIN)).unwrap() >= MIN_VERSION);
        assert!(version_ok(Path::new("/nonexistent/git")).is_err());
        for ok in ["a", "0", "pty-tool", "a.b_c-d", &"a".repeat(63)] {
            assert!(evo_id_syntax_ok(ok), "{ok}");
        }
        for bad in ["", "-a", ".a", "A", "a/b", "a b", &"a".repeat(64)] {
            assert!(!evo_id_syntax_ok(bad), "{bad}");
        }
        assert!(is_oid(&"a".repeat(40)) && !is_oid(&"A".repeat(40)) && !is_oid("abc"));
    }

    #[test]
    fn exec_kills_its_child_on_timeout() {
        let t = Instant::now();
        let mut c = Command::new("/bin/sleep");
        c.arg("30");
        assert!(matches!(exec(c, None, Duration::from_millis(100)), Err(GitError::Timeout { .. })));
        assert!(t.elapsed() < Duration::from_secs(5));
        let mut c = Command::new("/bin/cat");
        c.env_clear();
        assert_eq!(exec(c, Some(b"abc"), Duration::from_secs(5)).unwrap().stdout, b"abc");
    }

    #[test]
    fn init_bare_and_ref_cas() {
        let fx = Fx::new("git-refs");
        let gd = &fx.git.git_dir;
        assert_eq!(std::fs::read_to_string(gd.join("info/attributes")).unwrap(), LINEAGE_ATTRIBUTES);
        assert_eq!(fx.git.run(&["config", "core.logAllRefUpdates"], None, GIT_TIMEOUT).unwrap().text(), "true");
        assert_eq!(std::fs::read_to_string(gd.join("HEAD")).unwrap().trim(), "ref: refs/heads/main");
        assert!(fx.git.git_home().is_dir());
        fx.write("a.txt", b"a\n");
        let c1 = fx.commit("one");
        fx.write("a.txt", b"b\n");
        let c2 = fx.commit("two");
        fx.sync();
        let g = &fx.git;
        assert_eq!(g.rev_parse_commit("refs/agent/main").as_deref(), Some(c2.as_str()));
        assert_eq!(g.rev_parse_commit("refs/heads/nope"), None);
        assert_eq!(g.update_ref("refs/heads/main", &c1, None, None), Err(GitError::Usage("update_ref needs an explicit old value (\"\" = create-only)".into())));
        g.update_ref("refs/heads/main", &c1, Some(""), Some("init: install")).unwrap();
        assert!(matches!(g.update_ref("refs/heads/main", &c2, Some(""), None), Err(GitError::Cas { .. })), "create-only");
        assert!(matches!(g.update_ref("refs/heads/main", &c2, Some(&c2), None), Err(GitError::Cas { .. })), "stale old");
        g.update_ref("refs/heads/main", &c2, Some(&c1), Some("init: handoff")).unwrap();
        assert_eq!(g.rev_parse_commit("refs/heads/main").as_deref(), Some(c2.as_str()));
        let log = std::fs::read_to_string(gd.join("logs/refs/heads/main")).unwrap();
        assert!(log.contains("init: handoff") && log.contains("init-kernel <kernel@init.invalid>"), "reflog: {log}");
        assert!(matches!(g.update_ref("refs/heads/main", "main", Some(&c2), None), Err(GitError::Usage(_))));
        assert!(matches!(g.update_ref("HEAD", &c2, Some(&c2), None), Err(GitError::Usage(_))));
        g.update_ref("refs/init/candidates/j1", &c2, Some(""), None).unwrap();
        assert!(matches!(g.delete_ref("refs/init/candidates/j1", &c1), Err(GitError::Cas { .. })));
        g.delete_ref("refs/init/candidates/j1", &c2).unwrap();
        assert_eq!(g.rev_parse_commit("refs/init/candidates/j1"), None);
        assert!(g.is_ancestor(&c1, &c2).unwrap());
        assert!(!g.is_ancestor(&c2, &c1).unwrap());
        assert!(g.is_ancestor("refs/heads/nope", &c1).is_err());
        assert_eq!(g.rev_list_count(&format!("{c1}..{c2}")).unwrap(), 1);
        assert_eq!(g.commit_time(&c2).unwrap(), 1_700_000_000);
    }

    #[test]
    fn fetch_resolves_and_archives_once() {
        let fx = Fx::new("git-fetch");
        fx.write("a.txt", b"a\n");
        let base = fx.commit("base");
        fx.set_main(&base);
        fx.ag(&["switch", "-q", "-c", "evo/pty-tool"]);
        fx.write("b.txt", b"b\n");
        let cand = fx.commit("candidate");
        let g = &fx.git;
        assert!(g.validate_evo_id("pty-tool"));
        assert!(!g.validate_evo_id("a..b"), "check-ref-format refuses ..");
        assert!(!g.validate_evo_id("a.lock"), "check-ref-format refuses .lock");
        assert!(!g.validate_evo_id("-x"));
        let up = "/usr/bin/git-upload-pack";
        let sha = g.fetch(&fx.agent, "refs/heads/evo/pty-tool", "refs/init/candidates/j-000001", up).unwrap();
        assert_eq!(sha, cand);
        assert_eq!(g.rev_parse_commit(&format!("refs/init/archive/{sha}")).as_deref(), Some(sha.as_str()));
        assert!(!g.git_dir.join("FETCH_HEAD").exists());
        // A second fetch of the same SHA leaves the archive ref alone.
        let again = g.fetch(&fx.agent, "refs/heads/evo/pty-tool", "refs/init/candidates/j-000002", up).unwrap();
        assert_eq!(again, cand);
        assert!(!g.archive_ref(&cand, "x").unwrap());
        assert!(g.fetch(&fx.agent, "refs/heads/evo/nope", "refs/init/candidates/j3", up).is_err());
        assert!(matches!(g.fetch(Path::new("rel"), "refs/heads/a", "refs/init/candidates/j4", up), Err(GitError::Usage(_))));
    }

    /// `gate --quick` revs (S-REAL: a clean worktree sent `HEAD` and the kernel fetched
    /// `refs/heads/HEAD`; a dirty-tree snapshot id was fetched as a branch).
    #[test]
    fn fetch_rev_takes_head_a_branch_a_ref_and_a_raw_commit_id() {
        let fx = Fx::new("git-fetch-rev");
        fx.write("a.txt", b"a\n");
        let base = fx.commit("base");
        fx.set_main(&base);
        fx.ag(&["switch", "-q", "-c", "evo/quick"]);
        fx.write("b.txt", b"b\n");
        let head = fx.commit("quick candidate");
        // A dirty-tree snapshot as initctl makes it: a commit no ref names.
        fx.write("c.txt", b"c\n");
        fx.ag(&["add", "-A"]);
        let tree = fx.ag(&["write-tree"]);
        let snap = fx.ag(&["commit-tree", &tree, "-p", "HEAD", "-m", "initctl: quick-gate snapshot"]);
        fx.ag(&["reset", "-q", "--hard"]);
        let g = &fx.git;
        let up = "/usr/bin/git-upload-pack";
        assert_eq!(g.fetch_rev(&fx.agent, "HEAD", "refs/init/quick/q1", up).unwrap(), head, "HEAD of the worktree");
        assert_eq!(g.fetch_rev(&fx.agent, "evo/quick", "refs/init/quick/q2", up).unwrap(), head, "a branch");
        assert_eq!(g.fetch_rev(&fx.agent, "refs/heads/main", "refs/init/quick/q3", up).unwrap(), base, "a full ref");
        assert_eq!(g.fetch_rev(&fx.agent, &base, "refs/init/quick/q4", up).unwrap(), base, "a reachable commit id");
        assert_eq!(g.fetch_rev(&fx.agent, &snap, "refs/init/quick/q5", up).unwrap(), snap, "an unreferenced snapshot id");
        assert_eq!(g.rev_parse_commit("refs/init/quick/q5").as_deref(), Some(snap.as_str()));
        assert!(g.fetch_rev(&fx.agent, "evo/nope", "refs/init/quick/q6", up).is_err());
        assert!(g.fetch_rev(&fx.agent, &"0".repeat(40), "refs/init/quick/q7", up).is_err(), "an id the repo lacks");
        for bad in ["", "-x", "a:b", "HEAD~1", "evo/*", "a b", "+x"] {
            assert!(matches!(g.fetch_rev(&fx.agent, bad, "refs/init/quick/q8", up), Err(GitError::Usage(_))), "{bad:?}");
        }
    }

    fn code(r: Result<(), CandidateError>) -> &'static str {
        match r {
            Ok(()) => "ok",
            Err(e) => e.code,
        }
    }

    #[test]
    fn candidate_checks_follow_the_table() {
        let fx = Fx::new("git-checks");
        fx.write("README", b"seed\n");
        let a0 = fx.commit("a0");
        // Upstream: a maintainer commit on top of a0, published as refs/upstream/seed-v1.
        fx.ag(&["switch", "-q", "-c", "up"]);
        fx.write("up.txt", b"upstream\n");
        let up = fx.commit_as("Maintainer", "m@example.org", "seed v1");
        fx.write("mine.txt", b"x\n");
        let on_up = fx.commit("agent on upstream");
        // A foreign commit that is not upstream.
        fx.ag(&["switch", "-q", "-c", "foreign", &a0]);
        fx.write("f.txt", b"f\n");
        fx.commit_as("Someone", "s@example.org", "foreign");
        let foreign = fx.commit("agent on foreign");
        // Not a descendant: an orphan root.
        fx.ag(&["switch", "-q", "--orphan", "orphan"]);
        fx.write("o.txt", b"o\n");
        let orphan = fx.commit("orphan");
        fx.set_main(&a0);
        let g = &fx.git;
        g.update_ref("refs/upstream/seed-v1", &up, Some(""), None).unwrap();
        let p = "refs/upstream/";
        assert_eq!(code(g.check_candidate("refs/heads/main", &on_up, p)), "ok");
        assert_eq!(code(g.check_candidate("refs/heads/main", &foreign, p)), "E_FOREIGN_COMMIT");
        assert_eq!(code(g.check_candidate("refs/heads/main", &orphan, p)), "E_NOT_DESCENDANT");
        let tight = CandidateLimits { max_commits: 1, ..CandidateLimits::default() };
        assert_eq!(code(g.check_candidate_with("refs/heads/main", &on_up, p, &tight)), "E_TOO_LARGE");
        let few = CandidateLimits { max_entries: 2, ..CandidateLimits::default() };
        assert_eq!(code(g.check_candidate_with("refs/heads/main", &on_up, p, &few)), "E_TREE");
        let small = CandidateLimits { max_blob_bytes: 10, ..CandidateLimits::default() };
        assert_eq!(code(g.check_candidate_with("refs/heads/main", &on_up, p, &small)), "E_TREE");
        assert_eq!(code(g.check_candidate("refs/heads/nope", &on_up, p)), "E_INTERNAL");
    }

    #[test]
    fn tree_checks_refuse_submodules_and_escaping_links() {
        let fx = Fx::new("git-tree");
        fx.write("README", b"seed\n");
        let base = fx.commit("base");
        let mut cases: Vec<(&str, String)> = Vec::new();
        // Each case is a branch off `base`; `f` builds the tree and may commit it itself.
        let mut branch = |name: &str, f: &dyn Fn(&Fx) -> Option<String>| {
            fx.ag(&["switch", "-q", "-c", name, &base]);
            let sha = f(&fx).unwrap_or_else(|| fx.commit(name));
            cases.push((if name.starts_with("ok") { "ok" } else { "E_TREE" }, sha));
        };
        branch("ok-links", &|fx| {
            fx.write("dir/file", b"x\n");
            fx.link("dir/up", "../README");
            fx.link("dir/self", ".");
            fx.link("top", "dir/self/file");
            None
        });
        branch("sub", &|fx| {
            // A gitlink staged directly (no `add -A`, which would drop it again).
            fx.ag(&["update-index", "--add", "--cacheinfo", &format!("160000,{base},vendored")]);
            fx.ag(&["commit", "-q", "-m", "sub"]);
            Some(fx.ag(&["rev-parse", "HEAD"]))
        });
        branch("escape", &|fx| {
            fx.link("evil", "../outside");
            None
        });
        branch("absolute", &|fx| {
            fx.link("abs", "/etc/passwd");
            None
        });
        branch("deep-escape", &|fx| {
            fx.link("a/b/c", "../../../x");
            None
        });
        // Lexically inside, but `a/s` is the root, so `a/u` climbs above it.
        branch("chained", &|fx| {
            fx.link("s", ".");
            fx.link("a/s", "..");
            fx.link("a/u", "s/s/../..");
            None
        });
        branch("loop", &|fx| {
            fx.link("l1", "l2");
            fx.link("l2", "l1");
            None
        });
        fx.set_main(&base);
        for (want, sha) in &cases {
            let r = fx.git.check_candidate("refs/heads/main", sha, "refs/upstream/");
            assert_eq!(code(r.clone()), *want, "{sha}: {r:?}");
        }
    }

    #[test]
    fn export_ignores_in_tree_gitattributes_sv20() {
        let fx = Fx::new("git-export");
        fx.write(".gitattributes", b"secret.txt export-ignore\nsubst.txt export-subst\ncrlf.txt text eol=crlf\n* ident\n");
        fx.write("secret.txt", b"hidden from export?\n");
        fx.write("subst.txt", b"$Format:%H$ $Id$\n");
        fx.write("crlf.txt", b"one\ntwo\n");
        fx.write("dir/x.rs", b"fn main() {}\n");
        fx.link("dir/ln", "x.rs");
        let sha = fx.commit("attrs");
        fx.sync();
        let out = fx.dir.join("export");
        fx.git.export_tree(&sha, &out).unwrap();
        for f in [".gitattributes", "secret.txt", "subst.txt", "crlf.txt", "dir/x.rs"] {
            let blob = fx.git.run(&["cat-file", "blob", &format!("{sha}:{f}")], None, GIT_TIMEOUT).unwrap().stdout;
            assert_eq!(std::fs::read(out.join(f)).unwrap(), blob, "{f} differs from its blob");
        }
        assert_eq!(std::fs::read_link(out.join("dir/ln")).unwrap(), Path::new("x.rs"));
        assert!(matches!(fx.git.export_tree("nope", &out), Err(GitError::Usage(_))));
        assert!(fx.git.export_tree(&"0".repeat(40), &fx.dir.join("export2")).is_err());
    }

    #[test]
    fn clone_for_root_checks_out_detached_with_shared_objects() {
        let fx = Fx::new("git-clone");
        fx.write("src/main.rs", b"fn main() {}\n");
        let c1 = fx.commit("one");
        fx.write("src/main.rs", b"fn main() { }\n");
        fx.commit("two");
        fx.set_main(&c1);
        let dest = fx.dir.join("probe/abc/body");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fx.git.clone_for_root(&dest, &c1).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join(".git/HEAD")).unwrap().trim(), c1);
        assert_eq!(std::fs::read(dest.join("src/main.rs")).unwrap(), b"fn main() {}\n");
        let alt = std::fs::read_to_string(dest.join(".git/objects/info/alternates")).unwrap();
        assert_eq!(Path::new(alt.trim()), fx.git.git_dir.join("objects"));
        let cfg = std::fs::read_to_string(dest.join(".git/config")).unwrap();
        assert!(cfg.contains(&format!("url = {}", fx.git.git_dir.display())), "{cfg}");
        assert!(matches!(fx.git.clone_for_root(&dest, &c1), Err(GitError::Usage(_))), "dest must be new");
    }
}
