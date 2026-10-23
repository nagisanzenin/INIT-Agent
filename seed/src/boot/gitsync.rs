//! The `body/` git sync at boot (body/boot.md §3.6; procedure: evolution.md §3.6). Non-fatal:
//! every failure is a warning and boot continues on whatever `body/` contains.
//!
//! Each git call is one `Command` (no shell) with `-C <body>`, run through tools.md §3.5.7's
//! `run_group` with the per-command timeout and a merged capture. The script is reason-agnostic:
//! an ancestry test, never `INIT_BOOT_REASON`, picks the branch.
//!
//! Rust-side rules on top of the shell form (all of them only ever do less than the script):
//! - a timeout or a spawn failure stops the script (the next command would wait another 30 s);
//! - `main` never moves before the evidence branch `rolled-back/<sha12>` exists, and an existing
//!   branch of that name that points elsewhere is never forced: `-2`, `-3` … is used instead;
//! - `origin/main` and old `main` are resolved to SHAs once and every later command uses them;
//! - no git command runs unless `<body>/.git` exists, and repository discovery cannot leave
//!   `body/` (`GIT_CEILING_DIRECTORIES` = its parent, and the `GIT_DIR`-style variables are
//!   removed), so a missing or corrupt clone never makes git act on a repository above it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{Value, json};

use crate::tools::{CaptureSpec, Captured, RunSpec, run_group};

/// Which part of the script runs (boot.md §3.6 table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncMode {
    /// normal mode, any reason: the whole script.
    Full,
    /// safe mode: step 1 (fetch) only.
    FetchOnly,
    /// probe mode: nothing.
    Skip,
}

/// How git is run: `git` (resolved on `PATH` by `Command`), the environment, the per-command timeout.
#[derive(Clone, Debug)]
pub struct GitEnv {
    pub git: PathBuf,
    /// `None`: inherit this process's environment (production). `Some`: exactly this map (tests).
    /// `GIT_TERMINAL_PROMPT=0` and `GIT_EDITOR=true` are always added.
    pub base_env: Option<BTreeMap<String, String>>,
    pub timeout: Duration,
}

impl GitEnv {
    pub fn production() -> GitEnv {
        GitEnv {
            git: PathBuf::from("git"),
            base_env: None,
            timeout: Duration::from_secs(30),
        }
    }
}

/// The `git_sync` boot fact (boot.md §3.8).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitSync {
    pub fetched: bool,
    /// `"ff"` · `"none"` · `"ref"` · `"resync"`; `None` when the script stopped before step 3.
    pub action: Option<String>,
    pub drift: bool,
    /// `"stash@{0}"` when a pop failed and the stash was kept.
    pub stash: Option<String>,
    /// `"rolled-back/<sha12 of old main>"`.
    pub rolled_back: Option<String>,
    pub error: Option<String>,
    /// stderr-style warnings for the boot facts' `warnings`.
    pub warnings: Vec<String>,
}

impl GitSync {
    /// `{action, drift, error, fetched, rolled_back, stash}` (sorted keys).
    pub fn to_value(&self) -> Value {
        json!({"action": self.action, "drift": self.drift, "error": self.error,
               "fetched": self.fetched, "rolled_back": self.rolled_back, "stash": self.stash})
    }
}

/// The value of `GitSync.stash` when a pop failed (git keeps the entry on top of the stash list).
pub const STASH_KEPT: &str = "stash@{0}";

/// `GitSync.error` when `<body>/.git` does not exist (no git command is run then).
pub const NOT_A_REPO: &str = "body/ is not a git repository";

/// Variables that would point git at another repository than `-C <body>`.
const REPO_VARS: [&str; 7] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
];

/// At most this many bytes of git's output go into one warning.
const OUTPUT_MAX: usize = 400;

/// Runs the sync in `body` (`$INIT_HOME/body`). Never panics, never deletes anything.
pub fn sync(body: &Path, mode: SyncMode, git: &GitEnv) -> GitSync {
    let mut cx = Cx {
        body,
        env: git,
        r: GitSync::default(),
    };
    if mode == SyncMode::Skip {
        return cx.r;
    }
    // No git command at all unless body/ is itself a clone (`.git` dir, or a gitfile): git would
    // otherwise discover a repository above it. GIT_CEILING_DIRECTORIES is the second guard.
    if !body.join(".git").exists() {
        cx.r.error = Some(NOT_A_REPO.into());
        cx.warn(format!(
            "{NOT_A_REPO} ({} is missing); nothing was run",
            body.join(".git").display()
        ));
        return cx.r;
    }
    // `Err(Stop)` only says the script ended early; the reason is already in `cx.r`.
    let _ = script(&mut cx, mode);
    // Some commands fail silently on a lock (`stash push` prints nothing): name the likely cause.
    if cx.r.error.is_some() && body.join(".git/index.lock").exists() {
        cx.warn(
            "body/.git/index.lock exists (left by a crashed git?); commands that write the index \
             fail until it is removed"
                .into(),
        );
    }
    cx.r
}

/// The script stopped early; the reason was recorded as a warning (and usually `error`).
struct Stop;

/// One finished git command. `code` is `None` when it was killed by a signal.
struct Out {
    code: Option<i32>,
    text: String,
}

impl Out {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

struct Cx<'a> {
    body: &'a Path,
    env: &'a GitEnv,
    r: GitSync,
}

/// evolution.md §3.6, step for step (boot.md §3.6 numbering).
fn script(cx: &mut Cx, mode: SyncMode) -> Result<(), Stop> {
    // 1. Fetch. A failure stops everything: never move `main` against a stale `origin/main`.
    cx.must(&["fetch", "--quiet", "origin"])?;
    cx.r.fetched = true;
    if mode == SyncMode::FetchOnly {
        return Ok(());
    }

    // 2. on_main.
    let o = cx.git(&["symbolic-ref", "-q", "HEAD"])?;
    let on_main = match o.code {
        Some(0) => o.text.lines().any(|l| l.trim() == "refs/heads/main"),
        Some(1) => false, // detached HEAD
        _ => return Err(cx.failed(&["symbolic-ref", "-q", "HEAD"], &o)),
    };
    let Some(new) = cx.resolve("refs/remotes/origin/main")? else {
        cx.fail("origin/main does not exist after the fetch".into());
        return Err(Stop);
    };
    let old = cx.resolve("refs/heads/main")?;

    // 3./4. The ancestry test decides the branch. A missing local `main` counts as "forward".
    let forward = match old.as_deref() {
        None => true,
        Some(o) if o == new => {
            cx.r.action = Some("none".into());
            return Ok(());
        }
        Some(o) => {
            let args = ["merge-base", "--is-ancestor", o, new.as_str()];
            let out = cx.git(&args)?;
            match out.code {
                Some(0) => true,
                Some(1) => false,
                _ => return Err(cx.failed(&args, &out)),
            }
        }
    };

    if forward {
        // 3. Lineage `main` only moved forward (promote, data-only promotion).
        if !on_main {
            return cx.move_main(&new);
        }
        let args = ["merge", "--ff-only", "--quiet", new.as_str()];
        let out = cx.git(&args)?;
        cx.r.action = Some("ff".into());
        if !out.ok() {
            // The fast-forward would touch dirty paths (or git could not run it): leave the tree.
            cx.r.drift = true;
            if out.text.contains("would be overwritten") {
                cx.warn(format!(
                    "fast-forward of main to {} refused, local changes in the paths it touches \
                     (drift; main stays at {}): {}",
                    short(&new),
                    old.as_deref().map(short).unwrap_or("(unborn)"),
                    summary(&out.text)
                ));
            } else {
                let _ = cx.failed(&args, &out);
            }
        }
        return Ok(());
    }

    // 4. Lineage `main` moved backward, or the agent committed on its own `main`: keep
    //    everything, then reset `main`.
    let Some(old) = old else {
        return Ok(()); // unreachable: a missing `main` is "forward"
    };
    let rb = cx.evidence_branch(&old)?;
    cx.r.rolled_back = Some(rb);
    if !on_main {
        return cx.move_main(&new);
    }
    cx.r.action = Some("resync".into());
    let before = cx.stash_tip()?;
    let msg = format!("init: resync to {}", short(&new));
    let push = ["stash", "push", "--include-untracked", "-m", msg.as_str()];
    let out = cx.git(&push)?;
    if !out.ok() {
        // Not fatal: `switch` below never discards local changes (it carries or refuses).
        let _ = cx.failed(&push, &out);
    }
    let after = match cx.stash_tip() {
        Ok(a) => a,
        Err(Stop) => {
            cx.warn("local changes may have been stashed; see `git stash list`".into());
            return Err(Stop);
        }
    };
    let stashed = after != before;
    let switch = ["switch", "-C", "main", new.as_str()];
    let switched = match cx.git(&switch) {
        Ok(o) if o.ok() => Ok(()),
        Ok(o) => {
            cx.r.drift = true;
            let _ = cx.failed(&switch, &o);
            Ok(()) // still pop below: it puts the changes back onto the old tree
        }
        Err(Stop) => Err(Stop),
    };
    // Pop only the stash created above: with no local changes `stash push` creates none, and a
    // blind pop would apply an older, unrelated stash.
    if stashed {
        if switched.is_err() {
            cx.r.stash = Some(STASH_KEPT.into()); // the tree is in an unknown state: do not pop
            return Err(Stop);
        }
        let pop = ["stash", "pop", "--index"];
        match cx.git(&pop) {
            Ok(o) if o.ok() => {}
            Ok(o) => {
                cx.r.stash = Some(STASH_KEPT.into()); // git keeps the entry when a pop fails
                cx.warn(format!(
                    "stash pop --index failed; your local changes are kept in {STASH_KEPT}: {}",
                    summary(&o.text)
                ));
            }
            Err(Stop) => {
                cx.r.stash = Some(STASH_KEPT.into());
                return Err(Stop);
            }
        }
    }
    switched
}

impl Cx<'_> {
    /// Runs `git -C <body> <args>`. `Err` (already recorded) on a spawn failure or a timeout.
    fn git(&mut self, args: &[&str]) -> Result<Out, Stop> {
        let mut cmd = Command::new(&self.env.git);
        if let Some(m) = &self.env.base_env {
            cmd.env_clear();
            cmd.envs(m);
        }
        for k in REPO_VARS {
            cmd.env_remove(k);
        }
        if let Some(parent) = self.body.parent().filter(|p| !p.as_os_str().is_empty()) {
            cmd.env("GIT_CEILING_DIRECTORIES", parent);
        }
        cmd.env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true");
        cmd.arg("-C").arg(self.body).args(args);
        let spec = RunSpec {
            stdin: None,
            timeout: self.env.timeout,
            cancel: None,
            on_spawn: None,
            capture: CaptureSpec::Merged {
                spill_dir: None,
                name: "git".into(),
            },
        };
        match run_group(cmd, spec) {
            Err(e) => {
                self.fail(format!(
                    "git {}: cannot run {}: {e}",
                    args.join(" "),
                    self.env.git.display()
                ));
                Err(Stop)
            }
            Ok(g) if g.timed_out => {
                self.fail(format!(
                    "git {}: timed out after {:?} (killed)",
                    args.join(" "),
                    self.env.timeout
                ));
                Err(Stop)
            }
            Ok(g) => Ok(Out {
                code: g.status.and_then(|s| s.code()),
                text: match g.output {
                    Captured::Merged { text, .. } => text,
                    Captured::Separate { .. } => String::new(),
                },
            }),
        }
    }

    /// A command that must exit 0, or the script stops.
    fn must(&mut self, args: &[&str]) -> Result<Out, Stop> {
        let o = self.git(args)?;
        if o.ok() {
            Ok(o)
        } else {
            Err(self.failed(args, &o))
        }
    }

    /// `rev-parse -q --verify <rev>^{commit}`: `Some(sha)`, or `None` when it does not exist.
    fn resolve(&mut self, rev: &str) -> Result<Option<String>, Stop> {
        let spec = format!("{rev}^{{commit}}");
        let args = ["rev-parse", "-q", "--verify", spec.as_str()];
        let o = self.git(&args)?;
        match o.code {
            Some(0) => match o.text.lines().map(str::trim).find(|l| is_sha(l)) {
                Some(sha) => Ok(Some(sha.to_string())),
                None => Err(self.failed(&args, &o)),
            },
            Some(1) => Ok(None),
            _ => Err(self.failed(&args, &o)),
        }
    }

    fn stash_tip(&mut self) -> Result<Option<String>, Stop> {
        self.resolve("refs/stash")
    }

    /// `git branch -f main <new>` → `action: "ref"`; a failure is drift. The tree is untouched.
    fn move_main(&mut self, new: &str) -> Result<(), Stop> {
        let args = ["branch", "-f", "main", new];
        let o = self.git(&args)?;
        self.r.action = Some("ref".into());
        if !o.ok() {
            self.r.drift = true;
            let _ = self.failed(&args, &o);
        }
        Ok(())
    }

    /// `git branch -f rolled-back/<sha12> <old>`, keeping old `main` reachable before it moves.
    /// An existing branch of that name at `old` is reused; one that points elsewhere (the agent
    /// moved it) is never forced, a `-2`, `-3` … suffix is used instead.
    fn evidence_branch(&mut self, old: &str) -> Result<String, Stop> {
        let base = format!("rolled-back/{}", short(old));
        for n in 1..=20 {
            let name = if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            match self.resolve(&format!("refs/heads/{name}"))? {
                Some(at) if at == old => return Ok(name),
                Some(_) => continue,
                None => {
                    self.must(&["branch", "-f", name.as_str(), old])?;
                    return Ok(name);
                }
            }
        }
        self.fail(format!(
            "no free branch name for {base}; main was not moved"
        ));
        Err(Stop)
    }

    /// Records a failed command (warning + `error`) and returns the stop marker.
    fn failed(&mut self, args: &[&str], o: &Out) -> Stop {
        let status = match o.code {
            Some(c) => format!("exit {c}"),
            None => "killed by a signal".into(),
        };
        self.fail(format!(
            "git {} failed ({status}): {}",
            args.join(" "),
            summary(&o.text)
        ));
        Stop
    }

    /// A failure: a warning, and the first one also becomes `error`.
    fn fail(&mut self, msg: String) {
        if self.r.error.is_none() {
            self.r.error = Some(msg.clone());
        }
        self.warn(msg);
    }

    fn warn(&mut self, msg: String) {
        self.r.warnings.push(format!("git_sync: {msg}"));
    }
}

/// The first 12 hex digits of a SHA (`is_sha` guarantees the length).
fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

fn is_sha(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// git's output on one line: non-empty lines joined by ` | `, at most `OUTPUT_MAX` bytes.
fn summary(text: &str) -> String {
    let mut s = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    if s.is_empty() {
        return "(no output)".into();
    }
    if s.len() > OUTPUT_MAX {
        let mut cut = OUTPUT_MAX;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push('…');
    }
    s
}

// boot.md §8 item 13. The script takes no boot reason (`sync` has no such parameter, and the
// ancestry test decides), so the per-`INIT_BOOT_REASON` variants of the test plan are the same
// runs and are not repeated here.
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    /// A test home with a fixture lineage (`lineage/body.git`), a maintainer clone that publishes
    /// to it (`maint`, standing in for the kernel's lineage writes) and the body clone (`body`).
    struct Fx {
        home: PathBuf,
        env: BTreeMap<String, String>,
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.home); // exact path, under target/test-homes/wp13/
        }
    }

    impl Fx {
        fn new(name: &str) -> Fx {
            let home = std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute()) // G0: the slot's out/target (the export is read-only)
                .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
                .join("test-homes/wp13")
                .join(format!(
                    "{name}-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::SeqCst)
                ));
            if home.exists() {
                fs::remove_dir_all(&home).unwrap(); // a stale home of a crashed run, same pid
            }
            fs::create_dir_all(&home).unwrap();
            let home_s = home.to_str().unwrap().to_string();
            let mut env = BTreeMap::new();
            env.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
            env.insert("HOME".into(), home_s.clone());
            env.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
            env.insert("GIT_CONFIG_GLOBAL".into(), "/dev/null".into());
            // Never let a fixture command discover the repository this crate lives in.
            env.insert("GIT_CEILING_DIRECTORIES".into(), home_s);
            for k in ["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"] {
                env.insert(k.into(), "INIT Agent".into());
            }
            for k in ["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"] {
                env.insert(k.into(), "agent@init.local".into());
            }
            env.insert("LANG".into(), "C".into());
            Fx { home, env }
        }

        /// Lineage `main` = c1 (`f.txt` = "one", `keep.txt` = "keep"); `body/` cloned on `main`.
        fn setup(name: &str) -> (Fx, String) {
            let fx = Fx::new(name);
            let lineage = fx.home.join("lineage/body.git");
            fs::create_dir_all(&lineage).unwrap();
            fx.git(&[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                lineage.to_str().unwrap(),
            ]);
            let maint = fx.p("maint");
            fx.git(&["init", "-q", "-b", "main", maint.to_str().unwrap()]);
            fx.write("maint/keep.txt", "keep\n");
            let c1 = fx.commit("maint", "f.txt", "one\n");
            fx.at(
                "maint",
                &["remote", "add", "origin", lineage.to_str().unwrap()],
            );
            fx.at("maint", &["push", "-q", "origin", "main"]);
            fx.git(&[
                "clone",
                "-q",
                lineage.to_str().unwrap(),
                fx.body().to_str().unwrap(),
            ]);
            (fx, c1)
        }

        fn p(&self, rel: &str) -> PathBuf {
            self.home.join(rel)
        }

        fn body(&self) -> PathBuf {
            self.p("body")
        }

        fn cmd(&self, args: &[&str]) -> (i32, String) {
            let o = Command::new("git")
                .env_clear()
                .envs(&self.env)
                .args(args)
                .output()
                .unwrap();
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            (o.status.code().unwrap_or(-1), s)
        }

        fn git(&self, args: &[&str]) -> String {
            let (code, out) = self.cmd(args);
            assert_eq!(code, 0, "git {args:?}: {out}");
            out.trim().to_string()
        }

        /// `git -C <home>/<dir> <args>`.
        fn at(&self, dir: &str, args: &[&str]) -> String {
            let d = self.p(dir);
            let mut a = vec!["-C", d.to_str().unwrap()];
            a.extend_from_slice(args);
            self.git(&a)
        }

        /// The SHA of `rev` in `dir`, or `None` when it does not exist.
        fn rev(&self, dir: &str, rev: &str) -> Option<String> {
            let d = self.p(dir);
            let (code, out) = self.cmd(&[
                "-C",
                d.to_str().unwrap(),
                "rev-parse",
                "-q",
                "--verify",
                rev,
            ]);
            (code == 0).then(|| out.trim().to_string())
        }

        fn write(&self, rel: &str, text: &str) {
            fs::write(self.p(rel), text).unwrap();
        }

        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.p(rel)).unwrap()
        }

        /// Writes `file` in `dir`, commits everything, returns the new SHA.
        fn commit(&self, dir: &str, file: &str, text: &str) -> String {
            self.write(&format!("{dir}/{file}"), text);
            self.at(dir, &["add", "-A"]);
            self.at(
                dir,
                &["commit", "-q", "-m", &format!("{file}: {}", text.trim())],
            );
            self.at(dir, &["rev-parse", "HEAD"])
        }

        /// The kernel's lineage write: lineage `main` := `sha` (forward or backward).
        fn publish(&self, sha: &str) {
            self.at(
                "maint",
                &[
                    "push",
                    "-q",
                    "-f",
                    "origin",
                    &format!("{sha}:refs/heads/main"),
                ],
            );
        }

        fn genv(&self) -> GitEnv {
            GitEnv {
                git: PathBuf::from("git"),
                base_env: Some(self.env.clone()),
                timeout: Duration::from_secs(30),
            }
        }

        fn sync(&self, mode: SyncMode) -> GitSync {
            sync(&self.body(), mode, &self.genv())
        }

        /// A `git` wrapper that logs each argv to `git.log`, then runs the real git.
        fn recording(&self) -> GitEnv {
            let bin = self.p("bin");
            fs::create_dir_all(&bin).unwrap();
            let w = bin.join("git-rec");
            let log = self.p("git.log");
            fs::write(
                &w,
                format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec git \"$@\"\n",
                    log.display()
                ),
            )
            .unwrap();
            fs::set_permissions(&w, fs::Permissions::from_mode(0o755)).unwrap();
            GitEnv {
                git: w,
                ..self.genv()
            }
        }

        fn log(&self) -> Vec<String> {
            fs::read_to_string(self.p("git.log"))
                .map(|s| s.lines().map(str::to_string).collect())
                .unwrap_or_default()
        }

        fn head(&self) -> String {
            self.at("body", &["symbolic-ref", "-q", "HEAD"])
        }

        fn stash_count(&self) -> usize {
            let s = self.at("body", &["stash", "list"]);
            s.lines().filter(|l| !l.is_empty()).count()
        }
    }

    fn ok(action: &str) -> GitSync {
        GitSync {
            fetched: true,
            action: Some(action.into()),
            ..GitSync::default()
        }
    }

    #[test]
    fn forward_clean_on_main_fast_forwards_then_none() {
        let (fx, _c1) = Fx::setup("ff");
        let c2 = fx.commit("maint", "f.txt", "two\n");
        fx.publish(&c2);
        assert_eq!(fx.sync(SyncMode::Full), ok("ff"));
        assert_eq!(
            fx.rev("body", "refs/heads/main").as_deref(),
            Some(c2.as_str())
        );
        assert_eq!(fx.read("body/f.txt"), "two\n");
        assert_eq!(fx.head(), "refs/heads/main");
        // Already equal: nothing to do.
        assert_eq!(fx.sync(SyncMode::Full), ok("none"));
        let v = ok("ff").to_value();
        assert_eq!(
            v.to_string(),
            r#"{"action":"ff","drift":false,"error":null,"fetched":true,"rolled_back":null,"stash":null}"#
        );
    }

    #[test]
    fn forward_dirty_untouched_path_still_fast_forwards() {
        let (fx, _c1) = Fx::setup("ffdirty");
        fx.write("body/keep.txt", "local keep\n");
        let c2 = fx.commit("maint", "f.txt", "two\n");
        fx.publish(&c2);
        assert_eq!(fx.sync(SyncMode::Full), ok("ff"));
        assert_eq!(
            fx.rev("body", "refs/heads/main").as_deref(),
            Some(c2.as_str())
        );
        assert_eq!(fx.read("body/keep.txt"), "local keep\n");
    }

    #[test]
    fn forward_touching_dirty_path_is_drift_and_tree_untouched() {
        let (fx, c1) = Fx::setup("drift");
        fx.write("body/f.txt", "local\n");
        let c2 = fx.commit("maint", "f.txt", "two\n");
        fx.publish(&c2);
        let r = fx.sync(SyncMode::Full);
        assert!(r.fetched && r.drift, "{r:?}");
        assert_eq!(r.action.as_deref(), Some("ff"));
        assert_eq!(
            r.error, None,
            "a refused fast-forward is drift, not an error"
        );
        assert_eq!(r.warnings.len(), 1, "{r:?}");
        assert!(r.warnings[0].starts_with("git_sync: "), "{r:?}");
        assert_eq!((r.stash, r.rolled_back), (None, None));
        assert_eq!(fx.rev("body", "refs/heads/main"), Some(c1));
        assert_eq!(fx.rev("body", "refs/remotes/origin/main"), Some(c2));
        assert_eq!(fx.read("body/f.txt"), "local\n");
    }

    #[test]
    fn forward_on_evo_branch_moves_only_the_ref() {
        let (fx, _c1) = Fx::setup("evo");
        fx.at("body", &["switch", "-q", "-c", "evo/x"]);
        let e1 = fx.commit("body", "evo.txt", "evo\n");
        fx.write("body/f.txt", "wip\n");
        let c2 = fx.commit("maint", "f.txt", "two\n");
        fx.publish(&c2);
        assert_eq!(fx.sync(SyncMode::Full), ok("ref"));
        assert_eq!(fx.rev("body", "refs/heads/main"), Some(c2));
        assert_eq!(fx.head(), "refs/heads/evo/x");
        assert_eq!(fx.rev("body", "refs/heads/evo/x"), Some(e1));
        assert_eq!(fx.read("body/f.txt"), "wip\n");
        assert_eq!(fx.read("body/evo.txt"), "evo\n");
    }

    /// Body `main` at c2, then lineage `main` rolled back to c1.
    fn rolled_back_fixture(name: &str) -> (Fx, String, String) {
        let (fx, c1) = Fx::setup(name);
        let c2 = fx.commit("maint", "f.txt", "two\n");
        fx.publish(&c2);
        fx.at("body", &["pull", "-q", "--ff-only"]);
        assert_eq!(
            fx.rev("body", "refs/heads/main").as_deref(),
            Some(c2.as_str())
        );
        fx.publish(&c1);
        (fx, c1, c2)
    }

    #[test]
    fn rollback_with_dirty_tree_stashes_resets_and_restores() {
        let (fx, c1, c2) = rolled_back_fixture("rollback");
        fx.write("body/keep.txt", "staged\n");
        fx.at("body", &["add", "keep.txt"]);
        fx.write("body/keep.txt", "staged then edited\n");
        fx.write("body/new.txt", "untracked\n");
        let rb = format!("rolled-back/{}", &c2[..12]);
        let r = fx.sync(SyncMode::Full);
        assert_eq!(
            r,
            GitSync {
                rolled_back: Some(rb.clone()),
                ..ok("resync")
            }
        );
        assert_eq!(fx.rev("body", &format!("refs/heads/{rb}")), Some(c2));
        assert_eq!(
            fx.rev("body", "refs/heads/main").as_deref(),
            Some(c1.as_str())
        );
        assert_eq!(fx.rev("body", "refs/remotes/origin/main"), Some(c1));
        assert_eq!(fx.head(), "refs/heads/main");
        assert_eq!(fx.read("body/f.txt"), "one\n");
        assert_eq!(fx.read("body/keep.txt"), "staged then edited\n");
        assert_eq!(fx.read("body/new.txt"), "untracked\n");
        // `--index`: the staged version is staged again.
        assert_eq!(fx.at("body", &["show", ":keep.txt"]), "staged");
        assert_eq!(fx.rev("body", "refs/stash"), None, "the stash was popped");
    }

    #[test]
    fn agent_commit_on_own_main_survives_on_rolled_back_branch() {
        let (fx, _c1) = Fx::setup("agentmain");
        let a1 = fx.commit("body", "agent.txt", "mine\n");
        let c2 = fx.commit("maint", "f.txt", "two\n");
        fx.publish(&c2);
        let rb = format!("rolled-back/{}", &a1[..12]);
        let r = fx.sync(SyncMode::Full);
        assert_eq!(
            r,
            GitSync {
                rolled_back: Some(rb.clone()),
                ..ok("resync")
            }
        );
        assert_eq!(fx.rev("body", &format!("refs/heads/{rb}")), Some(a1));
        assert_eq!(
            fx.rev("body", "refs/heads/main").as_deref(),
            Some(c2.as_str())
        );
        assert_eq!(fx.rev("body", "refs/remotes/origin/main"), Some(c2));
        assert_eq!(fx.read("body/f.txt"), "two\n");
        assert!(!fx.p("body/agent.txt").exists());
        assert_eq!(fx.stash_count(), 0);
    }

    #[test]
    fn agent_moved_rolled_back_branch_is_never_forced() {
        let (fx, _c1) = Fx::setup("rbtaken");
        let a1 = fx.commit("body", "agent.txt", "mine\n");
        let rb = format!("rolled-back/{}", &a1[..12]);
        // The agent already has a branch of that name somewhere else.
        let elsewhere = fx.at("body", &["rev-parse", "HEAD~1"]);
        fx.at("body", &["branch", &rb, &elsewhere]);
        let c2 = fx.commit("maint", "f.txt", "two\n");
        fx.publish(&c2);
        let r = fx.sync(SyncMode::Full);
        assert_eq!(r.rolled_back, Some(format!("{rb}-2")), "{r:?}");
        assert_eq!(fx.rev("body", &format!("refs/heads/{rb}")), Some(elsewhere));
        assert_eq!(fx.rev("body", &format!("refs/heads/{rb}-2")), Some(a1));
        assert_eq!(fx.rev("body", "refs/heads/main"), Some(c2));
    }

    #[test]
    fn conflicting_pop_keeps_the_stash() {
        let (fx, c1, c2) = rolled_back_fixture("conflict");
        fx.write("body/f.txt", "local\n"); // c2 → c1 changes f.txt too: the pop conflicts
        let r = fx.sync(SyncMode::Full);
        assert_eq!(r.stash.as_deref(), Some("stash@{0}"), "{r:?}");
        assert_eq!(r.action.as_deref(), Some("resync"));
        assert_eq!(r.rolled_back, Some(format!("rolled-back/{}", &c2[..12])));
        assert_eq!(r.to_value()["stash"], json!("stash@{0}"));
        assert!(!r.warnings.is_empty());
        assert_eq!(fx.rev("body", "refs/heads/main"), Some(c1.clone()));
        assert_eq!(fx.stash_count(), 1);
        let list = fx.at("body", &["stash", "list"]);
        assert!(
            list.contains(&format!("init: resync to {}", &c1[..12])),
            "{list}"
        );
        // The kept stash still holds the local edit.
        assert_eq!(fx.at("body", &["show", "stash@{0}:f.txt"]), "local");
    }

    #[test]
    fn no_local_changes_never_pops_an_older_stash() {
        let (fx, c1) = Fx::setup("oldstash");
        fx.write("body/f.txt", "old wip\n");
        fx.at("body", &["stash", "push", "-q", "-m", "older, unrelated"]);
        let s0 = fx.rev("body", "refs/stash").unwrap();
        let c2 = fx.commit("maint", "f.txt", "two\n");
        fx.publish(&c2);
        fx.at("body", &["pull", "-q", "--ff-only"]);
        fx.publish(&c1); // rollback, clean tree
        let r = fx.sync(SyncMode::Full);
        assert_eq!(
            r,
            GitSync {
                rolled_back: Some(format!("rolled-back/{}", &c2[..12])),
                ..ok("resync")
            }
        );
        assert_eq!(
            fx.rev("body", "refs/stash"),
            Some(s0),
            "refs/stash unchanged"
        );
        assert_eq!(fx.stash_count(), 1);
        assert_eq!(fx.read("body/f.txt"), "one\n");
        assert_eq!(fx.rev("body", "refs/heads/main"), Some(c1));
    }

    #[test]
    fn fetch_failure_stops_the_script() {
        let (fx, c1) = Fx::setup("nofetch");
        let c2 = fx.commit("maint", "f.txt", "two\n");
        fx.publish(&c2);
        fx.at("body", &["fetch", "-q"]); // origin/main = c2: a continued script would fast-forward
        fs::rename(fx.p("lineage/body.git"), fx.p("lineage/moved.git")).unwrap();
        let g = fx.recording();
        let r = sync(&fx.body(), SyncMode::Full, &g);
        assert!(!r.fetched, "{r:?}");
        assert_eq!((r.action.as_deref(), r.drift), (None, false));
        assert!(r.error.as_deref().unwrap().contains("fetch"), "{r:?}");
        assert_eq!(r.warnings.len(), 1, "{r:?}");
        let log = fx.log();
        assert_eq!(log.len(), 1, "{log:?}");
        assert!(log[0].ends_with("fetch --quiet origin"), "{log:?}");
        assert_eq!(fx.rev("body", "refs/heads/main"), Some(c1));
    }

    #[test]
    fn fetch_only_runs_only_the_fetch() {
        let (fx, c1) = Fx::setup("fetchonly");
        let c2 = fx.commit("maint", "f.txt", "two\n");
        fx.publish(&c2);
        let g = fx.recording();
        let r = sync(&fx.body(), SyncMode::FetchOnly, &g);
        assert_eq!(
            r,
            GitSync {
                fetched: true,
                ..GitSync::default()
            }
        );
        let body = fx.body();
        assert_eq!(
            fx.log(),
            vec![format!("-C {} fetch --quiet origin", body.display())]
        );
        assert_eq!(fx.rev("body", "refs/remotes/origin/main"), Some(c2));
        assert_eq!(fx.rev("body", "refs/heads/main"), Some(c1));
    }

    #[test]
    fn skip_runs_nothing() {
        let (fx, _c1) = Fx::setup("skip");
        let g = fx.recording();
        assert_eq!(sync(&fx.body(), SyncMode::Skip, &g), GitSync::default());
        assert!(fx.log().is_empty());
        assert!(!fx.p("git.log").exists());
    }

    #[test]
    fn leftover_index_lock_is_a_warning_on_forward() {
        let (fx, c1) = Fx::setup("lockff");
        let c2 = fx.commit("maint", "f.txt", "two\n");
        fx.publish(&c2);
        fx.write("body/.git/index.lock", "");
        let r = fx.sync(SyncMode::Full);
        assert!(r.fetched && r.drift, "{r:?}");
        assert!(r.error.as_deref().unwrap().contains("index.lock"), "{r:?}");
        assert!(!r.warnings.is_empty());
        assert_eq!(fx.rev("body", "refs/heads/main"), Some(c1));
        assert!(fx.p("body/.git/index.lock").exists(), "never deleted");
    }

    #[test]
    fn leftover_index_lock_is_a_warning_on_rollback() {
        let (fx, _c1, c2) = rolled_back_fixture("lockrb");
        fx.write("body/f.txt", "local\n");
        fx.write("body/.git/index.lock", "");
        let r = fx.sync(SyncMode::Full);
        assert!(r.fetched && r.drift, "{r:?}");
        assert!(r.error.is_some(), "{r:?}");
        assert!(
            r.warnings.last().unwrap().contains("index.lock exists"),
            "{r:?}"
        );
        assert_eq!(r.stash, None, "nothing was stashed");
        assert_eq!(r.rolled_back, Some(format!("rolled-back/{}", &c2[..12])));
        assert_eq!(
            fx.rev("body", "refs/heads/main"),
            Some(c2),
            "main not moved"
        );
        assert_eq!(fx.read("body/f.txt"), "local\n");
        assert!(fx.p("body/.git/index.lock").exists(), "never deleted");
    }

    #[test]
    fn timeout_is_a_warning() {
        let (fx, _c1) = Fx::setup("timeout");
        let g = GitEnv {
            timeout: Duration::ZERO,
            ..fx.genv()
        };
        let r = sync(&fx.body(), SyncMode::Full, &g);
        assert!(!r.fetched, "{r:?}");
        assert!(r.error.as_deref().unwrap().contains("timed out"), "{r:?}");
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn missing_or_corrupt_clone_is_a_warning_and_never_escapes_body() {
        let fx = Fx::new("corrupt");
        // Not a repository at all (and a missing body/): no git command runs, in either mode.
        fs::create_dir_all(fx.body()).unwrap();
        let g = fx.recording();
        for body in [fx.body(), fx.p("nothing")] {
            for mode in [SyncMode::Full, SyncMode::FetchOnly] {
                let r = sync(&body, mode, &g);
                assert!(!r.fetched, "{r:?}");
                assert_eq!(r.error.as_deref(), Some(NOT_A_REPO));
                assert_eq!(r.warnings.len(), 1, "{r:?}");
                assert_eq!(r.action, None);
            }
        }
        assert!(fx.log().is_empty(), "{:?}", fx.log());
        // A corrupt `.git` (git then treats it as no repository and would search upward; the
        // ceiling stops it at body/, and the crate's own repository above is never reached).
        fs::create_dir_all(fx.p("body/.git")).unwrap();
        fx.write("body/.git/HEAD", "garbage\n");
        let r = fx.sync(SyncMode::Full);
        assert!(!r.fetched, "{r:?}");
        assert!(
            r.error.as_deref().unwrap().contains("not a git repository"),
            "{r:?}"
        );
        // An unrunnable git is a warning too.
        let g = GitEnv {
            git: fx.p("bin/no-such-git"),
            ..fx.genv()
        };
        let r = sync(&fx.body(), SyncMode::Full, &g);
        assert!(r.error.as_deref().unwrap().contains("cannot run"), "{r:?}");
    }

    #[test]
    fn summary_is_one_bounded_line() {
        assert_eq!(summary("a\n\n  b  \n"), "a | b");
        assert_eq!(summary(""), "(no output)");
        let long = "é".repeat(500);
        let s = summary(&long);
        assert!(s.len() <= OUTPUT_MAX + '…'.len_utf8());
        assert!(s.ends_with('…'));
    }
}
