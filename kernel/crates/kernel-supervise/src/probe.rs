//! The staging probe (supervise.md §3.6.3): the primitive G2, G3 and G4 task runs use.
//!
//! `probe()` creates `probe/<id>/` ([`ProbeRoot`]: clone-based, no symlinks, D8), writes the probe
//! boot record, has the Root spawn the candidate (`kind:"probe"`, `profile:"body"`), and records
//! its stdout frames in `<root>/stdout.jsonl` (4 MiB cap). [`ProbeHandle::wait_ready`] and
//! [`ProbeHandle::window`] implement the readiness and probe-window rules of §3.5;
//! [`ProbeHandle::stop`] stops the instance (stop frame, `exit_grace_s`, then `kill`), runs
//! `keep_on_stop`, copies the agent-visible outputs (purpose `Probe` only) and deletes the root.
//!
//! The proxy lane: [`probe_scoped`] takes a [`ProbeScope`], opened on `<root>/run/proxy.sock`
//! before the body starts and closed in `stop()`, whose summary becomes `ProbeOutcome.scope`
//! (WP8b; the gate's implementation is `promote::proxy`). [`probe`] runs without one (null
//! scope). The probe's initctl lane (`<root>/run/kernel.sock`, initctl.md §3.4) is bound through
//! the process-wide [`LaneHook`] the kernel binary installs (WP7b); without one, no lane is bound.

use crate::config::KernelCfg;
use crate::git::Git;
use crate::instance::{ExitInfo, SpawnRequest, Spawner};
use crate::proberoot::{KeepError, ProbeRoot, SessionSrc};
use crate::spawnspec;
use init_proto::files::{self as pf, BootFile, BootReason, InitMode, ReadyFile};
use init_proto::root_link::{InstanceKind, KillMode};
use init_proto::stdio::{InputMode, ToBody};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The initctl server's probe lanes (initctl.md §3.4; WP7b): `open` binds
/// `probe/<id>/run/kernel.sock` before the probe body is spawned, `started` names its instance
/// (the macOS peer check reads `run/mark/<instance>`), `close` removes the lane before the root is
/// deleted. Installed once per kernel process by `init-kernel`; failures never fail the probe.
pub trait LaneHook: Send + Sync {
    fn open(&self, probe_id: &str, purpose: Purpose);
    fn started(&self, probe_id: &str, instance: &str);
    fn close(&self, probe_id: &str);
}

static LANE_HOOK: OnceLock<Arc<dyn LaneHook>> = OnceLock::new();

/// Install the probe-lane hook (once per process; `false` if one is already set).
pub fn set_lane_hook(h: Arc<dyn LaneHook>) -> bool {
    LANE_HOOK.set(h).is_ok()
}

fn lane_hook() -> Option<&'static Arc<dyn LaneHook>> {
    LANE_HOOK.get()
}

/// Probe instances running in this kernel (≤ `max_concurrent_probes`).
static RUNNING: AtomicUsize = AtomicUsize::new(0);
/// stdout.jsonl cap.
const STDOUT_CAP: u64 = 4 * 1024 * 1024;
/// stderr tail kept for the agent (last 1 MiB).
const STDERR_TAIL: usize = 1024 * 1024;

/// What runs: the candidate (or parent) SHA and the binary to clone into the root.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub sha: String,
    pub binary: PathBuf,
    /// The kept-component set directory (`bin/ext.d/<set>`), if any.
    pub ext_set_dir: Option<PathBuf>,
}

/// `LanePurpose` (kernel/initctl.md §3.4): `Probe` (G2, G3) or `Run` (G4 task runs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Probe,
    Run,
}

/// §3.6.3 `ProbeOpts`.
#[derive(Clone, Debug)]
pub struct ProbeOpts {
    pub session: SessionSrc,
    /// `probe` (G2, G3) | `normal` (G4 task runs).
    pub mode: InitMode,
    /// `promote` (G2) | `cold` (G3, G4).
    pub boot_reason: BootReason,
    pub purpose: Purpose,
    /// The gate job (destinations of `keep_on_stop` live under `cache/gate/<job>/`).
    pub job: String,
    /// `(source, path relative to the probe root)`, placed before spawn.
    pub extra_files: Vec<(PathBuf, PathBuf)>,
    /// Relative to the root; default `work`; G4 runs `work/task`.
    pub cwd: Option<PathBuf>,
    /// `(path relative to the root, destination under cache/gate/<job>/)`.
    pub keep_on_stop: Vec<(PathBuf, PathBuf)>,
}

/// Why a probe failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeFail {
    /// More than `max_concurrent_probes` running.
    Busy,
    /// The root could not be built.
    Root(String),
    /// The Root refused the spawn.
    Spawn(String),
    /// The candidate exited before `ready` (`gate.g2.probe_exit`).
    Exit(ExitInfo),
    /// No `ready` in time (`gate.g2.probe_timeout`).
    Timeout,
    /// `keep_on_stop` refused a path.
    KeepPath(String),
}

impl std::fmt::Display for ProbeFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// §3.6.3 `ProbeOutcome`.
#[derive(Clone, Debug)]
pub struct ProbeOutcome {
    pub ready_at: Option<Duration>,
    pub window_ok: bool,
    pub exit: Option<ExitInfo>,
    pub ready: Option<ReadyFile>,
    pub last_boot_event: Option<serde_json::Value>,
    /// `ProxyHandle::close_scope` summary (null until WP5 wires the proxy).
    pub scope: serde_json::Value,
    pub kept: Vec<PathBuf>,
    pub keep_error: Option<String>,
    /// `state/kernel/probe/<sha12>/` (purpose `Probe`), else None.
    pub outputs: Option<PathBuf>,
    /// The probe root could not be deleted (X0d): the error, also logged and alerted.
    pub delete_error: Option<String>,
}

/// The proxy lane of one probe (§3.6.3: `probe()` opens the scope with the socket path it chose,
/// `<root>/run/proxy.sock`, and closes it in `stop()`).
pub trait ProbeScope: Send + Sync {
    /// Bind the lane at `socket` before the body is spawned. `probe_id` is the root's id.
    fn open(&self, socket: &Path, probe_id: &str) -> Result<(), String>;
    /// The lane's current `ScopeSummary` (kernel/proxy.md §4.4).
    fn summary(&self) -> serde_json::Value;
    /// Close the lane; its final `ScopeSummary`.
    fn close(&self) -> serde_json::Value;
}

/// A running probe.
pub struct ProbeHandle {
    sp: Arc<dyn Spawner>,
    home: PathBuf,
    root: Option<ProbeRoot>,
    instance: String,
    boot_id: String,
    sha: String,
    opts: ProbeOpts,
    stdin: Mutex<Option<std::fs::File>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    started: Instant,
    ready_at: Mutex<Option<Duration>>,
    ready: Mutex<Option<ReadyFile>>,
    seq: Mutex<u64>,
    exit_grace: Duration,
    pumps: Vec<std::thread::JoinHandle<()>>,
    window_ok: Mutex<bool>,
    scope: Option<Arc<dyn ProbeScope>>,
}

struct Slot;
impl Drop for Slot {
    fn drop(&mut self) {
        RUNNING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// §3.6.3: start a probe of `c` (no proxy lane; see [`probe_scoped`]).
pub fn probe(sp: Arc<dyn Spawner>, home: &Path, cfg: &KernelCfg, c: &Candidate, o: ProbeOpts) -> Result<ProbeHandle, ProbeFail> {
    probe_scoped(sp, home, cfg, c, o, None)
}

/// §3.6.3: start a probe of `c` with its proxy lane `scope` (opened before spawn, closed in `stop()`).
pub fn probe_scoped(sp: Arc<dyn Spawner>, home: &Path, cfg: &KernelCfg, c: &Candidate, o: ProbeOpts, scope: Option<Arc<dyn ProbeScope>>) -> Result<ProbeHandle, ProbeFail> {
    if RUNNING.fetch_add(1, Ordering::SeqCst) >= cfg.supervise.max_concurrent_probes as usize {
        RUNNING.fetch_sub(1, Ordering::SeqCst);
        return Err(ProbeFail::Busy);
    }
    let slot = Slot;
    let git = Git::body(&cfg.supervise.git, home);
    let id = crate::sys::random_hex(6);
    let root = ProbeRoot::create(home, &id, &git, &c.sha, &c.binary, c.ext_set_dir.as_deref(), &o.session, &o.extra_files).map_err(|e| ProbeFail::Root(e.to_string()))?;
    let session = match &o.session {
        SessionSrc::Copy { sid, .. } => sid.clone(),
        SessionSrc::Fresh => crate::layout::new_session_id(),
    };
    let boot_id = crate::sys::random_hex(8);
    let mode = match o.mode {
        InitMode::Normal => "normal",
        InitMode::Probe => "probe",
        InitMode::Safe => "safe",
    };
    let reason = serde_json::to_value(o.boot_reason).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    if let Some(bf) = probe_boot_file(home, root.root(), &boot_id, &session, &c.sha, o.mode, o.boot_reason) {
        let _ = crate::fsx::write_json(&root.boot_json_path(), &bf);
    }
    let run_dir = root.root().join("run");
    if let Some(sc) = &scope {
        let _ = std::fs::create_dir_all(&run_dir);
        if let Err(e) = sc.open(&run_dir.join("proxy.sock"), root.id()) {
            let _ = delete_root(root, home);
            return Err(ProbeFail::Root(format!("proxy lane: {e}")));
        }
    }
    if let Some(h) = lane_hook() {
        let _ = std::fs::create_dir_all(&run_dir);
        h.open(root.id(), o.purpose);
    }
    let test_vars = spawnspec::own_test_vars();
    let env = spawnspec::body_env(&spawnspec::BodyEnv {
        root: root.root(),
        run_dir: &run_dir,
        session: &session,
        sha: &c.sha,
        boot_id: &boot_id,
        boot_reason: &reason,
        body_id: "probe",
        mode,
        kernel_version: crate::supervisor::KERNEL_VERSION,
        sandbox: &cfg.sandbox,
        test_mode: cfg.test_mode,
        test_vars: &test_vars,
        access: None,
    });
    let cwd = root.root().join(o.cwd.clone().unwrap_or_else(|| PathBuf::from("work")));
    let _ = std::fs::create_dir_all(&cwd);
    let req = SpawnRequest {
        kind: InstanceKind::Probe,
        profile: "body".into(),
        params: root.params(&cfg.sandbox),
        program: root.root().join("bin/init-body"),
        args: vec![],
        env,
        cwd,
        lane: None,
        replaces: None,
    };
    let s = match sp.spawn(&req) {
        Ok(s) => s,
        Err(e) => {
            if let Some(sc) = &scope {
                let _ = sc.close();
            }
            if let Some(h) = lane_hook() {
                h.close(root.id());
            }
            let _ = delete_root(root, home);
            return Err(ProbeFail::Spawn(e.to_string()));
        }
    };
    if let Some(h) = lane_hook() {
        h.started(root.id(), &s.instance);
    }
    // stdout → <root>/stdout.jsonl (4 MiB cap); stderr → a 1 MiB tail in memory.
    let mut pumps = Vec::new();
    let stdout_path = root.stdout_path();
    let out = s.stdout;
    pumps.push(std::thread::spawn(move || {
        let mut f = std::fs::File::from(out);
        let mut w = std::fs::OpenOptions::new().create(true).append(true).open(&stdout_path).ok();
        let mut n_written = 0u64;
        let mut buf = [0u8; 8192];
        while let Ok(n) = f.read(&mut buf) {
            if n == 0 {
                break;
            }
            if let Some(w) = w.as_mut() {
                let take = (STDOUT_CAP.saturating_sub(n_written) as usize).min(n);
                if take > 0 && w.write_all(&buf[..take]).is_ok() {
                    n_written += take as u64;
                }
            }
        }
    }));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let e2 = stderr.clone();
    let err = s.stderr;
    pumps.push(std::thread::spawn(move || {
        let mut f = std::fs::File::from(err);
        let mut buf = [0u8; 8192];
        while let Ok(n) = f.read(&mut buf) {
            if n == 0 {
                break;
            }
            let mut t = e2.lock().unwrap();
            t.extend_from_slice(&buf[..n]);
            if t.len() > STDERR_TAIL {
                let cut = t.len() - STDERR_TAIL;
                t.drain(..cut);
            }
        }
    }));
    std::mem::forget(slot); // released when the handle is stopped or dropped
    Ok(ProbeHandle {
        sp,
        home: home.to_path_buf(),
        root: Some(root),
        instance: s.instance,
        boot_id,
        sha: c.sha.clone(),
        opts: o,
        stdin: Mutex::new(Some(std::fs::File::from(s.stdin))),
        stderr,
        started: Instant::now(),
        ready_at: Mutex::new(None),
        ready: Mutex::new(None),
        seq: Mutex::new(0),
        exit_grace: cfg.supervise.exit_grace(),
        pumps,
        window_ok: Mutex::new(false),
        scope,
    })
}

/// The probe boot record: the live `boot.json` as a template (kernel-written), with the probe's
/// identity, `mode`, `reason` and no provisional, crash or update-failed data.
fn probe_boot_file(home: &Path, root: &Path, boot_id: &str, session: &str, sha: &str, mode: InitMode, reason: BootReason) -> Option<BootFile> {
    let mut b: BootFile = std::fs::read(home.join(pf::paths::BOOT)).ok().and_then(|x| pf::decode(&x).ok())?;
    b.body_id = "probe".into();
    b.boot_id = boot_id.into();
    b.boot_seq = 0;
    b.branch = None;
    b.cause = None;
    b.crash = None;
    b.env.init_home = root.to_string_lossy().into_owned();
    b.from_sha = None;
    b.mode = mode;
    b.reason = reason;
    b.provisional = None;
    b.pending = pf::Pending::default();
    b.safe = None;
    b.session = session.into();
    b.sha = sha.into();
    b.spawned_at = crate::clock::now_rfc3339();
    b.update_failed = None;
    Some(b)
}

impl ProbeHandle {
    /// The probe root.
    pub fn root(&self) -> &Path {
        self.root.as_ref().map(|r| r.root()).unwrap_or(Path::new(""))
    }

    /// The Root's instance id.
    pub fn instance(&self) -> &str {
        &self.instance
    }

    fn read_ready(&self) -> Option<ReadyFile> {
        let b = crate::fsx::read_nofollow(&self.root().join(pf::paths::READY), 64 * 1024, false).ok()?;
        pf::decode::<ReadyFile>(&b).ok().filter(|r| r.boot_id == self.boot_id)
    }

    /// Wait for `ready` with this probe's `boot_id` (polled every 100 ms, §3.5).
    pub fn wait_ready(&self, timeout: Duration) -> Result<ReadyFile, ProbeFail> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(r) = self.read_ready() {
                *self.ready_at.lock().unwrap() = Some(self.started.elapsed());
                *self.ready.lock().unwrap() = Some(r.clone());
                return Ok(r);
            }
            if let Some(e) = self.sp.wait_exit(&self.instance, Duration::from_millis(100)) {
                return Err(ProbeFail::Exit(e));
            }
            if Instant::now() >= deadline {
                return Err(ProbeFail::Timeout);
            }
        }
    }

    /// The probe window (§3.5, G2): after ready, `window` with the process alive the whole time
    /// and ≥ 2 heartbeat updates carrying the probe's `boot_id`.
    pub fn window(&self, window: Duration) -> bool {
        let hb = self.root().join(pf::paths::HEARTBEAT);
        let deadline = Instant::now() + window;
        let mut seen: Vec<String> = Vec::new();
        loop {
            if self.sp.wait_exit(&self.instance, Duration::from_millis(250)).is_some() {
                *self.window_ok.lock().unwrap() = false;
                return false;
            }
            if let Ok(b) = crate::fsx::read_nofollow(&hb, 64 * 1024, false) {
                if let Ok(h) = pf::decode::<pf::HeartbeatFile>(&b) {
                    if h.boot_id == self.boot_id && !seen.contains(&h.ts) {
                        seen.push(h.ts);
                    }
                }
            }
            if Instant::now() >= deadline {
                let ok = seen.len() >= 2;
                *self.window_ok.lock().unwrap() = ok;
                return ok;
            }
        }
    }

    /// The proxy lane's current summary (null without a lane).
    pub fn scope_summary(&self) -> serde_json::Value {
        self.scope.as_ref().map(|s| s.summary()).unwrap_or(serde_json::Value::Null)
    }

    /// Whether the instance has exited (non-blocking beyond `wait`).
    pub fn exited(&self, wait: Duration) -> Option<ExitInfo> {
        self.sp.wait_exit(&self.instance, wait)
    }

    /// The SHA this probe runs.
    pub fn sha(&self) -> &str {
        &self.sha
    }

    /// Kernel-originated input (the G3 instruction, G4 task prompts): the owner's `input` frame.
    pub fn send_input(&self, text: &str) -> u64 {
        let mut seq = self.seq.lock().unwrap();
        *seq += 1;
        let f = ToBody::Input { id: crate::sys::random_hex(8), seq: *seq, text: text.into(), ts: crate::clock::now_rfc3339(), mode: InputMode::Steer };
        if let (Ok(b), Some(w)) = (init_proto::stdio::encode_to_body(&f), self.stdin.lock().unwrap().as_mut()) {
            let _ = w.write_all(&b);
        }
        *seq
    }

    /// Stop (stop frame, `exit_grace_s`, then `kill`), wait for `exited`, `keep_on_stop`, copy the
    /// agent-visible outputs (purpose `Probe`), delete the root.
    pub fn stop(mut self) -> ProbeOutcome {
        let stop = ToBody::Stop { reason: "probe_done".into(), grace_ms: self.exit_grace.as_millis() as u64 };
        if let Some(mut w) = self.stdin.lock().unwrap().take() {
            if let Ok(b) = init_proto::stdio::encode_to_body(&stop) {
                let _ = w.write_all(&b);
            }
        }
        let exit = match self.sp.wait_exit(&self.instance, self.exit_grace) {
            Some(e) => Some(e),
            None => {
                let _ = self.sp.kill(&self.instance, KillMode::Kill);
                self.sp.wait_exit(&self.instance, Duration::from_secs(30))
            }
        };
        for p in self.pumps.drain(..) {
            let _ = p.join();
        }
        let root = self.root.take().expect("probe root");
        let last_boot_event = root.last_boot_event();
        let (kept, keep_error) = match root.keep_on_stop(&self.opts.keep_on_stop, &self.home, &self.opts.job) {
            Ok(k) => (k, None),
            Err(KeepError::KeepPath(m)) | Err(KeepError::Io(m)) => (vec![], Some(m)),
        };
        let outputs = if self.opts.purpose == Purpose::Probe {
            let sha12: String = self.sha.chars().take(12).collect();
            let tail = self.stderr.lock().unwrap().clone();
            root.copy_outputs_for_agent(&self.home, &sha12, &root.stdout_path(), &tail, exit.as_ref()).ok()
        } else {
            None // G4 runs stay invisible to the agent (SV-32)
        };
        let ready = self.ready.lock().unwrap().clone().or_else(|| {
            let b = crate::fsx::read_nofollow(&root.ready_path(), 64 * 1024, false).ok()?;
            pf::decode::<ReadyFile>(&b).ok().filter(|r| r.boot_id == self.boot_id)
        });
        let scope = self.scope.take().map(|s| s.close()).unwrap_or(serde_json::Value::Null);
        if let Some(h) = lane_hook() {
            h.close(root.id());
        }
        let delete_error = delete_root(root, &self.home);
        RUNNING.fetch_sub(1, Ordering::SeqCst);
        ProbeOutcome {
            ready_at: *self.ready_at.lock().unwrap(),
            window_ok: *self.window_ok.lock().unwrap(),
            exit,
            ready,
            last_boot_event,
            scope,
            kept,
            keep_error,
            outputs,
            delete_error,
        }
    }
}

/// Delete a probe root by exact path and surface a failure (X0d): a kernel log line and an
/// `alerts.jsonl` line (`source:"probe"`, `code:"probe_root_leak"`), and the error for the
/// caller's [`ProbeOutcome::delete_error`]. A leaked root used to be ignored silently.
pub(crate) fn delete_root(root: ProbeRoot, home: &Path) -> Option<String> {
    let path = root.root().to_path_buf();
    let e = root.delete(home).err()?;
    let rel = path.strip_prefix(home).unwrap_or(&path).display().to_string();
    let msg = format!("probe root {rel} not deleted: {e}");
    crate::klog::log("probe", &msg);
    let line = serde_json::json!({"v": 1, "ts": crate::clock::now_rfc3339(), "source": "probe", "code": "probe_root_leak", "message": msg});
    let _ = std::fs::create_dir_all(home.join("state/kernel"));
    let _ = crate::fsx::append_line(&home.join("state/kernel/alerts.jsonl"), &init_proto::icj::to_line(&line).unwrap_or_default());
    Some(msg)
}

impl Drop for ProbeHandle {
    fn drop(&mut self) {
        // A handle dropped without stop(): kill the instance and remove the root.
        if let Some(root) = self.root.take() {
            let _ = self.sp.kill(&self.instance, KillMode::Kill);
            let _ = self.sp.wait_exit(&self.instance, Duration::from_secs(10));
            if let Some(sc) = self.scope.take() {
                let _ = sc.close();
            }
            if let Some(h) = lane_hook() {
                h.close(root.id());
            }
            let _ = delete_root(root, &self.home);
            RUNNING.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
