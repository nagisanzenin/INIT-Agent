//! The G0 runner (gate.md §3.3): steps in order, stop at the first failure, per-step timeouts and the
//! total budget, logs, and the agent-visible diagnostics.
//!
//! Commands go through a [`Spawner`] so the build sandbox (WP6a) can wrap them; [`DirectSpawner`]
//! runs them unsandboxed (tests). On a timeout the runner kills only the child it was handed, through
//! its `Child` handle, then waits for it.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use super::{
    Action, Cmd, DIAG_CAP, EnvSpec, Finding, LOG_CAP, Step, TOTAL_TIMEOUT_S, code, env, policy,
    shape, steps,
};

/// One command to start. The spawner must start `program` with `args`, an environment of exactly
/// `env` (after clearing its own), cwd `cwd`, stdin from `/dev/null`, and the given stdout/stderr.
pub struct SpawnRequest {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, OsString)>,
    pub cwd: PathBuf,
    pub stdout: Stdio,
    pub stderr: Stdio,
}

/// Starts G0's commands. The runner owns the returned child: it polls it, and on timeout kills it
/// through this handle and waits for it. A sandboxing spawner returns the handle of the process
/// whose death ends the command.
pub trait Spawner {
    fn spawn(&self, req: SpawnRequest) -> io::Result<Child>;
}

/// Runs commands directly, without a sandbox (tests; until WP6a's build sandbox exists).
#[derive(Debug, Clone, Copy, Default)]
pub struct DirectSpawner;

impl Spawner for DirectSpawner {
    fn spawn(&self, req: SpawnRequest) -> io::Result<Child> {
        Command::new(&req.program)
            .args(&req.args)
            .env_clear()
            .envs(req.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&req.cwd)
            .stdin(Stdio::null())
            .stdout(req.stdout)
            .stderr(req.stderr)
            .spawn()
    }
}

/// What G0 runs on.
#[derive(Debug, Clone)]
pub struct G0Spec {
    /// The export root (`cache/build/slot/src`): cwd of every command, and the tree checked.
    pub root: PathBuf,
    pub env: EnvSpec,
    /// Full step logs go to `<log_dir>/<step>.log` (`vault/gate/jobs/<job>/g0/`).
    pub log_dir: PathBuf,
    /// The parent's root `Cargo.toml` text, for `lints_changed`; `None` leaves it unrecorded.
    pub parent_cargo_toml: Option<String>,
    /// Total budget (`gate.g0.timeout_s`, [`TOTAL_TIMEOUT_S`]).
    pub total_timeout: Duration,
    /// ADR-011 A1 (the free path): the [`code::ADVISORY`] commands (clippy and the tests) are
    /// advisory. A failure, a non-zero exit or a step-limit timeout, is recorded in
    /// [`G0Report::advisory`] and on its [`StepRecord`] (`pass: false`, `advisory: true`), and G0
    /// goes on. `false` (the gated path, every probe and inner gate) keeps them blocking.
    pub advisory: bool,
}

impl G0Spec {
    pub fn new(root: PathBuf, env: EnvSpec, log_dir: PathBuf) -> Self {
        G0Spec {
            root,
            env,
            log_dir,
            parent_cargo_toml: None,
            total_timeout: Duration::from_secs(TOTAL_TIMEOUT_S),
            advisory: false,
        }
    }
}

/// One step's record (gate.md §3.3 "each step's exit code and wall time").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepRecord {
    pub name: &'static str,
    pub pass: bool,
    /// Exit code of the last command run; `None` for in-process steps or a killed command.
    pub exit: Option<i32>,
    pub wall_ms: u64,
    pub timed_out: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    /// The step failed only in advisory commands ([`G0Spec::advisory`]); G0 went on.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub advisory: bool,
}

/// One failed advisory command ([`G0Spec::advisory`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Advisory {
    pub step: &'static str,
    pub code: &'static str,
    /// `None` for a killed command or one that could not start.
    pub exit: Option<i32>,
    pub timed_out: bool,
    /// At most [`DIAG_CAP`] bytes, error lines first.
    pub diagnostics: String,
}

/// G0's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct G0Report {
    pub pass: bool,
    /// The failure code (gate.md §4.1), if G0 failed.
    pub code: Option<&'static str>,
    /// The failing step's name.
    pub step: Option<&'static str>,
    pub wall_ms: u64,
    /// The steps that ran, in order; the last one failed if `!pass`.
    pub steps: Vec<StepRecord>,
    /// Agent-visible: at most [`DIAG_CAP`] bytes of the failing step's diagnostics, error lines
    /// before warning lines.
    pub diagnostics: String,
    /// Every finding of a failing policy or shape step.
    pub findings: Vec<Finding>,
    pub has_ext: bool,
    pub build_config_present: bool,
    /// Whether `[lints]` / `[workspace.lints]` of the root `Cargo.toml` differ from the parent's
    /// (informational; `None` without a parent text).
    pub lints_changed: Option<bool>,
    /// The advisory commands that failed, in order ([`G0Spec::advisory`]); `pass` ignores them.
    pub advisory: Vec<Advisory>,
}

/// Runs G0 on `spec.root`.
pub fn run(spec: &G0Spec, spawner: &dyn Spawner) -> G0Report {
    let t0 = Instant::now();
    let deadline = t0 + spec.total_timeout;
    let root = &spec.root;
    let has_ext = root.join("ext/Cargo.toml").is_file();
    let envv = env(&spec.env);
    let lints_changed = spec.parent_cargo_toml.as_deref().map(|p| {
        lints_changed(
            p,
            &fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default(),
        )
    });
    let mut rep = G0Report {
        pass: false,
        code: None,
        step: None,
        wall_ms: 0,
        steps: Vec::new(),
        diagnostics: String::new(),
        findings: Vec::new(),
        has_ext,
        build_config_present: !policy::config_files(root).is_empty(),
        lints_changed,
        advisory: Vec::new(),
    };
    let _ = fs::create_dir_all(&spec.log_dir);
    for d in [&spec.env.home, &spec.env.tmpdir, &spec.env.cargo_home] {
        let _ = fs::create_dir_all(d);
    }

    let mut metas: Vec<Value> = Vec::new();
    for step in steps(has_ext) {
        let started = Instant::now();
        let out = if started >= deadline {
            StepOutcome::fail(
                code::TIMEOUT,
                format!(
                    "G0 exceeded its total budget ({} s) before step `{}`",
                    spec.total_timeout.as_secs(),
                    step.name
                ),
            )
            .timed_out()
        } else {
            match &step.action {
                Action::Commands { cmds } => run_commands(
                    spec, spawner, &envv, &step, cmds, started, deadline, &mut metas,
                ),
                Action::Policy => StepOutcome::findings(policy::check_tree(root).findings),
                Action::Shape => run_shape(root, has_ext, &metas),
            }
        };
        let log = &out.log[..out.log.len().min(LOG_CAP)];
        let _ = fs::write(spec.log_dir.join(format!("{}.log", step.name)), log);
        rep.steps.push(StepRecord {
            name: step.name,
            pass: out.code.is_none() && out.advisory.is_empty(),
            exit: out.exit,
            wall_ms: ms(started.elapsed()),
            timed_out: out.timed_out,
            code: out.code.or(out.advisory.first().map(|a| a.code)),
            advisory: out.code.is_none() && !out.advisory.is_empty(),
        });
        rep.advisory.extend(out.advisory);
        if let Some(c) = out.code {
            rep.code = Some(c);
            rep.step = Some(step.name);
            rep.diagnostics = out.diagnostics;
            rep.findings = out.findings;
            break;
        }
    }
    rep.pass = rep.code.is_none();
    rep.wall_ms = ms(t0.elapsed());
    rep
}

/// Whether the `[lints]` and `[workspace.lints]` tables differ between two `Cargo.toml` texts (if
/// either does not parse, whether the texts differ).
pub fn lints_changed(parent: &str, candidate: &str) -> bool {
    fn lints(text: &str) -> Option<(Option<toml::Value>, Option<toml::Value>)> {
        let doc: toml::Table = text.parse().ok()?;
        Some((
            doc.get("lints").cloned(),
            doc.get("workspace").and_then(|w| w.get("lints")).cloned(),
        ))
    }
    match (lints(parent), lints(candidate)) {
        (Some(a), Some(b)) => a != b,
        _ => parent != candidate,
    }
}

#[derive(Default)]
struct StepOutcome {
    code: Option<&'static str>,
    exit: Option<i32>,
    timed_out: bool,
    log: Vec<u8>,
    diagnostics: String,
    findings: Vec<Finding>,
    /// Advisory failures ([`G0Spec::advisory`]); the step goes on past each.
    advisory: Vec<Advisory>,
}

impl StepOutcome {
    fn fail(code: &'static str, diagnostics: String) -> Self {
        StepOutcome {
            code: Some(code),
            log: diagnostics.clone().into_bytes(),
            diagnostics,
            ..Default::default()
        }
    }

    fn timed_out(mut self) -> Self {
        self.timed_out = true;
        self
    }

    /// An in-process step's outcome: pass without findings; else the first finding's code (callers
    /// sort findings by code order), every finding in the log, the first 4 KiB for the agent.
    fn findings(findings: Vec<Finding>) -> Self {
        let lines: Vec<String> = findings
            .iter()
            .map(|f| format!("{}: {}", f.code, f.detail))
            .collect();
        let text = lines.join("\n");
        let mut o = StepOutcome {
            log: text.clone().into_bytes(),
            ..Default::default()
        };
        if let Some(first) = findings.first() {
            o.code = Some(first.code);
            o.diagnostics = cap_lines(lines.iter().map(String::as_str), DIAG_CAP);
            o.findings = findings;
        }
        o
    }
}

#[allow(clippy::too_many_arguments)]
fn run_commands(
    spec: &G0Spec,
    spawner: &dyn Spawner,
    envv: &[(String, OsString)],
    step: &Step,
    cmds: &[Cmd],
    started: Instant,
    deadline: Instant,
    metas: &mut Vec<Value>,
) -> StepOutcome {
    let step_deadline = match step.timeout_s {
        Some(s) => (started + Duration::from_secs(s)).min(deadline),
        None => deadline,
    };
    let mut o = StepOutcome::default();
    for c in cmds {
        let is_metadata = c.argv.get(1).is_some_and(|s| s == "metadata");
        o.log
            .extend_from_slice(format!("$ {}\n", c.argv.join(" ")).as_bytes());
        let x = exec(
            spawner,
            SpawnRequest {
                program: spec.env.toolchain_bin.join(&c.argv[0]),
                args: c.argv[1..].to_vec(),
                env: envv.to_vec(),
                cwd: spec.root.clone(),
                stdout: Stdio::null(),
                stderr: Stdio::null(),
            },
            step_deadline,
            is_metadata,
        );
        o.log.extend_from_slice(&x.output);
        if x.output_total > x.output.len() as u64 {
            o.log.extend_from_slice(
                format!("\n[output truncated: {} bytes total]", x.output_total).as_bytes(),
            );
        }
        let status = match &x.status {
            Status::Exited(st) => format!(
                "exit {}",
                st.code()
                    .map_or_else(|| "signal".to_string(), |c| c.to_string())
            ),
            Status::TimedOut => "timed out; killed".to_string(),
            Status::Error(e) => format!("error: {e}"),
        };
        o.log
            .extend_from_slice(format!("\n[{status}; {} ms]\n", x.wall_ms).as_bytes());
        if spec.advisory && code::ADVISORY.contains(&c.code) {
            // ADR-011 A1: recorded, never blocking. A3 reads the `test` step after going live.
            let (exit, timed_out, diag) = match &x.status {
                Status::Exited(st) if st.success() => {
                    if o.advisory.is_empty() {
                        o.exit = st.code();
                    }
                    continue;
                }
                Status::Exited(st) => (st.code(), false, diagnostics(&x.output)),
                Status::TimedOut => (
                    None,
                    true,
                    diagnostics_with_head(
                        &format!(
                            "error: advisory step `{}` timed out; `{}` was killed",
                            step.name,
                            c.argv.join(" ")
                        ),
                        &x.output,
                    ),
                ),
                Status::Error(e) => (
                    None,
                    false,
                    format!("error: failed to run `{}`: {e}", c.argv.join(" ")),
                ),
            };
            o.exit = exit;
            o.timed_out |= timed_out;
            o.advisory.push(Advisory {
                step: step.name,
                code: c.code,
                exit,
                timed_out,
                diagnostics: cap_lines(diag.lines(), DIAG_CAP),
            });
            continue;
        }
        match x.status {
            Status::Error(e) => {
                o.code = Some(c.code);
                o.diagnostics = cap_lines(
                    [format!("error: failed to run `{}`: {e}", c.argv.join(" ")).as_str()],
                    DIAG_CAP,
                );
                return o;
            }
            Status::TimedOut => {
                o.code = Some(code::TIMEOUT);
                o.timed_out = true;
                let limit = if step_deadline < deadline {
                    format!("its {} s step limit", step.timeout_s.unwrap_or(0))
                } else {
                    format!("the total G0 budget ({} s)", spec.total_timeout.as_secs())
                };
                let head = format!(
                    "error: step `{}` exceeded {limit}; `{}` was killed",
                    step.name,
                    c.argv.join(" ")
                );
                o.diagnostics = diagnostics_with_head(&head, &x.output);
                return o;
            }
            Status::Exited(st) => {
                if o.advisory.is_empty() {
                    o.exit = st.code(); // else the step keeps its first advisory failure's exit
                }
                if !st.success() {
                    o.code = Some(c.code);
                    o.diagnostics = diagnostics(&x.output);
                    return o;
                }
                if is_metadata {
                    match serde_json::from_slice::<Value>(&x.stdout) {
                        Ok(v) if v.get("packages").is_some_and(Value::is_array) => metas.push(v),
                        Ok(_) => {
                            o.code = Some(c.code);
                            o.diagnostics =
                                "error: `cargo metadata` output has no `packages` array".into();
                            return o;
                        }
                        Err(e) => {
                            o.code = Some(c.code);
                            o.diagnostics =
                                format!("error: `cargo metadata` output is not JSON: {e}");
                            return o;
                        }
                    }
                }
            }
        }
    }
    o
}

fn run_shape(root: &Path, has_ext: bool, metas: &[Value]) -> StepOutcome {
    let Some(body_meta) = metas.first() else {
        return StepOutcome::fail(
            code::METADATA,
            "error: no `cargo metadata` output for the shape check".into(),
        );
    };
    let manifests = match shape::find_manifests(root) {
        Ok(m) => m,
        Err(e) => {
            return StepOutcome::fail(
                code::UNCHECKED_CRATE,
                format!("error: cannot list the tree: {e}"),
            );
        }
    };
    let body_lock = fs::read_to_string(root.join("Cargo.lock")).ok();
    let ext_lock = if has_ext {
        fs::read_to_string(root.join("ext/Cargo.lock")).ok()
    } else {
        None
    };
    let input = shape::ShapeInput {
        body_meta,
        ext_meta: if has_ext { metas.get(1) } else { None },
        manifests: &manifests,
        body_lock: body_lock.as_deref(),
        ext_lock: ext_lock.as_deref(),
    };
    // The kernel reads these files outside any sandbox, so only files inside the tree are read.
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let read = |p: &Path| -> io::Result<String> {
        let c = p.canonicalize()?;
        if !c.starts_with(&canon_root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the target root is outside the tree",
            ));
        }
        fs::read_to_string(c)
    };
    StepOutcome::findings(shape::check(&input, &read))
}

enum Status {
    Exited(ExitStatus),
    TimedOut,
    Error(io::Error),
}

struct Exec {
    status: Status,
    /// stdout and stderr interleaved (stderr only when stdout is captured separately), ≤ LOG_CAP.
    output: Vec<u8>,
    output_total: u64,
    /// stdout, when captured separately (≤ LOG_CAP).
    stdout: Vec<u8>,
    wall_ms: u64,
}

/// How long to wait for the output pipes to drain after the child has ended (a grandchild that
/// kept a pipe open must not block G0).
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Runs one command until it exits or `deadline` passes. `req.stdout`/`req.stderr` are replaced by
/// pipes. With `split_stdout`, stdout is captured on its own (cargo metadata's JSON).
fn exec(
    spawner: &dyn Spawner,
    mut req: SpawnRequest,
    deadline: Instant,
    split_stdout: bool,
) -> Exec {
    let start = Instant::now();
    let done = |status, output: (Vec<u8>, u64), stdout: Vec<u8>| Exec {
        status,
        output: output.0,
        output_total: output.1,
        stdout,
        wall_ms: ms(start.elapsed()),
    };
    let pipes = (|| -> io::Result<_> {
        let (log_r, log_w) = io::pipe()?;
        let (out_stdio, out_r) = if split_stdout {
            let (r, w) = io::pipe()?;
            (Stdio::from(w), Some(r))
        } else {
            (Stdio::from(log_w.try_clone()?), None)
        };
        Ok((log_r, Stdio::from(log_w), out_stdio, out_r))
    })();
    let (log_r, err_stdio, out_stdio, out_r) = match pipes {
        Ok(p) => p,
        Err(e) => return done(Status::Error(e), (Vec::new(), 0), Vec::new()),
    };
    let log_cap = Capture::start(log_r, LOG_CAP);
    let out_cap = out_r.map(|r| Capture::start(r, LOG_CAP));
    req.stdout = out_stdio;
    req.stderr = err_stdio;
    // `req` (holding our copies of the write ends) is consumed here, so the readers see EOF once
    // the child (and anything it handed the pipes to) has closed them.
    let child = spawner.spawn(req);
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            let stdout = out_cap.map(|c| c.finish(DRAIN_GRACE).0).unwrap_or_default();
            return done(Status::Error(e), log_cap.finish(DRAIN_GRACE), stdout);
        }
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Status::Exited(st),
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Status::Error(e);
            }
        }
        let now = Instant::now();
        if now >= deadline {
            // Only the child we spawned, by its handle.
            let _ = child.kill();
            let _ = child.wait();
            break Status::TimedOut;
        }
        thread::sleep((deadline - now).min(Duration::from_millis(20)));
    };
    let stdout = out_cap.map(|c| c.finish(DRAIN_GRACE).0).unwrap_or_default();
    done(status, log_cap.finish(DRAIN_GRACE), stdout)
}

/// Drains a pipe on its own thread, keeping at most `cap` bytes (and counting the rest).
struct Capture {
    buf: Arc<Mutex<(Vec<u8>, u64)>>,
    eof: mpsc::Receiver<()>,
}

impl Capture {
    fn start(mut r: impl Read + Send + 'static, cap: usize) -> Capture {
        let buf = Arc::new(Mutex::new((Vec::new(), 0u64)));
        let (tx, eof) = mpsc::channel();
        let b = Arc::clone(&buf);
        thread::spawn(move || {
            let mut chunk = vec![0u8; 64 * 1024];
            loop {
                match r.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut g = b.lock().unwrap_or_else(PoisonError::into_inner);
                        let room = cap.saturating_sub(g.0.len());
                        g.0.extend_from_slice(&chunk[..n.min(room)]);
                        g.1 += n as u64;
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            let _ = tx.send(());
        });
        Capture { buf, eof }
    }

    /// Waits up to `grace` for EOF, then takes what was captured: (bytes, total bytes seen).
    fn finish(self, grace: Duration) -> (Vec<u8>, u64) {
        let _ = self.eof.recv_timeout(grace);
        let mut g = self.buf.lock().unwrap_or_else(PoisonError::into_inner);
        (std::mem::take(&mut g.0), g.1)
    }
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Cargo's right-aligned status lines (`   Compiling foo`), dropped from the agent's view.
fn is_progress(line: &str) -> bool {
    const VERBS: [&str; 21] = [
        "Compiling",
        "Checking",
        "Finished",
        "Running",
        "Doc-tests",
        "Fresh",
        "Locking",
        "Locked",
        "Blocking",
        "Building",
        "Documenting",
        "Updating",
        "Downloading",
        "Downloaded",
        "Adding",
        "Packaging",
        "Verifying",
        "Archiving",
        "Executable",
        "Removed",
        "Waiting",
    ];
    if !line.starts_with(' ') {
        return false;
    }
    let t = line.trim_start();
    VERBS.iter().any(|v| {
        t.strip_prefix(v)
            .is_some_and(|r| r.starts_with(' ') || r.is_empty())
    })
}

/// libtest lines about passing tests, which only spend the agent's 4 KiB.
fn is_test_noise(line: &str) -> bool {
    line == "running 0 tests"
        || line.starts_with("test result: ok.")
        || (line.starts_with("test ")
            && (line.ends_with(" ... ok") || line.ends_with(" ... ignored")))
}

/// 0 = error line, 1 = other, 2 = warning line.
fn class(line: &str) -> u8 {
    let t = line.trim_start();
    if t.starts_with("error") || line.contains(": error:") || line.contains(": error[") {
        0
    } else if t.starts_with("warning") || line.contains(": warning:") || line.contains(": warning[")
    {
        2
    } else {
        1
    }
}

/// The agent-visible diagnostics of a failed command (gate.md §3.3): cargo progress and blank lines
/// dropped, error lines first, then other lines (test output), then warning lines, each group in
/// its original order, capped at [`DIAG_CAP`] bytes.
pub(crate) fn diagnostics(output: &[u8]) -> String {
    let text = String::from_utf8_lossy(output);
    let mut groups: [Vec<&str>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for line in text.lines() {
        if line.trim().is_empty() || is_progress(line) || is_test_noise(line) {
            continue;
        }
        groups[class(line) as usize].push(line);
    }
    cap_lines(groups.iter().flatten().copied(), DIAG_CAP)
}

fn diagnostics_with_head(head: &str, output: &[u8]) -> String {
    let rest = diagnostics(output);
    cap_lines(std::iter::once(head).chain(rest.lines()), DIAG_CAP)
}

/// Joins lines with `\n`, keeping at most `cap` bytes (cut at a char boundary).
fn cap_lines<'a>(lines: impl IntoIterator<Item = &'a str>, cap: usize) -> String {
    let mut out = String::new();
    for line in lines {
        let sep = usize::from(!out.is_empty());
        if out.len() + sep + line.len() <= cap {
            if sep == 1 {
                out.push('\n');
            }
            out.push_str(line);
            continue;
        }
        let room = cap.saturating_sub(out.len() + sep);
        if room > 0 {
            if sep == 1 {
                out.push('\n');
            }
            let mut end = room;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            out.push_str(&line[..end]);
        }
        break;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{remove_home, test_home};
    use std::cell::RefCell;
    use std::io::Write;

    /// Runs `/bin/sh -c <script>` instead of cargo, picking the script by subcommand (and `$E`).
    struct Fake {
        scripts: Vec<(&'static str, bool, String)>,
        calls: RefCell<Vec<String>>,
        fail_spawn: bool,
    }

    impl Fake {
        fn new(scripts: Vec<(&'static str, bool, String)>) -> Self {
            Fake {
                scripts,
                calls: RefCell::new(Vec::new()),
                fail_spawn: false,
            }
        }
    }

    impl Spawner for Fake {
        fn spawn(&self, req: SpawnRequest) -> io::Result<Child> {
            let sub = req.args[0].clone();
            let ext = req.args.iter().any(|a| a == "--manifest-path");
            assert!(req.program.ends_with("cargo"), "{:?}", req.program);
            let wrapper = req
                .env
                .iter()
                .find(|(k, _)| k == "RUSTC_WRAPPER")
                .map(|(_, v)| v.clone());
            assert_eq!(wrapper, Some(OsString::new()));
            self.calls
                .borrow_mut()
                .push(format!("{sub}{}", if ext { " $E" } else { "" }));
            if self.fail_spawn {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no cargo here"));
            }
            let script = self
                .scripts
                .iter()
                .find(|(s, e, _)| *s == sub && *e == ext)
                .map(|(_, _, sc)| sc.clone())
                .unwrap_or_else(|| "exit 0".into());
            Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .current_dir(&req.cwd)
                .stdin(Stdio::null())
                .stdout(req.stdout)
                .stderr(req.stderr)
                .spawn()
        }
    }

    struct Tree {
        home: PathBuf,
        root: PathBuf,
        meta: PathBuf,
    }

    impl Tree {
        /// A one-package tree and its synthetic metadata JSON (written to `<home>/meta.json`).
        fn new(name: &str) -> Self {
            let home = test_home(name);
            let root = home.join("src");
            fs::create_dir_all(root.join("src")).unwrap();
            fs::write(
                root.join("Cargo.toml"),
                "[package]\nname = \"b\"\nversion = \"0.1.0\"\n[lints.rust]\nx = \"deny\"\n",
            )
            .unwrap();
            fs::write(
                root.join("Cargo.lock"),
                "version = 4\n[[package]]\nname = \"b\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
            fs::write(
                root.join("src/main.rs"),
                "#![forbid(unsafe_code)]\nfn main() {}\n",
            )
            .unwrap();
            let t = Tree {
                meta: home.join("meta.json"),
                home,
                root,
            };
            t.write_meta(&t.root.join("src/main.rs"));
            t
        }

        fn write_meta(&self, main_rs: &Path) {
            let r = self.root.canonicalize().unwrap();
            let main_rs = main_rs.canonicalize().unwrap();
            let id = format!("path+file://{}#b@0.1.0", r.display());
            let meta = serde_json::json!({
                "packages": [{"name": "b", "version": "0.1.0", "id": id,
                              "manifest_path": r.join("Cargo.toml"), "dependencies": [],
                              "targets": [{"kind": ["bin"], "crate_types": ["bin"], "name": "b", "src_path": main_rs}]}],
                "workspace_members": [id], "workspace_root": r, "version": 1});
            fs::write(&self.meta, meta.to_string()).unwrap();
        }

        fn spec(&self) -> G0Spec {
            let out = self.home.join("out");
            let env = EnvSpec {
                toolchain_bin: "/nonexistent/toolchain/bin".into(),
                home: out.join("home"),
                tmpdir: out.join("tmp"),
                cargo_home: out.join("cargo-home"),
                target_dir: out.join("target"),
                source_date_epoch: 1,
                init_home: self.home.join("ih"),
                sdkroot: None,
                developer_dir: None,
            };
            G0Spec::new(self.root.clone(), env, self.home.join("logs"))
        }

        fn cat_meta(&self) -> (&'static str, bool, String) {
            (
                "metadata",
                false,
                format!(
                    "echo 'warning: noise on stderr' >&2; cat '{}'",
                    self.meta.display()
                ),
            )
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            remove_home(&self.home);
        }
    }

    #[test]
    fn all_steps_pass_in_order() {
        let t = Tree::new("g0-run-pass");
        let fake = Fake::new(vec![
            t.cat_meta(),
            ("test", false, "echo 'test result: ok'".into()),
        ]);
        let mut spec = t.spec();
        spec.parent_cargo_toml = Some(fs::read_to_string(t.root.join("Cargo.toml")).unwrap());
        let r = run(&spec, &fake);
        assert!(r.pass, "{r:#?}");
        assert_eq!(r.code, None);
        assert_eq!(
            *fake.calls.borrow(),
            ["metadata", "check", "clippy", "test"]
        );
        let names: Vec<_> = r.steps.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            [
                "metadata",
                "build_config",
                "shape",
                "check",
                "clippy",
                "test"
            ]
        );
        assert!(r.steps.iter().all(|s| s.pass && !s.timed_out));
        assert_eq!(r.steps[0].exit, Some(0));
        assert_eq!(r.steps[1].exit, None);
        assert!(!r.has_ext && !r.build_config_present);
        assert_eq!(r.lints_changed, Some(false));
        for n in names {
            assert!(spec.log_dir.join(format!("{n}.log")).is_file(), "{n}.log");
        }
        let log = fs::read_to_string(spec.log_dir.join("metadata.log")).unwrap();
        assert!(log.starts_with("$ cargo metadata --config"), "{log}");
        assert!(
            log.contains("warning: noise on stderr") && log.contains("[exit 0;"),
            "{log}"
        );
        assert!(spec.env.cargo_home.is_dir() && spec.env.tmpdir.is_dir() && spec.env.home.is_dir());
        let test_log = fs::read_to_string(spec.log_dir.join("test.log")).unwrap();
        assert!(test_log.contains("test result: ok"));
        // the report serializes with the gate.md §3.13 step fields
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["steps"][3]["name"], "check");
        assert_eq!(j["steps"][3]["exit"], 0);
        assert!(j["steps"][3]["wall_ms"].is_u64());
    }

    #[test]
    fn failing_command_stops_and_orders_diagnostics() {
        let t = Tree::new("g0-run-check");
        let script = "printf '    Checking b v0.1.0 (/x)\\nsrc/a.rs:1:1: warning: unused\\n\\nnote: some context\\n\
                      src/main.rs:2:5: error[E0425]: cannot find value\\nwarning: 1 warning emitted\\n\
                      error: could not compile `b`\\n' >&2; exit 101";
        let fake = Fake::new(vec![t.cat_meta(), ("check", false, script.into())]);
        let r = run(&t.spec(), &fake);
        assert!(!r.pass);
        assert_eq!(r.code, Some(code::CHECK));
        assert_eq!(r.step, Some("check"));
        assert_eq!(*fake.calls.borrow(), ["metadata", "check"]);
        assert_eq!(r.steps.last().unwrap().exit, Some(101));
        assert_eq!(
            r.diagnostics,
            "src/main.rs:2:5: error[E0425]: cannot find value\nerror: could not compile `b`\nnote: some context\n\
             src/a.rs:1:1: warning: unused\nwarning: 1 warning emitted"
        );
        let log = fs::read_to_string(t.spec().log_dir.join("check.log")).unwrap();
        assert!(
            log.contains("    Checking b v0.1.0") && log.contains("[exit 101;"),
            "{log}"
        );
    }

    /// ADR-011 A1: on the free path clippy and the tests are advisory; `check` stays blocking; the
    /// gated path (the default) is unchanged.
    #[test]
    fn advisory_clippy_and_tests_on_the_free_path() {
        let t = Tree::new("g0-run-advisory");
        let fails = |sub: &'static str| {
            (
                sub,
                false,
                format!("echo 'error: {sub} says no' >&2; exit 101"),
            )
        };
        let fake = Fake::new(vec![t.cat_meta(), fails("clippy"), fails("test")]);
        let mut spec = t.spec();
        spec.advisory = true;
        let r = run(&spec, &fake);
        assert!(r.pass, "{r:#?}");
        assert_eq!((r.code, r.step), (None, None));
        assert_eq!(*fake.calls.borrow(), ["metadata", "check", "clippy", "test"]);
        let by = |n: &str| r.steps.iter().find(|s| s.name == n).unwrap().clone();
        let (clippy, test, check) = (by("clippy"), by("test"), by("check"));
        assert!(!clippy.pass && clippy.advisory && clippy.code == Some(code::CLIPPY));
        assert!(!test.pass && test.advisory && test.exit == Some(101));
        assert!(check.pass && !check.advisory);
        assert_eq!(by("metadata").exit, Some(0));
        assert_eq!(
            r.advisory.iter().map(|a| (a.step, a.code, a.exit)).collect::<Vec<_>>(),
            [("clippy", code::CLIPPY, Some(101)), ("test", code::TEST, Some(101))]
        );
        assert!(r.advisory[1].diagnostics.contains("error: test says no"));
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["steps"][5]["advisory"], true);
        assert!(j["steps"][3].get("advisory").is_none(), "only on advisory failures");
        // The same tree, gated: clippy fails G0.
        let fake = Fake::new(vec![t.cat_meta(), fails("clippy"), fails("test")]);
        let r = run(&t.spec(), &fake);
        assert_eq!((r.pass, r.code, r.step), (false, Some(code::CLIPPY), Some("clippy")));
        assert!(r.advisory.is_empty());
        // `check` is never advisory.
        let fake = Fake::new(vec![t.cat_meta(), fails("check")]);
        let r = run(&spec, &fake);
        assert_eq!((r.pass, r.code), (false, Some(code::CHECK)));
    }

    #[test]
    fn timeout_kills_the_child() {
        let t = Tree::new("g0-run-timeout");
        let fake = Fake::new(vec![
            t.cat_meta(),
            ("clippy", false, "echo started >&2; exec sleep 30".into()),
        ]);
        let mut spec = t.spec();
        spec.total_timeout = Duration::from_millis(1500);
        let t0 = Instant::now();
        let r = run(&spec, &fake);
        assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
        assert_eq!(r.code, Some(code::TIMEOUT));
        assert_eq!(r.step, Some("clippy"));
        let last = r.steps.last().unwrap();
        assert!(last.timed_out && last.exit.is_none(), "{last:?}");
        assert!(
            r.diagnostics
                .starts_with("error: step `clippy` exceeded the total G0 budget (1 s)"),
            "{}",
            r.diagnostics
        );
        assert!(r.diagnostics.contains("started"));
    }

    #[test]
    fn budget_spent_before_a_step_is_a_timeout() {
        let t = Tree::new("g0-run-budget");
        let fake = Fake::new(vec![t.cat_meta()]);
        let mut spec = t.spec();
        spec.total_timeout = Duration::ZERO;
        let r = run(&spec, &fake);
        assert_eq!(r.code, Some(code::TIMEOUT));
        assert_eq!(r.step, Some("metadata"));
        assert!(fake.calls.borrow().is_empty());
    }

    #[test]
    fn metadata_must_be_json() {
        let t = Tree::new("g0-run-meta");
        let fake = Fake::new(vec![("metadata", false, "echo 'not json'".into())]);
        let r = run(&t.spec(), &fake);
        assert_eq!(r.code, Some(code::METADATA));
        assert!(r.diagnostics.contains("not JSON"), "{}", r.diagnostics);
        let fake = Fake::new(vec![("metadata", false, "echo '{}'".into())]);
        assert_eq!(run(&t.spec(), &fake).code, Some(code::METADATA));
        let fake = Fake::new(vec![(
            "metadata",
            false,
            "echo 'error: bad manifest' >&2; exit 101".into(),
        )]);
        let r = run(&t.spec(), &fake);
        assert_eq!(r.code, Some(code::METADATA));
        assert_eq!(r.diagnostics, "error: bad manifest");
    }

    #[test]
    fn spawn_failure_fails_the_step() {
        let t = Tree::new("g0-run-spawn");
        let mut fake = Fake::new(vec![]);
        fake.fail_spawn = true;
        let r = run(&t.spec(), &fake);
        assert_eq!(r.code, Some(code::METADATA));
        assert!(
            r.diagnostics.contains("failed to run `cargo metadata")
                && r.diagnostics.contains("no cargo here")
        );
    }

    #[test]
    fn policy_failure_stops_before_any_build() {
        let t = Tree::new("g0-run-policy");
        fs::create_dir_all(t.root.join(".cargo")).unwrap();
        fs::write(
            t.root.join(".cargo/config.toml"),
            "[build]\nrustc-wrapper = \"./evil.sh\"\nrustflags = [\"-Awarnings\"]\n",
        )
        .unwrap();
        let fake = Fake::new(vec![t.cat_meta()]);
        let r = run(&t.spec(), &fake);
        assert_eq!(r.code, Some(code::BUILD_CONFIG));
        assert_eq!(r.step, Some("build_config"));
        assert_eq!(*fake.calls.borrow(), ["metadata"]);
        assert!(r.build_config_present);
        assert_eq!(r.findings.len(), 2);
        assert_eq!(r.diagnostics.lines().count(), 2);
        assert!(
            r.diagnostics
                .starts_with("gate.g0.build_config: .cargo/config.toml:"),
            "{}",
            r.diagnostics
        );
    }

    #[test]
    fn shape_failure_and_reads_stay_in_the_tree() {
        let t = Tree::new("g0-run-shape");
        fs::write(t.root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let fake = Fake::new(vec![t.cat_meta()]);
        let r = run(&t.spec(), &fake);
        assert_eq!(r.code, Some(code::FORBID_MISSING));
        assert_eq!(r.step, Some("shape"));
        assert!(
            r.diagnostics
                .contains("`src/main.rs` (bin `b` of package `b`)"),
            "{}",
            r.diagnostics
        );

        // a target root outside the tree is never read
        let outside = t.home.join("outside.rs");
        fs::write(&outside, "#![forbid(unsafe_code)]\n").unwrap();
        t.write_meta(&outside);
        let r = run(&t.spec(), &Fake::new(vec![t.cat_meta()]));
        assert_eq!(r.code, Some(code::FORBID_MISSING));
        assert!(
            r.diagnostics.contains("outside the tree"),
            "{}",
            r.diagnostics
        );

        // a stray manifest
        t.write_meta(&t.root.join("src/main.rs"));
        fs::write(
            t.root.join("src/main.rs"),
            "#![forbid(unsafe_code)]\nfn main() {}\n",
        )
        .unwrap();
        fs::create_dir_all(t.root.join("tools/x")).unwrap();
        fs::write(
            t.root.join("tools/x/Cargo.toml"),
            "[package]\nname = \"x\"\n",
        )
        .unwrap();
        let r = run(&t.spec(), &Fake::new(vec![t.cat_meta()]));
        assert_eq!(r.code, Some(code::UNCHECKED_CRATE));
    }

    #[test]
    fn lints_changed_compares_lint_tables() {
        let a = "[package]\nname = \"a\"\n[lints.rust]\nunsafe_code = \"forbid\"\n";
        assert!(!lints_changed(a, a));
        assert!(!lints_changed(
            a,
            "[package]\nname = \"renamed\"\n[lints.rust]\nunsafe_code = \"forbid\"\n"
        ));
        assert!(lints_changed(a, "[package]\nname = \"a\"\n"));
        assert!(lints_changed(
            a,
            "[package]\nname = \"a\"\n[lints.clippy]\nall = \"allow\"\n"
        ));
        let w = "[workspace]\nmembers = []\n[workspace.lints.clippy]\nall = \"deny\"\n";
        assert!(!lints_changed(w, w));
        assert!(lints_changed(
            w,
            "[workspace]\nmembers = []\n[workspace.lints.clippy]\nall = \"warn\"\n"
        ));
        assert!(lints_changed("not = [toml", "x = 1"));
        assert!(!lints_changed("not = [toml", "not = [toml"));
    }

    #[test]
    fn capture_caps_and_counts() {
        let (r, mut w) = io::pipe().unwrap();
        let c = Capture::start(r, 10);
        w.write_all(&[b'x'; 100]).unwrap();
        drop(w);
        let (bytes, total) = c.finish(Duration::from_secs(5));
        assert_eq!(bytes.len(), 10);
        assert_eq!(total, 100);
    }

    #[test]
    fn diagnostics_cap_and_progress_filter() {
        let mut out =
            String::from("   Compiling a v0.1.0\n    Finished `dev` profile\n     Running tests\n");
        for i in 0..400 {
            out.push_str(&format!("src/x.rs:{i}:1: warning: w{i}\n"));
        }
        out.push_str("error: could not compile `a`\n");
        let d = diagnostics(out.as_bytes());
        assert!(
            d.len() <= DIAG_CAP && d.len() > DIAG_CAP - 40,
            "{}",
            d.len()
        );
        assert!(
            d.starts_with("error: could not compile `a`\nsrc/x.rs:0:1: warning: w0"),
            "{d}"
        );
        assert!(!d.contains("Compiling") && !d.contains("Finished") && !d.contains("Running"));
        // char-boundary cut
        let s = cap_lines(["ééééé"], 5);
        assert_eq!(s, "éé");
        assert_eq!(cap_lines(["ab", "cd"], 4), "ab\nc");
        assert_eq!(cap_lines(["ab", "cd"], 3), "ab");
        let t = "running 0 tests\ntest result: ok. 0 passed\nrunning 2 tests\ntest a ... ok\ntest b ... FAILED\n\
                 test c ... ignored\ntest result: FAILED. 1 passed; 1 failed\nerror: test failed\n";
        assert_eq!(
            diagnostics(t.as_bytes()),
            "error: test failed\nrunning 2 tests\ntest b ... FAILED\ntest result: FAILED. 1 passed; 1 failed"
        );
        assert!(!is_progress("Compiling at column 0"));
        assert!(is_progress("   Compiling foo v1"));
    }
}
