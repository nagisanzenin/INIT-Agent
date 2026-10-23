//! A bounded watchdog for process-spawning tests (K-FLAKE): no test may hang.
//!
//! Every process-spawning integration test arms one as its first line:
//!
//! ```ignore
//! let _wd = init_testkit::watchdog::arm("sv10", Duration::from_secs(90));
//! ```
//!
//! If the test is still running after its bound, the watchdog
//! 1. prints a clear message, into the test's captured output and straight to stderr (the CI log);
//! 2. sends `SIGKILL` to the children this test registered ([`spawn`], [`output`], [`track`]), by
//!    pid, each only after `waitid(WNOWAIT)` confirms the pid is still an unreaped child of this
//!    process (so a reused pid is never signalled), plus the process group that child leads, if it
//!    leads one (a zombie or live leader keeps its group id reserved). Nothing scans the process
//!    table: this is never a sweep;
//! 3. fails the test: with its children gone the test thread normally unblocks, and dropping the
//!    guard then panics with the message. If the test thread is still blocked after [`GRACE`], the
//!    watchdog kills every child registered in this process the same way and exits the test binary
//!    with code 101 and the message, so CI fails instead of hanging.
//!
//! Bounds are monotonic: `recv_timeout` and `Instant` use `CLOCK_UPTIME_RAW` on macOS, which stops
//! while the machine sleeps, so a laptop that sleeps mid-run neither fires a watchdog early nor
//! shortens anyone's bound. Pick bounds 3-10x the test's normal runtime under a full parallel run.
//!
//! **Why tests hung (sv10, 2026-10-20).** On macOS std creates pipes and socketpairs with
//! `pipe()`/`socketpair()` and only then sets `FD_CLOEXEC` (there is no `pipe2`), so a child that
//! another thread spawns in that window inherits the pipe. `Command::output()` reads its pipes to
//! EOF, so it then blocks until that unrelated, possibly long-lived, child exits. The fake Roots
//! spawn bodies in the test process while the test (and the in-process supervisor) run `git` with
//! `output()`; when the body that inherits the pipe is the test's own (sv10's rollback body is
//! spawned while the test reads `main`), the test waits for a body that only its own `stop()`
//! ends: a deadlock with no timeout. [`close_leaked_fds`] (used by [`spawn`]) keeps such fds out
//! of the long-lived children the test kit spawns, and [`output`] waits for the child's exit, never
//! for EOF on a pipe. The watchdog bounds whatever is left.

use std::cell::RefCell;
use std::io::{self, Read, Seek, Write};
use std::os::fd::RawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

/// How long the watchdog waits, after killing a test's children, for the test thread to finish
/// before it exits the test binary.
pub const GRACE: Duration = Duration::from_secs(30);

/// Seconds, for bounds.
pub const fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

/// One test's registered children.
struct Reg {
    name: String,
    pids: Mutex<Vec<i32>>,
}

/// A test's registry of spawned children. Helper threads that spawn on a test's behalf (a fake
/// Root's broker) take the test's scope with [`current`] and [`Scope::enter`] it.
#[derive(Clone)]
pub struct Scope(Arc<Reg>);

thread_local! {
    static CURRENT: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

/// Every scope ever armed in this process (weak) plus the unscoped registrations, for the last
/// resort.
fn all() -> &'static Mutex<Vec<Weak<Reg>>> {
    static ALL: OnceLock<Mutex<Vec<Weak<Reg>>>> = OnceLock::new();
    ALL.get_or_init(Mutex::default)
}

fn unscoped() -> &'static Arc<Reg> {
    static U: OnceLock<Arc<Reg>> = OnceLock::new();
    U.get_or_init(|| {
        let r = Arc::new(Reg { name: "(unscoped)".into(), pids: Mutex::default() });
        lock(all()).push(Arc::downgrade(&r));
        r
    })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Scope {
    fn new(name: &str) -> Scope {
        let r = Arc::new(Reg { name: name.to_string(), pids: Mutex::default() });
        let mut a = lock(all());
        a.retain(|w| w.strong_count() > 0);
        a.push(Arc::downgrade(&r));
        Scope(r)
    }

    /// The test this scope belongs to.
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// Register a child this test spawned (by pid).
    pub fn track(&self, pid: u32) {
        lock(&self.0.pids).push(pid as i32);
    }

    /// Make this the current scope of this thread until the guard drops.
    pub fn enter(&self) -> Entered {
        let prev = CURRENT.with(|c| c.replace(Some(self.clone())));
        Entered { prev }
    }

    fn kill_children(&self) -> Vec<i32> {
        kill_registered(&self.0)
    }
}

/// Restores the previous scope of the thread on drop.
pub struct Entered {
    prev: Option<Scope>,
}

impl Drop for Entered {
    fn drop(&mut self) {
        let prev = self.prev.take();
        CURRENT.with(|c| *c.borrow_mut() = prev);
    }
}

/// The current thread's scope (the armed test's, or one a helper thread entered).
pub fn current() -> Option<Scope> {
    CURRENT.with(|c| c.borrow().clone())
}

/// Register a child spawned by this thread with the current scope (or with the process-wide
/// unscoped list, which only the last resort kills).
pub fn track(pid: u32) {
    match current() {
        Some(s) => s.track(pid),
        None => lock(&unscoped().pids).push(pid as i32),
    }
}

/// Is `pid` still an unreaped child of this process (running or a zombie)? `waitid(WNOWAIT |
/// WNOHANG)` neither reaps nor blocks; a pid that was reaped (and may have been reused) or was
/// never ours fails with `ECHILD`.
pub fn is_own_unreaped_child(pid: i32) -> bool {
    if pid <= 1 {
        return false;
    }
    // SAFETY: siginfo_t is plain data, zero-initialised and written by the kernel.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) == 0
    }
}

/// `SIGKILL` each registered pid that is still our unreaped child, and the process group it
/// leads. Returns the pids signalled.
fn kill_registered(r: &Reg) -> Vec<i32> {
    let pids: Vec<i32> = lock(&r.pids).clone();
    let mut killed = Vec::new();
    for pid in pids {
        if !is_own_unreaped_child(pid) {
            continue;
        }
        // SAFETY: plain syscalls on a pid that is our own unreaped child (checked just above), so
        // neither the pid nor a group it leads can belong to anyone else.
        unsafe {
            if libc::getpgid(pid) == pid {
                libc::killpg(pid, libc::SIGKILL);
            }
            libc::kill(pid, libc::SIGKILL);
        }
        killed.push(pid);
    }
    killed
}

/// Kill every registered child of this process (the last resort).
fn kill_all_registered() -> Vec<i32> {
    let regs: Vec<Arc<Reg>> = lock(all()).iter().filter_map(Weak::upgrade).collect();
    regs.iter().flat_map(|r| kill_registered(r)).collect()
}

/// The armed watchdog of one test; disarmed when dropped.
pub struct Watchdog {
    name: String,
    bound: Duration,
    started: Instant,
    disarm: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    fired: Arc<Mutex<Option<String>>>,
    _entered: Entered,
}

/// Arm the watchdog for the current test: fail it (see the module docs) if it runs longer than
/// `bound`. Children spawned through this module on this thread, or on helper threads that
/// [`Scope::enter`] the scope from [`current`], belong to this test.
pub fn arm(name: &str, bound: Duration) -> Watchdog {
    let scope = Scope::new(name);
    let entered = scope.enter();
    let (tx, rx) = mpsc::channel::<()>();
    let fired: Arc<Mutex<Option<String>>> = Arc::default();
    let f = fired.clone();
    let n = name.to_string();
    let thread = std::thread::Builder::new()
        .name(format!("watchdog:{name}"))
        .spawn(move || {
            if rx.recv_timeout(bound) != Err(RecvTimeoutError::Timeout) {
                return; // disarmed
            }
            let killed = scope.kill_children();
            let msg = format!(
                "watchdog: test `{n}` did not finish within its bound of {bound:?} (3-10x its normal runtime): a wait in the test or the product has no bound. Sent SIGKILL to its own children by pid: {killed:?}"
            );
            *lock(&f) = Some(msg.clone());
            eprintln!("{msg}"); // into the test's captured output
            let _ = writeln!(io::stderr(), "{msg}"); // and the CI log, at once
            if rx.recv_timeout(GRACE) == Err(RecvTimeoutError::Timeout) {
                let all = kill_all_registered();
                let _ = writeln!(
                    io::stderr(),
                    "watchdog: test `{n}` is still blocked {GRACE:?} after its children were killed; killed every child this test binary registered ({all:?}) and exiting with 101 so CI fails instead of hanging"
                );
                std::process::exit(101);
            }
        })
        .expect("spawn the watchdog thread");
    Watchdog {
        name: name.to_string(),
        bound,
        started: Instant::now(),
        disarm: Some(tx),
        thread: Some(thread),
        fired,
        _entered: entered,
    }
}

impl Watchdog {
    /// Elapsed since arming.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed();
        if let Some(tx) = self.disarm.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // `INIT_WATCHDOG_LOG=<file>`: one line per test (name, ms, bound ms), to size the bounds.
        if let Some(p) = std::env::var_os("INIT_WATCHDOG_LOG") {
            let line = format!("{}\t{}\t{}\t{}\n", self.name, elapsed.as_millis(), self.bound.as_millis(), if std::thread::panicking() { "panicked" } else { "ok" });
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = f.write_all(line.as_bytes());
            }
        }
        let fired = lock(&self.fired).take();
        if let Some(msg) = fired {
            if std::thread::panicking() {
                eprintln!("{msg}");
            } else {
                panic!("{msg}");
            }
        }
    }
}

/// Make the child close every fd at or above `from` that lacks `FD_CLOEXEC` (one another thread
/// created a moment ago and had not flagged yet: see the module docs), so a long-lived child never
/// holds someone else's pipe open. Fds the caller passes on purpose below `from` (stdio, a dup2
/// to 3 and 4 from an earlier `pre_exec`) are untouched, and so is std's own `FD_CLOEXEC` error
/// pipe. The scan runs to the highest fd open now plus a margin, because the soft fd limit can be
/// a million.
pub fn close_leaked_fds(cmd: &mut Command, from: RawFd) {
    let hi = highest_open_fd().saturating_add(256).max(from + 64);
    // SAFETY: the closure runs in the forked child before exec and calls only fcntl and close,
    // which are async-signal-safe.
    unsafe {
        cmd.pre_exec(move || {
            for fd in from..=hi {
                let fl = libc::fcntl(fd, libc::F_GETFD);
                if fl >= 0 && fl & libc::FD_CLOEXEC == 0 {
                    libc::close(fd);
                }
            }
            Ok(())
        });
    }
}

/// The highest fd this process has open (from `/dev/fd`), or 1024 when it cannot be listed.
fn highest_open_fd() -> RawFd {
    match std::fs::read_dir("/dev/fd") {
        Ok(rd) => rd.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<RawFd>().ok()).max().unwrap_or(1024),
        Err(_) => 1024,
    }
}

/// Spawn a long-lived child for the current test: leaked fds closed in the child
/// ([`close_leaked_fds`] from 3), and the pid registered so the watchdog can kill it.
pub fn spawn(cmd: &mut Command) -> io::Result<Child> {
    spawn_keeping(cmd, 3)
}

/// [`spawn`], keeping fds below `from` (e.g. 5 for a kernel that inherits fds 3 and 4).
pub fn spawn_keeping(cmd: &mut Command, from: RawFd) -> io::Result<Child> {
    close_leaked_fds(cmd, from);
    let c = cmd.spawn()?;
    track(c.id());
    Ok(c)
}

/// `cmd.output()` without pipes: stdout and stderr go to unlinked temporary files and the call
/// waits for the child's **exit**, never for EOF on a pipe that an unrelated child may have
/// inherited (module docs). Stdin is `/dev/null`. The child is registered with the current scope,
/// so a hung one is killed by the test's watchdog.
pub fn output(cmd: &mut Command) -> io::Result<Output> {
    let mut out = scratch_file()?;
    let mut err = scratch_file()?;
    cmd.stdin(Stdio::null()).stdout(out.try_clone()?).stderr(err.try_clone()?);
    let mut c = spawn(cmd)?;
    let status: ExitStatus = c.wait()?;
    let read = |f: &mut std::fs::File| -> io::Result<Vec<u8>> {
        let mut v = Vec::new();
        f.rewind()?;
        f.read_to_end(&mut v)?;
        Ok(v)
    };
    Ok(Output { status, stdout: read(&mut out)?, stderr: read(&mut err)? })
}

/// `cmd.status()` for the current test: spawned through [`spawn`] (stdio inherited), so the
/// watchdog can kill a hung one.
pub fn status(cmd: &mut Command) -> io::Result<ExitStatus> {
    spawn(cmd)?.wait()
}

/// An unlinked, read-write temporary file under `kernel/target/test-homes/kflake/`.
fn scratch_file() -> io::Result<std::fs::File> {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = scratch_dir();
    std::fs::create_dir_all(&dir)?;
    let p = dir.join(format!("out-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let f = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&p)?;
    std::fs::remove_file(&p)?;
    Ok(f)
}

fn scratch_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/kflake")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_child_check_is_true_until_reaped() {
        let mut c = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = c.id() as i32;
        assert!(is_own_unreaped_child(pid), "running child");
        c.kill().unwrap();
        // A zombie is still ours (and keeps its pid reserved).
        std::thread::sleep(Duration::from_millis(100));
        assert!(is_own_unreaped_child(pid), "zombie child");
        c.wait().unwrap();
        assert!(!is_own_unreaped_child(pid), "reaped");
        assert!(!is_own_unreaped_child(1), "launchd/init is never ours");
        assert!(!is_own_unreaped_child(std::process::id() as i32), "not our own pid");
    }

    #[test]
    fn output_waits_for_exit_not_for_a_leaked_pipe() {
        let _wd = arm("output_waits_for_exit_not_for_a_leaked_pipe", secs(20));
        // A child that forks a grandchild holding stdout for 30 s: `Command::output` would block
        // for 30 s on the pipe; `output` returns when the child exits.
        let t = Instant::now();
        let o = output(Command::new("/bin/sh").args(["-c", "echo hi; (sleep 30 &) ; exit 3"])).unwrap();
        assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
        assert_eq!(o.status.code(), Some(3));
        assert_eq!(o.stdout, b"hi\n");
    }

    #[test]
    fn spawned_children_do_not_inherit_unflagged_fds() {
        let _wd = arm("spawned_children_do_not_inherit_unflagged_fds", secs(20));
        // An fd without FD_CLOEXEC, as a concurrent pipe() leaves it for a moment.
        let (r, w) = std::io::pipe().unwrap();
        use std::os::fd::AsRawFd;
        let raw = w.as_raw_fd();
        // SAFETY: clearing FD_CLOEXEC on an fd we own.
        unsafe { libc::fcntl(raw, libc::F_SETFD, 0) };
        let o = output(Command::new("/bin/sh").args(["-c", &format!("[ -e /dev/fd/{raw} ] && echo leaked || echo closed")])).unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "closed");
        drop((r, w));
    }

    #[test]
    fn a_hung_test_fails_and_its_child_is_killed() {
        let r = std::thread::spawn(|| {
            let _wd = arm("hung", Duration::from_millis(300));
            let mut c = spawn(Command::new("/bin/sleep").arg("30").stdout(Stdio::null())).unwrap();
            // Blocks until the watchdog kills the child, then the guard's drop panics.
            let st = c.wait().unwrap();
            assert_eq!(std::os::unix::process::ExitStatusExt::signal(&st), Some(libc::SIGKILL));
        })
        .join();
        let e = r.expect_err("the test fails");
        let msg = e.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.starts_with("watchdog: test `hung` did not finish within"), "{msg}");
    }

    #[test]
    fn a_finished_test_does_not_fire() {
        let wd = arm("quick", secs(5));
        let mut c = spawn(Command::new("/bin/sh").args(["-c", "exit 0"])).unwrap();
        assert!(c.wait().unwrap().success());
        assert!(wd.elapsed() < secs(5));
        drop(wd);
    }
}
