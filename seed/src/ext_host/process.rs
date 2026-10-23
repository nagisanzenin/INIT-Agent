//! The `init:ext/process` host: one-shot `run` and pipe sessions (ext/host.md §3.13, ext/wit.md
//! §3.2, §4.3).
//!
//! Every child is spawned here with `process_group(0)`, so it leads its own group; the host holds
//! its pid (= pgid) and kills only that group (`tools::bash::kill_group`, which refuses pgid ≤ 1),
//! then reaps it. Nothing here ever signals a process the host did not spawn.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::bindings::v1::init::ext::process::{
    ProcessError, ReadResult, RunOutput, RunRequest, SpawnRequest,
};
use super::state::GuestRoots;
use crate::tools::bash::{CaptureSpec, Captured, RunSpec, Signal, kill_group, run_group};

/// ext/wit.md §4.3.
pub const RUN_OUTPUT_MAX: usize = 1 << 20;
pub const RUN_STDIN_MAX: usize = 1 << 20;
pub const SESSIONS_PER_EXT: usize = 4;
pub const SESSIONS_TOTAL: usize = 16;
pub const SESSION_IDLE: Duration = Duration::from_secs(30 * 60);
pub const SESSION_BUFFER_MAX: usize = 4 << 20;
pub const READ_MAX: usize = 1 << 20;
pub const READ_QUIET: Duration = Duration::from_millis(50);
pub const WRITE_MAX: usize = 1 << 20;
pub const WRITE_QUEUED_MAX: usize = 4 << 20;

type OutPair = Arc<(Mutex<OutBuf>, Condvar)>;

#[derive(Default)]
struct OutBuf {
    data: VecDeque<u8>,
    truncated: bool,
    eof: bool,
    last_byte_at: Option<Instant>,
}

struct Session {
    child: Child,
    pgid: i32,
    /// The writer thread owns `ChildStdin`; dropping the sender ends it.
    stdin_tx: Option<SyncSender<Vec<u8>>>,
    queued: Arc<AtomicUsize>,
    out: OutPair,
    last_used: Instant,
    threads: Vec<JoinHandle<()>>,
}

pub(crate) struct ProcHost {
    sessions: Mutex<BTreeMap<(String, u32), Session>>,
    /// Per extension, starts at 1; ids are never reused within one body process.
    next_id: Mutex<BTreeMap<String, u32>>,
    idle: Mutex<Duration>,
}

fn denied(m: impl Into<String>) -> ProcessError {
    ProcessError::Denied(m.into())
}

fn spawn_error(e: &io::Error, argv0: &str) -> ProcessError {
    match e.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => {
            ProcessError::NotFound(format!("{argv0}: {e}"))
        }
        _ => ProcessError::Io(format!("{argv0}: {e}")),
    }
}

/// Maps a guest directory (`/work[/rel]` or `/data[/rel]`) to a host directory: no `..`, no NUL,
/// must exist, and must still be inside its root after resolving symlinks.
pub(crate) fn map_guest_dir(roots: &GuestRoots, guest: &str) -> Result<PathBuf, ProcessError> {
    if guest.contains('\0') {
        return Err(denied("cwd contains a NUL byte"));
    }
    let (root, rest) = if let Some(r) = strip_root(guest, "/work") {
        (&roots.work, r)
    } else if let Some(r) = strip_root(guest, "/data") {
        (&roots.data, r)
    } else {
        return Err(denied(format!("cwd {guest:?} is outside /work and /data")));
    };
    let rel = Path::new(rest);
    for c in rel.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => {
                return Err(denied(format!(
                    "cwd {guest:?}: \"..\" and absolute components are not allowed"
                )));
            }
        }
    }
    let croot = root
        .canonicalize()
        .map_err(|e| denied(format!("cwd {guest:?}: {e}")))?;
    let p = croot
        .join(rel)
        .canonicalize()
        .map_err(|e| denied(format!("cwd {guest:?}: {e}")))?;
    if !p.starts_with(&croot) {
        return Err(denied(format!("cwd {guest:?} resolves outside its root")));
    }
    if !p.is_dir() {
        return Err(denied(format!("cwd {guest:?} is not a directory")));
    }
    Ok(p)
}

/// `"/work"` → `""`, `"/work/a/b"` → `"a/b"`, `"/workx"` → None.
fn strip_root<'a>(guest: &'a str, root: &str) -> Option<&'a str> {
    let rest = guest.strip_prefix(root)?;
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix('/')
    }
}

fn kill_and_reap(s: &mut Session) {
    if !kill_group(s.pgid, Signal::Kill) {
        let _ = s.child.kill(); // the leader alone, if `kill` could not be run
    }
    let _ = s.child.wait();
    s.stdin_tx = None;
    // The reader ends at EOF once the group is dead; never wait long for a stray holder of the pipe.
    let end = Instant::now() + Duration::from_secs(1);
    for t in s.threads.drain(..) {
        while !t.is_finished() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(2));
        }
        if t.is_finished() {
            let _ = t.join();
        }
    }
}

impl ProcHost {
    pub(crate) fn new() -> Self {
        ProcHost {
            sessions: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(BTreeMap::new()),
            idle: Mutex::new(SESSION_IDLE),
        }
    }

    /// Test hook (host.md H14: "idle reap with a 1 s test override").
    pub(crate) fn set_idle_timeout(&self, d: Duration) {
        *self.idle.lock().unwrap_or_else(|e| e.into_inner()) = d;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, u32), Session>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `run`: the same helper the `bash` tool uses (tools.md §3.5.7), separate capture.
    pub(crate) fn run(
        &self,
        roots: &GuestRoots,
        req: RunRequest,
        deadline: Instant,
        cancel: &AtomicBool,
    ) -> Result<RunOutput, ProcessError> {
        let Some(argv0) = req.argv.first() else {
            return Err(denied("argv is empty"));
        };
        if req.stdin.len() > RUN_STDIN_MAX {
            return Err(denied(format!(
                "stdin is {} bytes; the limit is 1 MiB",
                req.stdin.len()
            )));
        }
        let cwd = map_guest_dir(roots, &req.cwd)?;
        let mut cmd = Command::new(argv0);
        cmd.args(&req.argv[1..]).current_dir(&cwd);
        for (k, v) in &req.env {
            cmd.env(k, v);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        let timeout = Duration::from_millis(req.timeout_ms as u64).min(left);
        let spec = RunSpec {
            stdin: if req.stdin.is_empty() {
                None
            } else {
                Some(&req.stdin)
            },
            timeout,
            cancel: Some(cancel),
            on_spawn: None,
            capture: CaptureSpec::Separate {
                max_each: RUN_OUTPUT_MAX,
            },
        };
        let spawned_ms = crate::tools::memcap::wall_ms();
        let out = run_group(cmd, spec).map_err(|e| spawn_error(&e, argv0))?;
        // MEM-RUNAWAY (tools.md §3.5.8): the Root's watchdog says why a process died by a signal, on stderr
        let root_kill = out
            .status
            .and_then(|s| std::os::unix::process::ExitStatusExt::signal(&s))
            .filter(|_| !out.timed_out && !out.cancelled)
            .and_then(|_| crate::tools::memcap::Config::from_env().lookup(out.pgid, spawned_ms));
        let (stdout, mut stderr, truncated) = match out.output {
            Captured::Separate {
                stdout,
                stderr,
                stdout_total,
                stderr_total,
            } => {
                let t = stdout_total > stdout.len() as u64 || stderr_total > stderr.len() as u64;
                (stdout, stderr, t)
            }
            Captured::Merged { text, .. } => (text.into_bytes(), Vec::new(), false),
        };
        if let Some(k) = root_kill {
            stderr.extend_from_slice(format!("\n[{}]\n", k.text()).as_bytes());
        }
        Ok(RunOutput {
            exit_code: out.status.and_then(|s| s.code()),
            stdout,
            stderr,
            timed_out: out.timed_out,
            truncated,
        })
    }

    pub(crate) fn spawn(
        &self,
        ext: &str,
        roots: &GuestRoots,
        req: SpawnRequest,
    ) -> Result<u32, ProcessError> {
        if req.pty {
            return Err(denied("pty is not supported by this body"));
        }
        let Some(argv0) = req.argv.first() else {
            return Err(denied("argv is empty"));
        };
        let cwd = map_guest_dir(roots, &req.cwd)?;
        {
            let g = self.lock();
            let mine = g.keys().filter(|(e, _)| e == ext).count();
            if mine >= SESSIONS_PER_EXT {
                return Err(denied(format!(
                    "session limit reached: {SESSIONS_PER_EXT} per extension"
                )));
            }
            if g.len() >= SESSIONS_TOTAL {
                return Err(denied(format!(
                    "session limit reached: {SESSIONS_TOTAL} per body"
                )));
            }
        }
        let (r, w) = io::pipe().map_err(|e| ProcessError::Io(e.to_string()))?;
        let w2 = w.try_clone().map_err(|e| ProcessError::Io(e.to_string()))?;
        let mut cmd = Command::new(argv0);
        cmd.args(&req.argv[1..])
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(w2)
            .stderr(w)
            .process_group(0);
        for (k, v) in &req.env {
            cmd.env(k, v);
        }
        let spawned = cmd.spawn();
        drop(cmd); // the parent's write ends close here, or EOF never comes
        let mut child = spawned.map_err(|e| spawn_error(&e, argv0))?;
        let pgid = child.id() as i32;
        let stdin = child.stdin.take();

        let out: OutPair = Arc::new((Mutex::new(OutBuf::default()), Condvar::new()));
        let reader = {
            let out = out.clone();
            let mut r = r;
            std::thread::Builder::new()
                .name(format!("ext-proc-r-{pgid}"))
                .spawn(move || {
                    let mut buf = vec![0u8; 64 << 10];
                    loop {
                        let n = match r.read(&mut buf) {
                            Ok(0) | Err(_) => 0,
                            Ok(n) => n,
                        };
                        let (m, cv) = &*out;
                        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
                        if n == 0 {
                            g.eof = true;
                            cv.notify_all();
                            return;
                        }
                        g.data.extend(&buf[..n]);
                        if g.data.len() > SESSION_BUFFER_MAX {
                            let drop_n = g.data.len() - SESSION_BUFFER_MAX;
                            g.data.drain(..drop_n);
                            g.truncated = true;
                        }
                        g.last_byte_at = Some(Instant::now());
                        cv.notify_all();
                    }
                })
        };
        let queued = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(256);
        let writer = {
            let queued = queued.clone();
            std::thread::Builder::new()
                .name(format!("ext-proc-w-{pgid}"))
                .spawn(move || {
                    let mut stdin = stdin;
                    for chunk in rx {
                        let n = chunk.len();
                        if let Some(s) = stdin.as_mut()
                            && s.write_all(&chunk).and_then(|_| s.flush()).is_err()
                        {
                            stdin = None; // the child closed its stdin; keep draining the queue
                        }
                        queued.fetch_sub(n, Ordering::SeqCst);
                    }
                })
        };
        let mut threads = Vec::new();
        let mut thread_err = None;
        for t in [reader, writer] {
            match t {
                Ok(h) => threads.push(h),
                Err(e) => thread_err = Some(e),
            }
        }
        let mut s = Session {
            child,
            pgid,
            stdin_tx: Some(tx),
            queued,
            out,
            last_used: Instant::now(),
            threads,
        };
        if let Some(e) = thread_err {
            kill_and_reap(&mut s);
            return Err(ProcessError::Io(format!("thread spawn failed: {e}")));
        }
        let id = {
            let mut ids = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
            let n = ids.entry(ext.to_string()).or_insert(1);
            let id = *n;
            *n = n.saturating_add(1);
            id
        };
        self.lock().insert((ext.to_string(), id), s);
        Ok(id)
    }

    pub(crate) fn write(&self, ext: &str, id: u32, data: Vec<u8>) -> Result<(), ProcessError> {
        if data.len() > WRITE_MAX {
            return Err(denied(format!(
                "write is {} bytes; the limit is 1 MiB per call",
                data.len()
            )));
        }
        let mut g = self.lock();
        let Some(s) = g.get_mut(&(ext.to_string(), id)) else {
            return Err(denied(format!("unknown session {id}")));
        };
        s.last_used = Instant::now();
        let n = data.len();
        if s.queued.load(Ordering::SeqCst) + n > WRITE_QUEUED_MAX {
            return Err(denied("more than 4 MiB queued for the session's stdin"));
        }
        let Some(tx) = &s.stdin_tx else {
            return Err(denied("the session's stdin is closed"));
        };
        s.queued.fetch_add(n, Ordering::SeqCst);
        match tx.try_send(data) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                s.queued.fetch_sub(n, Ordering::SeqCst);
                Err(denied("the session's stdin queue is full"))
            }
            Err(TrySendError::Disconnected(_)) => {
                s.queued.fetch_sub(n, Ordering::SeqCst);
                Err(denied("the session's stdin is closed"))
            }
        }
    }

    pub(crate) fn read(
        &self,
        ext: &str,
        id: u32,
        wait_ms: u32,
        until: Option<String>,
        deadline: Instant,
        cancel: &AtomicBool,
    ) -> Result<ReadResult, ProcessError> {
        let out = {
            let mut g = self.lock();
            let Some(s) = g.get_mut(&(ext.to_string(), id)) else {
                return Err(denied(format!("unknown session {id}")));
            };
            s.last_used = Instant::now();
            s.out.clone()
        };
        let start = Instant::now();
        let wait =
            Duration::from_millis(wait_ms as u64).min(deadline.saturating_duration_since(start));
        let end = start + wait;
        let until = until.filter(|u| !u.is_empty()).map(String::into_bytes);
        let (m, cv) = &*out;
        let mut b = m.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let now = Instant::now();
            let found = match &until {
                Some(u) => contains(&b.data, u),
                None => {
                    !b.data.is_empty()
                        && b.last_byte_at
                            .is_some_and(|t| now.duration_since(t) >= READ_QUIET)
                }
            };
            if found || b.eof || now >= end || cancel.load(Ordering::SeqCst) {
                break;
            }
            let mut slice = READ_QUIET.min(end - now);
            if until.is_none()
                && !b.data.is_empty()
                && let Some(t) = b.last_byte_at
            {
                let q = (t + READ_QUIET).saturating_duration_since(now);
                slice = slice.min(q.max(Duration::from_millis(1)));
            }
            b = cv
                .wait_timeout(b, slice)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        let take = b.data.len().min(READ_MAX);
        let data: Vec<u8> = b.data.drain(..take).collect();
        let truncated = std::mem::take(&mut b.truncated);
        let drained = b.eof && b.data.is_empty();
        drop(b);
        let (mut exited, mut exit_code) = (false, None);
        if drained {
            let mut g = self.lock();
            if let Some(s) = g.get_mut(&(ext.to_string(), id))
                && let Ok(Some(st)) = s.child.try_wait()
            {
                exited = true;
                exit_code = st.code();
            }
        }
        Ok(ReadResult {
            data,
            truncated,
            exited,
            exit_code,
        })
    }

    pub(crate) fn close(&self, ext: &str, id: u32) -> Result<(), ProcessError> {
        let s = self.lock().remove(&(ext.to_string(), id));
        let Some(mut s) = s else {
            return Err(denied(format!("unknown session {id}")));
        };
        kill_and_reap(&mut s);
        Ok(())
    }

    pub(crate) fn sessions(&self, ext: &str) -> Vec<u32> {
        self.lock()
            .keys()
            .filter(|(e, _)| e == ext)
            .map(|(_, id)| *id)
            .collect()
    }

    /// Process-group ids of the open sessions (tests and the reload report).
    pub(crate) fn session_pgids(&self) -> Vec<i32> {
        self.lock().values().map(|s| s.pgid).collect()
    }

    pub(crate) fn count(&self) -> usize {
        self.lock().len()
    }

    /// Closes sessions idle longer than the idle timeout (called at every `call` and
    /// `begin_reload`; no timer thread).
    pub(crate) fn reap_idle(&self, now: Instant) {
        let idle = *self.idle.lock().unwrap_or_else(|e| e.into_inner());
        let stale: Vec<Session> = {
            let mut g = self.lock();
            let keys: Vec<(String, u32)> = g
                .iter()
                .filter(|(_, s)| now.saturating_duration_since(s.last_used) >= idle)
                .map(|(k, _)| k.clone())
                .collect();
            keys.iter().filter_map(|k| g.remove(k)).collect()
        };
        for mut s in stale {
            kill_and_reap(&mut s);
        }
    }

    /// Kills every session group (every orderly body exit).
    pub(crate) fn shutdown(&self) {
        let all: Vec<Session> = std::mem::take(&mut *self.lock()).into_values().collect();
        for mut s in all {
            kill_and_reap(&mut s);
        }
    }
}

impl Drop for ProcHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn contains(hay: &VecDeque<u8>, needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    let (a, b) = hay.as_slices();
    if b.is_empty() {
        return a.windows(needle.len()).any(|w| w == needle);
    }
    let v: Vec<u8> = hay.iter().copied().collect();
    v.windows(needle.len()).any(|w| w == needle)
}
