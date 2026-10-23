//! `Phase` and `Status` (loop.md §3.1), shared with the heartbeat thread (boot.md §3.12).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

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
