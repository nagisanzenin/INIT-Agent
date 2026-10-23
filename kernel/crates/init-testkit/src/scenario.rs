//! The scenario runner (testing.md §3.6): JSON scenario files executed by [`run_scenario`], each
//! also a `#[test]` through [`scenario_test!`](crate::scenario_test).
//!
//! **Three ways to start** (one per scenario):
//! - `start_kernel`: a kernel under `init-fakeroot` (§3.5b). The kernel is `fixture.kernel`
//!   (`"fake"` = `init-fakekernel`, the default; `"real"` = `init-kernel`, from `$INIT_KERNEL_BIN`).
//!   Such a scenario may be `"env": "live"`.
//! - `start_body`: the runner itself plays the kernel for one body (`fixture.body`: `"fake"` =
//!   `init-fakebody`, the default; `"real"` = `$INIT_BODY_BIN`), speaking stdio-jsonl/1 with the
//!   supervise.md §4.1 environment. With a `mock` block the mock serves `INIT_PROXY_SOCK` in socket
//!   mode. `"env": "live"` is allowed.
//! - `start_root`: the real `init-root` (`$INIT_ROOT_BIN`) in test mode. Always
//!   `"env": "disposable"`; a file that says otherwise is rejected.
//!
//! **The live/disposable guard** ([`crate::guard`]) runs before anything is created or spawned:
//! a disposable scenario reports `skipped: needs a disposable environment` (never `passed`) unless
//! `INIT_TEST_ENV=disposable` and the root-owned sentinel agree.
//!
//! **Steps** (`"op"`): `start_kernel`, `start_body`, `start_root` (each with optional `fault` →
//! `INIT_FAULT`, `env`, and `owner_sock_timeout_s`, default [`OWNER_SOCK_WAIT_S`]), `wait_file`,
//! `wait_socket` (`path`, `timeout_s` default 60, `serving`: see [`wait_socket`] and
//! [`wait_serving`]; `wait_file` reads its path, so it cannot wait for a socket), `send_user`,
//! `interrupt`, `send_frame`, `wait_frame`,
//! `wait_event`, `wait_root_log`, `wait_job`, `wait_exit`, `stop_kernel`, `stop_body`, the Root steps
//! `root_cmd` (approval-type commands are refused, testing.md §1 property 4), `panic` (`channel`:
//! `socket`, `cli`, `latch`, `signal`), `clock` (`advance_s`), `kill_root` (`signal`), and the
//! fake-Root helpers `root_level`, `root_event`, `root_request`; plus `sleep`.
//! `wait_event`, `wait_frame` and `wait_root_log` keep a cursor: each match must come after the
//! previous one of the same kind (file order).
//!
//! **Expectations**: `events_in_order`, `events_absent`, `files` (`root:` prefix for `ROOT_HOME`),
//! `git`, `g4`, `mock`, `processes`, `r4` (`verify`, `records_in_order`), `level`, and the WP3
//! additions `link_in_order`/`link_absent` (the fake Root's link log), `body_frames_in_order`,
//! `exit`. Matching follows [`crate::matcher`].
//!
//! Every scenario runs in a fresh home under `$INIT_TEST_SCRATCH/<id>-<pid>-<hex>/` (default
//! `$TMPDIR/init-tests`), deleted by exact path on success and kept on failure.

use crate::fakeroot::OwnerClient;
use crate::matcher::{self, Captures};
use crate::mockllm::http::{Listen, MockServer};
use crate::mockllm::{self, Mock, MockConfig, Script};
use crate::{guard, sys, util};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn default_timeout() -> f64 {
    30.0
}

/// How long `start_kernel` and `start_root` wait for the Root's `run/owner.sock`, unless the step
/// sets `owner_sock_timeout_s` (FIX6: 10 s was too short while the fake Root SHA-256es a 154 MB
/// debug kernel at start).
pub const OWNER_SOCK_WAIT_S: f64 = 60.0;

fn default_socket_timeout() -> f64 {
    60.0
}

/// A scenario file.
#[derive(Clone, Debug, Deserialize)]
pub struct Scenario {
    pub v: u32,
    pub id: String,
    #[serde(default)]
    pub title: String,
    pub env: String,
    #[serde(default)]
    pub fixture: Fixture,
    #[serde(default)]
    pub kernel_config: Option<Value>,
    #[serde(default)]
    pub mock: Option<MockSpec>,
    /// The real Root's block (charter, ledger, sealed fixtures; testing.md §3.4.6).
    #[serde(default)]
    pub root: Option<Value>,
    /// The fake Root's options.
    #[serde(default)]
    pub fakeroot: Option<FakeRootSpec>,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub expect: Expect,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Fixture {
    /// A template INIT_HOME for `init-kernel install --template` (real kernel only).
    pub home: Option<String>,
    /// `fake` (default) or `real`.
    pub body: Option<String>,
    /// `fake` (default) or `real`.
    pub kernel: Option<String>,
    /// `{"<dest under INIT_HOME/work>": "<source path>"}` copied before start.
    #[serde(default)]
    pub work: BTreeMap<String, String>,
    pub fakebody_mode: Option<String>,
    pub fakekernel_mode: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct MockSpec {
    pub script: Option<String>,
    pub tasks: Option<String>,
    #[serde(default)]
    pub flags: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct FakeRootSpec {
    pub mode: Option<String>,
    pub charter: Option<String>,
}

/// One step.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    StartKernel {
        fault: Option<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        /// Default [`OWNER_SOCK_WAIT_S`].
        owner_sock_timeout_s: Option<f64>,
    },
    StartBody {
        fault: Option<String>,
        mode: Option<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    StartRoot {
        fault: Option<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        /// Default [`OWNER_SOCK_WAIT_S`].
        owner_sock_timeout_s: Option<f64>,
    },
    /// Wait for a Unix socket to accept a connection ([`wait_socket`]); with `serving`, also for
    /// an initctl server behind it to answer `ping` itself ([`wait_serving`]).
    WaitSocket {
        path: String,
        #[serde(default = "default_socket_timeout")]
        timeout_s: f64,
        #[serde(default)]
        serving: bool,
    },
    WaitFile {
        path: String,
        #[serde(default = "default_timeout")]
        timeout_s: f64,
        json: Option<Value>,
        #[serde(default)]
        absent: bool,
    },
    SendUser {
        text: String,
    },
    Interrupt,
    SendFrame {
        frame: Value,
    },
    WaitFrame {
        #[serde(rename = "match")]
        m: Value,
        #[serde(default = "default_timeout")]
        timeout_s: f64,
        capture: Option<Value>,
    },
    WaitEvent {
        #[serde(rename = "match")]
        m: Value,
        #[serde(default = "default_timeout")]
        timeout_s: f64,
        capture: Option<Value>,
    },
    WaitRootLog {
        #[serde(rename = "match")]
        m: Value,
        #[serde(default = "default_timeout")]
        timeout_s: f64,
        capture: Option<Value>,
    },
    WaitJob {
        cmd: String,
        #[serde(default)]
        terminal: bool,
        #[serde(default = "default_timeout")]
        timeout_s: f64,
    },
    WaitExit {
        expect_exit: Option<i32>,
        #[serde(default = "default_timeout")]
        timeout_s: f64,
    },
    StopKernel {
        expect_exit: Option<i32>,
        #[serde(default)]
        grace_ms: Option<u64>,
    },
    StopBody {
        expect_exit: Option<i32>,
        #[serde(default)]
        grace_ms: Option<u64>,
    },
    RootCmd {
        /// CLI words (`["panic"]`, `["test", "clock", "--advance", "5"]`).
        #[serde(default)]
        argv: Vec<String>,
        /// Or an owner-socket request (`cmd` plus `args`).
        cmd: Option<String>,
        #[serde(default)]
        args: Value,
        expect_ok: Option<bool>,
    },
    Panic {
        channel: String,
    },
    Clock {
        advance_s: u64,
    },
    KillRoot {
        #[serde(default = "default_signal")]
        signal: String,
    },
    RootLevel {
        to: u8,
    },
    RootEvent {
        ev: String,
        #[serde(default)]
        data: Value,
    },
    RootRequest {
        cmd: String,
        #[serde(default)]
        args: Value,
        expect_ok: Option<bool>,
        capture: Option<Value>,
    },
    Sleep {
        ms: u64,
    },
}

fn default_signal() -> String {
    "KILL".into()
}

impl Step {
    fn name(&self) -> &'static str {
        match self {
            Step::StartKernel { .. } => "start_kernel",
            Step::StartBody { .. } => "start_body",
            Step::StartRoot { .. } => "start_root",
            Step::WaitFile { .. } => "wait_file",
            Step::WaitSocket { .. } => "wait_socket",
            Step::SendUser { .. } => "send_user",
            Step::Interrupt => "interrupt",
            Step::SendFrame { .. } => "send_frame",
            Step::WaitFrame { .. } => "wait_frame",
            Step::WaitEvent { .. } => "wait_event",
            Step::WaitRootLog { .. } => "wait_root_log",
            Step::WaitJob { .. } => "wait_job",
            Step::WaitExit { .. } => "wait_exit",
            Step::StopKernel { .. } => "stop_kernel",
            Step::StopBody { .. } => "stop_body",
            Step::RootCmd { .. } => "root_cmd",
            Step::Panic { .. } => "panic",
            Step::Clock { .. } => "clock",
            Step::KillRoot { .. } => "kill_root",
            Step::RootLevel { .. } => "root_level",
            Step::RootEvent { .. } => "root_event",
            Step::RootRequest { .. } => "root_request",
            Step::Sleep { .. } => "sleep",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct FileExpect {
    pub path: String,
    pub exists: Option<bool>,
    pub json: Option<Value>,
    pub contains: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct GitExpect {
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub exists: Option<bool>,
    pub notes: Option<String>,
    pub on: Option<String>,
    pub jsonl_contains: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct MockExpect {
    pub expect_failures: Option<usize>,
    pub protocol_errors: Option<usize>,
    pub exhausted: Option<bool>,
    pub prefix_constant: Option<bool>,
    pub requests: Option<usize>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProcExpect {
    pub bodies_running_at_end: Option<u64>,
    pub agent_zone_running: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct R4Expect {
    #[serde(default)]
    pub verify: bool,
    #[serde(default)]
    pub records_in_order: Vec<Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Expect {
    #[serde(default)]
    pub events_in_order: Vec<Value>,
    #[serde(default)]
    pub events_absent: Vec<Value>,
    #[serde(default)]
    pub files: Vec<FileExpect>,
    #[serde(default)]
    pub git: Vec<GitExpect>,
    pub g4: Option<Value>,
    pub mock: Option<MockExpect>,
    pub processes: Option<ProcExpect>,
    pub r4: Option<R4Expect>,
    pub level: Option<u8>,
    #[serde(default)]
    pub link_in_order: Vec<Value>,
    #[serde(default)]
    pub link_absent: Vec<Value>,
    #[serde(default)]
    pub body_frames_in_order: Vec<Value>,
    pub exit: Option<i32>,
}

/// How a scenario ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    /// Not run (e.g. `skipped: needs a disposable environment`). Never counts as passed.
    Skipped(String),
    Failed(String),
}

/// The result of one run.
#[derive(Clone, Debug)]
pub struct Report {
    pub id: String,
    pub outcome: Outcome,
    /// Kept on failure.
    pub scratch: Option<PathBuf>,
    pub captures: Captures,
    pub elapsed: Duration,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.outcome {
            Outcome::Passed => write!(f, "{}: passed in {:.1?}", self.id, self.elapsed),
            Outcome::Skipped(r) => write!(f, "{}: {r}", self.id),
            Outcome::Failed(m) => write!(
                f,
                "{}: FAILED: {m}\nscratch kept at {:?}",
                self.id, self.scratch
            ),
        }
    }
}

/// Runner options.
#[derive(Clone, Debug, Default)]
pub struct RunOpts {
    /// The `INIT_TEST_ENV` value the guard sees (default: this process's). The sentinel is always
    /// the real, fixed one, so this cannot make a live account disposable.
    pub test_env: Option<String>,
    /// Override `$INIT_TEST_SCRATCH`.
    pub scratch_root: Option<PathBuf>,
    /// Keep the scratch directory even on success.
    pub keep: bool,
}

/// Run a scenario file with default options.
pub fn run_scenario(path: impl AsRef<Path>) -> Report {
    run_scenario_with(path.as_ref(), &RunOpts::default())
}

/// Words that would be an approval step; v0.3 has none (D4), and the runner refuses them.
const APPROVALS: [&str; 4] = ["approve", "deny", "clear-safe", "clear_safe"];

fn is_approval(word: &str) -> bool {
    let w = word.trim_start_matches(['/', '-']).to_ascii_lowercase();
    APPROVALS.contains(&w.as_str())
}

/// Static checks before anything runs.
pub fn validate(sc: &Scenario) -> Result<(), String> {
    if sc.v != 1 {
        return Err(format!("scenario v={} (this runner reads v=1)", sc.v));
    }
    if sc.env != "live" && sc.env != "disposable" {
        return Err(format!(
            "env must be \"live\" or \"disposable\", not {:?}",
            sc.env
        ));
    }
    let starts: Vec<&str> = sc
        .steps
        .iter()
        .filter(|s| {
            matches!(
                s,
                Step::StartKernel { .. } | Step::StartBody { .. } | Step::StartRoot { .. }
            )
        })
        .map(Step::name)
        .collect();
    if starts.contains(&"start_root") && sc.env != "disposable" {
        return Err("a scenario that starts the real init-root must be \"env\": \"disposable\" (testing.md §3.1a)".into());
    }
    let mut kinds: Vec<&str> = starts.clone();
    kinds.dedup();
    if kinds.len() > 1 {
        return Err(format!("one start kind per scenario, got {kinds:?}"));
    }
    for s in &sc.steps {
        if let Step::RootCmd { argv, cmd, .. } = s
            && (argv.iter().any(|a| is_approval(a)) || cmd.as_deref().is_some_and(is_approval))
        {
            return Err(
                "root_cmd: approval-type commands do not exist in v0.3 (testing.md §1 property 4)"
                    .into(),
            );
        }
        if let Step::SendUser { text } = s
            && text
                .split_whitespace()
                .next()
                .is_some_and(|w| w.starts_with('/') && is_approval(w))
        {
            return Err("send_user: approval-type owner lines do not exist in v0.3 (D4)".into());
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RootKind {
    None,
    Fake,
    Real,
}

struct BodyProc {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: Arc<Mutex<Vec<Value>>>,
    exit: Option<Option<i32>>,
}

struct Ctx {
    sc: Scenario,
    base: PathBuf,
    scratch: PathBuf,
    home: PathBuf,
    rh: PathBuf,
    mock: Option<MockServer>,
    root: Option<Child>,
    root_kind: RootKind,
    body: Option<BodyProc>,
    caps: Captures,
    ev_cursor: usize,
    frame_cursor: usize,
    link_cursor: usize,
    last_status: Option<Value>,
    kernel_exit: Option<Value>,
}

type R<T = ()> = Result<T, String>;

/// Run a scenario file.
pub fn run_scenario_with(path: &Path, opts: &RunOpts) -> Report {
    let t0 = Instant::now();
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            return report(
                path.display().to_string(),
                Outcome::Failed(format!("{}: {e}", path.display())),
                None,
                t0,
            );
        }
    };
    let sc: Scenario = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => {
            return report(
                path.display().to_string(),
                Outcome::Failed(format!("{}: {e}", path.display())),
                None,
                t0,
            );
        }
    };
    let id = sc.id.clone();
    if let Err(e) = validate(&sc) {
        return report(id, Outcome::Failed(e), None, t0);
    }
    // The guard runs before anything is created or spawned.
    if sc.env == "disposable" {
        let env = opts
            .test_env
            .clone()
            .or_else(|| std::env::var(guard::ENV_VAR).ok());
        let v = guard::check_with_env(env.as_deref());
        if !v.is_disposable() {
            return report(id, Outcome::Skipped(v.skip_message()), None, t0);
        }
    }
    let root = opts
        .scratch_root
        .clone()
        .or_else(|| std::env::var_os("INIT_TEST_SCRATCH").map(PathBuf::from))
        .unwrap_or_else(|| std::env::temp_dir().join("init-tests"));
    let _ = fs::create_dir_all(&root);
    let root = root.canonicalize().unwrap_or(root);
    let short: String = id
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(12)
        .collect();
    let scratch = root.join(format!(
        "{short}-{}-{}",
        std::process::id(),
        util::random_hex(2)
    ));
    if let Err(e) = fs::create_dir_all(&scratch) {
        return report(
            id,
            Outcome::Failed(format!("{}: {e}", scratch.display())),
            None,
            t0,
        );
    }
    let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut cx = Ctx {
        sc,
        base,
        home: scratch.join("h"),
        rh: scratch.join("r"),
        scratch: scratch.clone(),
        mock: None,
        root: None,
        root_kind: RootKind::None,
        body: None,
        caps: Captures::new(),
        ev_cursor: 0,
        frame_cursor: 0,
        link_cursor: 0,
        last_status: None,
        kernel_exit: None,
    };
    let res = cx.run();
    let res = match res {
        Ok(()) => cx.final_status().and_then(|_| cx.check_expectations()),
        Err(e) => Err(e),
    };
    let diag = if res.is_err() {
        cx.diagnostics()
    } else {
        String::new()
    };
    let cleanup = cx.cleanup();
    let outcome = match (res, cleanup) {
        (Ok(()), Ok(())) => Outcome::Passed,
        (Ok(()), Err(e)) => Outcome::Failed(format!("cleanup: {e}")),
        (Err(e), _) => Outcome::Failed(format!("{e}\n{diag}")),
    };
    let keep = opts.keep || outcome != Outcome::Passed;
    if !keep {
        let _ = fs::remove_dir_all(&scratch);
    }
    let mut r = report(id, outcome, keep.then_some(scratch), t0);
    r.captures = cx.caps.clone();
    r
}

fn report(id: String, outcome: Outcome, scratch: Option<PathBuf>, t0: Instant) -> Report {
    Report {
        id,
        outcome,
        scratch,
        captures: Captures::new(),
        elapsed: t0.elapsed(),
    }
}

fn secs(s: f64) -> Duration {
    Duration::from_millis((s.max(0.0) * 1000.0) as u64)
}

impl Ctx {
    fn resolve(&self, p: &str) -> PathBuf {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            return pb;
        }
        let a = self.base.join(p);
        if a.exists() {
            return a;
        }
        self.base
            .parent()
            .map(|b| b.join(p))
            .filter(|x| x.exists())
            .unwrap_or(a)
    }

    fn bin(&self, name: &str) -> R<PathBuf> {
        util::bin_path(name).ok_or_else(|| {
            let short = name.strip_prefix("init-").unwrap_or(name);
            format!(
                "binary `{name}` not found: build it or set INIT_{}_BIN",
                short.to_ascii_uppercase().replace('-', "_")
            )
        })
    }

    fn run(&mut self) -> R {
        for (i, step) in self.sc.steps.clone().into_iter().enumerate() {
            let name = step.name();
            self.step(step)
                .map_err(|e| format!("step {} ({name}): {e}", i + 1))?;
        }
        Ok(())
    }

    fn start_mock(&mut self, mode: mockllm::Mode, listen: Listen) -> R {
        let Some(spec) = self.sc.mock.clone() else {
            return Ok(());
        };
        let script = match &spec.script {
            Some(s) => Script::load(&self.resolve(s))?,
            None => Script::empty(),
        };
        let mut cfg = MockConfig::new(mode, script);
        cfg.record_dir = Some(self.scratch.join("m"));
        cfg.tasks_dir = spec.tasks.as_ref().map(|t| self.resolve(t));
        let mut it = spec.flags.iter();
        while let Some(f) = it.next() {
            match f.as_str() {
                "--strict" => {
                    if it.next().map(String::as_str) == Some("deepseek") {
                        cfg.strict_deepseek = true;
                    }
                }
                "--simulate-cache" => cfg.simulate_cache = true,
                "--no-probe-responder" => cfg.probe_responder = false,
                "--no-g3-responder" => cfg.g3_responder = false,
                "--record-bodies" => {
                    cfg.record_bodies = it.next().map(String::as_str) != Some("none")
                }
                other => {
                    return Err(format!(
                        "mock flag `{other}` is not supported by the runner"
                    ));
                }
            }
        }
        let mock = Mock::new(cfg).map_err(|e| e.to_string())?;
        self.mock = Some(MockServer::start(mock, listen).map_err(|e| format!("mock: {e}"))?);
        Ok(())
    }

    fn prepare_home(&mut self, body_kind: &str) -> R {
        for d in [
            "bin",
            "state/run",
            "state/sessions",
            "state/kernel",
            "work/home",
            "work/tmp",
            "run",
            "kernel/bin",
        ] {
            fs::create_dir_all(self.home.join(d)).map_err(|e| e.to_string())?;
        }
        let body = match body_kind {
            "fake" => self.bin("init-fakebody")?,
            "real" => self.bin("init-body")?,
            other => return Err(format!("fixture.body {other:?}: expected fake or real")),
        };
        let dest = self.home.join("bin/init-body");
        if !dest.exists() {
            fs::copy(&body, &dest).map_err(|e| format!("install body: {e}"))?;
        }
        for (dst, src) in self.sc.fixture.work.clone() {
            let s = self.resolve(&src);
            let d = self.home.join("work").join(&dst);
            copy_tree(&s, &d).map_err(|e| format!("work {src} → {dst}: {e}"))?;
        }
        Ok(())
    }

    fn step(&mut self, step: Step) -> R {
        match step {
            Step::StartKernel {
                fault,
                env,
                owner_sock_timeout_s,
            } => self.start_kernel(fault, env, owner_sock_timeout_s),
            Step::StartBody { fault, mode, env } => self.start_body(fault, mode, env),
            Step::StartRoot {
                fault,
                env,
                owner_sock_timeout_s,
            } => self.start_root(fault, env, owner_sock_timeout_s),
            Step::WaitSocket {
                path,
                timeout_s,
                serving,
            } => {
                let p = self.path(&path);
                let r = if serving {
                    wait_serving(&p, secs(timeout_s))
                } else {
                    wait_socket(&p, secs(timeout_s))
                };
                r.map(|_| ())
                    .map_err(|e| format!("wait_socket {path}: {e}"))
            }
            Step::WaitFile {
                path,
                timeout_s,
                json,
                absent,
            } => {
                let p = self.path(&path);
                let found = util::poll(secs(timeout_s), Duration::from_millis(50), || {
                    if absent {
                        return (!p.exists()).then_some(());
                    }
                    let b = fs::read(&p).ok()?;
                    match &json {
                        None => Some(()),
                        Some(m) => {
                            let v: Value = serde_json::from_slice(&b).ok()?;
                            matcher::matches(&matcher::substitute(m, &self.caps), &v).then_some(())
                        }
                    }
                });
                found.ok_or_else(|| {
                    format!(
                        "timed out after {timeout_s} s waiting for {} {}",
                        path,
                        if absent { "to disappear" } else { "" }
                    )
                })
            }
            Step::SendUser { text } => self
                .owner()?
                .frame(&json!({"t": "line", "text": text}))
                .map_err(|e| e.to_string()),
            Step::Interrupt => {
                if self.body.is_some() {
                    self.write_frame(json!({"t": "interrupt"}))
                } else {
                    self.owner()?
                        .frame(&json!({"t": "interrupt"}))
                        .map_err(|e| e.to_string())
                }
            }
            Step::SendFrame { frame } => self.write_frame(matcher::substitute(&frame, &self.caps)),
            Step::WaitFrame {
                m,
                timeout_s,
                capture,
            } => {
                let m = matcher::substitute(&m, &self.caps);
                let frames = self
                    .body
                    .as_ref()
                    .ok_or("no body started (start_body)")?
                    .frames
                    .clone();
                let start = self.frame_cursor;
                let hit = util::poll(secs(timeout_s), Duration::from_millis(20), || {
                    let f = frames.lock().unwrap_or_else(|e| e.into_inner());
                    f.iter()
                        .enumerate()
                        .skip(start)
                        .find(|(_, x)| matcher::matches(&m, x))
                        .map(|(i, x)| (i, x.clone()))
                });
                let (i, v) = hit.ok_or_else(|| {
                    format!("timed out after {timeout_s} s waiting for frame {m}")
                })?;
                self.frame_cursor = i + 1;
                if let Some(c) = capture {
                    matcher::capture(&c, &v, &mut self.caps)?;
                }
                Ok(())
            }
            Step::WaitEvent {
                m,
                timeout_s,
                capture,
            } => {
                let m = matcher::substitute(&m, &self.caps);
                let start = self.ev_cursor;
                let hit = util::poll(secs(timeout_s), Duration::from_millis(50), || {
                    self.events()
                        .into_iter()
                        .enumerate()
                        .skip(start)
                        .find(|(_, e)| matcher::matches(&m, e))
                });
                let (i, v) = hit.ok_or_else(|| {
                    format!("timed out after {timeout_s} s waiting for event {m}")
                })?;
                self.ev_cursor = i + 1;
                if let Some(c) = capture {
                    matcher::capture(&c, &v, &mut self.caps)?;
                }
                Ok(())
            }
            Step::WaitRootLog {
                m,
                timeout_s,
                capture,
            } => {
                let m = matcher::substitute(&m, &self.caps);
                let start = self.link_cursor;
                let hit = util::poll(secs(timeout_s), Duration::from_millis(50), || {
                    self.root_log()
                        .into_iter()
                        .enumerate()
                        .skip(start)
                        .find(|(_, e)| matcher::matches(&m, e))
                });
                let (i, v) = hit.ok_or_else(|| {
                    format!("timed out after {timeout_s} s waiting for root log {m}")
                })?;
                self.link_cursor = i + 1;
                if let Some(c) = capture {
                    matcher::capture(&c, &v, &mut self.caps)?;
                }
                Ok(())
            }
            Step::WaitJob {
                cmd,
                terminal,
                timeout_s,
            } => {
                let dir = self.home.join(init_proto::files::paths::JOBS_DIR);
                let hit = util::poll(secs(timeout_s), Duration::from_millis(200), || {
                    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
                        .ok()?
                        .flatten()
                        .map(|e| e.path())
                        .collect();
                    files.sort();
                    files.iter().rev().find_map(|f| {
                        let v: Value = serde_json::from_slice(&fs::read(f).ok()?).ok()?;
                        let state = init_proto::initctl::JobState::parse(v["state"].as_str()?);
                        (v["cmd"] == cmd.as_str() && (!terminal || state.is_terminal()))
                            .then_some(v)
                    })
                });
                hit.map(|_| ()).ok_or_else(|| {
                    format!(
                        "timed out after {timeout_s} s waiting for a {cmd} job{}",
                        if terminal { " to finish" } else { "" }
                    )
                })
            }
            Step::WaitExit {
                expect_exit,
                timeout_s,
            } => {
                let code = self.wait_body_exit(secs(timeout_s))?;
                check_exit(expect_exit, code)
            }
            Step::StopBody {
                expect_exit,
                grace_ms,
            } => {
                let g = grace_ms.unwrap_or(5_000);
                self.write_frame(json!({"t": "stop", "reason": "runner", "grace_ms": g}))?;
                let code = self.wait_body_exit(Duration::from_millis(g + 10_000))?;
                check_exit(expect_exit, code)
            }
            Step::StopKernel {
                expect_exit,
                grace_ms,
            } => match self.root_kind {
                RootKind::Fake => {
                    let r = self
                        .owner_request("stop", json!({"grace_ms": grace_ms.unwrap_or(30_000)}))?;
                    let exit = r["result"]["exit"].clone();
                    self.kernel_exit = Some(exit.clone());
                    check_exit(expect_exit, exit["code"].as_i64().map(|c| c as i32))
                }
                RootKind::Real => {
                    let code = self.root_cli(&["stop".into()])?;
                    check_exit(Some(0), Some(code))
                }
                RootKind::None => Err("no kernel started".into()),
            },
            Step::RootCmd {
                argv,
                cmd,
                args,
                expect_ok,
            } => self.root_cmd(argv, cmd, args, expect_ok),
            Step::Panic { channel } => self.panic(&channel),
            Step::Clock { advance_s } => match self.root_kind {
                RootKind::Fake => self
                    .owner_request("test.clock", json!({"advance_ms": advance_s * 1000}))
                    .map(|_| ()),
                RootKind::Real => {
                    let c = self.root_cli(&[
                        "test".into(),
                        "clock".into(),
                        "--advance".into(),
                        advance_s.to_string(),
                    ])?;
                    check_exit(Some(0), Some(c))
                }
                RootKind::None => Err("no Root started".into()),
            },
            Step::KillRoot { signal } => {
                let sig = sys::signal_by_name(&signal)
                    .ok_or_else(|| format!("unknown signal {signal}"))?;
                let child = self.root.as_mut().ok_or("no Root started")?;
                if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                    return Err("the Root already exited".into());
                }
                sys::kill(child.id() as i32, sig).map_err(|e| e.to_string())?;
                if sig == sys::SIGKILL {
                    let _ = child.wait();
                }
                Ok(())
            }
            Step::RootLevel { to } => self
                .owner_request("test.level", json!({"to": to}))
                .map(|_| ()),
            Step::RootEvent { ev, data } => self
                .owner_request(
                    "test.event",
                    json!({"ev": ev, "data": matcher::substitute(&data, &self.caps)}),
                )
                .map(|_| ()),
            Step::RootRequest {
                cmd,
                args,
                expect_ok,
                capture,
            } => {
                let r = self.owner_request_raw(&cmd, matcher::substitute(&args, &self.caps))?;
                if let Some(want) = expect_ok
                    && r["ok"] != want
                {
                    return Err(format!("{cmd}: expected ok={want}, got {r}"));
                }
                if let Some(c) = capture {
                    matcher::capture(&c, &r, &mut self.caps)?;
                }
                Ok(())
            }
            Step::Sleep { ms } => {
                std::thread::sleep(Duration::from_millis(ms));
                Ok(())
            }
        }
    }

    fn path(&self, p: &str) -> PathBuf {
        match p.strip_prefix("root:") {
            Some(r) => self.rh.join(r),
            None => self.home.join(p),
        }
    }

    fn fault_env(cmd: &mut Command, fault: &Option<String>, env: &BTreeMap<String, String>) -> R {
        if let Some(f) = fault {
            crate::fault::Plan::parse(f).map_err(|e| format!("fault: {e}"))?;
            cmd.env("INIT_FAULT", f);
        }
        cmd.envs(env);
        Ok(())
    }

    fn start_kernel(
        &mut self,
        fault: Option<String>,
        env: BTreeMap<String, String>,
        owner_sock_timeout_s: Option<f64>,
    ) -> R {
        if self
            .root
            .as_mut()
            .is_some_and(|c| c.try_wait().ok().flatten().is_none())
        {
            return Err("a Root is already running".into());
        }
        let kernel_kind = self
            .sc
            .fixture
            .kernel
            .clone()
            .unwrap_or_else(|| "fake".into());
        let body_kind = self
            .sc
            .fixture
            .body
            .clone()
            .unwrap_or_else(|| "fake".into());
        let kernel = match kernel_kind.as_str() {
            "fake" => self.bin("init-fakekernel")?,
            "real" => self.bin("init-kernel")?,
            other => return Err(format!("fixture.kernel {other:?}: expected fake or real")),
        };
        if kernel_kind == "real"
            && !self.home.join("kernel").exists()
            && let Some(t) = self.sc.fixture.home.clone()
        {
            let out = crate::watchdog::output(Command::new(&kernel)
                .args(["install", "--template"])
                .arg(self.resolve(&t))
                .arg("--home")
                .arg(&self.home))
                .map_err(|e| format!("init-kernel install: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "init-kernel install --template failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
        }
        self.prepare_home(&body_kind)?;
        if let Some(kc) = self.sc.kernel_config.clone() {
            write_kernel_config(&self.home.join("kernel/config.toml"), &kc)?;
        }
        if self.mock.is_none() {
            self.start_mock(mockllm::Mode::Upstream, Listen::Tcp("127.0.0.1:0".into()))?;
        }
        let fr = self.sc.fakeroot.clone().unwrap_or_default();
        let mut cmd = Command::new(self.bin("init-fakeroot")?);
        cmd.arg("--root-home")
            .arg(&self.rh)
            .arg("--home")
            .arg(&self.home)
            .arg("--kernel")
            .arg(&kernel);
        if let Some(c) = &fr.charter {
            cmd.arg("--charter").arg(self.resolve(c));
        }
        if let Some(m) = &fr.mode {
            cmd.args(["--mode", m]);
        }
        if let Some(port) = self.mock.as_ref().and_then(|m| m.port()) {
            cmd.args(["--upstream", &format!("http://127.0.0.1:{port}")]);
        }
        if let Some(m) = &self.sc.fixture.fakekernel_mode {
            cmd.env("FAKEKERNEL_MODE", m);
        }
        if let Some(m) = &self.sc.fixture.fakebody_mode {
            cmd.env("INIT_TEST_FAKEBODY_MODE", m);
        }
        Self::fault_env(&mut cmd, &fault, &env)?;
        let log =
            fs::File::create(self.scratch.join("fakeroot.stderr")).map_err(|e| e.to_string())?;
        cmd.stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .process_group(0);
        let child = crate::watchdog::spawn(&mut cmd).map_err(|e| format!("init-fakeroot: {e}"))?;
        self.root = Some(child);
        self.root_kind = RootKind::Fake;
        let sock = self.rh.join("run/owner.sock");
        if sock.as_os_str().len() > 103 {
            return Err(format!(
                "{}: socket path longer than 103 bytes; use a shorter INIT_TEST_SCRATCH",
                sock.display()
            ));
        }
        wait_socket(
            &sock,
            secs(owner_sock_timeout_s.unwrap_or(OWNER_SOCK_WAIT_S)),
        )
        .map_err(|e| format!("the fake Root did not serve owner.sock: {e}"))?;
        Ok(())
    }

    fn start_body(
        &mut self,
        fault: Option<String>,
        mode: Option<String>,
        env: BTreeMap<String, String>,
    ) -> R {
        if self
            .body
            .as_mut()
            .is_some_and(|b| b.child.try_wait().ok().flatten().is_none())
        {
            return Err("a body is already running".into());
        }
        let body_kind = self
            .sc
            .fixture
            .body
            .clone()
            .unwrap_or_else(|| "fake".into());
        self.prepare_home(&body_kind)?;
        if self.mock.is_none() {
            let sock = self.home.join("run/proxy.sock");
            if sock.as_os_str().len() > 103 {
                return Err(format!(
                    "{}: socket path longer than 103 bytes",
                    sock.display()
                ));
            }
            self.start_mock(mockllm::Mode::Socket, Listen::Unix(sock))?;
        }
        let session = fs::read_to_string(self.home.join("state/session"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| util::is_session_id(s))
            .unwrap_or_else(|| util::new_session_id(util::now_ms()));
        let reason = if self.body.is_some() { "crash" } else { "cold" };
        let benv = crate::fakekernel::body_env(
            &self.home,
            &"0".repeat(40),
            &util::random_hex(8),
            &session,
            reason,
            "normal",
            "b0",
        );
        let mut cmd = Command::new(self.home.join("bin/init-body"));
        cmd.env_clear()
            .envs(&benv)
            .current_dir(self.home.join("work"));
        if let Some(m) = mode.or_else(|| self.sc.fixture.fakebody_mode.clone()) {
            cmd.env("FAKEBODY_MODE", m);
        }
        Self::fault_env(&mut cmd, &fault, &env)?;
        let err = fs::File::create(self.scratch.join("body.stderr")).map_err(|e| e.to_string())?;
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(err)
            .process_group(0);
        let mut child = crate::watchdog::spawn(&mut cmd).map_err(|e| format!("body: {e}"))?;
        let stdin = child.stdin.take();
        let out = child.stdout.take().ok_or("no stdout")?;
        let frames = Arc::new(Mutex::new(Vec::new()));
        let fr = frames.clone();
        std::thread::spawn(move || {
            let mut r = BufReader::new(out);
            let mut l = Vec::new();
            loop {
                l.clear();
                match r.read_until(b'\n', &mut l) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let v = match init_proto::stdio::decode_from_body(&l) {
                    Ok(f) => serde_json::from_slice::<Value>(
                        &init_proto::stdio::encode_from_body(&f).unwrap_or_default(),
                    )
                    .unwrap_or(Value::Null),
                    Err(e) => {
                        json!({"t": "_bad", "error": e.to_string(), "raw": String::from_utf8_lossy(&l)})
                    }
                };
                fr.lock().unwrap_or_else(|e| e.into_inner()).push(v);
            }
        });
        self.body = Some(BodyProc {
            child,
            stdin,
            frames,
            exit: None,
        });
        Ok(())
    }

    fn start_root(
        &mut self,
        fault: Option<String>,
        env: BTreeMap<String, String>,
        owner_sock_timeout_s: Option<f64>,
    ) -> R {
        // Disposable only (validated before the run). The real Root is driven through its CLI
        // (root/root.md §4.3) with `INIT_ROOT_HOME` pointing at this scenario's ROOT_HOME.
        let root_bin = self.bin("init-root")?;
        let block = self.sc.root.clone().unwrap_or(json!({}));
        let kernel_kind = self
            .sc
            .fixture
            .kernel
            .clone()
            .unwrap_or_else(|| "fake".into());
        self.prepare_home(
            &self
                .sc
                .fixture
                .body
                .clone()
                .unwrap_or_else(|| "fake".into()),
        )?;
        if !self.rh.join("charter.toml").exists() {
            let mut install = Command::new(&root_bin);
            install
                .arg("install")
                .arg("--home")
                .arg(&self.home)
                .arg("--root-home")
                .arg(&self.rh);
            if let Some(c) = block["charter"].as_str() {
                install.arg("--charter").arg(self.resolve(c));
            }
            if let Some(extra) = block["install_args"].as_array() {
                install.args(extra.iter().filter_map(Value::as_str));
            }
            install
                .env("INIT_ROOT_HOME", &self.rh)
                .env("HOME", self.scratch.join("home"));
            let out = crate::watchdog::output(&mut install)
                .map_err(|e| format!("init-root install: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "init-root install failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
        }
        if kernel_kind == "fake" {
            let kb = self.home.join("kernel/bin");
            fs::create_dir_all(&kb).map_err(|e| e.to_string())?;
            fs::copy(self.bin("init-fakekernel")?, kb.join("init-kernel"))
                .map_err(|e| format!("install fake kernel: {e}"))?;
        }
        let mut cmd = Command::new(&root_bin);
        cmd.arg("start")
            .env("INIT_ROOT_HOME", &self.rh)
            .env("HOME", self.scratch.join("home"));
        if let Some(now) = block["now"].as_str() {
            cmd.env("INIT_ROOT_TEST_NOW", now);
        }
        if let Some(m) = &self.sc.fixture.fakekernel_mode {
            cmd.env("INIT_TEST_FAKEKERNEL_MODE", m);
        }
        if let Some(m) = &self.sc.fixture.fakebody_mode {
            cmd.env("INIT_TEST_FAKEBODY_MODE", m);
        }
        Self::fault_env(&mut cmd, &fault, &env)?;
        let log = fs::File::create(self.scratch.join("root.stderr")).map_err(|e| e.to_string())?;
        cmd.stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .process_group(0);
        self.root = Some(crate::watchdog::spawn(&mut cmd).map_err(|e| format!("init-root start: {e}"))?);
        self.root_kind = RootKind::Real;
        let sock = self.rh.join("run/owner.sock");
        let wait = secs(owner_sock_timeout_s.unwrap_or(OWNER_SOCK_WAIT_S));
        util::poll(wait, Duration::from_millis(50), || {
            sock.exists().then_some(())
        })
        .ok_or("init-root did not create owner.sock")?;
        Ok(())
    }

    fn root_cli(&self, argv: &[String]) -> R<i32> {
        let out = crate::watchdog::output(Command::new(self.bin("init-root")?)
            .args(argv)
            .env("INIT_ROOT_HOME", &self.rh)
            .env("HOME", self.scratch.join("home")))
            .map_err(|e| format!("init-root {argv:?}: {e}"))?;
        Ok(out.status.code().unwrap_or(-1))
    }

    fn owner(&self) -> R<OwnerClient> {
        if self.root_kind == RootKind::None {
            return Err("no Root started (start_kernel or start_root)".into());
        }
        OwnerClient::connect(&self.rh.join("run/owner.sock"))
            .map_err(|e| format!("owner.sock: {e}"))
    }

    fn owner_request_raw(&self, cmd: &str, args: Value) -> R<Value> {
        self.owner()?
            .request(cmd, args)
            .map_err(|e| format!("{cmd}: {e}"))
    }

    fn owner_request(&self, cmd: &str, args: Value) -> R<Value> {
        let r = self.owner_request_raw(cmd, args)?;
        if r["ok"] != true {
            return Err(format!("{cmd}: {r}"));
        }
        Ok(r)
    }

    fn root_cmd(
        &mut self,
        argv: Vec<String>,
        cmd: Option<String>,
        args: Value,
        expect_ok: Option<bool>,
    ) -> R {
        let ok = match (self.root_kind, cmd) {
            (RootKind::None, _) => return Err("no Root started".into()),
            (_, Some(c)) => {
                self.owner_request_raw(&c, if args.is_null() { json!({}) } else { args })?["ok"]
                    == true
            }
            (RootKind::Real, None) => self.root_cli(&argv)? == 0,
            (RootKind::Fake, None) => {
                // The fake Root speaks requests, not a CLI: translate the common words.
                let (c, a) = match argv
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .as_slice()
                {
                    [w @ ("status" | "stop" | "start" | "panic" | "resume" | "ledger")] => {
                        (w.to_string(), json!({}))
                    }
                    ["test", "clock", "--advance", s] => (
                        "test.clock".to_string(),
                        json!({"advance_ms": s.parse::<u64>().map_err(|_| "bad --advance")? * 1000}),
                    ),
                    other => {
                        return Err(format!(
                            "root_cmd {other:?} has no fake-Root equivalent; use {{\"cmd\", \"args\"}}"
                        ));
                    }
                };
                self.owner_request_raw(&c, a)?["ok"] == true
            }
        };
        match expect_ok {
            Some(want) if want != ok => Err(format!(
                "root_cmd {argv:?}: expected ok={want}, got ok={ok}"
            )),
            _ => Ok(()),
        }
    }

    fn panic(&mut self, channel: &str) -> R {
        match channel {
            "socket" => self
                .owner()?
                .frame(&json!({"t": "panic"}))
                .map_err(|e| e.to_string())?,
            "cli" => match self.root_kind {
                RootKind::Real => {
                    self.root_cli(&["panic".into()])?;
                }
                _ => {
                    self.owner_request("panic", json!({}))?;
                }
            },
            "latch" => {
                let p = self.rh.join("run/panic");
                fs::write(
                    &p,
                    serde_json::to_vec(&json!({"at": util::ts(), "channel": "latch", "v": 1}))
                        .unwrap_or_default(),
                )
                .map_err(|e| e.to_string())?;
            }
            "signal" => {
                let c = self.root.as_ref().ok_or("no Root started")?;
                sys::kill(c.id() as i32, sys::SIGUSR1).map_err(|e| e.to_string())?;
            }
            other => {
                return Err(format!(
                    "panic channel {other:?}: socket | cli | latch | signal"
                ));
            }
        }
        if self.root_kind == RootKind::Fake {
            util::poll(Duration::from_secs(5), Duration::from_millis(20), || {
                self.owner_request("status", json!({}))
                    .ok()
                    .filter(|s| s["result"]["slot"] == "panicked")
                    .map(|_| ())
            })
            .ok_or("the fake Root did not reach panicked")?;
        } else {
            std::thread::sleep(Duration::from_millis(1500));
        }
        Ok(())
    }

    fn write_frame(&mut self, mut frame: Value) -> R {
        if frame.get("v").is_none() {
            frame["v"] = json!(1);
        }
        let line = serde_json::to_vec(&frame).map_err(|e| e.to_string())?;
        init_proto::stdio::decode_to_body(&line)
            .map_err(|e| format!("not a kernel→body frame: {e}: {frame}"))?;
        let b = self.body.as_mut().ok_or("no body started (start_body)")?;
        let w = b.stdin.as_mut().ok_or("body stdin closed")?;
        w.write_all(&line)
            .and_then(|_| w.write_all(b"\n"))
            .and_then(|_| w.flush())
            .map_err(|e| format!("write to body: {e}"))
    }

    fn wait_body_exit(&mut self, t: Duration) -> R<Option<i32>> {
        let b = self.body.as_mut().ok_or("no body started")?;
        if let Some(c) = b.exit {
            return Ok(c);
        }
        let deadline = Instant::now() + t;
        loop {
            if let Some(s) = b.child.try_wait().map_err(|e| e.to_string())? {
                b.exit = Some(s.code());
                return Ok(s.code());
            }
            if Instant::now() >= deadline {
                return Err(format!("the body did not exit within {t:?}"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn events(&self) -> Vec<Value> {
        let mut files: Vec<PathBuf> = fs::read_dir(self.home.join("state/sessions"))
            .map(|d| {
                d.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files.iter().flat_map(|f| util::read_jsonl(f)).collect()
    }

    fn root_log(&self) -> Vec<Value> {
        util::read_jsonl(&self.rh.join("logs/fakeroot.jsonl"))
    }

    fn final_status(&mut self) -> R {
        if self.root_kind == RootKind::Fake
            && self
                .root
                .as_mut()
                .is_some_and(|c| c.try_wait().ok().flatten().is_none())
        {
            self.last_status = self
                .owner_request("status", json!({}))
                .ok()
                .map(|r| r["result"].clone());
        }
        if self.root_kind == RootKind::Fake && self.last_status.is_none() {
            self.last_status = fs::read(self.rh.join("state/root.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
        }
        Ok(())
    }

    fn check_expectations(&mut self) -> R {
        let ex = self.sc.expect.clone();
        let caps = self.caps.clone();
        if !ex.events_in_order.is_empty() || !ex.events_absent.is_empty() {
            let evs = self.events();
            matcher::in_order(&ex.events_in_order, &evs, &caps).map_err(|i| {
                format!(
                    "events_in_order: no match for #{} {}",
                    i + 1,
                    ex.events_in_order[i]
                )
            })?;
            for a in &ex.events_absent {
                let m = matcher::substitute(a, &caps);
                if let Some(e) = evs.iter().find(|e| matcher::matches(&m, e)) {
                    return Err(format!("events_absent: {a} matched {e}"));
                }
            }
        }
        if !ex.link_in_order.is_empty() || !ex.link_absent.is_empty() {
            let log = self.root_log();
            matcher::in_order(&ex.link_in_order, &log, &caps).map_err(|i| {
                format!(
                    "link_in_order: no match for #{} {}",
                    i + 1,
                    ex.link_in_order[i]
                )
            })?;
            for a in &ex.link_absent {
                let m = matcher::substitute(a, &caps);
                if let Some(e) = log.iter().find(|e| matcher::matches(&m, e)) {
                    return Err(format!("link_absent: {a} matched {e}"));
                }
            }
        }
        if !ex.body_frames_in_order.is_empty() {
            let frames = self
                .body
                .as_ref()
                .ok_or("body_frames_in_order needs start_body")?
                .frames
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            matcher::in_order(&ex.body_frames_in_order, &frames, &caps).map_err(|i| {
                format!(
                    "body_frames_in_order: no match for #{} {}",
                    i + 1,
                    ex.body_frames_in_order[i]
                )
            })?;
        }
        if let Some(want) = ex.exit {
            let got = match (self.body.is_some(), &self.kernel_exit) {
                (true, _) => self.wait_body_exit(Duration::from_secs(10))?,
                (false, Some(k)) => k["code"].as_i64().map(|c| c as i32),
                _ => return Err("exit: nothing exited".into()),
            };
            check_exit(Some(want), got)?;
        }
        for f in &ex.files {
            let p = self.path(
                matcher::substitute(&json!(f.path), &caps)
                    .as_str()
                    .unwrap_or(&f.path),
            );
            let exists = p.exists();
            if let Some(e) = f.exists
                && e != exists
            {
                return Err(format!("files: {} exists={exists}, expected {e}", f.path));
            }
            if let Some(m) = &f.json {
                let v: Value = serde_json::from_slice(
                    &fs::read(&p).map_err(|e| format!("files: {}: {e}", f.path))?,
                )
                .map_err(|e| format!("files: {}: {e}", f.path))?;
                if !matcher::matches(&matcher::substitute(m, &caps), &v) {
                    return Err(format!("files: {} = {v} does not match {m}", f.path));
                }
            }
            if let Some(c) = &f.contains {
                let t = fs::read_to_string(&p).map_err(|e| format!("files: {}: {e}", f.path))?;
                if !t.contains(c.as_str()) {
                    return Err(format!("files: {} does not contain {c:?}", f.path));
                }
            }
        }
        for g in &ex.git {
            self.check_git(g, &caps)?;
        }
        if let Some(g4) = &ex.g4
            && g4["verdict_consistent"] == true
        {
            self.check_g4()?;
        }
        if let Some(m) = &ex.mock {
            let lines = self
                .mock
                .as_ref()
                .map(|s| s.mock().log())
                .unwrap_or_default();
            let sum = mockllm::summarize(&lines);
            let cmp = |name: &str, want: Option<usize>, got: usize| match want {
                Some(w) if w != got => Err(format!("mock.{name}: {got}, expected {w}")),
                _ => Ok(()),
            };
            cmp("expect_failures", m.expect_failures, sum.expect_failures)?;
            cmp("protocol_errors", m.protocol_errors, sum.protocol_errors)?;
            cmp("requests", m.requests, sum.requests)?;
            if let Some(e) = m.exhausted
                && e != sum.exhausted
            {
                return Err(format!("mock.exhausted: {}, expected {e}", sum.exhausted));
            }
            if m.prefix_constant == Some(true) {
                let mut sessions: Vec<&str> = lines.iter().map(|l| l.session.as_str()).collect();
                sessions.dedup();
                if let Some(s) = sessions
                    .iter()
                    .find(|s| !mockllm::prefix_constant(&lines, s))
                {
                    return Err(format!(
                        "mock.prefix_constant: session {s} changed its prefix"
                    ));
                }
            }
        }
        if let Some(p) = &ex.processes {
            let (bodies, zone) = match self.root_kind {
                RootKind::Fake => {
                    let s = self.last_status.clone().unwrap_or(Value::Null);
                    (
                        s["bodies"].as_u64().unwrap_or(0),
                        !s["kernel"].is_null()
                            || s["instances"].as_array().is_some_and(|a| !a.is_empty()),
                    )
                }
                RootKind::None => {
                    let alive = self
                        .body
                        .as_mut()
                        .is_some_and(|b| b.child.try_wait().ok().flatten().is_none());
                    (u64::from(alive), alive)
                }
                RootKind::Real => {
                    let out = crate::watchdog::output(Command::new(self.bin("init-root")?)
                        .args(["status", "--json"])
                        .env("INIT_ROOT_HOME", &self.rh))
                        .map_err(|e| e.to_string())?;
                    let s: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
                    let bodies = s["instances"]
                        .as_array()
                        .map(|a| a.iter().filter(|i| i["kind"] == "body").count() as u64)
                        .unwrap_or(0);
                    (
                        bodies,
                        !s["kernel"].is_null() && !s["kernel"]["pid"].is_null(),
                    )
                }
            };
            if let Some(b) = p.bodies_running_at_end
                && b != bodies
            {
                return Err(format!(
                    "processes.bodies_running_at_end: {bodies}, expected {b}"
                ));
            }
            if let Some(z) = p.agent_zone_running
                && z != zone
            {
                return Err(format!(
                    "processes.agent_zone_running: {zone}, expected {z}"
                ));
            }
        }
        if let Some(r4) = &ex.r4 {
            let recs = util::read_jsonl(&self.rh.join("lineage/lineage.jsonl"));
            if r4.verify {
                verify_chain(&recs)?;
                if self.root_kind == RootKind::Real {
                    let c = self.root_cli(&["audit".into(), "verify".into()])?;
                    check_exit(Some(0), Some(c))
                        .map_err(|e| format!("r4.verify (init-root audit verify): {e}"))?;
                }
            }
            matcher::in_order(&r4.records_in_order, &recs, &caps).map_err(|i| {
                format!(
                    "r4.records_in_order: no match for #{} {}",
                    i + 1,
                    r4.records_in_order[i]
                )
            })?;
        }
        if let Some(l) = ex.level {
            let got = match self.root_kind {
                RootKind::Fake => self.last_status.as_ref().and_then(|s| s["level"].as_u64()),
                RootKind::Real => {
                    let out = crate::watchdog::output(Command::new(self.bin("init-root")?)
                        .args(["status", "--json"])
                        .env("INIT_ROOT_HOME", &self.rh))
                        .map_err(|e| e.to_string())?;
                    let s: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
                    s["level"].as_u64().or(s["ladder"]["level"].as_u64())
                }
                RootKind::None => None,
            };
            if got != Some(l as u64) {
                return Err(format!("level: {got:?}, expected {l}"));
            }
        }
        Ok(())
    }

    fn check_git(&self, g: &GitExpect, caps: &Captures) -> R {
        let sub = |s: &str| {
            matcher::substitute(&json!(s), caps)
                .as_str()
                .unwrap_or(s)
                .to_string()
        };
        let git = |args: &[&str]| -> R<(bool, String)> {
            let out = crate::watchdog::output(Command::new("git")
                .arg(format!("--git-dir={}", self.home.join(&g.repo).display()))
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("HOME", &self.scratch))
                .map_err(|e| format!("git: {e}"))?;
            Ok((
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
            ))
        };
        if let Some(r) = &g.reference {
            let r = sub(r);
            let (ok, _) = git(&["rev-parse", "--verify", "--quiet", &r])?;
            if let Some(want) = g.exists
                && want != ok
            {
                return Err(format!("git: {}:{r} exists={ok}, expected {want}", g.repo));
            }
        }
        if let (Some(notes), Some(on)) = (&g.notes, &g.on) {
            let (ok, text) = git(&["notes", &format!("--ref={notes}"), "show", &sub(on)])?;
            if !ok {
                return Err(format!("git: no {notes} note on {}", sub(on)));
            }
            if let Some(m) = &g.jsonl_contains {
                let m = matcher::substitute(m, caps);
                if !text
                    .lines()
                    .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                    .any(|v| matcher::matches(&m, &v))
                {
                    return Err(format!(
                        "git: the {notes} note on {} has no line matching {m}",
                        sub(on)
                    ));
                }
            }
        }
        Ok(())
    }

    /// testing.md §3.6 `g4.verdict_consistent`, on the job records of `state/kernel/jobs/`.
    fn check_g4(&self) -> R {
        let dir = self.home.join(init_proto::files::paths::JOBS_DIR);
        let jobs: Vec<Value> = fs::read_dir(&dir)
            .map_err(|e| format!("g4: {}: {e}", dir.display()))?
            .flatten()
            .filter_map(|e| serde_json::from_slice(&fs::read(e.path()).ok()?).ok())
            .collect();
        let g4: Vec<&Value> = jobs
            .iter()
            .filter(|j| {
                j["cmd"] == "g4"
                    || j["stages"]
                        .as_array()
                        .is_some_and(|s| s.iter().any(|x| x["stage"] == "G4"))
            })
            .collect();
        let last = g4.last().ok_or("g4: no G4 job record")?;
        let verdict = last["result"]["verdict"]
            .as_str()
            .or(last["verdict"].as_str())
            .unwrap_or("");
        match verdict {
            "accept" => {
                let lkg: Value = serde_json::from_slice(
                    &fs::read(self.home.join(init_proto::files::paths::LKG))
                        .map_err(|e| format!("g4: lkg.json: {e}"))?,
                )
                .map_err(|e| e.to_string())?;
                if lkg["sha"] != last["sha"] {
                    return Err(format!(
                        "g4: accept, but lkg.json.sha {} != candidate {}",
                        lkg["sha"], last["sha"]
                    ));
                }
            }
            "reject" => {
                if !self
                    .events()
                    .iter()
                    .any(|e| e["type"] == "boot" && e["reason"] == "rollback")
                {
                    return Err("g4: reject without a boot{reason:rollback}".into());
                }
            }
            "inconclusive" => {
                if !g4.iter().any(|j| j["args"]["escalated"] == true) {
                    return Err("g4: inconclusive without exactly one escalated re-run".into());
                }
            }
            other => return Err(format!("g4: unknown verdict {other:?}")),
        }
        Ok(())
    }

    fn diagnostics(&self) -> String {
        let mut d = String::new();
        let evs = self.events();
        d.push_str(&format!("--- last {} events ---\n", evs.len().min(50)));
        for e in evs.iter().rev().take(50).rev() {
            d.push_str(&format!("{e}\n"));
        }
        if let Some(m) = &self.mock {
            let l = m.mock().log();
            d.push_str(&format!("--- last {} mock requests ---\n", l.len().min(20)));
            for x in l.iter().rev().take(20).rev() {
                d.push_str(&format!("{}\n", x.to_json()));
            }
        }
        if let Ok(rd) = fs::read_dir(self.home.join(init_proto::files::paths::JOBS_DIR)) {
            d.push_str("--- job records ---\n");
            for e in rd.flatten() {
                d.push_str(&format!(
                    "{}: {}\n",
                    e.path().display(),
                    fs::read_to_string(e.path()).unwrap_or_default()
                ));
            }
        }
        if let Some(s) = &self.last_status {
            d.push_str(&format!("--- fake Root status ---\n{s}\n"));
        }
        let body = self
            .body
            .as_ref()
            .map(|b| b.frames.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .unwrap_or_default();
        if !body.is_empty() {
            d.push_str("--- last body frames ---\n");
            for f in body.iter().rev().take(20).rev() {
                d.push_str(&format!("{f}\n"));
            }
        }
        d
    }

    /// Stop everything this scenario started, by held handles only.
    fn cleanup(&mut self) -> R {
        if let Some(b) = self.body.as_mut()
            && b.child.try_wait().ok().flatten().is_none()
        {
            drop(b.stdin.take()); // EOF: the contract says exit within 10 s
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline && b.child.try_wait().ok().flatten().is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
            if b.child.try_wait().ok().flatten().is_none() {
                // Not reaped yet, so the group id is still ours.
                let _ = sys::killpg(b.child.id() as i32, sys::SIGKILL);
                let _ = b.child.wait();
            }
        }
        if let Some(mut c) = self.root.take()
            && c.try_wait().ok().flatten().is_none()
        {
            match self.root_kind {
                RootKind::Fake => {
                    let _ = self.owner().and_then(|mut o| {
                        o.request("shutdown", json!({})).map_err(|e| e.to_string())
                    });
                }
                RootKind::Real => {
                    let _ = self.root_cli(&["stop".into()]);
                    let _ = sys::kill(c.id() as i32, sys::SIGTERM);
                }
                RootKind::None => {}
            }
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline && c.try_wait().ok().flatten().is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
            if c.try_wait().ok().flatten().is_none() {
                let _ = c.kill();
                let _ = c.wait();
                return Err(
                    "the Root did not shut down within 20 s; killed by its held pid".into(),
                );
            }
        }
        if let Some(m) = self.mock.take() {
            m.stop();
        }
        Ok(())
    }
}

/// Wait until `path` accepts a connection: a Unix socket with a listener behind it (a stale
/// socket file or a missing path keeps it waiting). Returns how long it took. The connection is
/// closed at once without a byte sent.
pub fn wait_socket(path: &Path, timeout: Duration) -> R<Duration> {
    let t0 = Instant::now();
    util::poll(timeout, Duration::from_millis(20), || {
        UnixStream::connect(path).ok().map(|_| ())
    })
    .map(|()| t0.elapsed())
    .ok_or_else(|| {
        format!(
            "{} accepted no connection within {:.1} s",
            path.display(),
            timeout.as_secs_f64()
        )
    })
}

/// True for the answer of an initctl lane whose server is not ready yet (FIX6):
/// `E_BUSY{reason:"starting"}`.
pub fn is_starting(answer: &Value) -> bool {
    answer["ok"] == false
        && answer["error"]["code"] == "E_BUSY"
        && answer["error"]["data"]["reason"] == "starting"
}

/// One initctl `ping` over `path`: the final answer frame.
fn ping_once(path: &Path) -> R<Value> {
    let mut s = UnixStream::connect(path).map_err(|e| format!("connect: {e}"))?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
    s.write_all(b"{\"args\":{},\"cmd\":\"ping\",\"id\":\"wait-socket\",\"v\":1}\n")
        .map_err(|e| format!("send: {e}"))?;
    let mut line = String::new();
    BufReader::new(s)
        .read_line(&mut line)
        .map_err(|e| format!("read: {e}"))?;
    if line.is_empty() {
        return Err("closed without an answer".into());
    }
    serde_json::from_str(&line).map_err(|e| format!("answer is not JSON: {e}"))
}

/// [`wait_socket`] for an initctl lane socket, until the server itself answers: a `ping` gets a
/// final answer other than `E_BUSY{reason:"starting"}` ([`is_starting`]). The kernel binds
/// `run/kernel.sock` before its server is ready and answers "starting" until then (FIX6). Any
/// other answer counts, `E_PEER` included: the server is up, it just does not serve this process.
pub fn wait_serving(path: &Path, timeout: Duration) -> R<Duration> {
    let t0 = Instant::now();
    let mut last: String;
    loop {
        match ping_once(path) {
            Ok(v) if is_starting(&v) => last = "E_BUSY{reason:\"starting\"}".into(),
            Ok(_) => return Ok(t0.elapsed()),
            Err(e) => last = e,
        }
        if t0.elapsed() >= timeout {
            return Err(format!(
                "{} not serving within {:.1} s (last: {last})",
                path.display(),
                timeout.as_secs_f64()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn check_exit(want: Option<i32>, got: Option<i32>) -> R {
    match want {
        Some(w) if got != Some(w) => Err(format!("exit {got:?}, expected {w}")),
        _ => Ok(()),
    }
}

/// Verify an R4 hash chain (root/root.md §3.10.1): `seq` +1, `prev` = previous `hash`, `hash` =
/// SHA-256 of the ICJ of `{at, by, data, kind, prev, seq, v}`, and each line re-serializes byte for
/// byte. Signatures are checked by `init-root audit verify` for the real Root; the fake Root's
/// records are unsigned.
pub fn verify_chain(recs: &[Value]) -> R {
    let zero = format!("sha256:{}", "0".repeat(64));
    let mut prev = zero;
    for (i, r) in recs.iter().enumerate() {
        if r["seq"].as_u64() != Some(i as u64) {
            return Err(format!("r4: record {i} has seq {}", r["seq"]));
        }
        if r["prev"] != prev.as_str() {
            return Err(format!("r4: record {i}: prev {} != {prev}", r["prev"]));
        }
        let body = json!({"at": r["at"], "by": r["by"], "data": r["data"], "kind": r["kind"], "prev": r["prev"], "seq": r["seq"], "v": r["v"]});
        let bytes = init_proto::icj::to_vec(&body).map_err(|e| format!("r4: record {i}: {e}"))?;
        let h = format!("sha256:{}", util::sha256_hex(&bytes));
        if r["hash"] != h.as_str() {
            return Err(format!("r4: record {i}: hash mismatch"));
        }
        prev = h;
    }
    Ok(())
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        fs::create_dir_all(dst)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dst.join(e.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(p) = dst.parent() {
            fs::create_dir_all(p)?;
        }
        fs::copy(src, dst).map(|_| ())
    }
}

/// Write a scenario's `kernel_config` (JSON) as `kernel/config.toml`, merged over an existing one.
fn write_kernel_config(path: &Path, kc: &Value) -> R {
    let mut base: toml::Table = fs::read_to_string(path)
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();
    let add: toml::Table =
        toml::Table::try_from(kc.clone()).map_err(|e| format!("kernel_config: {e}"))?;
    fn merge(a: &mut toml::Table, b: toml::Table) {
        for (k, v) in b {
            match (a.get_mut(&k), v) {
                (Some(toml::Value::Table(x)), toml::Value::Table(y)) => merge(x, y),
                (_, v) => {
                    a.insert(k, v);
                }
            }
        }
    }
    merge(&mut base, add);
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    fs::write(path, toml::to_string(&base).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// Generate a `#[test]` for a scenario file (path relative to the crate root). A skipped scenario
/// prints its reason and does not fail; it never counts as passed.
#[macro_export]
macro_rules! scenario_test {
    ($name:ident, $file:expr) => {
        #[test]
        fn $name() {
            let path = ::std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join($file);
            let r = $crate::scenario::run_scenario(&path);
            match &r.outcome {
                $crate::scenario::Outcome::Passed => {}
                $crate::scenario::Outcome::Skipped(why) => eprintln!("{}: {}", r.id, why),
                $crate::scenario::Outcome::Failed(_) => panic!("{r}"),
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(env: &str, steps: Value) -> Scenario {
        serde_json::from_value(json!({"v": 1, "id": "T", "env": env, "steps": steps})).unwrap()
    }

    #[test]
    fn validation() {
        assert!(validate(&sc("live", json!([{"op": "start_kernel"}]))).is_ok());
        let e = validate(&sc("live", json!([{"op": "start_root"}]))).unwrap_err();
        assert!(e.contains("disposable"), "{e}");
        assert!(validate(&sc("disposable", json!([{"op": "start_root"}]))).is_ok());
        assert!(validate(&sc("lab", json!([]))).is_err());
        assert!(
            validate(&sc(
                "live",
                json!([{"op": "start_kernel"}, {"op": "start_body"}])
            ))
            .is_err()
        );
        assert!(
            validate(&sc(
                "disposable",
                json!([{"op": "start_root"}, {"op": "root_cmd", "argv": ["clear-safe"]}])
            ))
            .is_err()
        );
        assert!(
            validate(&sc(
                "live",
                json!([{"op": "start_kernel"}, {"op": "send_user", "text": "/approve j-1"}])
            ))
            .is_err()
        );
        assert!(
            validate(&sc(
                "live",
                json!([{"op": "start_kernel"}, {"op": "root_cmd", "cmd": "approve"}])
            ))
            .is_err()
        );
        assert!(
            serde_json::from_value::<Scenario>(
                json!({"v": 1, "id": "T", "env": "live", "steps": [{"op": "fly"}]})
            )
            .is_err()
        );
    }

    #[test]
    fn wait_socket_steps_parse() {
        let s = sc(
            "live",
            json!([{"op": "start_kernel", "owner_sock_timeout_s": 5},
                   {"op": "wait_socket", "path": "run/kernel.sock"},
                   {"op": "wait_socket", "path": "root:run/owner.sock", "timeout_s": 2, "serving": true}]),
        );
        assert!(validate(&s).is_ok());
        assert!(matches!(
            &s.steps[0],
            Step::StartKernel { owner_sock_timeout_s: Some(t), .. } if *t == 5.0
        ));
        assert!(matches!(
            &s.steps[1],
            Step::WaitSocket { timeout_s, serving: false, .. } if *timeout_s == 60.0
        ));
        assert!(matches!(
            &s.steps[2],
            Step::WaitSocket { timeout_s, serving: true, .. } if *timeout_s == 2.0
        ));
        assert_eq!(s.steps[1].name(), "wait_socket");
        assert!(
            serde_json::from_value::<Scenario>(json!({"v": 1, "id": "T", "env": "live",
                "steps": [{"op": "wait_socket", "path": "x", "bogus": 1}]}))
            .is_err()
        );
    }

    /// `wait_socket` waits for a listener (not for the path to exist), `wait_serving` skips the
    /// "starting" answers of an early-bound initctl socket.
    #[test]
    fn wait_socket_and_wait_serving() {
        use std::os::unix::net::UnixListener;
        let d = util::test_dir("wsock");
        // A plain file and a missing path never accept.
        let file = d.join("f.sock");
        fs::write(&file, b"").unwrap();
        assert!(wait_socket(&file, Duration::from_millis(200)).is_err());
        let missing = d.join("m.sock");
        let e = wait_socket(&missing, Duration::from_millis(200)).unwrap_err();
        assert!(e.contains("accepted no connection"), "{e}");
        // A stale socket file (its listener is gone) does not count either.
        let stale = d.join("s.sock");
        drop(UnixListener::bind(&stale).unwrap());
        assert!(stale.exists());
        assert!(wait_socket(&stale, Duration::from_millis(200)).is_err());
        // A listener that appears after 300 ms is waited for.
        let late = d.join("l.sock");
        let l2 = late.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let l = UnixListener::bind(&l2).unwrap();
            std::thread::sleep(Duration::from_millis(500));
            drop(l);
        });
        let took = wait_socket(&late, Duration::from_secs(10)).unwrap();
        assert!(took >= Duration::from_millis(250), "{took:?}");
        t.join().unwrap();
        // `serving`: two "starting" answers, then the server's own `ping` answer.
        let k = d.join("k.sock");
        let l = UnixListener::bind(&k).unwrap();
        let srv = std::thread::spawn(move || {
            for i in 0..3 {
                let (s, _) = l.accept().unwrap();
                let mut line = String::new();
                BufReader::new(s.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let req: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(req["cmd"], "ping");
                let answer = if i < 2 {
                    json!({"error": {"code": "E_BUSY", "data": {"reason": "starting", "retry_after_ms": 1000}, "details": [], "message": "the kernel is starting; retry in 1 s"}, "id": req["id"], "ok": false, "v": 1})
                } else {
                    json!({"id": req["id"], "ok": true, "result": {"kernel_version": "0.1.0", "lane": "live", "proto": 1, "time": "2026-10-20T00:00:00Z"}, "v": 1})
                };
                (&s).write_all(format!("{answer}\n").as_bytes()).unwrap();
            }
        });
        wait_serving(&k, Duration::from_secs(10)).unwrap();
        srv.join().unwrap();
        assert!(is_starting(
            &json!({"ok": false, "error": {"code": "E_BUSY", "data": {"reason": "starting"}}})
        ));
        assert!(!is_starting(
            &json!({"ok": false, "error": {"code": "E_BUSY", "data": {"reason": "pipeline"}}})
        ));
        assert!(!is_starting(
            &json!({"ok": false, "error": {"code": "E_PEER"}})
        ));
        fs::remove_dir_all(&d).unwrap(); // exact path
    }

    #[test]
    fn chain_verification() {
        let mut recs = Vec::new();
        let mut prev = format!("sha256:{}", "0".repeat(64));
        for i in 0..3u64 {
            let body = json!({"at": "2026-10-20T00:00:00.000Z", "by": "root", "data": {"i": i}, "kind": "x", "prev": prev, "seq": i, "v": 1});
            let h = format!(
                "sha256:{}",
                util::sha256_hex(&init_proto::icj::to_vec(&body).unwrap())
            );
            let mut r = body.clone();
            r["hash"] = json!(h);
            r["sig"] = json!("fake:unsigned");
            recs.push(r);
            prev = h;
        }
        assert!(verify_chain(&recs).is_ok());
        recs[1]["data"]["i"] = json!(9);
        assert!(verify_chain(&recs).unwrap_err().contains("record 1"));
    }

    #[test]
    fn kernel_config_merges() {
        let d = util::test_dir("kcfg");
        let p = d.join("kernel/config.toml");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(
            &p,
            "schema_version = 2\n[supervise]\nready_timeout_s = 90\nprobation_s = 120\n",
        )
        .unwrap();
        write_kernel_config(
            &p,
            &json!({"supervise": {"ready_timeout_s": 30}, "proxy": {"test_mode": true}}),
        )
        .unwrap();
        let t: toml::Table = fs::read_to_string(&p).unwrap().parse().unwrap();
        assert_eq!(t["supervise"]["ready_timeout_s"].as_integer(), Some(30));
        assert_eq!(t["supervise"]["probation_s"].as_integer(), Some(120));
        assert_eq!(t["proxy"]["test_mode"].as_bool(), Some(true));
        fs::remove_dir_all(&d).unwrap();
    }
}
