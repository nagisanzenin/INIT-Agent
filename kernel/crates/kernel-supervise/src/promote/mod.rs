//! The T2 promote pipeline (supervise.md §3.6, evolution.md §3.5, kernel/gate.md) — WP8b.
//!
//! ```text
//! promote evo/<id>
//!   sync:  fetch (sandboxed upload-pack) · archive ref · kernel checks · evo validator · scope
//!   job:   export + build (slot) · G0 · G1 (selfmap) · G2 (two probes) · G3 (scripted self-edit)
//!          · [sync G4 for dep / t0_escalated] · awaiting_boundary · overlapped handoff (supervise)
//!          · provisional · async G4 job (+ one escalated re-run) · gate record to Root lineage
//!          (lineage before LKG) · LKG commit (supervise) · refs/notes/gate
//! ```
//!
//! The pure rules are `kernel-gate` (`g0`…`g4`, `accept`, `record`); this module drives them with
//! supervise's primitives: the build slot, [`crate::probe::probe_scoped`], the hardened `$G`, the
//! Root's spawn broker and the outbox.
//!
//! Seams, so the initctl server (WP7) and tests can call it:
//! - [`Job`]: kernel-serve's `JobCtl` (progress, stage results, `awaiting_boundary`, one terminal);
//! - [`GateProxy`]: the in-process proxy's gate API (kernel-proxy `ProxyHandle`: `open_gate`,
//!   `open_scope`, `close_scope`, `scope_summary`, trace `reconstruct`);
//! - [`Supervise`]: the running supervisor ([`crate::supervisor::Handle`] implements it);
//! - [`GateLineage`]: Root lineage through the outbox ([`crate::outbox::Outbox`] implements it).
//!
//! Entry points: [`promote`], [`gate_quick`], [`rollback`], plus [`run_g4_job`] and
//! [`provisional_watch`] for supervise's async side.

pub mod admit;
pub mod config;
pub mod g4run;
pub mod g4v2;
pub mod pipeline;
pub mod proxy;
pub mod record;
pub mod screen;
pub mod stages;
pub mod t0blob;
#[cfg(test)]
mod tests;

pub use config::GateCfg;
pub use pipeline::{gate_quick, promote, provisional_watch, rollback, run_g4_job};
pub use proxy::{GateProxy, GateScope, NoGateProxy, RequestView};

use crate::config::KernelCfg;
use crate::instance::Spawner;
use init_proto::envelope::ErrorBody;
use init_proto::initctl::{JobState, Stage, StageResult};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// An initctl job as the pipeline drives it (kernel-serve's `JobCtl` behind a 10-line adapter).
pub trait Job: Send + Sync {
    fn id(&self) -> String;
    fn cancelled(&self) -> bool;
    fn progress(&self, state: JobState, stage: Option<Stage>, pct: u8, msg: &str);
    fn stage_done(&self, r: StageResult);
    /// The `awaiting_boundary` milestone (initctl.md §3.6); `false` if the job is gone.
    fn awaiting_boundary(&self) -> bool;
    fn succeed(&self, result: Value);
    fn fail(&self, err: ErrorBody);
}

/// Creates kernel-internal jobs (an asynchronous G4 has its own `j-` id, gate.md §3.1).
pub trait Jobs: Send + Sync {
    fn create(&self, cmd: &str, args: Value) -> Arc<dyn Job>;
}

/// The running supervisor, as the pipeline uses it.
pub trait Supervise: Send + Sync {
    /// Hand off to `bin/init-body.staged` (blocks until the handoff commits or fails).
    fn handoff(&self, job: &str, sha: &str, staged_sha256: &str, branch: Option<String>) -> Result<(), String>;
    /// G4 accepted the current provisional `sha`: the LKG commit (§3.6.6).
    fn g4_pass(&self, sha: &str, job: &str) -> Result<(), String>;
    /// G4 rejected `sha`: roll back at the next idle point (§3.6.7).
    fn g4_reject(&self, sha: &str, job: &str, reason: &str);
    /// `initctl rollback` (immediate) or a provisional expiry (at idle).
    fn rollback(&self, reason: &str, immediate: bool) -> Result<(), String>;
    fn snapshot(&self) -> Option<SupState>;
    /// A kernel notice to the body (`gate` stage events).
    fn notice(&self, kind: &str, data: Value);
    /// A promotion without a handoff (supervise.md §3.6.8, T0/T1 data): `main` moves, the version
    /// becomes LKG at once (`p.lkg`) or provisional. Blocks until the supervisor has done it.
    fn data_only(&self, p: crate::supervisor::DataOnly) -> Result<(), String> {
        let _ = p;
        Err("data-only promotions are not supported by this supervisor".into())
    }
}

/// The part of supervise's snapshot the pipeline reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupState {
    pub current_sha: String,
    pub lkg_sha: String,
    pub provisional: bool,
    pub busy: bool,
}

/// Root lineage through the outbox (the full record also stays in `vault/lineage.jsonl`).
pub trait GateLineage: Send + Sync {
    /// `lineage.append` and wait for the Root's `seq` ("lineage before LKG").
    fn append_wait(&self, kind: &str, data: Value) -> Result<u64, String>;
    /// A kernel-local line in `vault/lineage.jsonl` only.
    fn local(&self, kind: &str, data: Value);
}

impl GateLineage for crate::outbox::Outbox {
    fn append_wait(&self, kind: &str, data: Value) -> Result<u64, String> {
        self.post_wait(kind, data).map(|r| r.seq).map_err(|e| e.to_string())
    }
    fn local(&self, kind: &str, data: Value) {
        let _ = self.local_only(kind, data);
    }
}

impl Supervise for crate::supervisor::Handle {
    fn handoff(&self, job: &str, sha: &str, staged_sha256: &str, branch: Option<String>) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let kind = crate::machine::HandoffKind::Promote { job: job.into(), sha: sha.into(), staged_sha256: staged_sha256.into(), branch };
        if !self.send(crate::supervisor::Cmd::Handoff { kind, reply: Some(tx) }) {
            return Err("the supervisor is gone".into());
        }
        rx.recv().unwrap_or_else(|_| Err("the supervisor is gone".into()))
    }
    fn g4_pass(&self, sha: &str, job: &str) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(crate::supervisor::Cmd::G4Pass { sha: sha.into(), job: job.into(), reply: Some(tx) });
        rx.recv().unwrap_or_else(|_| Err("the supervisor is gone".into()))
    }
    fn g4_reject(&self, sha: &str, job: &str, reason: &str) {
        self.send(crate::supervisor::Cmd::G4Reject { sha: sha.into(), job: job.into(), reason: reason.into() });
    }
    fn rollback(&self, reason: &str, immediate: bool) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(crate::supervisor::Cmd::Rollback { reason: reason.into(), immediate, reply: Some(tx) });
        rx.recv().unwrap_or_else(|_| Err("the supervisor is gone".into()))
    }
    fn snapshot(&self) -> Option<SupState> {
        let s = crate::supervisor::Handle::snapshot(self)?;
        Some(SupState {
            current_sha: s.current.sha.clone(),
            lkg_sha: s.lkg_sha.clone(),
            provisional: matches!(s.mode, crate::machine::Mode::Provisional(_)),
            // A handoff or rollback in progress (not the body's own `state{busy}`: the agent
            // calls `initctl promote` from inside a turn, so its body is always busy then).
            busy: s.handoff.is_some() || matches!(s.slot, crate::machine::Slot::Handoff | crate::machine::Slot::RollingBack),
        })
    }
    fn notice(&self, kind: &str, data: Value) {
        self.send(crate::supervisor::Cmd::Notice { kind: kind.into(), data });
    }
    fn data_only(&self, p: crate::supervisor::DataOnly) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel();
        if !self.send(crate::supervisor::Cmd::DataOnly { p, reply: Some(tx) }) {
            return Err("the supervisor is gone".into());
        }
        rx.recv().unwrap_or_else(|_| Err("the supervisor is gone".into()))
    }
}

/// A test hook called at the gate's fault points.
pub type FaultHook = Arc<dyn Fn(&str) + Send + Sync>;

/// Kernel identity for the gate record (`kernel.*`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KernelInfo {
    pub version: String,
    pub build_sha256: String,
    pub config_sha256: String,
    pub charter_sha256: String,
}

/// Everything a gate job needs. One per kernel; cheap to share.
pub struct Env {
    /// `$INIT_HOME` (canonical).
    pub home: PathBuf,
    pub cfg: KernelCfg,
    pub gate: GateCfg,
    /// The Root's spawn broker: builds, probes, checks, selfmap.
    pub spawner: Arc<dyn Spawner>,
    /// G0's commands (kernel-gate's runner owns a child handle; see Deviations in WP8b.md).
    pub g0: Arc<dyn kernel_gate::g0::Spawner + Send + Sync>,
    pub proxy: Arc<dyn GateProxy>,
    pub lineage: Arc<dyn GateLineage>,
    pub sup: Arc<dyn Supervise>,
    pub jobs: Arc<dyn Jobs>,
    /// The `--upload-pack` command of the sandboxed fetch (`$INIT_HOME/kernel/bin/init-kernel
    /// sandboxed-upload-pack` in production).
    pub upload_pack: String,
    /// The level read from the Root at job start.
    pub level: u8,
    pub kernel: KernelInfo,
    /// The default model id (`[model] id`).
    pub model: String,
    /// One G0–G3 pipeline at a time (§3.1 "Concurrency").
    pub pipeline_busy: AtomicBool,
    /// One G4 job at a time; the current one's id.
    pub g4_running: Mutex<Option<String>>,
    /// Gate counter (holdout rotation).
    pub gates: AtomicU64,
    /// G4 jobs started per provisional sha (F12 retries, §3.12 escalation).
    pub g4_attempts: Mutex<std::collections::BTreeMap<String, u32>>,
    /// Test hook: the `gate.g4.mid` and `gate.after_update_ref_main` fault points also call this.
    pub fault: Option<FaultHook>,
    /// G4 v2 (M7): the Root's sealed slice client (production: the outbox's control link). `None`
    /// (test homes): every G4 takes the v0.3 kernel path.
    pub sealed: Option<Arc<dyn g4v2::SealedLink>>,
    /// ADR-011 (X2): the Root's half of the free path (`change.open`, `change.veto`, `store.put`)
    /// and its `extend.mode`. `None` (test homes, older wiring): always the gated path.
    pub extend: Option<Arc<dyn admit::ExtendLink>>,
    /// The single build slot: G0–G3 of `promote` and `gate --quick` take it in turn (FIFO by the
    /// mutex; supervise.md §3.6 "Builds are serialized").
    pub slot_lock: Mutex<()>,
}

impl Env {
    /// The `$G` prefix on `lineage/body.git`.
    pub fn git(&self) -> crate::git::Git {
        crate::git::Git::body(&self.cfg.supervise.git, &self.home)
    }

    /// A fault point (`INIT_FAULT` under the feature, plus the test hook).
    pub fn point(&self, name: &str) {
        crate::fault::point(name);
        if let Some(f) = &self.fault {
            f(name);
        }
    }

    /// `L0`…`L4`.
    pub fn level_str(&self) -> String {
        format!("L{}", self.level)
    }
}

/// An in-memory [`Job`] (tests, and kernel-internal jobs until kernel-serve's store is wired).
#[derive(Default)]
pub struct MemJob {
    pub id: String,
    pub events: Mutex<Vec<Value>>,
    pub result: Mutex<Option<Result<Value, ErrorBody>>>,
    pub cancel: AtomicBool,
    pub boundary: AtomicBool,
    pub cv: std::sync::Condvar,
}

impl MemJob {
    pub fn new(id: &str) -> Arc<MemJob> {
        Arc::new(MemJob { id: id.into(), ..Default::default() })
    }

    /// Wait for the terminal result.
    pub fn wait(&self, timeout: std::time::Duration) -> Option<Result<Value, ErrorBody>> {
        let g = self.result.lock().unwrap();
        let (g, _) = self.cv.wait_timeout_while(g, timeout, |r| r.is_none()).unwrap();
        g.clone()
    }

    /// Wait for `awaiting_boundary` or the terminal result.
    pub fn wait_boundary(&self, timeout: std::time::Duration) -> bool {
        let g = self.result.lock().unwrap();
        let (g, _) = self.cv.wait_timeout_while(g, timeout, |r| r.is_none() && !self.boundary.load(Ordering::SeqCst)).unwrap();
        g.is_none() && self.boundary.load(Ordering::SeqCst)
    }

    pub fn stages(&self) -> Vec<StageResult> {
        self.events.lock().unwrap().iter().filter_map(|e| serde_json::from_value(e.get("stage_done")?.clone()).ok()).collect()
    }
}

impl Job for MemJob {
    fn id(&self) -> String {
        self.id.clone()
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    fn progress(&self, state: JobState, stage: Option<Stage>, pct: u8, msg: &str) {
        self.events.lock().unwrap().push(serde_json::json!({"state": state, "stage": stage, "pct": pct, "msg": msg}));
    }
    fn stage_done(&self, r: StageResult) {
        self.events.lock().unwrap().push(serde_json::json!({"stage_done": r}));
    }
    fn awaiting_boundary(&self) -> bool {
        self.boundary.store(true, Ordering::SeqCst);
        self.cv.notify_all();
        !self.cancelled()
    }
    fn succeed(&self, result: Value) {
        let mut g = self.result.lock().unwrap();
        if g.is_none() {
            *g = Some(Ok(result));
        }
        self.cv.notify_all();
    }
    fn fail(&self, err: ErrorBody) {
        let mut g = self.result.lock().unwrap();
        if g.is_none() {
            *g = Some(Err(err));
        }
        self.cv.notify_all();
    }
}

/// [`Jobs`] handing out [`MemJob`]s `j-9xxxxx` (tests; kernel-serve's store replaces it).
#[derive(Default)]
pub struct MemJobs {
    pub n: AtomicU64,
    pub all: Mutex<Vec<Arc<MemJob>>>,
}

impl Jobs for MemJobs {
    fn create(&self, cmd: &str, args: Value) -> Arc<dyn Job> {
        let n = self.n.fetch_add(1, Ordering::SeqCst) + 1;
        let j = MemJob::new(&format!("j-9{n:05}"));
        j.events.lock().unwrap().push(serde_json::json!({"cmd": cmd, "args": args}));
        self.all.lock().unwrap().push(j.clone());
        j
    }
}

impl MemJobs {
    /// The jobs created for `cmd`, in order.
    pub fn of(&self, cmd: &str) -> Vec<Arc<MemJob>> {
        self.all.lock().unwrap().iter().filter(|j| j.events.lock().unwrap().first().and_then(|e| e["cmd"].as_str().map(|c| c == cmd)) == Some(true)).cloned().collect()
    }
}
