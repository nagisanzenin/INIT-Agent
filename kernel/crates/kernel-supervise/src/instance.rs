//! Sandbox instances (supervise.md §3.4): what the kernel asks the Root's spawn broker for, and
//! how the rest of the crate waits for an instance to end.
//!
//! The kernel never spawns or signals an agent-zone process itself. [`Spawner`] is the seam:
//! in production it is the control link ([`crate::link::RootLink`]); in unit tests it is
//! [`LocalSpawner`], which runs the program unsandboxed as the test's own child and stops it
//! only by the pid it holds.

use init_proto::root_link::{InstanceKind, KillMode, SpawnArgs};
use std::collections::{BTreeMap, HashMap};
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// A `LaunchSpec` (§3.4) in its wire shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnRequest {
    pub kind: InstanceKind,
    /// `body`, `build`, `check`, `git-read`.
    pub profile: String,
    pub params: BTreeMap<String, String>,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The complete environment; nothing is inherited (§4.1).
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    /// `b0`…`b15` for bodies.
    pub lane: Option<String>,
    /// The A of an overlapped handoff.
    pub replaces: Option<String>,
}

impl SpawnRequest {
    /// The `spawn` args of root/root.md §4.1.3.
    pub fn to_args(&self) -> SpawnArgs {
        SpawnArgs {
            kind: self.kind.clone(),
            profile: self.profile.clone(),
            params: self.params.clone(),
            program: self.program.to_string_lossy().into_owned(),
            args: self.args.clone(),
            env: self.env.clone(),
            cwd: self.cwd.to_string_lossy().into_owned(),
            lane: self.lane.clone(),
            replaces: self.replaces.clone(),
        }
    }
}

/// A started instance: the Root's id and pid, plus the three pipe ends received over fd 4.
#[derive(Debug)]
pub struct Spawned {
    pub instance: String,
    pub pid: i32,
    pub started_at: String,
    pub stdin: OwnedFd,
    pub stdout: OwnedFd,
    pub stderr: OwnedFd,
}

/// How an instance ended (the Root's `exited` event).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub at: String,
}

impl ExitInfo {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// Why a spawn or kill failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnError {
    /// The Root refused (`root.*` code and message).
    Refused { code: String, message: String },
    /// The control link is down or timed out.
    Link(String),
    /// Local I/O.
    Io(String),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::Refused { code, message } => write!(f, "{code}: {message}"),
            SpawnError::Link(m) => write!(f, "control link: {m}"),
            SpawnError::Io(m) => write!(f, "io: {m}"),
        }
    }
}

impl std::error::Error for SpawnError {}

/// The spawn broker as the kernel sees it.
pub trait Spawner: Send + Sync {
    /// `spawn` (blocks up to 10 s for the Root's answer and the fds).
    fn spawn(&self, req: &SpawnRequest) -> Result<Spawned, SpawnError>;
    /// `kill{instance, mode}`; the instance then ends with an `exited` event.
    fn kill(&self, instance: &str, mode: KillMode) -> Result<(), SpawnError>;
    /// Wait for the instance's `exited` event.
    fn wait_exit(&self, instance: &str, timeout: Duration) -> Option<ExitInfo>;
}

/// `exited` events by instance id, for threads that wait on one instance (builds, probes,
/// git reads). The supervisor also receives every event on its own channel.
#[derive(Default)]
pub struct ExitBoard {
    exits: Mutex<HashMap<String, ExitInfo>>,
    cv: Condvar,
}

impl ExitBoard {
    pub fn new() -> Arc<ExitBoard> {
        Arc::new(ExitBoard::default())
    }

    /// Record an exit (called by the link reader).
    pub fn record(&self, instance: &str, info: ExitInfo) {
        let mut g = self.exits.lock().unwrap();
        if g.len() > 4096 {
            g.clear(); // bounded; waiters only care about recent instances
        }
        g.insert(instance.to_string(), info);
        self.cv.notify_all();
    }

    /// The exit of `instance`, if it has ended.
    pub fn get(&self, instance: &str) -> Option<ExitInfo> {
        self.exits.lock().unwrap().get(instance).cloned()
    }

    /// Wait up to `timeout` for `instance` to end.
    pub fn wait(&self, instance: &str, timeout: Duration) -> Option<ExitInfo> {
        let deadline = Instant::now() + timeout;
        let mut g = self.exits.lock().unwrap();
        loop {
            if let Some(e) = g.get(instance) {
                return Some(e.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            g = self.cv.wait_timeout(g, deadline - now).unwrap().0;
        }
    }
}

/// Test spawner: runs the program unsandboxed as this process's own child (its own process
/// group), passes back the pipe ends, reaps it on a thread, and stops it only by the pid it
/// holds (`kill(pid, SIGTERM)` or `Child::kill`). Never used by the kernel at run time.
pub struct LocalSpawner {
    board: Arc<ExitBoard>,
    children: Arc<Mutex<HashMap<String, Arc<Mutex<std::process::Child>>>>>,
    /// Extra environment added to every spawn (test plans).
    pub extra_env: BTreeMap<String, String>,
}

impl Default for LocalSpawner {
    fn default() -> Self {
        LocalSpawner { board: ExitBoard::new(), children: Arc::default(), extra_env: BTreeMap::new() }
    }
}

impl LocalSpawner {
    pub fn new() -> LocalSpawner {
        LocalSpawner::default()
    }

    /// The board this spawner records exits on.
    pub fn board(&self) -> Arc<ExitBoard> {
        self.board.clone()
    }
}

impl Spawner for LocalSpawner {
    fn spawn(&self, req: &SpawnRequest) -> Result<Spawned, SpawnError> {
        use std::os::unix::process::{CommandExt, ExitStatusExt};
        use std::process::{Command, Stdio};
        let io = |e: std::io::Error| SpawnError::Io(e.to_string());
        let (in_r, in_w) = std::io::pipe().map_err(io)?;
        let (out_r, out_w) = std::io::pipe().map_err(io)?;
        let (err_r, err_w) = std::io::pipe().map_err(io)?;
        let mut cmd = Command::new(&req.program);
        cmd.args(&req.args)
            .env_clear()
            .envs(&req.env)
            .envs(&self.extra_env)
            .current_dir(&req.cwd)
            .process_group(0)
            .stdin(Stdio::from(in_r))
            .stdout(Stdio::from(out_w))
            .stderr(Stdio::from(err_w));
        // Like the Root's `exec-clean`: no fd beyond stdio. On macOS a pipe another thread is
        // creating has no FD_CLOEXEC for a moment (no pipe2), and a child spawned then would hold
        // it open, so a concurrent `Command::output` would wait for this child (K-FLAKE).
        let hi = std::fs::read_dir("/dev/fd").map(|rd| rd.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok()).max().unwrap_or(1024)).unwrap_or(1024) + 256;
        // SAFETY: runs in the forked child before exec; fcntl and close are async-signal-safe.
        unsafe {
            cmd.pre_exec(move || {
                for fd in 3..=hi {
                    let fl = libc::fcntl(fd, libc::F_GETFD);
                    if fl >= 0 && fl & libc::FD_CLOEXEC == 0 {
                        libc::close(fd);
                    }
                }
                Ok(())
            });
        }
        let child = cmd.spawn().map_err(io)?;
        drop(cmd);
        let pid = child.id() as i32;
        let instance = crate::sys::random_hex(6);
        let child = Arc::new(Mutex::new(child));
        self.children.lock().unwrap().insert(instance.clone(), child.clone());
        let board = self.board.clone();
        let children = self.children.clone();
        let id = instance.clone();
        std::thread::spawn(move || {
            // Poll try_wait so `kill` can lock the child meanwhile.
            let status = loop {
                if let Ok(Some(s)) = child.lock().unwrap().try_wait() {
                    break s;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            children.lock().unwrap().remove(&id);
            board.record(&id, ExitInfo { code: status.code(), signal: status.signal(), at: crate::clock::now_rfc3339() });
        });
        Ok(Spawned {
            instance,
            pid,
            started_at: crate::clock::now_rfc3339(),
            stdin: OwnedFd::from(in_w),
            stdout: OwnedFd::from(out_r),
            stderr: OwnedFd::from(err_r),
        })
    }

    fn kill(&self, instance: &str, mode: KillMode) -> Result<(), SpawnError> {
        let child = self.children.lock().unwrap().get(instance).cloned();
        let Some(child) = child else { return Ok(()) }; // already ended
        let mut c = child.lock().unwrap();
        if let Ok(Some(_)) = c.try_wait() {
            return Ok(());
        }
        // The child leads its own process group (`process_group(0)`), as the Root's instances do,
        // and it is unreaped (checked above under the same lock), so that group is ours: signal it
        // first, as the Root does (zone.rs `kill`), so a build's grandchildren (rustc, build
        // scripts) end with it instead of holding its pipes open.
        let pid = c.id() as i32;
        let sig = match mode {
            KillMode::Term => libc::SIGTERM,
            KillMode::Kill => libc::SIGKILL,
        };
        // SAFETY: plain syscalls on a pid (and the group it leads) this spawner started and has
        // not yet reaped.
        unsafe {
            libc::killpg(pid, sig);
        }
        match mode {
            KillMode::Term => {
                // SAFETY: as above.
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
            KillMode::Kill => {
                let _ = c.kill();
            }
        }
        Ok(())
    }

    fn wait_exit(&self, instance: &str, timeout: Duration) -> Option<ExitInfo> {
        self.board.wait(instance, timeout)
    }
}

/// How long [`run_to_completion`] waits, once the instance has exited, for its stdout and
/// stderr to reach EOF. A process that left its group (so the Root's group kill missed it) can
/// hold the pipes open for as long as it lives; after this the pumps are abandoned.
pub const PUMP_DRAIN: Duration = Duration::from_secs(5);

/// Run an instance to completion: copy its stdout and stderr into `log` (capped at `log_cap`
/// bytes), kill it (`kill{mode:"kill"}`, which signals its process group) at `timeout`. Returns
/// the exit and whether it timed out. Used for builds, checks and git reads. Bounded: after the
/// exit it waits at most [`PUMP_DRAIN`] for the pipes, never for a descendant that still holds
/// them.
pub fn run_to_completion(
    sp: &dyn Spawner,
    req: &SpawnRequest,
    timeout: Duration,
    log: Option<&std::path::Path>,
    log_cap: u64,
) -> Result<(ExitInfo, bool), SpawnError> {
    use std::io::{Read, Write};
    let s = sp.spawn(req)?;
    drop(s.stdin);
    let sink: Arc<Mutex<Option<std::fs::File>>> = Arc::new(Mutex::new(match log {
        Some(p) => Some(std::fs::OpenOptions::new().create(true).append(true).open(p).map_err(|e| SpawnError::Io(e.to_string()))?),
        None => None,
    }));
    let written = Arc::new(Mutex::new(0u64));
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    for fd in [s.stdout, s.stderr] {
        let sink = sink.clone();
        let written = written.clone();
        let done = done_tx.clone();
        std::thread::spawn(move || {
            let _done = DropSend(done);
            let mut f = std::fs::File::from(fd);
            let mut buf = [0u8; 8192];
            while let Ok(n) = f.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut w = written.lock().unwrap();
                if let Some(out) = sink.lock().unwrap().as_mut() {
                    let room = log_cap.saturating_sub(*w) as usize;
                    let take = room.min(n);
                    if take > 0 {
                        let _ = out.write_all(&buf[..take]);
                        *w += take as u64;
                    }
                }
            }
        });
    }
    drop(done_tx);
    let mut timed_out = false;
    let exit = match sp.wait_exit(&s.instance, timeout) {
        Some(e) => e,
        None => {
            timed_out = true;
            let _ = sp.kill(&s.instance, KillMode::Kill);
            sp.wait_exit(&s.instance, Duration::from_secs(30)).unwrap_or(ExitInfo { code: None, signal: Some(9), at: crate::clock::now_rfc3339() })
        }
    };
    // Both pumps end at EOF; give them PUMP_DRAIN, then stop logging and leave them to finish
    // whenever the last holder of the pipes goes.
    let deadline = Instant::now() + PUMP_DRAIN;
    for _ in 0..2 {
        if done_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).is_err() {
            *sink.lock().unwrap_or_else(|e| e.into_inner()) = None;
            break;
        }
    }
    Ok((exit, timed_out))
}

/// Sends on drop, so a pump that ends in any way (EOF, error, panic) reports it.
struct DropSend(std::sync::mpsc::Sender<()>);

impl Drop for DropSend {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(program: &str, args: &[&str]) -> SpawnRequest {
        SpawnRequest {
            kind: InstanceKind::Build,
            profile: "build".into(),
            params: BTreeMap::new(),
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: BTreeMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string())]),
            cwd: "/".into(),
            lane: None,
            replaces: None,
        }
    }

    #[test]
    fn local_spawner_runs_and_times_out() {
        let sp = LocalSpawner::new();
        let d = crate::testutil::test_dir("instance");
        std::fs::create_dir_all(&d).unwrap();
        let log = d.join("out.log");
        let (e, to) = run_to_completion(&sp, &req("/bin/sh", &["-c", "echo hello; echo err >&2; exit 3"]), Duration::from_secs(10), Some(&log), 1 << 20).unwrap();
        assert_eq!((e.code, to), (Some(3), false));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("hello") && text.contains("err"));
        let (e, to) = run_to_completion(&sp, &req("/bin/sleep", &["30"]), Duration::from_millis(200), None, 0).unwrap();
        assert!(to);
        assert_eq!(e.signal, Some(9));
        crate::testutil::remove_test_dir(&d);
    }

    /// A timed-out build whose script left a grandchild holding stdout: the kill signals the
    /// instance's group, so the grandchild ends too and the call returns at its bound, not when
    /// the grandchild would have exited (30 s).
    #[test]
    fn timeout_kills_the_group_so_a_grandchild_cannot_hold_the_pipes() {
        let sp = LocalSpawner::new();
        let t = Instant::now();
        let (e, to) = run_to_completion(&sp, &req("/bin/sh", &["-c", "/bin/sleep 30 & echo started; /bin/sleep 30"]), Duration::from_millis(500), None, 0).unwrap();
        assert!(to);
        assert_eq!(e.signal, Some(9));
        assert!(t.elapsed() < Duration::from_secs(4), "returned after {:?}", t.elapsed());
    }

    /// A descendant that outlives the instance (a daemonised build-script child, or one that left
    /// the group so a group kill misses it) holds stdout for 20 s; the pipes are abandoned after
    /// PUMP_DRAIN instead of waiting for it. It ends on its own (`sleep 20`); nothing signals it.
    #[test]
    fn pump_drain_is_bounded_when_a_descendant_escapes_the_group() {
        let sp = LocalSpawner::new();
        let d = crate::testutil::test_dir("instance-drain");
        std::fs::create_dir_all(&d).unwrap();
        let log = d.join("out.log");
        let t = Instant::now();
        let (e, to) = run_to_completion(&sp, &req("/bin/sh", &["-c", "/bin/sleep 20 & echo bye; exit 0"]), Duration::from_secs(10), Some(&log), 1 << 20).unwrap();
        assert_eq!((e.code, to), (Some(0), false));
        let took = t.elapsed();
        assert!(took >= PUMP_DRAIN - Duration::from_millis(500) && took < PUMP_DRAIN + Duration::from_secs(5), "returned after {took:?}");
        assert!(std::fs::read_to_string(&log).unwrap().contains("bye"));
        crate::testutil::remove_test_dir(&d);
    }
}
