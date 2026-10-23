//! Repo- and state-dependent reads of the validator (evolution.md §3.9 steps 1, 2 and 10), behind
//! [`EvoRepo`], and [`GitCliRepo`], which runs the `git` CLI with supervise.md §3.11.2's hardened
//! prefix `$G` on the kernel-owned lineage repo. Nothing here runs agent code or reads git config
//! outside the lineage repo.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use super::ReproRef;
use super::grammar;
use super::proc::{self, GIT_TIMEOUT};

/// One `git diff --numstat` row. `None` counts are git's `-` for binary files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumstatEntry {
    pub added: Option<u64>,
    pub deleted: Option<u64>,
    pub path: String,
}

/// What the validator needs from lineage and kernel state. All SHAs are full 40-hex ids.
pub trait EvoRepo {
    /// Step 1: `rev-parse --verify <rev>^{commit}`; `None` if it is not a commit.
    fn resolve_commit(&self, rev: &str) -> io::Result<Option<String>>;
    /// Step 2: `rev-list --parents -n 1 <commit>`: the parents, in order.
    fn parents(&self, commit: &str) -> io::Result<Vec<String>>;
    /// The raw message: everything after the first blank line of `cat-file commit <commit>`.
    fn message(&self, commit: &str) -> io::Result<Vec<u8>>;
    /// `diff --name-only <parent> <candidate>` (no rename detection).
    fn changed_paths(&self, parent: &str, candidate: &str) -> io::Result<Vec<String>>;
    /// Paths that are new in `candidate` (`diff --diff-filter=A`), for the deletion shape.
    fn added_paths(&self, parent: &str, candidate: &str) -> io::Result<Vec<String>>;
    /// `diff --numstat <parent> <candidate>` (no rename detection).
    fn numstat(&self, parent: &str, candidate: &str) -> io::Result<Vec<NumstatEntry>>;
    /// `rev-parse refs/upstream/<tag>^{commit}`; `None` if the tag is not in lineage.
    fn upstream_tag_commit(&self, tag: &str) -> io::Result<Option<String>>;
    /// `merge-base --is-ancestor <ancestor> <descendant>`; false if `ancestor` is not a commit.
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> io::Result<bool>;
    /// True iff `candidate`'s tree equals the tree git produces for `git revert <target>` on `parent`.
    /// A conflicted revert is `false`.
    fn revert_tree_matches(&self, parent: &str, target: &str, candidate: &str) -> io::Result<bool>;
    /// `components[].id → compensates_for` of `parent`'s committed `self.json`; `None` if it has none.
    fn parent_components(&self, parent: &str) -> io::Result<Option<BTreeMap<String, String>>>;
    /// Whether a `Repro` reference exists (step 10).
    fn repro_exists(&self, r: &ReproRef) -> io::Result<bool>;
    /// Whether a thinning window is open (§3.12; `thin.no_window`).
    fn thinning_window_open(&self) -> io::Result<bool>;
    /// The kernel's proposal record `state/kernel/evo/proposals/<id>.json` for a `Proposal:` id
    /// (evolution-v2.md §3.4.7, §4.2); `None` if it does not exist. The default knows no proposals.
    fn proposal(&self, id: &str) -> io::Result<Option<ProposalView>> {
        let _ = id;
        Ok(None)
    }
}

/// The two fields of a proposal record the validator reads (evolution-v2.md §3.4.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalView {
    /// `proposed`, `variants`, `screening`, `screened`, …, or `abandoned:<why>`.
    pub state: String,
    /// `chosen.sha` as recorded (any string); `None` when `chosen` is null, absent or has no string `sha`.
    pub chosen_sha: Option<String>,
}

/// Looks up a proxy `req_id` (16 hex, lowercase) with visibility `agent` in `vault/traces/v1/`.
pub type TraceLookup = Box<dyn Fn(&str) -> io::Result<bool> + Send + Sync>;

/// The `-c` options of the hardened prefix `$G` (supervise.md §3.11.2).
const HARDENED_CONFIG: [&str; 12] = [
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
    "user.name=INIT Kernel",
    "user.email=kernel@init.local",
];

/// Read cap for a session log when looking for an event id (K-F2 "caps the read size").
pub const SESSION_READ_CAP: u64 = 64 << 20;
/// Read cap for `state/kernel/thinning.json`.
pub const STATE_READ_CAP: u64 = 1 << 20;
/// Read cap for a committed `self.json`.
pub const SELF_JSON_CAP: usize = 16 << 20;
/// Read cap for `vault/gate/jobs/<job>/runs.jsonl` and `state/kernel/evo/frictions.jsonl`.
pub const INDEX_READ_CAP: u64 = 64 << 20;
/// Size limit of a proposal record, `state/kernel/evo/proposals/<id>.json`.
pub const PROPOSAL_READ_CAP: u64 = 1 << 20;

/// [`EvoRepo`] over the `git` CLI with the hardened `$G` prefix, plus the kernel's state directories.
pub struct GitCliRepo {
    /// Absolute git binary (`[supervise] git`).
    pub git: PathBuf,
    /// `$INIT_HOME/lineage/body.git`.
    pub git_dir: PathBuf,
    /// `HOME` for `$G`: `$INIT_HOME/cache/git-home`.
    pub git_home: PathBuf,
    /// `$INIT_HOME/vault` (frozen pool tasks under `gate/pool/<id>/`, run records under
    /// `gate/jobs/<job>/runs.jsonl`).
    pub vault_dir: PathBuf,
    /// `$INIT_HOME/state` (`tasks/<id>/`, `sessions/<sid>.jsonl`, `kernel/thinning.json`,
    /// `kernel/evo/frictions.jsonl`, `kernel/evo/proposals/<id>.json`).
    pub state_dir: PathBuf,
    /// Throwaway kernel-owned worktree for the revert check: `$INIT_HOME/cache/revert-check`.
    pub revert_check_dir: PathBuf,
    /// `trace:` lookup. The proxy's trace reader lives elsewhere; with `None`, every trace is
    /// "not found" (`repro.not_found`).
    pub trace_lookup: Option<TraceLookup>,
    /// Per-command timeout (120 s).
    pub timeout: Duration,
}

impl GitCliRepo {
    /// The standard layout under `$INIT_HOME`.
    pub fn new(git: impl Into<PathBuf>, init_home: &Path) -> Self {
        GitCliRepo {
            git: git.into(),
            git_dir: init_home.join("lineage/body.git"),
            git_home: init_home.join("cache/git-home"),
            vault_dir: init_home.join("vault"),
            state_dir: init_home.join("state"),
            revert_check_dir: init_home.join("cache/revert-check"),
            trace_lookup: None,
            timeout: GIT_TIMEOUT,
        }
    }

    /// Sets the `trace:` lookup.
    pub fn with_trace_lookup(mut self, f: TraceLookup) -> Self {
        self.trace_lookup = Some(f);
        self
    }

    /// The hardened environment and `-c` options, without `--git-dir`.
    fn hardened(&self) -> Command {
        let mut c = Command::new(&self.git);
        c.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.git_home)
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_NO_LAZY_FETCH", "1");
        for kv in HARDENED_CONFIG {
            c.arg("-c").arg(kv);
        }
        c
    }

    /// `$G <args>`, run from the (kernel-owned) git dir.
    fn g(&self, args: &[&str]) -> Command {
        let mut c = self.hardened();
        c.arg(format!("--git-dir={}", self.git_dir.display()))
            .args(args)
            .current_dir(&self.git_dir);
        c
    }

    fn run(&self, cmd: Command) -> io::Result<Output> {
        proc::run(cmd, None, self.timeout)
    }

    fn check(out: Output, what: &str) -> io::Result<Vec<u8>> {
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(io::Error::other(format!(
                "git {what} exited with {}: {}",
                out.status,
                proc::stderr_tail(&out.stderr)
            )))
        }
    }

    fn git_ok(&self, args: &[&str]) -> io::Result<Vec<u8>> {
        Self::check(
            self.run(self.g(args))?,
            args.first().copied().unwrap_or("?"),
        )
    }

    fn git_line(&self, args: &[&str]) -> io::Result<String> {
        Ok(String::from_utf8_lossy(&self.git_ok(args)?)
            .trim()
            .to_string())
    }

    /// `rev-parse --verify -q <spec>`: `None` when git says it does not resolve.
    fn rev_parse_opt(&self, spec: &str) -> io::Result<Option<String>> {
        let out = self.run(self.g(&["rev-parse", "--verify", "-q", spec]))?;
        if !out.status.success() {
            return Ok(None);
        }
        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(grammar::is_hex(&sha, 40).then_some(sha))
    }

    fn nul_paths(bytes: &[u8]) -> Vec<String> {
        bytes
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect()
    }

    /// Removes the revert-check worktree by its exact path, then prunes stale worktree entries.
    fn remove_revert_worktree(&self) {
        let dir = self.revert_check_dir.display().to_string();
        let _ = self.run(self.g(&["worktree", "remove", "--force", &dir]));
        if self.revert_check_dir.exists() {
            let _ = fs::remove_dir_all(&self.revert_check_dir);
        }
        let _ = self.run(self.g(&["worktree", "prune"]));
    }

    /// `$G` inside the revert-check worktree: `--git-dir` from its `.git` file, `--work-tree` = the dir.
    fn in_worktree(&self, args: &[&str]) -> io::Result<Output> {
        let dot_git = fs::read_to_string(self.revert_check_dir.join(".git"))?;
        let wt_git_dir = dot_git
            .trim()
            .strip_prefix("gitdir: ")
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "unexpected worktree .git file")
            })?
            .to_string();
        let mut c = self.hardened();
        c.arg(format!("--git-dir={wt_git_dir}"))
            .arg(format!("--work-tree={}", self.revert_check_dir.display()))
            .args(args)
            .current_dir(&self.revert_check_dir);
        self.run(c)
    }
}

/// Opens an agent-writable file for reading per supervise.md K-F2 as far as std allows: the path must
/// be a regular file and not a symlink (`lstat`), and the opened file must be that same inode (`fstat`
/// dev/ino), so a symlink swapped in after the `lstat` is rejected. `None` if absent or not a regular
/// file. (`O_NOFOLLOW | O_NONBLOCK` need `libc`, which this crate does not depend on.)
fn open_regular_nofollow(path: &Path) -> io::Result<Option<File>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !meta.file_type().is_file() {
        return Ok(None);
    }
    let file = File::open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() || opened.dev() != meta.dev() || opened.ino() != meta.ino() {
        return Ok(None);
    }
    Ok(Some(file))
}

/// True if `path` is a real directory (a symlink to one does not count).
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
}

/// The `id` of a session event or a friction line.
#[derive(serde::Deserialize)]
struct IdLine {
    id: Option<String>,
}

/// The `run_id` of a RunRecord line (gate.md §4.4).
#[derive(serde::Deserialize)]
struct RunIdLine {
    run_id: Option<String>,
}

/// True if some line of the JSONL file at `path` (opened per [`open_regular_nofollow`], at most `cap`
/// bytes read) parses as `T` and satisfies `hit`. Lines that do not parse, such as a torn last line, are
/// skipped. An absent file is `false`.
fn jsonl_any<T: serde::de::DeserializeOwned>(
    path: &Path,
    cap: u64,
    hit: impl Fn(&T) -> bool,
) -> io::Result<bool> {
    let Some(file) = open_regular_nofollow(path)? else {
        return Ok(false);
    };
    for line in BufReader::new(file.take(cap)).split(b'\n') {
        if serde_json::from_slice::<T>(&line?).is_ok_and(|v| hit(&v)) {
            return Ok(true);
        }
    }
    Ok(false)
}

impl EvoRepo for GitCliRepo {
    fn resolve_commit(&self, rev: &str) -> io::Result<Option<String>> {
        if rev.starts_with('-') {
            return Ok(None);
        }
        self.rev_parse_opt(&format!("{rev}^{{commit}}"))
    }

    fn parents(&self, commit: &str) -> io::Result<Vec<String>> {
        let line = self.git_line(&["rev-list", "--parents", "-n", "1", commit])?;
        Ok(line
            .split_whitespace()
            .skip(1)
            .map(str::to_string)
            .collect())
    }

    fn message(&self, commit: &str) -> io::Result<Vec<u8>> {
        let raw = self.git_ok(&["cat-file", "commit", commit])?;
        Ok(match raw.windows(2).position(|w| w == b"\n\n") {
            Some(i) => raw[i + 2..].to_vec(),
            None => Vec::new(),
        })
    }

    fn changed_paths(&self, parent: &str, candidate: &str) -> io::Result<Vec<String>> {
        let out = self.git_ok(&[
            "diff",
            "--no-ext-diff",
            "--no-renames",
            "--name-only",
            "-z",
            parent,
            candidate,
        ])?;
        Ok(Self::nul_paths(&out))
    }

    fn added_paths(&self, parent: &str, candidate: &str) -> io::Result<Vec<String>> {
        let out = self.git_ok(&[
            "diff",
            "--no-ext-diff",
            "--no-renames",
            "--name-only",
            "--diff-filter=A",
            "-z",
            parent,
            candidate,
        ])?;
        Ok(Self::nul_paths(&out))
    }

    fn numstat(&self, parent: &str, candidate: &str) -> io::Result<Vec<NumstatEntry>> {
        let out = self.git_ok(&[
            "diff",
            "--no-ext-diff",
            "--no-renames",
            "--numstat",
            "-z",
            parent,
            candidate,
        ])?;
        let mut entries = Vec::new();
        for rec in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            let rec = String::from_utf8_lossy(rec);
            let mut it = rec.splitn(3, '\t');
            let (Some(a), Some(d), Some(path)) = (it.next(), it.next(), it.next()) else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unexpected numstat record '{rec}'"),
                ));
            };
            let count = |s: &str| -> io::Result<Option<u64>> {
                if s == "-" {
                    return Ok(None);
                }
                s.parse::<u64>().map(Some).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("bad numstat count '{s}'"),
                    )
                })
            };
            entries.push(NumstatEntry {
                added: count(a)?,
                deleted: count(d)?,
                path: path.to_string(),
            });
        }
        Ok(entries)
    }

    fn upstream_tag_commit(&self, tag: &str) -> io::Result<Option<String>> {
        if !grammar::is_seed_tag(tag) {
            return Ok(None);
        }
        self.rev_parse_opt(&format!("refs/upstream/{tag}^{{commit}}"))
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> io::Result<bool> {
        let (Some(a), Some(d)) = (
            self.resolve_commit(ancestor)?,
            self.resolve_commit(descendant)?,
        ) else {
            return Ok(false);
        };
        let out = self.run(self.g(&["merge-base", "--is-ancestor", &a, &d]))?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(io::Error::other(format!(
                "git merge-base exited with {}: {}",
                out.status,
                proc::stderr_tail(&out.stderr)
            ))),
        }
    }

    /// git 2.39 has no `merge-tree --merge-base`, so this is §3.9 step 10's worktree method: a throwaway,
    /// kernel-owned worktree of the lineage repo at `revert_check_dir`, `revert --no-commit`, `write-tree`,
    /// compare with `<candidate>^{tree}`, then remove the worktree by its exact path. Promotions are
    /// serialized by supervise, so one fixed directory suffices.
    fn revert_tree_matches(&self, parent: &str, target: &str, candidate: &str) -> io::Result<bool> {
        let Some(want) = self.rev_parse_opt(&format!("{candidate}^{{tree}}"))? else {
            return Ok(false);
        };
        self.remove_revert_worktree();
        if let Some(up) = self.revert_check_dir.parent() {
            fs::create_dir_all(up)?;
        }
        let dir = self.revert_check_dir.display().to_string();
        self.git_ok(&["worktree", "add", "--detach", &dir, parent])?;
        let result = (|| {
            if !self
                .in_worktree(&["revert", "--no-commit", target])?
                .status
                .success()
            {
                return Ok(false);
            }
            let tree = Self::check(self.in_worktree(&["write-tree"])?, "write-tree")?;
            Ok(String::from_utf8_lossy(&tree).trim() == want)
        })();
        self.remove_revert_worktree();
        result
    }

    fn parent_components(&self, parent: &str) -> io::Result<Option<BTreeMap<String, String>>> {
        let spec = format!("{parent}:self.json");
        if !self
            .run(self.g(&["cat-file", "-e", &spec]))?
            .status
            .success()
        {
            return Ok(None);
        }
        let bytes = self.git_ok(&["cat-file", "blob", &spec])?;
        if bytes.len() > SELF_JSON_CAP {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "self.json is larger than 16 MiB",
            ));
        }
        let bad = |what: &str| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parent self.json: {what}"),
            )
        };
        let v: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| bad(&e.to_string()))?;
        let mut map = BTreeMap::new();
        for c in v
            .get("components")
            .and_then(|c| c.as_array())
            .ok_or_else(|| bad("no components array"))?
        {
            let id = c
                .get("id")
                .and_then(|x| x.as_str())
                .ok_or_else(|| bad("component without id"))?;
            let tag = c
                .get("compensates_for")
                .and_then(|x| x.as_str())
                .ok_or_else(|| bad("component without compensates_for"))?;
            map.insert(id.to_string(), tag.to_string());
        }
        Ok(Some(map))
    }

    fn repro_exists(&self, r: &ReproRef) -> io::Result<bool> {
        match r {
            ReproRef::Task(id) => {
                if !grammar::is_task_id(id) {
                    return Ok(false);
                }
                Ok(is_real_dir(&self.vault_dir.join("gate/pool").join(id))
                    || is_real_dir(&self.state_dir.join("tasks").join(id)))
            }
            ReproRef::Event { session, id } => {
                if !grammar::is_session_id(session) {
                    return Ok(false);
                }
                let path = self
                    .state_dir
                    .join("sessions")
                    .join(format!("{session}.jsonl"));
                let id = id.to_ascii_lowercase();
                jsonl_any(&path, SESSION_READ_CAP, |e: &IdLine| {
                    e.id.as_deref() == Some(id.as_str())
                })
            }
            ReproRef::Trace(id) => match &self.trace_lookup {
                Some(f) => f(id),
                None => Ok(false),
            },
            ReproRef::Run { gate, run } => {
                if !grammar::is_gate_id(gate) || !grammar::is_run_id(run) {
                    return Ok(false);
                }
                let job = self.vault_dir.join("gate/jobs").join(gate);
                if !is_real_dir(&job) {
                    return Ok(false);
                }
                jsonl_any(&job.join("runs.jsonl"), INDEX_READ_CAP, |r: &RunIdLine| {
                    r.run_id.as_deref() == Some(run.as_str())
                })
            }
            ReproRef::Friction(id) => {
                if !grammar::is_friction_id(id) {
                    return Ok(false);
                }
                let path = self.state_dir.join("kernel/evo/frictions.jsonl");
                jsonl_any(&path, INDEX_READ_CAP, |f: &IdLine| {
                    f.id.as_deref() == Some(id.as_str())
                })
            }
        }
    }

    fn proposal(&self, id: &str) -> io::Result<Option<ProposalView>> {
        if !grammar::is_proposal_id(id) {
            return Ok(None);
        }
        let path = self
            .state_dir
            .join("kernel/evo/proposals")
            .join(format!("{id}.json"));
        let Some(file) = open_regular_nofollow(&path)? else {
            return Ok(None);
        };
        let bad = |what: String| {
            io::Error::new(io::ErrorKind::InvalidData, format!("proposal {id}: {what}"))
        };
        let mut bytes = Vec::new();
        file.take(PROPOSAL_READ_CAP + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > PROPOSAL_READ_CAP {
            return Err(bad("larger than 1 MiB".into()));
        }
        let v: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| bad(e.to_string()))?;
        let state = v
            .get("state")
            .and_then(|s| s.as_str())
            .ok_or_else(|| bad("no string state".into()))?;
        let chosen_sha = v
            .get("chosen")
            .and_then(|c| c.get("sha"))
            .and_then(|s| s.as_str())
            .map(str::to_string);
        Ok(Some(ProposalView {
            state: state.to_string(),
            chosen_sha,
        }))
    }

    fn thinning_window_open(&self) -> io::Result<bool> {
        let Some(file) = open_regular_nofollow(&self.state_dir.join("kernel/thinning.json"))?
        else {
            return Ok(false);
        };
        let mut bytes = Vec::new();
        file.take(STATE_READ_CAP).read_to_end(&mut bytes)?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("thinning.json: {e}"))
        })?;
        Ok(v.get("state").and_then(|s| s.as_str()) == Some("thinning"))
    }
}
