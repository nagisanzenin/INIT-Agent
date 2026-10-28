//! The body-slot state machine (supervise.md §3.3, §3.5, §3.6.4, §3.6.7, §3.7, §3.8.3).
//!
//! A pure machine: [`Machine::on`] takes an event and the current time and returns actions;
//! it performs no I/O, so every row of §3.3 is unit-testable without processes (SV-1…SV-4,
//! SV-10, SV-11, SV-28). The driver ([`crate::supervisor`]) executes the actions and turns
//! the world (the Root's `exited` events, the ready and heartbeat files, body frames, timers)
//! back into events.
//!
//! Panic is not a kernel state (§3.8.1). The T2 pipeline drives this machine only through
//! [`Ev::StartHandoff`] / [`Ev::StartRollback`].

use crate::config::SuperviseCfg;
use init_proto::envelope::ErrorBody;
use init_proto::files::{BootReason, CrashInfo, InitMode, UpdateFailedStage};
use init_proto::initctl::{Counters, Mode as ModeName, Slot as SlotName};
use init_proto::root_link::{KillMode, RootErrorCode};
use init_proto::stdio::{PauseReason, ToBody};
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// The current time, monotonic and wall-clock (ms since the epoch).
#[derive(Clone, Copy, Debug)]
pub struct Now {
    pub mono: Instant,
    pub wall_ms: i64,
}

impl Now {
    pub fn real() -> Now {
        Now { mono: Instant::now(), wall_ms: crate::clock::now_ms() }
    }
    /// `self` advanced by `d` (tests).
    pub fn plus(self, d: Duration) -> Now {
        Now { mono: self.mono + d, wall_ms: self.wall_ms + d.as_millis() as i64 }
    }
}

/// A spawn request id, local to this kernel run.
pub type Tag = u64;

/// Which file under `bin/` a spawn runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinSel {
    /// `bin/init-body`
    Current,
    /// `bin/init-body.lkg`
    Lkg,
    /// `bin/init-body.prev`
    Prev,
    /// `bin/init-body.staged`
    Staged,
}

impl BinSel {
    pub fn file_name(self) -> &'static str {
        match self {
            BinSel::Current => "init-body",
            BinSel::Lkg => "init-body.lkg",
            BinSel::Prev => "init-body.prev",
            BinSel::Staged => "init-body.staged",
        }
    }
}

/// A version of the body: the commit and whether it is provisional.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ver {
    pub sha: String,
    pub provisional: bool,
}

/// `Provisional` details (§3.6.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prov {
    pub sha: String,
    pub depth: u32,
    pub since_ms: i64,
    pub probation_until: Option<Instant>,
}

/// Body safe mode (§3.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafeInfo {
    pub entered_ms: i64,
    pub reason: String,
    /// The Root started the kernel in system safe mode.
    pub system: bool,
    pub crashes: Vec<i64>,
}

/// The body mode while a body runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Provisional(Prov),
    Safe(SafeInfo),
}

impl Mode {
    pub fn name(&self) -> ModeName {
        match self {
            Mode::Normal => ModeName::Normal,
            Mode::Provisional(_) => ModeName::Provisional,
            Mode::Safe(_) => ModeName::Safe,
        }
    }
    pub fn is_safe(&self) -> bool {
        matches!(self, Mode::Safe(_))
    }
}

/// Why the kernel halted (§3.3, §3.7, §3.9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HaltReason {
    Config,
    CrashLoopSafe,
    Layout,
    Tooling,
    NoLkg,
}

impl HaltReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            HaltReason::Config => "config",
            HaltReason::CrashLoopSafe => "crash_loop_safe",
            HaltReason::Layout => "layout",
            HaltReason::Tooling => "tooling",
            HaltReason::NoLkg => "no_lkg",
        }
    }
}

/// What a handoff installs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandoffKind {
    /// A gated candidate from `bin/init-body.staged` (§3.6.4).
    Promote { job: String, sha: String, staged_sha256: String, branch: Option<String> },
    /// Back to LKG from `bin/init-body.lkg` (§3.6.7). With `to` (ADR-013 G2, the free path): back to an
    /// admitted version whose newer changes are all data-only (t0), so the running `bin/init-body` is
    /// its binary too; lineage `main` moves before the new body boots, so its `body/` resync takes the
    /// target's T0 data, and the version stays provisional (its change group is still open).
    Rollback { reason: String, to: Option<String> },
}


/// The phases of §3.6.4.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Pausing { deadline: Instant },
    Fencing { deadline: Instant },
    AwaitSpawn,
    AwaitReady { deadline: Instant },
    Settling { until: Instant },
    Retiring { deadline: Instant, termed: bool },
    Aborting { stage: UpdateFailedStage, reason: String, exit: Option<(Option<i32>, Option<i32>)> },
}

impl Phase {
    pub fn name(&self) -> &'static str {
        match self {
            Phase::Pausing { .. } => "pausing",
            Phase::Fencing { .. } => "fencing",
            Phase::AwaitSpawn | Phase::AwaitReady { .. } => "await_ready",
            Phase::Settling { .. } => "settling",
            Phase::Retiring { .. } => "retiring",
            Phase::Aborting { .. } => "aborting",
        }
    }
}

/// An overlapped handoff in progress (A = `old`, B = `new`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handoff {
    pub token: String,
    pub kind: HandoffKind,
    pub phase: Phase,
    pub old: String,
    pub old_sha: String,
    pub new_tag: Option<Tag>,
    pub new: Option<String>,
    pub started_ms: i64,
    /// A died while paused (§3.6.4: commit anyway if B becomes ready).
    pub old_dead: bool,
}

/// What `Stopping` is doing (§3.8.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopPhase {
    /// `prepare_swap`: waiting for `paused`.
    Pausing,
    /// `stop` frames sent; waiting for exits.
    Stopping,
    /// Killed after the grace period; waiting for `exited`.
    Killing,
}

/// The body slot (§3.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    ColdStart,
    /// Spawn requested (`inst` None until the Root answers), waiting for `ready`.
    Starting { tag: Tag, inst: Option<String>, deadline: Option<Instant> },
    Running,
    Handoff,
    RollingBack,
    Backoff { until: Instant, attempt: u32 },
    Stopped,
    DiskFull { since: Instant },
    Halted { reason: HaltReason },
    Stopping { req: String, exit_code: i32, phase: StopPhase, deadline: Instant, token: Option<String> },
}

impl Slot {
    pub fn name(&self) -> SlotName {
        match self {
            Slot::ColdStart => SlotName::ColdStart,
            Slot::Starting { .. } => SlotName::Starting,
            Slot::Running => SlotName::Running,
            Slot::Handoff => SlotName::Handoff,
            Slot::RollingBack => SlotName::RollingBack,
            Slot::Backoff { .. } => SlotName::Backoff,
            Slot::Stopped => SlotName::Stopped,
            Slot::DiskFull { .. } => SlotName::DiskFull,
            Slot::Halted { .. } => SlotName::Halted,
            Slot::Stopping { .. } => SlotName::Stopping,
        }
    }
}

/// What the machine asks the driver to spawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnPlan {
    pub tag: Tag,
    pub bin: BinSel,
    pub sha: String,
    pub reason: BootReason,
    pub mode: InitMode,
    pub replaces: Option<String>,
    pub crash: Option<CrashInfo>,
    pub branch: Option<String>,
    pub from_sha: Option<String>,
    /// `boot.json.cause.kind` for a `handoff` boot without a body change (ADR-009:
    /// `workspace_change`); None otherwise.
    pub cause: Option<String>,
}

/// Events.
#[derive(Clone, Debug, PartialEq)]
pub enum Ev {
    /// Cold start finished (§3.9 steps 1–11): start the primary.
    Start { reason: BootReason },
    /// Cold start found no usable body (`Halted{layout|tooling|no_lkg}`).
    Halt(HaltReason),
    /// A spawn request was answered.
    Spawned { tag: Tag, result: Result<String, String> },
    /// `state/run/ready` carries this instance's `boot_id`.
    Ready { inst: String },
    /// The Root's `exited` event. `disk_low`: free space below the run threshold right now.
    Exited { inst: String, code: Option<i32>, signal: Option<i32>, disk_low: bool, stderr_tail: String },
    HeartbeatStale { inst: String },
    Flood { inst: String },
    Busy { inst: String, busy: bool },
    Paused { inst: String, token: String },
    FenceOk,
    FenceBusy,
    Tick,
    /// Free space after a `DiskFull`: true when ≥ 2 × the run threshold.
    DiskRecovered,
    OwnerStop,
    OwnerResume,
    /// Any other owner line (it is already in the inbox).
    OwnerLine,
    RootStop { req: String, grace_ms: u64 },
    RootPrepareSwap { req: String, deadline_s: u64 },
    LinkLost,
    StartHandoff(HandoffKind),
    /// G4 passed for the current provisional SHA and the LKG commit finished (§3.6.6).
    LkgCommitted { sha: String },
    /// Leave body safe mode (a normal-mode kernel start after safe, §3.9 step 10).
    SafeCleared,
    /// Restart the primary on the same version and session at its next idle point (ADR-009: the
    /// owner's workspace changed, and a body's profile is fixed at spawn). It boots `handoff` with
    /// `boot.json.cause.kind = cause`; a second request before then only replaces the cause.
    Restart { cause: String },
}

/// Actions.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Spawn(SpawnPlan),
    Kill { inst: String, mode: KillMode },
    Send { inst: String, frame: ToBody },
    /// Redeliver every unacked input and notice to the instance (it became ready).
    Redeliver { inst: String },
    /// Install `bin/<from>` as `bin/init-body`, keeping the outgoing one as `.prev` (§3.6.5).
    InstallBin { from: BinSel },
    /// Move lineage `main` back (rollback, §3.6.7): keep `refs/init/rolled-back/<from>`.
    RollbackMain { from: String, to: String },
    /// `lineage.append` through the outbox (§4.7).
    Report { kind: &'static str, data: Value },
    /// A kernel-local `vault/lineage.jsonl` line only (failed swaps are non-events at the Root).
    LocalLineage { kind: &'static str, data: Value },
    Alert { code: &'static str, message: String },
    UpdateFailed { stage: UpdateFailedStage, reason: String, sha: String, from_sha: Option<String>, job: Option<String>, exit: Option<(Option<i32>, Option<i32>)>, log_tail: String },
    Notice { kind: &'static str, data: Value },
    FenceTry,
    FenceRelease,
    /// Commit a promote handoff (§3.6.4 step 5): bin renames, `update-ref main`, `promotion`.
    Commit { job: String, from_sha: String, to_sha: String, staged_sha256: String },
    /// Commit a rollback handoff (§3.6.7). `data_only`: the target runs the current binary and ext set
    /// (ADR-013 G2), so nothing in `bin/` is installed.
    CommitRollback { from_sha: String, to_sha: String, reason: String, data_only: bool },
    /// Undo [`Act::RollbackMain`] after a data-only rollback aborted: lineage `main` `from` → `to`.
    RestoreMain { from: String, to: String },
    Respond { req: String, result: Result<Value, ErrorBody> },
    /// Flush and exit the kernel process with this code (§3.13.5).
    Exit(i32),
    /// Stop L2 members (safe mode, demotion; §3.14).
    StopFleet,
    /// A handoff ended (the pipeline job completes or fails `E_HANDOFF_FAILED`).
    HandoffDone { ok: bool, reason: Option<String> },
    /// Resume the paused A after an abort, with the `update-failed.json` just written.
    ResumeA { inst: String, token: String },
    /// Write `supervisor.json` now.
    Persist,
}

/// Crash bookkeeping (§3.7; counted separately, statixagent #28).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CrashTrack {
    pub consecutive: u32,
    pub lkg_window: VecDeque<i64>,
    pub safe_window: VecDeque<i64>,
    pub config_exits: u32,
    pub last_exit: Option<(i64, Option<i32>, Option<i32>)>,
}

/// A body instance the machine knows about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inst {
    pub id: String,
    pub sha: String,
    pub ready_at: Option<Instant>,
}

/// The machine.
pub struct Machine {
    pub cfg: SuperviseCfg,
    pub slot: Slot,
    pub mode: Mode,
    pub current: Ver,
    pub prev: Option<Ver>,
    pub lkg_sha: String,
    pub primary: Option<Inst>,
    pub handoff: Option<Handoff>,
    pub crash: CrashTrack,
    pub counters: Counters,
    pub busy: bool,
    /// Killed or retiring instances whose `exited` has not arrived yet.
    pub zombies: BTreeMap<String, Instant>,
    /// A spawn waiting for zombies to be gone (the old body must release the log lock).
    pub deferred: Option<SpawnPlan>,
    owner_stop: Option<Instant>,
    pending_crash: Option<CrashInfo>,
    next_tag: Tag,
    rollback_pending: Option<(String, Instant, Option<String>)>,
    /// A graceful restart waiting for the next idle point (ADR-009).
    pub restart_pending: Option<String>,
    /// The primary was asked to stop for a restart: the cause and the kill deadline.
    restarting: Option<(String, Instant)>,
}

/// How long a deferred spawn waits for a zombie's `exited` before going ahead anyway.
const ZOMBIE_WAIT: Duration = Duration::from_secs(12);

impl Machine {
    pub fn new(cfg: SuperviseCfg, current: Ver, lkg_sha: String, mode: Mode) -> Machine {
        Machine {
            cfg,
            slot: Slot::ColdStart,
            mode,
            current,
            prev: None,
            lkg_sha,
            primary: None,
            handoff: None,
            crash: CrashTrack::default(),
            counters: Counters { crashes_total: 0, restarts_since_lkg: 0, rollbacks: 0, swaps_failed: 0, swaps_ok: 0 },
            busy: false,
            zombies: BTreeMap::new(),
            deferred: None,
            owner_stop: None,
            pending_crash: None,
            next_tag: 1,
            rollback_pending: None,
            restart_pending: None,
            restarting: None,
        }
    }

    fn tag(&mut self) -> Tag {
        let t = self.next_tag;
        self.next_tag += 1;
        t
    }

    fn init_mode(&self) -> InitMode {
        if self.mode.is_safe() { InitMode::Safe } else { InitMode::Normal }
    }

    /// The instance currently receiving owner input (None while paused, starting, …).
    pub fn relay_target(&self) -> Option<&str> {
        match self.slot {
            Slot::Running => self.primary.as_ref().filter(|p| p.ready_at.is_some()).map(|p| p.id.as_str()),
            _ => None,
        }
    }

    /// Plan a spawn of the primary and move to `Starting`.
    fn start(&mut self, bin: BinSel, reason: BootReason, crash: Option<CrashInfo>, now: Now) -> Vec<Act> {
        let tag = self.tag();
        let mode = self.init_mode();
        let bin = if mode == InitMode::Safe { BinSel::Lkg } else { bin };
        let sha = if bin == BinSel::Lkg { self.lkg_sha.clone() } else { self.current.sha.clone() };
        let plan = SpawnPlan { tag, bin, sha, reason, mode, replaces: None, crash, branch: None, from_sha: Some(self.current.sha.clone()), cause: None };
        self.slot = Slot::Starting { tag, inst: None, deadline: None };
        self.primary = None;
        self.busy = false;
        self.restart_pending = None; // a new primary reads the latest environment anyway
        self.launch(plan, now)
    }

    /// Emit the spawn now, or defer it until every zombie has exited.
    fn launch(&mut self, plan: SpawnPlan, now: Now) -> Vec<Act> {
        self.zombies.retain(|_, since| now.mono.duration_since(*since) < ZOMBIE_WAIT);
        if self.zombies.is_empty() {
            vec![Act::Spawn(plan), Act::Persist]
        } else {
            self.deferred = Some(plan);
            vec![Act::Persist]
        }
    }

    fn kill(&mut self, inst: &str, mode: KillMode, now: Now) -> Act {
        self.zombies.entry(inst.to_string()).or_insert(now.mono);
        Act::Kill { inst: inst.to_string(), mode }
    }

    fn primary_is(&self, inst: &str) -> bool {
        self.primary.as_ref().is_some_and(|p| p.id == inst)
    }

    /// Handle one event.
    pub fn on(&mut self, ev: Ev, now: Now) -> Vec<Act> {
        let mut acts = Vec::new();
        // Exits of zombies are bookkeeping first.
        if let Ev::Exited { inst, .. } = &ev {
            let was_zombie = self.zombies.remove(inst).is_some();
            if self.zombies.is_empty() {
                if let Some(p) = self.deferred.take() {
                    acts.push(Act::Spawn(p));
                }
            }
            let ours = self.primary_is(inst)
                || self.handoff.as_ref().is_some_and(|h| h.old == *inst || h.new.as_deref() == Some(inst.as_str()))
                || matches!(&self.slot, Slot::Starting { inst: Some(i), .. } if i == inst);
            if was_zombie && !ours {
                return acts;
            }
        }
        acts.extend(self.step(ev, now));
        acts
    }

    fn step(&mut self, ev: Ev, now: Now) -> Vec<Act> {
        // Rows that apply in any state.
        match &ev {
            Ev::RootStop { req, grace_ms } => return self.begin_stop(req.clone(), 0, Duration::from_millis(*grace_ms), now),
            Ev::RootPrepareSwap { req, deadline_s } => return self.begin_swap(req.clone(), Duration::from_secs(*deadline_s), now),
            Ev::Tick => return self.tick(now),
            _ => {}
        }
        if matches!(self.slot, Slot::Stopping { .. }) {
            return self.stopping(ev, now);
        }
        if self.handoff.is_some() {
            return self.handoff_ev(ev, now);
        }
        match (self.slot.clone(), ev) {
            (Slot::ColdStart, Ev::Start { reason }) => self.start(BinSel::Current, reason, None, now),
            (_, Ev::Halt(r)) => self.halt(r, None),
            (Slot::Starting { tag, inst: None, .. }, Ev::Spawned { tag: t, result }) if t == tag => match result {
                Ok(id) => {
                    let sha = self.deferred_sha_hint();
                    self.primary = Some(Inst { id: id.clone(), sha, ready_at: None });
                    self.slot = Slot::Starting { tag, inst: Some(id), deadline: Some(now.mono + self.cfg.ready_timeout()) };
                    vec![Act::Persist]
                }
                Err(e) => {
                    // Not a crash of the body: the Root refused or the link failed.
                    let attempt = self.crash.consecutive + 1;
                    self.slot = Slot::Backoff { until: now.mono + self.cfg.backoff(attempt), attempt };
                    vec![Act::Alert { code: "spawn_failed", message: e }, Act::Persist]
                }
            },
            (Slot::Starting { inst: Some(i), .. }, Ev::Ready { inst }) if i == inst => {
                if let Some(p) = self.primary.as_mut() {
                    p.ready_at = Some(now.mono);
                }
                self.crash.config_exits = 0;
                self.slot = Slot::Running;
                let mut acts = vec![Act::Redeliver { inst }, Act::Persist];
                acts.extend(self.maybe_restart(now));
                acts
            }
            (_, Ev::Exited { inst, code, signal, disk_low, stderr_tail }) if self.primary_is(&inst) => self.primary_exit(inst, code, signal, disk_low, stderr_tail, now),
            (Slot::Running, Ev::HeartbeatStale { inst }) | (Slot::Running, Ev::Flood { inst }) | (Slot::Starting { .. }, Ev::Flood { inst })
                if self.primary_is(&inst) =>
            {
                // A crash detected by the kernel: kill; the crash is counted when `exited` arrives.
                vec![self.kill(&inst, KillMode::Kill, now)]
            }
            (_, Ev::Busy { inst, busy }) if self.primary_is(&inst) => {
                self.busy = busy;
                let mut acts = self.maybe_deferred_rollback(now);
                acts.extend(self.maybe_restart(now));
                acts
            }
            (_, Ev::Restart { cause }) => {
                if self.restarting.is_some() {
                    return vec![]; // the stop is under way; the new body reads the latest access
                }
                self.restart_pending = Some(cause);
                self.maybe_restart(now)
            }
            (Slot::Running, Ev::OwnerStop) => match self.primary.clone() {
                Some(p) => {
                    self.owner_stop = Some(now.mono + Duration::from_millis(init_proto::stdio::STOP_GRACE_MS) + self.cfg.exit_grace());
                    vec![Act::Send { inst: p.id, frame: ToBody::Stop { reason: "owner".into(), grace_ms: init_proto::stdio::STOP_GRACE_MS } }]
                }
                None => vec![],
            },
            (Slot::Stopped, Ev::OwnerResume) | (Slot::Stopped, Ev::OwnerLine) => self.start(BinSel::Current, BootReason::Cold, None, now),
            (Slot::Halted { .. }, Ev::OwnerResume) => {
                self.crash = CrashTrack::default();
                if !self.mode.is_safe() {
                    self.mode = Mode::Safe(SafeInfo { entered_ms: now.wall_ms, reason: "resume".into(), system: false, crashes: vec![] });
                }
                self.start(BinSel::Lkg, BootReason::Cold, None, now)
            }
            (Slot::DiskFull { .. }, Ev::DiskRecovered) => self.start(BinSel::Current, BootReason::Crash, None, now),
            (Slot::Running, Ev::StartHandoff(kind)) => self.begin_handoff(kind, now),
            (_, Ev::StartHandoff(_)) => vec![Act::HandoffDone { ok: false, reason: Some(format!("body slot is {:?}, not running", self.slot.name())) }],
            (_, Ev::LkgCommitted { sha }) => {
                if self.current.sha == sha {
                    self.current.provisional = false;
                    self.mode = if self.mode.is_safe() { self.mode.clone() } else { Mode::Normal };
                }
                self.lkg_sha = sha;
                self.counters.restarts_since_lkg = 0;
                vec![Act::Persist]
            }
            (_, Ev::SafeCleared) => {
                if self.mode.is_safe() {
                    self.mode = Mode::Normal;
                    return vec![Act::Report { kind: "body_safe", data: json!({"on": false, "sha": self.lkg_sha, "crashes": []}) }, Act::Persist];
                }
                vec![]
            }
            (_, Ev::LinkLost) => vec![], // the driver stops bodies and exits 69 (§3.13.1)
            _ => vec![],
        }
    }

    fn deferred_sha_hint(&self) -> String {
        match &self.slot {
            Slot::Starting { .. } if self.mode.is_safe() => self.lkg_sha.clone(),
            _ => self.current.sha.clone(),
        }
    }

    fn halt(&mut self, r: HaltReason, tail: Option<String>) -> Vec<Act> {
        let msg = match &tail {
            Some(t) if !t.is_empty() => format!("halted: {}; stderr tail: {}", r.as_str(), crate::fsx::tail_str(t, 1024)),
            _ => format!("halted: {}", r.as_str()),
        };
        let code = if r == HaltReason::Config { "config_error" } else { "halted" };
        self.slot = Slot::Halted { reason: r.clone() };
        self.primary = None;
        vec![
            Act::Alert { code, message: msg },
            Act::Report { kind: "halted", data: json!({"reason": r.as_str()}) },
            Act::StopFleet,
            Act::Persist,
        ]
    }

    /// §3.3 crash rows and §3.7.
    fn primary_exit(&mut self, inst: String, code: Option<i32>, signal: Option<i32>, disk_low: bool, tail: String, now: Now) -> Vec<Act> {
        let was_starting = matches!(self.slot, Slot::Starting { .. });
        self.primary = None;
        self.busy = false;
        self.crash.last_exit = Some((now.wall_ms, code, signal));
        let _ = inst;
        // A restart we asked for (ADR-009): never a crash, whatever the exit; same version, same session.
        if let Some((cause, _)) = self.restarting.take() {
            let mut acts = self.start(BinSel::Current, BootReason::Handoff, None, now);
            for a in acts.iter_mut() {
                if let Act::Spawn(p) = a {
                    p.cause = Some(cause.clone());
                }
            }
            if let Some(p) = self.deferred.as_mut() {
                p.cause = Some(cause);
            }
            return acts;
        }
        // Exit 0: the body stopped (the owner asked); never a crash.
        if code == Some(0) || self.owner_stop.is_some() {
            self.owner_stop = None;
            self.slot = Slot::Stopped;
            return vec![Act::Persist];
        }
        // Exit 78 at boot: an owner configuration error, not a crash (§3.3, F-crash-boot (d)).
        if was_starting && code == Some(78) {
            self.crash.config_exits += 1;
            if self.crash.config_exits >= self.cfg.config_error_exits {
                return self.halt(HaltReason::Config, Some(tail));
            }
            self.slot = Slot::Backoff { until: now.mono + self.cfg.backoff(1), attempt: 0 };
            return vec![Act::Persist];
        }
        self.crash.config_exits = 0;
        if disk_low {
            self.slot = Slot::DiskFull { since: now.mono };
            return vec![Act::Alert { code: "disk_low", message: "free space below the run threshold at a body crash".into() }, Act::Persist];
        }
        self.counters.crashes_total += 1;
        self.counters.restarts_since_lkg += 1;
        let crash = Some(CrashInfo { exit_code: code, log_tail: crate::fsx::tail_str(&tail, init_proto::files::MAX_LOG_TAIL), signal });
        match self.mode.clone() {
            Mode::Provisional(p) => {
                let in_probation = p.probation_until.is_some_and(|u| now.mono < u);
                let cur = self.current.sha.clone();
                let (from, stage, target) = if in_probation {
                    self.counters.swaps_failed += 1;
                    let prev = self.prev.clone().unwrap_or(Ver { sha: self.lkg_sha.clone(), provisional: false });
                    (prev, UpdateFailedStage::Probation, BinSel::Prev)
                } else {
                    (Ver { sha: self.lkg_sha.clone(), provisional: false }, UpdateFailedStage::CrashProvisional, BinSel::Lkg)
                };
                self.counters.rollbacks += 1;
                let mut acts = vec![
                    Act::UpdateFailed { stage, reason: "crash".into(), sha: cur.clone(), from_sha: None, job: None, exit: Some((code, signal)), log_tail: crash.as_ref().unwrap().log_tail.clone() },
                    Act::InstallBin { from: target },
                    Act::RollbackMain { from: cur.clone(), to: from.sha.clone() },
                    Act::Report { kind: "body_rollback", data: json!({"from_sha": cur, "to_sha": from.sha, "reason": if in_probation { "probation" } else { "crash_provisional" }}) },
                ];
                self.prev = Some(self.current.clone());
                self.current = from.clone();
                self.mode = if from.sha == self.lkg_sha || !from.provisional {
                    Mode::Normal
                } else {
                    Mode::Provisional(Prov { sha: from.sha.clone(), depth: p.depth.saturating_sub(1).max(1), since_ms: now.wall_ms, probation_until: None })
                };
                acts.extend(self.start(BinSel::Current, BootReason::Rollback, crash, now));
                acts
            }
            Mode::Normal => {
                let win = self.cfg.crash_window().as_millis() as i64;
                self.crash.lkg_window.push_back(now.wall_ms);
                while self.crash.lkg_window.front().is_some_and(|t| now.wall_ms - t > win) {
                    self.crash.lkg_window.pop_front();
                }
                self.crash.consecutive += 1;
                let attempt = self.crash.consecutive;
                let mut acts = Vec::new();
                if self.crash.lkg_window.len() as u32 > self.cfg.crash_limit {
                    let crashes: Vec<i64> = self.crash.lkg_window.iter().copied().collect();
                    let stamps: Vec<String> = crashes.iter().map(|t| crate::clock::format_ms(*t)).collect();
                    self.mode = Mode::Safe(SafeInfo { entered_ms: now.wall_ms, reason: "crash_loop".into(), system: false, crashes });
                    self.crash.safe_window.clear();
                    acts.push(Act::Report { kind: "body_safe", data: json!({"on": true, "sha": self.lkg_sha, "crashes": stamps}) });
                    acts.push(Act::Alert { code: "safe_entered", message: format!("{} crashes on LKG within {} s: body safe mode", stamps.len(), self.cfg.crash_window_s) });
                    acts.push(Act::Notice { kind: "safe", data: json!({"on": true}) });
                    acts.push(Act::StopFleet);
                }
                self.slot = Slot::Backoff { until: now.mono + self.cfg.backoff(attempt), attempt };
                self.pending_crash = crash;
                acts.push(Act::Persist);
                acts
            }
            Mode::Safe(_) => {
                let win = self.cfg.crash_window().as_millis() as i64;
                self.crash.safe_window.push_back(now.wall_ms);
                while self.crash.safe_window.front().is_some_and(|t| now.wall_ms - t > win) {
                    self.crash.safe_window.pop_front();
                }
                self.crash.consecutive += 1;
                if self.crash.safe_window.len() as u32 > self.cfg.safe_crash_limit {
                    return self.halt(HaltReason::CrashLoopSafe, Some(tail));
                }
                let attempt = self.crash.consecutive;
                self.slot = Slot::Backoff { until: now.mono + self.cfg.backoff(attempt), attempt };
                self.pending_crash = crash;
                vec![Act::Persist]
            }
        }
    }

    fn tick(&mut self, now: Now) -> Vec<Act> {
        let mut acts = Vec::new();
        // A deferred spawn whose zombies never reported: go ahead after ZOMBIE_WAIT.
        if self.deferred.is_some() {
            self.zombies.retain(|_, since| now.mono.duration_since(*since) < ZOMBIE_WAIT);
            if self.zombies.is_empty() {
                acts.push(Act::Spawn(self.deferred.take().unwrap()));
            }
        }
        match self.slot.clone() {
            Slot::Starting { inst: Some(i), deadline: Some(d), .. } if now.mono >= d => {
                // No ready in time: a crash. Kill; the crash is handled at `exited`.
                if !self.zombies.contains_key(&i) {
                    acts.push(self.kill(&i, KillMode::Kill, now));
                }
            }
            Slot::Backoff { until, .. } if now.mono >= until => {
                let crash = self.pending_crash.take();
                let reason = if crash.is_some() { BootReason::Crash } else { BootReason::Cold };
                acts.extend(self.start(BinSel::Current, reason, crash, now));
            }
            Slot::Running => {
                if let (Some(d), Some(p)) = (self.owner_stop, self.primary.clone()) {
                    if now.mono >= d && !self.zombies.contains_key(&p.id) {
                        acts.push(self.kill(&p.id, KillMode::Kill, now));
                    }
                }
                if let (Some((_, d)), Some(p)) = (self.restarting.clone(), self.primary.clone()) {
                    if now.mono >= d && !self.zombies.contains_key(&p.id) {
                        acts.push(self.kill(&p.id, KillMode::Kill, now));
                    }
                }
                acts.extend(self.maybe_restart(now));
                if let Some(p) = &self.primary {
                    if p.ready_at.is_some_and(|r| now.mono.duration_since(r) >= self.cfg.backoff_reset_after()) && self.crash.consecutive != 0 {
                        self.crash.consecutive = 0;
                    }
                }
                if let Mode::Provisional(p) = &mut self.mode {
                    if p.probation_until.is_some_and(|u| now.mono >= u) {
                        p.probation_until = None;
                        acts.push(Act::Persist);
                    }
                }
                acts.extend(self.maybe_deferred_rollback(now));
            }
            Slot::Stopping { .. } => acts.extend(self.stopping(Ev::Tick, now)),
            _ => {}
        }
        if self.handoff.is_some() {
            acts.extend(self.handoff_tick(now));
        }
        acts
    }

    // ------------------------------------------------------------------------------------
    // Overlapped handoff (§3.6.4) and rollback (§3.6.7)
    // ------------------------------------------------------------------------------------

    /// Ask for a rollback to LKG: at the next idle point (or after `rollback_max_defer_s`).
    pub fn request_rollback(&mut self, reason: &str, immediate: bool, now: Now) -> Vec<Act> {
        self.request_rollback_to(reason, None, immediate, now)
    }

    /// Ask for a rollback to `to` (a data-only target, [`HandoffKind::Rollback`]) or, with `None` or
    /// `to` = LKG, to LKG.
    pub fn request_rollback_to(&mut self, reason: &str, to: Option<String>, immediate: bool, now: Now) -> Vec<Act> {
        let to = to.filter(|t| *t != self.lkg_sha);
        if self.current.sha == to.as_deref().unwrap_or(&self.lkg_sha) {
            return vec![Act::HandoffDone { ok: false, reason: Some(if to.is_some() { "current is the target" } else { "current is LKG" }.into()) }];
        }
        if immediate {
            return self.step(Ev::StartHandoff(HandoffKind::Rollback { reason: reason.into(), to }), now);
        }
        self.rollback_pending = Some((reason.to_string(), now.mono + self.cfg.rollback_max_defer(), to));
        self.maybe_deferred_rollback(now)
    }

    /// The version and binary a rollback handoff boots.
    pub fn rollback_target(&self, to: &Option<String>) -> (BinSel, String) {
        match to {
            Some(t) => (BinSel::Current, t.clone()),
            None => (BinSel::Lkg, self.lkg_sha.clone()),
        }
    }

    fn maybe_deferred_rollback(&mut self, now: Now) -> Vec<Act> {
        let Some((reason, force_at, to)) = self.rollback_pending.clone() else { return vec![] };
        if self.slot != Slot::Running || self.handoff.is_some() {
            return vec![];
        }
        if !self.busy || now.mono >= force_at {
            self.rollback_pending = None;
            return self.begin_handoff(HandoffKind::Rollback { reason, to }, now);
        }
        vec![]
    }

    /// ADR-009: a pending restart begins when the primary is ready and idle, and nothing else
    /// (a handoff, a rollback, an owner stop) is under way: `stop` to the body, then a new spawn at
    /// its exit (`primary_exit`), or a kill after the stop grace.
    fn maybe_restart(&mut self, now: Now) -> Vec<Act> {
        if self.restart_pending.is_none() || self.restarting.is_some() || self.slot != Slot::Running || self.handoff.is_some() {
            return vec![];
        }
        if self.busy || self.owner_stop.is_some() || self.rollback_pending.is_some() {
            return vec![];
        }
        let Some(p) = self.primary.clone().filter(|p| p.ready_at.is_some()) else { return vec![] };
        let cause = self.restart_pending.take().unwrap_or_default();
        let grace = Duration::from_millis(init_proto::stdio::STOP_GRACE_MS);
        self.restarting = Some((cause, now.mono + grace + self.cfg.exit_grace()));
        vec![Act::Send { inst: p.id, frame: ToBody::Stop { reason: "restart".into(), grace_ms: init_proto::stdio::STOP_GRACE_MS } }]
    }

    fn begin_handoff(&mut self, kind: HandoffKind, now: Now) -> Vec<Act> {
        let Some(a) = self.primary.clone().filter(|p| p.ready_at.is_some()) else {
            return vec![Act::HandoffDone { ok: false, reason: Some("no ready body".into()) }];
        };
        if self.mode.is_safe() {
            return vec![Act::HandoffDone { ok: false, reason: Some("safe mode".into()) }];
        }
        let token = crate::sys::random_hex(8);
        let reason = match kind {
            HandoffKind::Promote { .. } => PauseReason::Handoff,
            HandoffKind::Rollback { .. } => PauseReason::Rollback,
        };
        self.slot = if matches!(kind, HandoffKind::Rollback { .. }) { Slot::RollingBack } else { Slot::Handoff };
        self.handoff = Some(Handoff {
            token: token.clone(),
            kind,
            phase: Phase::Pausing { deadline: now.mono + self.cfg.pause_timeout() },
            old: a.id.clone(),
            old_sha: self.current.sha.clone(),
            new_tag: None,
            new: None,
            started_ms: now.wall_ms,
            old_dead: false,
        });
        vec![Act::Persist, Act::Send { inst: a.id, frame: ToBody::Pause { token, reason } }]
    }

    fn abort(&mut self, stage: UpdateFailedStage, reason: &str, exit: Option<(Option<i32>, Option<i32>)>, now: Now) -> Vec<Act> {
        let Some(h) = self.handoff.as_mut() else { return vec![] };
        let mut acts = vec![Act::FenceRelease];
        h.phase = Phase::Aborting { stage, reason: reason.to_string(), exit };
        if let Some(b) = h.new.clone() {
            if !self.zombies.contains_key(&b) {
                acts.push(self.kill(&b, KillMode::Kill, now));
            }
            // Resume A only after B's `exited` (so the OS has released B's lock).
            return acts;
        }
        if h.new_tag.is_some() {
            return acts; // the spawn is in flight; finish the abort when it is answered
        }
        acts.extend(self.finish_abort());
        acts
    }

    fn finish_abort(&mut self) -> Vec<Act> {
        let Some(h) = self.handoff.take() else { return vec![] };
        let Phase::Aborting { stage, reason, exit } = h.phase else { return vec![] };
        self.counters.swaps_failed += 1;
        let (sha, job) = match &h.kind {
            HandoffKind::Promote { sha, job, .. } => (sha.clone(), Some(job.clone())),
            HandoffKind::Rollback { to, .. } => (self.rollback_target(to).1, None),
        };
        let mut acts = vec![
            Act::UpdateFailed { stage: stage.clone(), reason: reason.clone(), sha: sha.clone(), from_sha: Some(h.old_sha.clone()), job: job.clone(), exit, log_tail: String::new() },
            Act::LocalLineage { kind: "handoff_failed", data: json!({"from": h.old_sha, "to": sha, "job": job, "stage": stage.as_str(), "reason": reason}) },
        ];
        if let HandoffKind::Rollback { to, .. } = &h.kind {
            acts.push(Act::Alert { code: "rollback_failed", message: format!("rollback to {} failed: {reason}", to.as_deref().map_or("LKG", |_| sha.as_str())) });
            if to.is_some() {
                acts.push(Act::RestoreMain { from: sha.clone(), to: h.old_sha.clone() });
            }
        }
        if h.old_dead {
            // A died while paused and B never became ready: apply the crash rules to the current version.
            self.slot = Slot::Running;
            acts.push(Act::HandoffDone { ok: false, reason: Some(reason) });
            let a = h.old.clone();
            self.primary = Some(Inst { id: a.clone(), sha: h.old_sha.clone(), ready_at: None });
            acts.extend(self.primary_exit(a, None, None, false, String::new(), Now::real()));
            return acts;
        }
        self.slot = Slot::Running;
        acts.push(Act::ResumeA { inst: h.old.clone(), token: h.token.clone() });
        acts.push(Act::HandoffDone { ok: false, reason: Some(reason) });
        acts.push(Act::Persist);
        acts
    }

    fn handoff_ev(&mut self, ev: Ev, now: Now) -> Vec<Act> {
        let h = self.handoff.clone().unwrap();
        match (h.phase.clone(), ev) {
            (Phase::Pausing { .. }, Ev::Paused { inst, token }) if inst == h.old && token == h.token => {
                crate::fault::point("supervise.handoff.after_paused");
                self.set_phase(Phase::Fencing { deadline: now.mono + Duration::from_secs(5) });
                vec![Act::Persist, Act::FenceTry]
            }
            (Phase::Fencing { .. }, Ev::FenceOk) => {
                crate::fault::point("supervise.handoff.after_fence");
                let tag = self.tag();
                let (bin, sha, reason, branch) = match &h.kind {
                    HandoffKind::Promote { sha, branch, .. } => (BinSel::Staged, sha.clone(), BootReason::Promote, branch.clone()),
                    HandoffKind::Rollback { to, .. } => {
                        let (bin, sha) = self.rollback_target(to);
                        (bin, sha, BootReason::Rollback, None)
                    }
                };
                // ADR-013 G2: a data-only rollback moves lineage `main` first, so B's `body/` resync
                // (evolution.md §3.6) checks out the target's T0 data before B builds its head.
                let early_main = matches!(&h.kind, HandoffKind::Rollback { to: Some(_), .. }).then(|| Act::RollbackMain { from: h.old_sha.clone(), to: sha.clone() });
                // The Root's extend.rollback (reason `extend_<signal>`) names itself in boot.json.cause,
                // so the new body never reads an older, stale cause (demo 2026-10-25, F4).
                let cause = match &h.kind {
                    HandoffKind::Rollback { reason, .. } if reason.starts_with("extend_") => Some(reason.clone()),
                    _ => None,
                };
                let plan = SpawnPlan { tag, bin, sha, reason, mode: InitMode::Normal, replaces: Some(h.old.clone()), crash: None, branch, from_sha: Some(h.old_sha.clone()), cause };
                let hm = self.handoff.as_mut().unwrap();
                hm.new_tag = Some(tag);
                hm.phase = Phase::AwaitSpawn;
                let mut acts = vec![Act::Persist];
                acts.extend(early_main);
                acts.push(Act::Spawn(plan));
                acts
            }
            (Phase::Fencing { deadline }, Ev::FenceBusy) => {
                if now.mono >= deadline {
                    self.abort(UpdateFailedStage::Fence, "fence: the paused body still holds the log lock", None, now)
                } else {
                    vec![] // retried on the next tick
                }
            }
            (Phase::AwaitSpawn, Ev::Spawned { tag, result }) if Some(tag) == h.new_tag => {
                crate::fault::point("supervise.handoff.after_spawn");
                match result {
                    Ok(id) => {
                        let hm = self.handoff.as_mut().unwrap();
                        hm.new = Some(id);
                        hm.phase = Phase::AwaitReady { deadline: now.mono + self.cfg.ready_timeout() };
                        vec![Act::FenceRelease, Act::Persist]
                    }
                    Err(e) => self.abort(UpdateFailedStage::Handoff, &format!("spawn failed: {e}"), None, now),
                }
            }
            (Phase::Aborting { .. }, Ev::Spawned { tag, result }) if Some(tag) == h.new_tag => match result {
                Ok(id) => {
                    self.handoff.as_mut().unwrap().new = Some(id.clone());
                    vec![self.kill(&id, KillMode::Kill, now)]
                }
                Err(_) => self.finish_abort(),
            },
            (Phase::AwaitReady { .. }, Ev::Ready { inst }) if h.new.as_deref() == Some(inst.as_str()) => {
                crate::fault::point("supervise.handoff.after_ready");
                self.set_phase(Phase::Settling { until: now.mono + self.cfg.settle() });
                vec![Act::Persist]
            }
            // B ends before commit → abort (or, when it was being killed for an abort, finish it).
            (phase, Ev::Exited { inst, code, signal, .. }) if h.new.as_deref() == Some(inst.as_str()) => match phase {
                Phase::Aborting { .. } => self.finish_abort(),
                Phase::Retiring { .. } => {
                    // B (now the primary) crashed while A retires: crash rules on the new version.
                    // The handoff did commit, so answer it as A's exit would have (K-FLAKE: without
                    // this the caller's reply was never sent and `promote` waited forever when B
                    // crashed before A had exited, e.g. SV-10 under load).
                    self.handoff = None;
                    let mut acts = vec![Act::HandoffDone { ok: true, reason: None }];
                    acts.extend(self.primary_exit(inst, code, signal, false, String::new(), now));
                    acts
                }
                _ => {
                    // B is gone already: nothing to kill or wait for.
                    let hm = self.handoff.as_mut().unwrap();
                    hm.new = None;
                    hm.new_tag = None;
                    self.abort(UpdateFailedStage::Handoff, &format!("candidate exited before commit (code {code:?}, signal {signal:?})"), Some((code, signal)), now)
                }
            },
            // A ends.
            (phase, Ev::Exited { inst, code, signal, disk_low, stderr_tail }) if inst == h.old => match phase {
                Phase::Retiring { .. } => {
                    self.handoff = None;
                    self.slot = Slot::Running;
                    vec![Act::HandoffDone { ok: true, reason: None }, Act::Persist]
                }
                Phase::Pausing { .. } | Phase::Fencing { .. } if h.new_tag.is_none() => {
                    // A died before B was spawned: a crash of A; the handoff is dropped.
                    self.handoff = None;
                    self.slot = Slot::Running;
                    let mut acts = vec![Act::FenceRelease, Act::HandoffDone { ok: false, reason: Some("live body died while pausing".into()) }];
                    acts.extend(self.primary_exit(inst, code, signal, disk_low, stderr_tail, now));
                    acts
                }
                Phase::Aborting { .. } => {
                    self.handoff.as_mut().unwrap().old_dead = true;
                    if h.new.is_none() && h.new_tag.is_none() { self.finish_abort() } else { vec![] }
                }
                _ => {
                    // B is spawned: if it becomes ready, commit anyway (A had flushed).
                    self.handoff.as_mut().unwrap().old_dead = true;
                    vec![]
                }
            },
            (_, Ev::Busy { inst, busy }) if inst == h.old => {
                self.busy = busy;
                vec![]
            }
            (_, Ev::HeartbeatStale { .. }) => vec![], // not evaluated during a handoff (§3.5)
            (_, Ev::Flood { inst }) if h.new.as_deref() == Some(inst.as_str()) => {
                self.abort(UpdateFailedStage::Handoff, "candidate flooded stdout", None, now)
            }
            (_, Ev::OwnerStop) | (_, Ev::OwnerLine) | (_, Ev::OwnerResume) => vec![], // queued in the inbox
            (_, Ev::StartHandoff(_)) => vec![Act::HandoffDone { ok: false, reason: Some("a handoff is already running".into()) }],
            _ => vec![],
        }
    }

    fn set_phase(&mut self, p: Phase) {
        if let Some(h) = self.handoff.as_mut() {
            h.phase = p;
        }
    }

    fn handoff_tick(&mut self, now: Now) -> Vec<Act> {
        let Some(h) = self.handoff.clone() else { return vec![] };
        match h.phase {
            Phase::Pausing { deadline } if now.mono >= deadline => self.abort(UpdateFailedStage::Pause, "pause_timeout", None, now),
            Phase::Fencing { deadline } => {
                if now.mono >= deadline {
                    self.abort(UpdateFailedStage::Fence, "fence: the paused body still holds the log lock", None, now)
                } else {
                    vec![Act::FenceTry]
                }
            }
            Phase::AwaitReady { deadline } if now.mono >= deadline => {
                self.abort(UpdateFailedStage::Handoff, &format!("no ready within {} s", self.cfg.ready_timeout_s), None, now)
            }
            Phase::Settling { until } if now.mono >= until => self.commit(now),
            Phase::Retiring { deadline, termed } if now.mono >= deadline => {
                if !termed {
                    self.set_phase(Phase::Retiring { deadline: now.mono + Duration::from_secs(5), termed: true });
                    vec![self.kill(&h.old, KillMode::Term, now)]
                } else {
                    // Swept by the Root; the old instance's `exited` completes the handoff.
                    let a = h.old.clone();
                    self.handoff = None;
                    self.slot = Slot::Running;
                    vec![self.kill(&a, KillMode::Kill, now), Act::HandoffDone { ok: true, reason: None }, Act::Persist]
                }
            }
            _ => vec![],
        }
    }

    fn commit(&mut self, now: Now) -> Vec<Act> {
        let h = self.handoff.clone().unwrap();
        let b = h.new.clone().unwrap();
        let mut acts = Vec::new();
        match &h.kind {
            HandoffKind::Promote { job, sha, staged_sha256, .. } => {
                acts.push(Act::Commit { job: job.clone(), from_sha: h.old_sha.clone(), to_sha: sha.clone(), staged_sha256: staged_sha256.clone() });
                let depth = match &self.mode {
                    Mode::Provisional(p) => p.depth + 1,
                    _ => 1,
                };
                self.prev = Some(self.current.clone());
                self.current = Ver { sha: sha.clone(), provisional: true };
                self.mode = Mode::Provisional(Prov { sha: sha.clone(), depth, since_ms: now.wall_ms, probation_until: Some(now.mono + self.cfg.probation()) });
                acts.push(Act::Notice { kind: "provisional", data: json!({"sha": sha, "depth": depth}) });
            }
            HandoffKind::Rollback { reason, to: None } => {
                acts.push(Act::CommitRollback { from_sha: h.old_sha.clone(), to_sha: self.lkg_sha.clone(), reason: reason.clone(), data_only: false });
                self.prev = Some(self.current.clone());
                self.current = Ver { sha: self.lkg_sha.clone(), provisional: false };
                self.mode = Mode::Normal;
                self.counters.rollbacks += 1;
            }
            HandoffKind::Rollback { reason, to: Some(t) } => {
                // ADR-013 G2: the target is an open (provisional) version on the running binary.
                acts.push(Act::CommitRollback { from_sha: h.old_sha.clone(), to_sha: t.clone(), reason: reason.clone(), data_only: true });
                self.prev = Some(self.current.clone());
                self.current = Ver { sha: t.clone(), provisional: true };
                let (depth, since_ms) = match &self.mode {
                    Mode::Provisional(p) => (p.depth.saturating_sub(1).max(1), p.since_ms),
                    _ => (1, now.wall_ms),
                };
                if !self.mode.is_safe() {
                    self.mode = Mode::Provisional(Prov { sha: t.clone(), depth, since_ms, probation_until: None });
                }
                self.counters.rollbacks += 1;
            }
        }
        self.counters.swaps_ok += 1;
        self.primary = Some(Inst { id: b.clone(), sha: self.current.sha.clone(), ready_at: Some(now.mono) });
        self.busy = false;
        if h.old_dead {
            self.handoff = None;
            self.slot = Slot::Running;
            acts.push(Act::Redeliver { inst: b });
            acts.push(Act::HandoffDone { ok: true, reason: None });
            acts.push(Act::Persist);
            return acts;
        }
        self.set_phase(Phase::Retiring { deadline: now.mono + self.cfg.exit_grace(), termed: false });
        self.zombies.insert(h.old.clone(), now.mono);
        crate::fault::point("supervise.retire.before_exit_frame");
        acts.push(Act::Send { inst: h.old.clone(), frame: ToBody::Exit { token: h.token.clone(), code: init_proto::stdio::HANDOFF_EXIT_CODE } });
        acts.push(Act::Redeliver { inst: b });
        acts.push(Act::Persist);
        acts
    }

    // ------------------------------------------------------------------------------------
    // Root `stop` and `prepare_swap` (§3.8.3)
    // ------------------------------------------------------------------------------------

    fn live_bodies(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        if let Some(p) = &self.primary {
            v.push(p.id.clone());
        }
        if let Some(h) = &self.handoff {
            if !v.contains(&h.old) {
                v.push(h.old.clone());
            }
            if let Some(b) = &h.new {
                if !v.contains(b) {
                    v.push(b.clone());
                }
            }
        }
        if let Slot::Starting { inst: Some(i), .. } = &self.slot {
            if !v.contains(i) {
                v.push(i.clone());
            }
        }
        v
    }

    fn begin_stop(&mut self, req: String, exit_code: i32, grace: Duration, now: Now) -> Vec<Act> {
        let bodies = self.live_bodies();
        let mut acts = vec![Act::FenceRelease];
        self.handoff = None;
        self.deferred = None;
        for b in &bodies {
            acts.push(Act::Send { inst: b.clone(), frame: ToBody::Stop { reason: "root_stop".into(), grace_ms: grace.as_millis() as u64 } });
        }
        self.slot = Slot::Stopping { req, exit_code, phase: StopPhase::Stopping, deadline: now.mono + grace, token: None };
        for b in bodies {
            self.zombies.insert(b, now.mono);
        }
        self.primary = None;
        acts.push(Act::Persist);
        acts.extend(self.stopping(Ev::Tick, now));
        acts
    }

    fn begin_swap(&mut self, req: String, deadline: Duration, now: Now) -> Vec<Act> {
        if self.handoff.is_some() {
            return vec![Act::Respond { req, result: Err(RootErrorCode::Busy.error("a body handoff is in progress")) }];
        }
        let Some(p) = self.primary.clone().filter(|p| p.ready_at.is_some() && self.slot == Slot::Running) else {
            // No live body: nothing to pause.
            return self.begin_stop(req, 75, Duration::ZERO, now);
        };
        let token = crate::sys::random_hex(8);
        self.slot = Slot::Stopping { req, exit_code: 75, phase: StopPhase::Pausing, deadline: now.mono + deadline, token: Some(token.clone()) };
        vec![Act::Send { inst: p.id, frame: ToBody::Pause { token, reason: PauseReason::Swap } }, Act::Persist]
    }

    fn stopping(&mut self, ev: Ev, now: Now) -> Vec<Act> {
        let Slot::Stopping { req, exit_code, phase, deadline, token } = self.slot.clone() else { return vec![] };
        match (phase, ev) {
            (StopPhase::Pausing, Ev::Paused { inst, token: t }) if token.as_deref() == Some(t.as_str()) => {
                crate::fault::point("supervise.stop.after_pause");
                self.zombies.insert(inst.clone(), now.mono);
                self.primary = None;
                self.slot = Slot::Stopping { req, exit_code, phase: StopPhase::Stopping, deadline: now.mono + Duration::from_secs(10), token };
                vec![Act::Send { inst, frame: ToBody::Stop { reason: "swap".into(), grace_ms: 10_000 } }]
            }
            (StopPhase::Pausing, Ev::Tick) if now.mono >= deadline => {
                // F31: a body did not pause in time → `root.busy`, resume it; the swap is a non-event.
                self.slot = Slot::Running;
                let mut acts = vec![Act::Respond { req, result: Err(RootErrorCode::Busy.error("a body did not pause before the deadline")) }];
                if let (Some(p), Some(t)) = (self.primary.clone(), token) {
                    acts.push(Act::Send { inst: p.id, frame: ToBody::Resume { token: t, update_failed: None } });
                }
                acts.push(Act::Persist);
                acts
            }
            (StopPhase::Pausing, Ev::Exited { inst, .. }) if self.primary_is(&inst) => {
                self.primary = None;
                self.done_stopping(req, exit_code)
            }
            (StopPhase::Stopping, Ev::Tick) | (StopPhase::Killing, Ev::Tick) | (StopPhase::Stopping, Ev::Exited { .. }) | (StopPhase::Killing, Ev::Exited { .. }) => {
                if self.zombies.is_empty() {
                    return self.done_stopping(req, exit_code);
                }
                if now.mono >= deadline {
                    if self_phase_is_killing(&self.slot) {
                        // Killed and still no `exited` after 5 s: answer anyway; the Root sweeps.
                        self.zombies.clear();
                        return self.done_stopping(req, exit_code);
                    }
                    let mut acts = Vec::new();
                    let z: Vec<String> = self.zombies.keys().cloned().collect();
                    for i in z {
                        acts.push(Act::Kill { inst: i, mode: KillMode::Kill });
                    }
                    self.slot = Slot::Stopping { req, exit_code, phase: StopPhase::Killing, deadline: now.mono + Duration::from_secs(5), token };
                    return acts;
                }
                vec![]
            }
            _ => vec![],
        }
    }

    fn done_stopping(&mut self, req: String, exit_code: i32) -> Vec<Act> {
        self.slot = Slot::Stopping { req: String::new(), exit_code, phase: StopPhase::Killing, deadline: Instant::now() + Duration::from_secs(3600), token: None };
        if req.is_empty() {
            return vec![];
        }
        vec![Act::Persist, Act::Respond { req, result: Ok(json!({})) }, Act::Exit(exit_code)]
    }

    /// `hb` and `owner.out{state}` fields.
    pub fn hb(&self) -> (String, String, bool, String) {
        let slot = serde_json::to_value(self.slot.name()).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let mode = serde_json::to_value(self.mode.name()).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let sha12: String = self.current.sha.chars().take(12).collect();
        (slot, mode, self.busy, sha12)
    }
}

fn self_phase_is_killing(s: &Slot) -> bool {
    matches!(s, Slot::Stopping { phase: StopPhase::Killing, .. })
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;
