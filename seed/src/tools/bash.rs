//! `bash` (tools.md §3.5) and the shared process-group helpers `run_group` / `kill_group`
//! (§3.5.7, also used by ext/host.md `process.run` and boot.md's git sync).
//!
//! One process group per call (`process_group(0)`), one merged pipe, a reader thread and a waiter
//! thread, no polling. Kills go through `/bin/kill -s SIG -- -PGID` (no `unsafe`, §3.5.4) and only
//! ever target a group this body spawned and holds the id of.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use super::args::{as_int, check_fields, req_str};
use super::{Limits, Ran, ToolCx, ToolError, Tools, commas};
use crate::model::{ContentBlock, ToolMeta, ToolResult};

/// Inline output limit, and the head/tail kept when it is exceeded (§3.5.2).
pub const INLINE: usize = 30_000;
pub const HEAD: usize = 4_000;
pub const TAIL: usize = 26_000;
/// Per-call spill cap and per-session spill budget (§4.4).
pub const SPILL_CAP: u64 = 64 << 20;
pub const SESSION_SPILL_CAP: u64 = 1 << 30;
/// TERM → KILL grace and the drain grace after the leader exits.
pub const TERM_GRACE: Duration = Duration::from_secs(2);
pub const DRAIN_GRACE: Duration = Duration::from_secs(2);
/// Cancel polling slice (§3.5.7).
pub const CANCEL_SLICE: Duration = Duration::from_millis(10);

pub const BG_NOTE: &str = "[background processes still writing to this command's output were killed; redirect their output (cmd > file 2>&1 &) to keep them running]";

#[derive(Clone, Debug)]
pub struct BashArgs {
    pub command: String,
    pub timeout_s: u64,
}

pub fn parse(m: &Map<String, Value>, lim: &Limits) -> Result<BashArgs, ToolError> {
    check_fields(m, &["command", "timeout"], "")?;
    let command = req_str(m, "command", "")?;
    let timeout_s = match m.get("timeout") {
        None => lim.bash_default_timeout_s,
        Some(v) => match as_int(v) {
            Some(t) if (1..=3600).contains(&t) => t as u64,
            _ => {
                return Err(ToolError::invalid(
                    "timeout must be an integer from 1 to 3600 (seconds)",
                ));
            }
        },
    };
    Ok(BashArgs { command, timeout_s })
}

// ---------------------------------------------------------------------------------------------
// Shell and kill (§3.5.1, §3.5.4)
// ---------------------------------------------------------------------------------------------

static BASH: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_bash_path(p: &Path) {
    *BASH.lock().unwrap_or_else(|e| e.into_inner()) = Some(p.to_path_buf());
}

fn bash_path() -> PathBuf {
    BASH.lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_else(|| PathBuf::from("/bin/bash"))
}

/// boot.md §3.2: the first absolute `PATH` entry holding an executable regular file `bash`,
/// else `/bin/bash`.
pub fn find_bash(path_var: Option<&str>) -> PathBuf {
    for dir in path_var.unwrap_or("").split(':') {
        let d = Path::new(dir);
        if !d.is_absolute() {
            continue;
        }
        let p = d.join("bash");
        if let Ok(m) = std::fs::metadata(&p)
            && m.is_file()
            && m.permissions().mode() & 0o111 != 0
        {
            return p;
        }
    }
    PathBuf::from("/bin/bash")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Term,
    Kill,
}

/// Signals a whole process group this body spawned. Returns true if the signal reached at least
/// one process. Never targets pgid ≤ 1 (`kill -- -1` would hit every process of the user).
pub fn kill_group(pgid: i32, sig: Signal) -> bool {
    if pgid <= 1 {
        return false;
    }
    let name = match sig {
        Signal::Term => "TERM",
        Signal::Kill => "KILL",
    };
    let target = format!("-{pgid}");
    for bin in ["/bin/kill", "/usr/bin/kill"] {
        if let Ok(st) = Command::new(bin)
            .args(["-s", name, "--", &target])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            return st.success();
        }
    }
    Command::new(bash_path())
        .args(["-c", &format!("kill -s {name} -- {target}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// True while any process of the group is alive (`kill -s 0`, which delivers nothing).
pub fn group_alive(pgid: i32) -> bool {
    pgid > 1
        && Command::new("/bin/kill")
            .args(["-s", "0", "--", &format!("-{pgid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
}

// ---------------------------------------------------------------------------------------------
// Spill files (§4.4)
// ---------------------------------------------------------------------------------------------

static SPILL_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Boot: the session's spill total, summed from its directory.
pub fn init_spill_total(dir: &Path) {
    let mut sum = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            sum += e.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    SPILL_TOTAL.store(sum, Ordering::Relaxed);
}

fn spill_budget_left() -> bool {
    SPILL_TOTAL.load(Ordering::Relaxed) < SESSION_SPILL_CAP
}

/// `[A-Za-z0-9_-]` kept, every other byte `_`, at most 64 bytes.
pub fn sanitize(id: &str) -> String {
    let s: String = id
        .bytes()
        .take(64)
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
                b as char
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() { "call".into() } else { s }
}

/// `<dir>/<id>.txt`, then `-2`, `-3` … on collision (`create_new`).
fn open_spill(dir: &Path, id: &str) -> io::Result<(PathBuf, File)> {
    std::fs::create_dir_all(dir)?;
    let base = sanitize(id);
    for n in 1.. {
        let name = if n == 1 {
            format!("{base}.txt")
        } else {
            format!("{base}-{n}.txt")
        };
        let p = dir.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < 10_000 => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// Writes a whole text to a new spill file (the generic §3.1 cap). None when no file was written.
pub fn spill_whole(dir: &Path, id: &str, bytes: &[u8]) -> Option<PathBuf> {
    if !spill_budget_left() {
        return None;
    }
    let (p, mut f) = open_spill(dir, id).ok()?;
    let n = bytes.len().min(SPILL_CAP as usize);
    f.write_all(&bytes[..n]).ok()?;
    SPILL_TOTAL.fetch_add(n as u64, Ordering::Relaxed);
    Some(p)
}

// ---------------------------------------------------------------------------------------------
// run_group (§3.5.7)
// ---------------------------------------------------------------------------------------------

pub struct RunSpec<'a> {
    /// None: `/dev/null`. Some: piped; a writer thread writes it, then closes the pipe.
    pub stdin: Option<&'a [u8]>,
    pub timeout: Duration,
    /// A plain cancel flag (the host's per-call `cancel`), watched in 10 ms slices.
    pub cancel: Option<&'a AtomicBool>,
    /// Called with the pgid right after spawn; false ⇒ the group is killed at once.
    pub on_spawn: Option<&'a (dyn Fn(i32) -> bool + Sync)>,
    pub capture: CaptureSpec,
}

pub enum CaptureSpec {
    /// One pipe for both streams (30,000 inline; head 4,000 + tail 26,000). `spill_dir: None` saves
    /// no file. `name` names the spill file (the call id, sanitized). Deviation: §3.5.7 has no
    /// `name` field; the spill file name needs the call id.
    Merged {
        spill_dir: Option<PathBuf>,
        name: String,
    },
    /// Two pipes; keep the LAST `max_each` bytes of each.
    Separate { max_each: usize },
}

#[derive(Debug)]
pub enum Captured {
    Merged {
        text: String,
        total: u64,
        spill_path: Option<PathBuf>,
        spill_full: bool,
    },
    Separate {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        stdout_total: u64,
        stderr_total: u64,
    },
}

#[derive(Debug)]
pub struct GroupOutcome {
    pub pgid: i32,
    pub status: Option<ExitStatus>,
    pub timed_out: bool,
    pub cancelled: bool,
    /// The group needed SIGKILL after the TERM grace.
    pub killed: bool,
    pub bg_killed: bool,
    pub output: Captured,
    pub wall: Duration,
}

/// The last `cap` bytes of a stream.
struct Ring {
    buf: VecDeque<u8>,
    cap: usize,
}

impl Ring {
    fn new(cap: usize) -> Ring {
        Ring {
            buf: VecDeque::with_capacity(cap.min(1 << 20)),
            cap,
        }
    }
    fn push(&mut self, b: &[u8]) {
        if b.len() >= self.cap {
            self.buf.clear();
            self.buf.extend(&b[b.len() - self.cap..]);
            return;
        }
        let over = (self.buf.len() + b.len()).saturating_sub(self.cap);
        self.buf.drain(..over);
        self.buf.extend(b);
    }
    fn take(self) -> Vec<u8> {
        self.buf.into()
    }
}

struct MergedCap {
    mem: Vec<u8>,
    tail: Ring,
    total: u64,
    newlines: u64,
    spill: Option<(PathBuf, File)>,
    spilled: u64,
    spill_full: bool,
    spill_refused: bool,
    spill_err: Option<String>,
}

fn capture_merged(mut r: impl Read, spill_dir: Option<&Path>, name: &str) -> MergedCap {
    let mut c = MergedCap {
        mem: Vec::new(),
        tail: Ring::new(TAIL),
        total: 0,
        newlines: 0,
        spill: None,
        spilled: 0,
        spill_full: false,
        spill_refused: false,
        spill_err: None,
    };
    let mut buf = vec![0u8; 65_536];
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let chunk = &buf[..n];
        let before = c.total as usize;
        c.total += n as u64;
        c.newlines += chunk.iter().filter(|&&b| b == b'\n').count() as u64;
        c.tail.push(chunk);
        if before < INLINE {
            let take = (INLINE - before).min(n);
            c.mem.extend_from_slice(&chunk[..take]);
            if take == n {
                continue;
            }
            // The first byte past 30,000: open the spill file and write what is in memory.
            if let Some(dir) = spill_dir {
                if spill_budget_left() {
                    match open_spill(dir, name) {
                        Ok((p, mut f)) => {
                            if let Err(e) = f.write_all(&c.mem) {
                                c.spill_err = Some(e.to_string());
                            } else {
                                c.spilled = c.mem.len() as u64;
                                c.spill = Some((p, f));
                            }
                        }
                        Err(e) => c.spill_err = Some(e.to_string()),
                    }
                } else {
                    c.spill_refused = true;
                }
            }
            write_spill(&mut c, &chunk[take..]);
        } else {
            write_spill(&mut c, chunk);
        }
    }
    if c.spilled > 0 {
        SPILL_TOTAL.fetch_add(c.spilled, Ordering::Relaxed);
    }
    c
}

fn write_spill(c: &mut MergedCap, b: &[u8]) {
    if c.spill_full || c.spill_err.is_some() {
        return;
    }
    let Some((_, f)) = c.spill.as_mut() else {
        return;
    };
    let room = (SPILL_CAP - c.spilled) as usize;
    let n = room.min(b.len());
    if let Err(e) = f.write_all(&b[..n]) {
        c.spill_err = Some(e.to_string());
        return;
    }
    c.spilled += n as u64;
    if n < b.len() {
        c.spill_full = true; // keep draining the pipe; drop the rest
    }
}

/// Builds the model-facing text from a merged capture (§3.5.2), cleaned.
fn merged_text(c: MergedCap) -> Captured {
    let spill_path = c.spill.as_ref().map(|(p, _)| p.clone());
    let total = c.total;
    let raw: Vec<u8> = if total as usize <= INLINE {
        c.mem
    } else {
        let head = &c.mem[..HEAD.min(c.mem.len())];
        let head = match head.iter().rposition(|&b| b == b'\n') {
            Some(i) => &head[..i],
            None => head,
        };
        let tail_all = c.tail.take();
        let tail: &[u8] = match tail_all.iter().take(1_000).position(|&b| b == b'\n') {
            Some(i) => &tail_all[i + 1..],
            None => &tail_all,
        };
        // Lines and bytes between head and tail; the middle is only known by its counts.
        let omitted = total - head.len() as u64 - tail.len() as u64;
        let nl = |b: &[u8]| b.iter().filter(|&&x| x == b'\n').count() as u64;
        let omitted_lines = c.newlines.saturating_sub(nl(head) + nl(tail));
        let marker = if c.spill_refused {
            format!(
                "\n[output omitted; session spill limit reached: {omitted_lines} lines ({} bytes) not shown]\n",
                commas(omitted)
            )
        } else {
            let where_ = match (&spill_path, &c.spill_err) {
                (Some(p), None) => format!("in {}", p.display()),
                (Some(p), Some(e)) => format!("in {} (incomplete: {e})", p.display()),
                (None, Some(e)) => format!("not saved ({e})"),
                (None, None) => "not saved".to_string(),
            };
            format!(
                "\n[... {omitted_lines} lines ({} bytes) omitted; full output ({} bytes) {where_} ...]\n",
                commas(omitted),
                commas(total)
            )
        };
        let mut v = Vec::with_capacity(head.len() + marker.len() + tail.len());
        v.extend_from_slice(head);
        v.extend_from_slice(marker.as_bytes());
        v.extend_from_slice(tail);
        v
    };
    Captured::Merged {
        text: clean(&raw),
        total,
        spill_path,
        spill_full: c.spill_full,
    }
}

/// ANSI CSI/OSC sequences removed, `\r`-redrawn lines collapsed, lossy UTF-8 (§3.5.2).
pub fn clean(b: &[u8]) -> String {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == 0x1b && i + 1 < b.len() {
            match b[i + 1] {
                b'[' => {
                    let mut j = i + 2;
                    while j < b.len() && !(0x40..=0x7e).contains(&b[j]) {
                        j += 1;
                    }
                    i = j + 1;
                    continue;
                }
                b']' => {
                    let mut j = i + 2;
                    loop {
                        if j >= b.len() {
                            i = b.len();
                            break;
                        }
                        if b[j] == 0x07 {
                            i = j + 1;
                            break;
                        }
                        if b[j] == 0x1b && j + 1 < b.len() && b[j + 1] == b'\\' {
                            i = j + 2;
                            break;
                        }
                        j += 1;
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(b[i]);
        i += 1;
    }
    let mut res = Vec::with_capacity(out.len());
    let mut first = true;
    for line in out.split(|&c| c == b'\n') {
        if !first {
            res.push(b'\n');
        }
        first = false;
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let line = match line.iter().rposition(|&c| c == b'\r') {
            Some(k) => &line[k + 1..],
            None => line,
        };
        res.extend_from_slice(line);
    }
    String::from_utf8_lossy(&res).into_owned()
}

/// Spawns `cmd` in its own process group and waits with timeout, cancel, TERM→KILL and drain.
/// Err only when spawn fails.
pub fn run_group(mut cmd: Command, spec: RunSpec) -> io::Result<GroupOutcome> {
    let mut readers: Vec<io::PipeReader> = Vec::new();
    match &spec.capture {
        CaptureSpec::Merged { .. } => {
            let (r, w) = io::pipe()?;
            let w2 = w.try_clone()?;
            cmd.stdout(w).stderr(w2);
            readers.push(r);
        }
        CaptureSpec::Separate { .. } => {
            let (r1, w1) = io::pipe()?;
            let (r2, w2) = io::pipe()?;
            cmd.stdout(w1).stderr(w2);
            readers.push(r1);
            readers.push(r2);
        }
    }
    cmd.stdin(if spec.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    cmd.process_group(0);
    let spawned = cmd.spawn();
    drop(cmd); // REQUIRED: the parent's copies of the write ends close here, or EOF never comes
    let mut child = spawned?;
    let t0 = Instant::now();
    let pg = child.id() as i32;
    if let Some(f) = spec.on_spawn
        && !f(pg)
    {
        kill_group(pg, Signal::Kill); // abort raced the spawn
    }
    let stdin_pipe = child.stdin.take();
    let cancel = spec.cancel;
    let is_cancelled = || cancel.map(|c| c.load(Ordering::SeqCst)).unwrap_or(false);
    std::thread::scope(|s| {
        if let (Some(mut pipe), Some(bytes)) = (stdin_pipe, spec.stdin) {
            s.spawn(move || {
                let _ = pipe.write_all(bytes); // BrokenPipe ignored: the child did not read
            });
        }
        let mut crx = Vec::new();
        match &spec.capture {
            CaptureSpec::Merged { spill_dir, name } => {
                let r = readers.pop().expect("one pipe");
                let (tx, rx) = mpsc::channel();
                let (dir, name) = (spill_dir.clone(), name.clone());
                s.spawn(move || {
                    let _ = tx.send(Cap::Merged(capture_merged(r, dir.as_deref(), &name)));
                });
                crx.push(rx);
            }
            CaptureSpec::Separate { max_each } => {
                for r in readers.drain(..) {
                    let (tx, rx) = mpsc::channel();
                    let cap = *max_each;
                    s.spawn(move || {
                        let _ = tx.send(Cap::Ring(capture_ring(r, cap)));
                    });
                    crx.push(rx);
                }
            }
        }
        let (wtx, wrx) = mpsc::channel();
        s.spawn(move || {
            let _ = wtx.send(child.wait());
        });
        let deadline = t0 + spec.timeout;
        let mut status: Option<ExitStatus> = None;
        let (mut timed_out, mut cancelled, mut killed) = (false, false, false);
        loop {
            let now = Instant::now();
            if now >= deadline {
                timed_out = true;
                break;
            }
            let slice = match cancel {
                Some(_) => CANCEL_SLICE.min(deadline - now),
                None => deadline - now,
            };
            match wrx.recv_timeout(slice) {
                Ok(st) => {
                    status = st.ok();
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {
                    if is_cancelled() {
                        cancelled = true;
                        break;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if timed_out || cancelled {
            kill_group(pg, Signal::Term);
            match wrx.recv_timeout(TERM_GRACE) {
                Ok(st) => status = st.ok(),
                Err(_) => {
                    kill_group(pg, Signal::Kill);
                    killed = true;
                    status = wrx.recv().ok().and_then(Result::ok);
                }
            }
        }
        let wall = t0.elapsed();
        // Drain: EOF arrives once every holder of the pipe's write end has exited.
        let drain_end = Instant::now() + DRAIN_GRACE;
        let mut bg_killed = false;
        let mut caps = Vec::new();
        for rx in crx {
            let cap = loop {
                let now = Instant::now();
                if bg_killed || now >= drain_end || is_cancelled() {
                    if !bg_killed {
                        kill_group(pg, Signal::Kill);
                        bg_killed = true;
                    }
                    break rx.recv().ok();
                }
                let slice = match cancel {
                    Some(_) => CANCEL_SLICE.min(drain_end - now),
                    None => drain_end - now,
                };
                match rx.recv_timeout(slice) {
                    Ok(c) => break Some(c),
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break None,
                }
            };
            caps.push(cap);
        }
        let output = match &spec.capture {
            CaptureSpec::Merged { .. } => match caps.pop().flatten() {
                Some(Cap::Merged(c)) => merged_text(c),
                _ => Captured::Merged {
                    text: String::new(),
                    total: 0,
                    spill_path: None,
                    spill_full: false,
                },
            },
            CaptureSpec::Separate { .. } => {
                let mut it = caps.into_iter().map(|c| match c {
                    Some(Cap::Ring(r)) => r,
                    _ => (Vec::new(), 0),
                });
                let (stdout, stdout_total) = it.next().unwrap_or_default();
                let (stderr, stderr_total) = it.next().unwrap_or_default();
                Captured::Separate {
                    stdout,
                    stderr,
                    stdout_total,
                    stderr_total,
                }
            }
        };
        Ok(GroupOutcome {
            pgid: pg,
            status,
            timed_out,
            cancelled,
            killed,
            bg_killed,
            output,
            wall,
        })
    })
}

enum Cap {
    Merged(MergedCap),
    Ring((Vec<u8>, u64)),
}

fn capture_ring(mut r: impl Read, cap: usize) -> (Vec<u8>, u64) {
    let mut ring = Ring::new(cap);
    let mut total = 0u64;
    let mut buf = vec![0u8; 65_536];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                ring.push(&buf[..n]);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    (ring.take(), total)
}

// ---------------------------------------------------------------------------------------------
// The bash tool (§3.5.6)
// ---------------------------------------------------------------------------------------------

fn secs(d: Duration) -> String {
    format!("{:.2}s", d.as_secs_f64())
}

pub fn run(tools: &Tools, a: &BashArgs, cx: &ToolCx) -> Ran {
    // MEM-RUNAWAY (§3.5.8): no new process while the Root reports the disk below its workload floor
    if let Some((free, floor)) = tools.mem.disk_low(super::memcap::wall_ms()) {
        let r = ToolError::new(
            "disk_floor",
            format!(
                "free disk ({free} MiB) is below the workload floor ({floor} MiB): no new processes until disk space is freed"
            ),
        )
        .hint("tell the owner; the Root kills running workloads too while the disk stays this low")
        .into_result(cx.call_id);
        return Ran {
            result: r,
            summary: "error: disk_floor".into(),
        };
    }
    let spawned_ms = super::memcap::wall_ms();
    let cmd = tools.bash_command(&a.command, cx);
    let abort = cx.abort;
    let on_spawn = move |pg: i32| abort.register_group(pg);
    let spec = RunSpec {
        stdin: None,
        timeout: Duration::from_secs(a.timeout_s),
        cancel: None, // Abort::trigger SIGKILLs every registered group (loop.md §3.5)
        on_spawn: Some(&on_spawn),
        capture: CaptureSpec::Merged {
            spill_dir: Some(cx.spill_dir.to_path_buf()),
            name: cx.call_id.to_string(),
        },
    };
    let o = match run_group(cmd, spec) {
        Ok(o) => o,
        Err(e) => {
            let r = ToolError::new(
                "spawn_failed",
                format!("could not start {}: {e}", tools.bash.display()),
            )
            .into_result(cx.call_id);
            return Ran {
                result: r,
                summary: "error: spawn_failed".into(),
            };
        }
    };
    cx.abort.unregister_group(o.pgid);
    let Captured::Merged {
        text,
        spill_path,
        spill_full,
        ..
    } = o.output
    else {
        unreachable!("bash captures merged")
    };
    let code = o.status.and_then(|s| s.code());
    let signal = o.status.and_then(|s| s.signal());
    let aborted = cx.abort.is_set() && code.is_none();
    // MEM-RUNAWAY: a signal this body did not send may be the Root's watchdog; its record comes before the signal
    let root_kill = (signal.is_some() && !o.timed_out && !aborted)
        .then(|| tools.mem.lookup(o.pgid, spawned_ms))
        .flatten();
    let mut out = text;
    let lines = out.lines().count();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if o.bg_killed && !aborted && !o.timed_out && root_kill.is_none() {
        out.push_str(BG_NOTE);
        out.push('\n');
    }
    if spill_full {
        out.push_str("[spill file capped at 64 MiB]\n");
    }
    let (trailer, summary) = if let Some(k) = &root_kill {
        (
            format!("[{} wall_time={}]", k.text(), secs(o.wall)),
            k.summary().to_string(),
        )
    } else if o.timed_out {
        let how = if o.killed {
            format!("TERM, then KILL after {}s", TERM_GRACE.as_secs())
        } else {
            "TERM".to_string()
        };
        (
            format!(
                "[timeout after {}s: process group killed ({how}) wall_time={}]",
                a.timeout_s,
                secs(o.wall)
            ),
            "timeout".to_string(),
        )
    } else if aborted {
        (
            format!("[aborted after {}: process group killed]", secs(o.wall)),
            "aborted".to_string(),
        )
    } else if let Some(c) = code {
        (
            format!("[exit_code={c} wall_time={}]", secs(o.wall)),
            format!("exit {c}, {lines} lines"),
        )
    } else {
        let sig = signal
            .map(|s| s.to_string())
            .unwrap_or_else(|| "none".into());
        (
            format!("[exit_code=none signal={sig} wall_time={}]", secs(o.wall)),
            format!("signal {sig}"),
        )
    };
    out.push_str(&trailer);
    let is_error = o.timed_out || aborted || code != Some(0);
    Ran {
        result: ToolResult {
            call_id: cx.call_id.to_string(),
            content: vec![ContentBlock::Text { text: out }],
            is_error,
            meta: ToolMeta {
                spill_path: spill_path.map(|p| p.display().to_string()),
                exit_code: code,
                signal,
                wall_ms: o.wall.as_millis() as u64,
            },
        },
        summary,
    }
}
