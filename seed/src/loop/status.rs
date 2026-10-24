//! `Phase` and `Status` (loop.md §3.1), shared with the heartbeat thread (boot.md §3.12).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

use super::frames::{Out, OutTx};
use crate::model::Abort;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Booting = 0,
    Idle = 1,
    Model = 2,
    Tools = 3,
    Paused = 4,
    Stopping = 5,
}

impl Phase {
    pub fn from_u8(v: u8) -> Phase {
        match v {
            1 => Phase::Idle,
            2 => Phase::Model,
            3 => Phase::Tools,
            4 => Phase::Paused,
            5 => Phase::Stopping,
            _ => Phase::Booting,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Booting => "booting",
            Phase::Idle => "idle",
            Phase::Model => "model",
            Phase::Tools => "tools",
            Phase::Paused => "paused",
            Phase::Stopping => "stopping",
        }
    }
    pub fn busy(self) -> bool {
        matches!(self, Phase::Model | Phase::Tools)
    }
}

pub struct Status {
    pub phase: AtomicU8,
    pub run: AtomicU64,
    pub turn: AtomicU64,
    /// ms since `start` of the last stream byte / tool event / log append.
    pub last_progress_ms: AtomicU64,
    pub leaf: Mutex<String>,
    pub start: Instant,
    /// `retract{seq}` decisions, shared by the input thread and the loop (loop.md §3.4).
    pub gate: RetractGate,
}

impl Default for Status {
    fn default() -> Self {
        Status {
            phase: AtomicU8::new(Phase::Booting as u8),
            run: AtomicU64::new(0),
            turn: AtomicU64::new(0),
            last_progress_ms: AtomicU64::new(0),
            leaf: Mutex::new(String::new()),
            start: Instant::now(),
            gate: RetractGate::default(),
        }
    }
}

impl Status {
    pub fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::SeqCst))
    }
    pub fn busy(&self) -> bool {
        self.phase().busy()
    }
    pub fn progress(&self) {
        self.last_progress_ms
            .store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
    pub fn set_leaf(&self, leaf: String) {
        *self.leaf.lock().unwrap_or_else(|p| p.into_inner()) = leaf;
    }
    pub fn leaf(&self) -> String {
        self.leaf.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// How many seqs each gate set remembers; the smallest go first.
pub const GATE_CAP: usize = 1024;

/// The two sets behind `RetractGate`, under one lock.
#[derive(Default, Debug)]
pub struct GateSets {
    /// Inputs the owner took back in time: never appended, whenever they arrive.
    pub retracted: BTreeSet<u64>,
    /// Inputs committed to an append: too late to retract.
    pub consumed: BTreeSet<u64>,
}

impl GateSets {
    pub fn consume(&mut self, seq: u64) {
        self.consumed.insert(seq);
        while self.consumed.len() > GATE_CAP {
            self.consumed.pop_first();
        }
    }
}

/// The retract gate (loop.md §3.4). The input thread answers `retract{seq}` at once; the loop
/// filters and marks inputs under the same lock right before every append, so each input is
/// either retracted (never appended) or consumed (acked normally), never both.
#[derive(Default)]
pub struct RetractGate {
    sets: Mutex<GateSets>,
    /// The input thread's own reply channel (`retracted`). `detach` drops it at exit so the
    /// output thread can finish while stdin stays open.
    out: Mutex<Option<OutTx>>,
}

impl RetractGate {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, GateSets> {
        self.sets.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `ok`: the input was not consumed yet and now never will be.
    pub fn retract(&self, seq: u64) -> bool {
        let mut g = self.lock();
        let ok = !g.consumed.contains(&seq);
        if ok {
            g.retracted.insert(seq);
            while g.retracted.len() > GATE_CAP {
                g.retracted.pop_first();
            }
        }
        ok
    }

    pub fn is_retracted(&self, seq: u64) -> bool {
        self.lock().retracted.contains(&seq)
    }

    pub fn attach(&self, out: OutTx) {
        *self.out.lock().unwrap_or_else(|p| p.into_inner()) = Some(out);
    }

    pub fn detach(&self) {
        self.out.lock().unwrap_or_else(|p| p.into_inner()).take();
    }

    /// Decides and replies `retracted{seq, ok}`.
    pub fn retract_and_reply(&self, seq: u64) {
        let ok = self.retract(seq);
        let out = self.out.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(o) = out {
            o.send(Out::Retracted { seq, ok });
        }
    }
}

/// What the heartbeat writes besides `Status` (boot.md §4.2).
#[derive(Clone, Debug)]
pub struct HeartbeatInfo {
    pub run_dir: PathBuf,
    pub boot_id: String,
    pub session: String,
    pub period: Duration,
}

/// One heartbeat line (boot.md §4.2), ICJ (sorted keys, integers only).
pub fn heartbeat_value(info: &HeartbeatInfo, st: &Status, abort: &Abort) -> serde_json::Value {
    let phase = st.phase();
    json!({
        "boot_id": info.boot_id,
        "busy": phase.busy(),
        "last_progress_ms": st.last_progress_ms.load(Ordering::Relaxed),
        "leaf": st.leaf(),
        "phase": phase.as_str(),
        "pid": std::process::id(),
        "run": st.run.load(Ordering::Relaxed),
        "session": info.session,
        "tool_pgids": abort.groups(),
        "ts": crate::log::clock::now_rfc3339_ms(),
        "turn": st.turn.load(Ordering::Relaxed),
        "uptime_ms": st.start.elapsed().as_millis() as u64,
        "v": 1,
    })
}

/// Atomic write (temp + rename, no fsync) of `state/run/heartbeat`.
pub fn write_heartbeat(info: &HeartbeatInfo, st: &Status, abort: &Abort) -> std::io::Result<()> {
    std::fs::create_dir_all(&info.run_dir)?;
    let tmp = info
        .run_dir
        .join(format!(".heartbeat.{}", std::process::id()));
    let mut line = heartbeat_value(info, st, abort).to_string();
    line.push('\n');
    std::fs::write(&tmp, line)?;
    std::fs::rename(&tmp, info.run_dir.join("heartbeat"))
}

/// The heartbeat thread (boot.md §3.12): writes at once, then every `period`, also while paused.
/// One stderr warning per minute on failure. Stops when `stop` is set.
pub fn spawn_heartbeat(
    info: HeartbeatInfo,
    st: Arc<Status>,
    abort: Arc<Abort>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("heartbeat".into())
        .spawn(move || {
            let mut last_warn: Option<Instant> = None;
            while !stop.load(Ordering::SeqCst) {
                if let Err(e) = write_heartbeat(&info, &st, &abort)
                    && last_warn.is_none_or(|t| t.elapsed() >= Duration::from_secs(60))
                {
                    eprintln!(
                        "{}",
                        json!({"type": "warning", "step": "heartbeat", "message": format!("write failed: {e}")})
                    );
                    last_warn = Some(Instant::now());
                }
                let until = Instant::now() + info.period;
                while !stop.load(Ordering::SeqCst) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(100).min(info.period));
                }
            }
        })
}

pub fn run_dir(home: &Path) -> PathBuf {
    home.join("state").join("run")
}
