//! `ext` dispatch: the worker thread per call, the epoch ticker, deadlines, owner abort, the
//! watchdog and stuck table, trap classification and the outcome text (ext/host.md §3.10–§3.12,
//! §4.2, §4.4).

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering::SeqCst};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use wasmtime::{Engine, EngineWeak, Store, Trap, UpdateDeadline, WasmBacktrace};
use wasmtime_wasi::I32Exit;
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;

use super::bindings::v1;
use super::process::ProcHost;
use super::registry::{ExtVersion, Pre, RawInfo, Registry};
use super::state::{CallState, Caps, GuestRoots, wasi_ctx};
use super::{
    ABORT_GRACE, CallCtx, DESCRIBE_TIMEOUT, EPOCH_TICK, ExtHost, ExtOutcome, INPUT_MAX, LOG_CAP,
    LOG_ROTATE, LOG_TAIL, MAX_STUCK, OUTPUT_MAX, OutcomeKind, WATCHDOG_GRACE, lock,
};

// ---------------------------------------------------------------------------------------------
// Epoch ticker (§3.11)
// ---------------------------------------------------------------------------------------------

/// Counts running calls; the `ext-epoch` thread ticks only while the count is above zero.
#[derive(Default)]
pub(crate) struct Ticker {
    active: Mutex<usize>,
    cv: Condvar,
}

impl Ticker {
    pub(crate) fn enter(&self) {
        *lock(&self.active) += 1;
        self.cv.notify_all();
    }

    pub(crate) fn exit(&self) {
        let mut a = lock(&self.active);
        *a = a.saturating_sub(1);
    }

    pub(crate) fn active(&self) -> usize {
        *lock(&self.active)
    }
}

pub(crate) fn start_ticker(engine: &Engine, t: Arc<Ticker>) -> std::io::Result<()> {
    let weak = engine.weak();
    std::thread::Builder::new()
        .name("ext-epoch".into())
        .spawn(move || ticker_loop(weak, t))
        .map(|_| ())
}

fn ticker_loop(engine: EngineWeak, t: Arc<Ticker>) {
    loop {
        {
            // Park while no call runs: zero idle wake-ups (one check every 5 s for engine drop).
            let mut a = lock(&t.active);
            while *a == 0 {
                a =
                    t.cv.wait_timeout(a, Duration::from_secs(5))
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                if engine.upgrade().is_none() {
                    return;
                }
            }
        }
        std::thread::sleep(EPOCH_TICK);
        match engine.upgrade() {
            Some(e) => e.increment_epoch(),
            None => return,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Stuck table (§3.12)
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct StuckTable {
    m: Mutex<BTreeMap<String, usize>>,
}

impl StuckTable {
    pub(crate) fn mark(&self, name: &str) {
        *lock(&self.m).entry(name.to_string()).or_insert(0) += 1;
    }

    pub(crate) fn release(&self, name: &str) {
        let mut g = lock(&self.m);
        if let Some(n) = g.get_mut(name) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                g.remove(name);
            }
        }
    }

    pub(crate) fn count(&self, name: &str) -> usize {
        lock(&self.m).get(name).copied().unwrap_or(0)
    }

    pub(crate) fn total(&self) -> usize {
        lock(&self.m).values().sum()
    }

    pub(crate) fn snapshot(&self) -> BTreeMap<String, usize> {
        lock(&self.m).clone()
    }
}

// ---------------------------------------------------------------------------------------------
// The job (one worker thread, one fresh Store)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Call,
    Describe,
}

pub(crate) enum Ret {
    Call(Result<String, v1::ToolError>),
    Describe(v1::ToolInfo),
}

pub(crate) struct CallResult {
    pub r: wasmtime::Result<Ret>,
    pub log: Vec<u8>,
    pub memory_denied: bool,
    pub cancelled: bool,
    pub past_deadline: bool,
    pub elapsed: Duration,
}

/// 0 running, 1 done, 2 abandoned.
const RUNNING: u8 = 0;
const DONE: u8 = 1;
const ABANDONED: u8 = 2;

struct Job {
    engine: Engine,
    pre: Pre,
    name: Arc<str>,
    op: Op,
    input: String,
    caps: Caps,
    roots: Option<GuestRoots>,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
    flag: Arc<AtomicU8>,
    procs: Arc<ProcHost>,
    ticker: Arc<Ticker>,
    stuck: Arc<StuckTable>,
}

impl Job {
    fn run(&self) -> CallResult {
        let t0 = Instant::now();
        let log = MemoryOutputPipe::new(LOG_CAP);
        let wasi = match wasi_ctx(self.roots.as_ref(), &log) {
            Ok(w) => w,
            Err(e) => {
                return CallResult {
                    r: Err(e),
                    log: log.contents().to_vec(),
                    memory_denied: false,
                    cancelled: false,
                    past_deadline: false,
                    elapsed: t0.elapsed(),
                };
            }
        };
        let state = CallState::new(
            wasi,
            self.name.clone(),
            self.deadline,
            self.cancel.clone(),
            self.caps,
            self.roots.clone(),
            self.procs.clone(),
        );
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limits);
        store.set_epoch_deadline(1); // check every tick
        store.epoch_deadline_callback(|cx| {
            let s = cx.data();
            if s.cancel.load(SeqCst) || Instant::now() >= s.deadline {
                Ok(UpdateDeadline::Interrupt)
            } else {
                Ok(UpdateDeadline::Continue(1))
            }
        });
        let r = match &self.pre {
            Pre::V1(pre) => pre.instantiate(&mut store).and_then(|t| match self.op {
                Op::Call => t.call_call(&mut store, &self.input).map(Ret::Call),
                Op::Describe => t.call_describe(&mut store).map(Ret::Describe),
            }),
        };
        let memory_denied = store.data().limits.memory_denied;
        drop(store); // frees the instance, its memory and its pool slot
        CallResult {
            r,
            log: log.contents().to_vec(),
            memory_denied,
            cancelled: self.cancel.load(SeqCst),
            past_deadline: Instant::now() >= self.deadline,
            elapsed: t0.elapsed(),
        }
    }
}

pub(crate) enum Waited {
    Done(CallResult),
    Panicked,
    SpawnFailed(String),
    Abandoned { aborted: bool },
}

impl ExtHost {
    /// Runs one job on its own thread and waits in 10 ms slices: owner abort (`abort`), the
    /// call deadline plus `WATCHDOG_GRACE`, and abandonment into the stuck table (§3.10, §3.12).
    #[allow(clippy::too_many_arguments)]
    fn run_job(
        &self,
        pre: &Pre,
        name: &str,
        op: Op,
        input: String,
        caps: Caps,
        roots: Option<GuestRoots>,
        timeout: Duration,
        abort: Option<&AtomicBool>,
    ) -> Waited {
        let start = Instant::now();
        let job = Job {
            engine: self.engine.clone(),
            pre: pre.clone(),
            name: Arc::from(name),
            op,
            input,
            caps,
            roots,
            deadline: start + timeout,
            cancel: Arc::new(AtomicBool::new(false)),
            flag: Arc::new(AtomicU8::new(RUNNING)),
            procs: self.procs.clone(),
            ticker: self.ticker.clone(),
            stuck: self.stuck.clone(),
        };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let flag = job.flag.clone();
        let cancel = job.cancel.clone();
        self.ticker.enter();
        let spawned = std::thread::Builder::new()
            .name(format!("ext-{name}"))
            .spawn(move || {
                let r = job.run();
                job.ticker.exit();
                if job
                    .flag
                    .compare_exchange(RUNNING, DONE, SeqCst, SeqCst)
                    .is_err()
                {
                    job.stuck.release(&job.name); // we were abandoned and counted as stuck
                }
                let _ = tx.send(r);
            });
        if let Err(e) = spawned {
            self.ticker.exit();
            return Waited::SpawnFailed(e.to_string());
        }
        let hard_end = start + timeout + WATCHDOG_GRACE;
        let mut abort_end: Option<Instant> = None;
        loop {
            match rx.recv_timeout(EPOCH_TICK) {
                Ok(r) => return Waited::Done(r),
                Err(RecvTimeoutError::Disconnected) => return Waited::Panicked,
                Err(RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            if abort_end.is_none() && abort.is_some_and(|a| a.load(SeqCst)) {
                cancel.store(true, SeqCst); // seen by the epoch callback and by process run/read
                abort_end = Some(now + ABORT_GRACE);
            }
            if abort_end.is_some_and(|t| now >= t) || now >= hard_end {
                if flag
                    .compare_exchange(RUNNING, ABANDONED, SeqCst, SeqCst)
                    .is_ok()
                {
                    self.stuck.mark(name);
                } else if let Ok(r) = rx.try_recv() {
                    return Waited::Done(r); // finished in the same instant
                }
                return Waited::Abandoned {
                    aborted: abort_end.is_some(),
                };
            }
        }
    }

    /// `describe` with no capabilities, a 1 s deadline, the loader thread waiting (§3.8 step 5).
    pub(crate) fn describe_pre(&self, pre: &Pre, name: &str) -> Result<RawInfo, String> {
        match self.run_job(
            pre,
            name,
            Op::Describe,
            String::new(),
            Caps::None,
            None,
            DESCRIBE_TIMEOUT,
            None,
        ) {
            Waited::Done(r) => match r.r {
                Ok(Ret::Describe(info)) => Ok(RawInfo::from(info)),
                Ok(Ret::Call(_)) => Err("describe: internal error".into()),
                Err(e) => {
                    let kind = classify(&e, r.memory_denied, false, r.past_deadline);
                    let what = match kind {
                        Kind::Timeout => format!(
                            "describe timed out after {} ms",
                            DESCRIBE_TIMEOUT.as_millis()
                        ),
                        Kind::Host(m) => format!("describe failed: {m}"),
                        _ => format!("describe trapped: {}", trap_display(&e)),
                    };
                    Err(with_log_hint(what, &r.log))
                }
            },
            Waited::Panicked => Err("describe: worker panicked".into()),
            Waited::SpawnFailed(e) => Err(format!("describe: thread spawn failed: {e}")),
            Waited::Abandoned { .. } => Err(format!(
                "describe timed out after {} ms (blocked in a host call; do not sleep in describe)",
                DESCRIBE_TIMEOUT.as_millis()
            )),
        }
    }

    /// The `ext` call (§3.10). `reg` is the batch's snapshot.
    pub fn call(&self, reg: &Registry, name: &str, input: &Value, cx: &CallCtx) -> ExtOutcome {
        self.procs.reap_idle(Instant::now());
        let Some(ver) = reg.exts.get(name).cloned() else {
            return unknown(name, reg);
        };
        if self.stuck.count(name) > 0 {
            let o = busy(&ver, "a previous call is still blocked in a host call");
            self.append_log(&ver, cx.call_id, &o, &[]);
            return o;
        }
        if self.stuck.total() >= MAX_STUCK {
            let o = busy(&ver, "too many blocked calls");
            self.append_log(&ver, cx.call_id, &o, &[]);
            return o;
        }
        let Ok(input) = serde_json::to_string(input) else {
            return plain(
                &ver,
                OutcomeKind::Host,
                "host error: input serialization",
                0,
            );
        };
        if input.len() > INPUT_MAX {
            return plain(
                &ver,
                OutcomeKind::InvalidInput,
                "invalid input: input exceeds 1 MiB",
                0,
            );
        }
        let data = self.paths.data_dir(name);
        if let Err(e) = fs::create_dir_all(&data) {
            return plain(
                &ver,
                OutcomeKind::Host,
                &format!("host error: cannot create {}: {e}", self.paths.rel(&data)),
                0,
            );
        }
        let roots = GuestRoots {
            work: cx.workspace.to_path_buf(),
            data,
        };
        let (o, log) = self.call_version(&ver, input, roots, cx.abort);
        self.append_log(&ver, cx.call_id, &o, &log);
        o
    }

    /// One call of one version with explicit roots (also `init-body ext call`). Returns the
    /// outcome and the captured log.
    pub(crate) fn call_version(
        &self,
        ver: &ExtVersion,
        input: String,
        roots: GuestRoots,
        abort: &AtomicBool,
    ) -> (ExtOutcome, Vec<u8>) {
        let timeout = Duration::from_millis(ver.info.timeout_ms as u64);
        let w = self.run_job(
            &ver.pre,
            &ver.name,
            Op::Call,
            input,
            Caps::Full,
            Some(roots),
            timeout,
            Some(abort),
        );
        finish(ver, w)
    }

    /// `state/ext/<name>/log`: one record per call, rotated at 1 MiB (§4.2). Best effort.
    pub(crate) fn append_log(&self, ver: &ExtVersion, call_id: &str, o: &ExtOutcome, log: &[u8]) {
        let dir = self.paths.state.join(&ver.name);
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join("log");
        let mut rec = format!(
            "--- {} call={} {} {} {} result={} {}ms log={}B\n",
            crate::log::clock::now_rfc3339_ms(),
            call_id,
            ver.name,
            ver.info.version,
            ver.origin.as_str(),
            kind_name(o.kind),
            o.elapsed_ms,
            log.len()
        )
        .into_bytes();
        rec.extend_from_slice(log);
        if !log.is_empty() && !log.ends_with(b"\n") {
            rec.push(b'\n');
        }
        if let Ok(m) = fs::metadata(&path)
            && m.len() + rec.len() as u64 > LOG_ROTATE
        {
            let _ = fs::rename(&path, dir.join("log.1"));
        }
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = f.write_all(&rec);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Classification and text (§3.12, §4.4)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Aborted,
    Oom,
    Exit(i32),
    Timeout,
    StackOverflow,
    Panic,
    Trap(String),
    Host(String),
}

pub(crate) fn classify(
    e: &wasmtime::Error,
    memory_denied: bool,
    cancelled: bool,
    past_deadline: bool,
) -> Kind {
    if cancelled {
        return Kind::Aborted; // owner abort wins over any trap it caused
    }
    if memory_denied {
        return Kind::Oom;
    }
    if let Some(x) = e.downcast_ref::<I32Exit>() {
        return Kind::Exit(x.0);
    }
    match e.downcast_ref::<Trap>() {
        Some(Trap::Interrupt) if past_deadline => Kind::Timeout,
        Some(Trap::Interrupt) => Kind::Trap("interrupt".into()),
        Some(Trap::StackOverflow) => Kind::StackOverflow,
        Some(Trap::UnreachableCodeReached) => Kind::Panic,
        Some(t) => Kind::Trap(t.to_string()),
        None => {
            let msg = chain(e);
            if e.downcast_ref::<wasmtime::PoolConcurrencyLimitError>()
                .is_some()
                || msg.contains("maximum concurrent limit")
            {
                Kind::Host(format!(
                    "instance pool exhausted (32 concurrent calls): {msg}"
                ))
            } else {
                Kind::Host(msg)
            }
        }
    }
}

/// `a: b: c` for an error and its causes.
pub(crate) fn chain(e: &wasmtime::Error) -> String {
    let mut parts: Vec<String> = Vec::new();
    for c in e.chain() {
        let s = c.to_string();
        if !parts.iter().any(|p| p == &s) {
            parts.push(s);
        }
    }
    parts.join(": ")
}

fn trap_display(e: &wasmtime::Error) -> String {
    match e.downcast_ref::<Trap>() {
        Some(t) => t.to_string(),
        None => chain(e),
    }
}

/// The last line of the log, for load errors (a describe panic message).
fn with_log_hint(what: String, log: &[u8]) -> String {
    let s = String::from_utf8_lossy(log);
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return what;
    }
    let tail = lines[lines.len().saturating_sub(2)..].join(" | ");
    let tail = cut(&tail, 400);
    format!("{what}; log: {tail}")
}

pub(crate) fn kind_name(k: OutcomeKind) -> &'static str {
    match k {
        OutcomeKind::Ok => "ok",
        OutcomeKind::InvalidInput => "invalid_input",
        OutcomeKind::Failed => "failed",
        OutcomeKind::Timeout => "timeout",
        OutcomeKind::Aborted => "aborted",
        OutcomeKind::Oom => "oom",
        OutcomeKind::StackOverflow => "stack_overflow",
        OutcomeKind::Panic => "panic",
        OutcomeKind::Trap => "trap",
        OutcomeKind::Exit => "exit",
        OutcomeKind::Busy => "busy",
        OutcomeKind::Unknown => "unknown",
        OutcomeKind::Host => "host",
    }
}

/// Cuts `s` to at most `max` bytes at a `char` boundary.
pub(crate) fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// The last `LOG_TAIL` bytes, starting at a `char` boundary.
fn log_tail(log: &[u8]) -> String {
    let s = String::from_utf8_lossy(log);
    if s.len() <= LOG_TAIL {
        return s.into_owned();
    }
    let mut i = s.len() - LOG_TAIL;
    while !s.is_char_boundary(i) {
        i += 1;
    }
    s[i..].to_string()
}

fn prefix(ver: &ExtVersion) -> String {
    format!("ext {} {}: ", ver.name, ver.info.version)
}

fn plain(ver: &ExtVersion, kind: OutcomeKind, msg: &str, elapsed_ms: u64) -> ExtOutcome {
    ExtOutcome {
        text: format!("{}{msg}", prefix(ver)),
        is_error: kind != OutcomeKind::Ok,
        kind,
        version: Some(ver.info.version.clone()),
        elapsed_ms,
        truncated: false,
    }
}

fn with_log(
    ver: &ExtVersion,
    kind: OutcomeKind,
    msg: &str,
    log: &[u8],
    elapsed_ms: u64,
) -> ExtOutcome {
    let mut o = plain(ver, kind, msg, elapsed_ms);
    if !log.is_empty() {
        o.text
            .push_str(&format!("\n--- log (last {LOG_TAIL} bytes) ---\n"));
        o.text.push_str(&log_tail(log));
    }
    o
}

fn busy(ver: &ExtVersion, why: &str) -> ExtOutcome {
    plain(ver, OutcomeKind::Busy, &format!("busy: {why}"), 0)
}

/// `unknown extension "<n>". Loaded: <name> — <description>; … Failed: … Not built: …`
pub(crate) fn unknown(name: &str, reg: &Registry) -> ExtOutcome {
    let loaded: Vec<String> = reg
        .exts
        .values()
        .map(|v| format!("{} — {}", v.name, v.info.description))
        .collect();
    let mut text = format!(
        "unknown extension \"{name}\". Loaded: {}.",
        if loaded.is_empty() {
            "none".to_string()
        } else {
            loaded.join("; ")
        }
    );
    if !reg.failed.is_empty() {
        let f: Vec<String> = reg
            .failed
            .iter()
            .map(|(n, e)| format!("{n} ({})", cut(e, 200)))
            .collect();
        text.push_str(&format!(" Failed: {}.", f.join("; ")));
    }
    if !reg.not_built.is_empty() {
        text.push_str(&format!(" Not built: {}.", reg.not_built.join(", ")));
    }
    text.push_str(" (input schemas: state/ext/catalog.json)");
    ExtOutcome {
        text,
        is_error: true,
        kind: OutcomeKind::Unknown,
        version: None,
        elapsed_ms: 0,
        truncated: false,
    }
}

/// Formats a finished, failed or abandoned job (§3.12 table).
pub(crate) fn finish(ver: &ExtVersion, w: Waited) -> (ExtOutcome, Vec<u8>) {
    let r = match w {
        Waited::Done(r) => r,
        Waited::Panicked => {
            return (
                plain(ver, OutcomeKind::Host, "host error: worker panicked", 0),
                Vec::new(),
            );
        }
        Waited::SpawnFailed(e) => {
            return (
                plain(
                    ver,
                    OutcomeKind::Host,
                    &format!("host error: thread spawn failed: {e}"),
                    0,
                ),
                Vec::new(),
            );
        }
        Waited::Abandoned { aborted } => {
            let o = if aborted {
                plain(
                    ver,
                    OutcomeKind::Aborted,
                    "aborted by owner interrupt; still blocked in a host call",
                    0,
                )
            } else {
                plain(
                    ver,
                    OutcomeKind::Timeout,
                    &format!(
                        "timed out after {} ms; the call was blocked in a host call and is still running",
                        ver.info.timeout_ms
                    ),
                    ver.info.timeout_ms as u64 + WATCHDOG_GRACE.as_millis() as u64,
                )
            };
            return (o, Vec::new());
        }
    };
    let ms = r.elapsed.as_millis() as u64;
    let o = match r.r {
        Ok(Ret::Call(Ok(s))) if r.cancelled => {
            // The guest returned while the abort was being delivered: still an abort.
            let _ = s;
            with_log(
                ver,
                OutcomeKind::Aborted,
                "aborted by owner interrupt",
                &r.log,
                ms,
            )
        }
        Ok(Ret::Call(Ok(s))) => {
            let truncated = s.len() > OUTPUT_MAX;
            let text = if truncated {
                cut(&s, OUTPUT_MAX).to_string()
            } else {
                s
            };
            ExtOutcome {
                text,
                is_error: false,
                kind: OutcomeKind::Ok,
                version: Some(ver.info.version.clone()),
                elapsed_ms: ms,
                truncated,
            }
        }
        Ok(Ret::Call(Err(v1::ToolError::InvalidInput(m)))) => plain(
            ver,
            OutcomeKind::InvalidInput,
            &format!("invalid input: {m}"),
            ms,
        ),
        Ok(Ret::Call(Err(v1::ToolError::Failed(m)))) => {
            plain(ver, OutcomeKind::Failed, &format!("failed: {m}"), ms)
        }
        Ok(Ret::Describe(_)) => plain(ver, OutcomeKind::Host, "host error: internal", ms),
        Err(e) => match classify(&e, r.memory_denied, r.cancelled, r.past_deadline) {
            Kind::Aborted => with_log(
                ver,
                OutcomeKind::Aborted,
                "aborted by owner interrupt",
                &r.log,
                ms,
            ),
            Kind::Oom => with_log(
                ver,
                OutcomeKind::Oom,
                "out of memory: linear memory limit is 256 MiB",
                &r.log,
                ms,
            ),
            Kind::Exit(c) => with_log(
                ver,
                OutcomeKind::Exit,
                &format!("called exit({c})"),
                &r.log,
                ms,
            ),
            Kind::Timeout => with_log(
                ver,
                OutcomeKind::Timeout,
                &format!("timed out after {} ms", ver.info.timeout_ms),
                &r.log,
                ms,
            ),
            Kind::StackOverflow => with_log(
                ver,
                OutcomeKind::StackOverflow,
                "trap: stack overflow (512 KiB wasm stack)",
                &r.log,
                ms,
            ),
            Kind::Panic => with_log(
                ver,
                OutcomeKind::Panic,
                "panicked (see log below)",
                &r.log,
                ms,
            ),
            Kind::Trap(t) => {
                let mut msg = format!("trap: {t}");
                if let Some(bt) = e.downcast_ref::<WasmBacktrace>() {
                    let frames: Vec<String> = bt
                        .to_string()
                        .lines()
                        .skip(1)
                        .take(20)
                        .map(String::from)
                        .collect();
                    if !frames.is_empty() {
                        msg.push('\n');
                        msg.push_str(&frames.join("\n"));
                    }
                }
                with_log(ver, OutcomeKind::Trap, &msg, &r.log, ms)
            }
            Kind::Host(m) => with_log(
                ver,
                OutcomeKind::Host,
                &format!("host error: {m}"),
                &r.log,
                ms,
            ),
        },
    };
    (o, r.log)
}
