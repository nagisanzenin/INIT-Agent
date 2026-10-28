//! The supervisor driver (supervise.md §3.2–§3.10, §3.13): it runs the cold start, owns the
//! [`Machine`], executes its actions, and turns the world back into events.
//!
//! Threads (§3.2): this one (single consumer of the event channel), `root-link` (fd 3/4),
//! `hb` (the `hb` event every `hb_interval_s`, independent of slow work here), `outbox`, and
//! three per body (stdin, stdout, stderr). Spawns and kills run on short-lived threads so a
//! slow Root never blocks the relay.

use crate::bodyio::{self, BodyIo, BodyMsg};
use crate::config::{self, KernelCfg};
use crate::fsx;
use crate::inbox::{Inbox, Offer};
use crate::instance::{Spawned, SpawnError, SpawnRequest, Spawner};
use crate::link::{Inbound, RootLink};
use crate::machine::{Act, Ev, HaltReason, Machine, Mode, Now, Prov, SafeInfo, Slot, SpawnPlan, Tag, Ver};
use crate::outbox::Outbox;
use crate::spawnspec::{self, BodyLane};
use init_proto::envelope::ErrorBody;
use init_proto::errors::ErrorCode;
use init_proto::files::{self as pf, BootFile, BootReason, InitMode, LkgFile, SupervisorFile, UpdateFailedFile, UpdateFailedStage};
use init_proto::icj::V1;
use init_proto::initctl::Sweep;
use init_proto::root_link::{
    AlertArgs, AlertLevel, BootBlock, ExitedData, HbData, HelloArgs, InstanceInfo, InstanceKind, KernelEvent, KernelRequest, KillMode,
    OwnerInFrame, OwnerOutData, OwnerOutFrame, RootEvent, RootRequest, ZoneBootReason, ZoneMode,
};
use init_proto::initctl::ReloadOutcome;
use init_proto::stdio::{Display, DisplayKind, FromBody, InputMode, ReloadKind, ToBody};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The kernel version (`hello.kernel_version`, `INIT_KERNEL_VERSION`).
pub const KERNEL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// How long a replayed early exit waits for the dead body's stderr to reach EOF (K-RACE).
const EARLY_EXIT_TAIL_WAIT: Duration = Duration::from_millis(500);

/// Commands from inside the kernel (the T2 pipeline, initctl through WP7's serve, tests).
#[derive(Debug)]
pub enum Cmd {
    /// G0–G3 passed: hand off to `bin/init-body.staged` (§3.6.4). The reply says how it ended.
    Handoff { kind: crate::machine::HandoffKind, reply: Option<Sender<Result<(), String>>> },
    /// `initctl rollback` (immediate) or a G4 failure (at the next idle point).
    Rollback { reason: String, immediate: bool, reply: Option<Sender<Result<(), String>>> },
    /// ADR-013 G2: roll back to the admitted version `to` (the parent of a change group). A data-only
    /// rollback when every change after `to` is t0, else to LKG when `to` is LKG; refused otherwise.
    RollbackTo { reason: String, to: String, immediate: bool, reply: Option<Sender<Result<(), String>>> },
    /// G4 passed for the current provisional SHA (gate.md): the LKG commit of §3.6.6.
    G4Pass { sha: String, job: String, reply: Option<Sender<Result<(), String>>> },
    /// G4 failed (or `inconclusive_final`): roll back to LKG at the next idle point (§3.6.7).
    G4Fail { sha: String, job: String, reason: String },
    /// Read-only snapshot for `status` and tests.
    Snapshot { reply: Sender<Snapshot> },
    /// What the promote pipeline borrows (WP8b): the Root broker, the outbox, the config.
    Services { reply: Sender<Services> },
    /// As `G4Fail`, for a gate job that reports its own full `gate` record (WP8b: one record
    /// per gate, gate.md §3.13).
    G4Reject { sha: String, job: String, reason: String },
    /// A kernel notice to the body (gate stage events, gate.md §3.13 "Events to the body").
    /// `owner`: extra fields for the owner's copy only (TUI-OBSERVE; null for none).
    Notice { kind: String, data: Value, owner: Value },
    /// TUI-OBSERVE: an event for the owner channel only (`gate_start`); nothing goes to the body.
    OwnerEvent { kind: String, data: Value },
    /// `initctl reload` (initctl.md §4.1): send the primary `reload{job, kind}`; the reply carries
    /// its `reload_done` (`Err` when no body takes the frame).
    Reload { job: String, kind: ReloadKind, components: Option<Vec<String>>, reply: Sender<Result<ReloadOutcome, String>> },
    /// A promotion without a handoff (§3.6.8, T0/T1 data): `main` moves, no body restarts.
    DataOnly { p: DataOnly, reply: Option<Sender<Result<(), String>>> },
}

/// A data-only promotion (supervise.md §3.6.8): the gate passed G0–G1 (t0) or G0–G3 (t1) for
/// `sha`, whose binary is functionally the running one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataOnly {
    pub job: String,
    pub sha: String,
    /// The running version the candidate was gated against (`main` must still be here).
    pub from: String,
    /// `t0`, `t0_escalated` or `t1`.
    pub scope: String,
    /// No G4 is (still) needed: commit LKG at once (`lkg.json.reason = "data_only"`). Otherwise
    /// the version becomes provisional and an asynchronous G4 follows.
    pub lkg: bool,
    /// The kept-component set the gate built (`bin/ext.d/<set>`, already published), if the
    /// candidate changes kept components.
    pub ext: Option<crate::extset::ExtSet>,
}

/// The supervisor's services for in-process jobs (the promote pipeline, WP8b).
#[derive(Clone)]
pub struct Services {
    pub home: PathBuf,
    pub cfg: KernelCfg,
    /// The Root's spawn broker (the control link).
    pub spawner: Arc<dyn Spawner>,
    pub outbox: Arc<Outbox>,
    pub level: u8,
    /// The control link itself (initctl's `level`, `kernel propose`; WP7b).
    pub link: Arc<RootLink>,
    /// The Root's boot block from `hello`.
    pub boot: BootBlock,
    /// ADR-011: the Root's charter `extend.mode` is `"free"` (`hello`'s `extend`, then `ceilings`).
    pub extend_free: Arc<std::sync::atomic::AtomicBool>,
}

/// A view of the supervisor for `status` and tests.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub slot: crate::machine::Slot,
    pub mode: Mode,
    pub current: Ver,
    pub lkg_sha: String,
    pub primary: Option<String>,
    pub counters: init_proto::initctl::Counters,
    pub busy: bool,
    pub level: u8,
    pub outbox_unanswered: usize,
    /// The control link to the Root is up.
    pub link_up: bool,
    /// The Root outbox is backed up (§3.13.3).
    pub outbox_backlog: bool,
    /// The Root's last `ledger.get` and `ceilings.get` results (or `ceilings` events) and the age
    /// of the ledger copy in seconds.
    pub ledger: Option<Value>,
    pub ceilings: Option<Value>,
    pub ledger_age_s: Option<u64>,
    /// Free bytes on the `INIT_HOME` filesystem (statvfs, refreshed every 5 s) and the build
    /// threshold of `[supervise.disk]`.
    pub free_bytes: u64,
    pub min_free_build_bytes: u64,
    /// The live lane's body instances: the primary and, during a handoff, the other side.
    pub instances: Vec<String>,
    /// The kernel's start time (RFC 3339) and the Root's boot reason and mode.
    pub kernel_started: String,
    pub boot_reason: String,
    pub system_safe: bool,
    /// The handoff in progress: `{phase, from, to, kind}`.
    pub handoff: Option<Value>,
    pub charter_sha256: String,
    /// ADR-011: charter `extend.mode = "free"`.
    pub extend_free: bool,
}

/// A handle that injects commands into a running supervisor.
#[derive(Clone)]
pub struct Handle {
    tx: Sender<DEv>,
    published: Arc<Mutex<Option<Snapshot>>>,
    home: Arc<PathBuf>,
}

impl Handle {
    /// The supervisor's `INIT_HOME` (as given, not canonicalized).
    pub fn home(&self) -> &Path {
        &self.home
    }
    pub fn send(&self, c: Cmd) -> bool {
        self.tx.send(DEv::Cmd(c)).is_ok()
    }
    /// The snapshot the supervisor last published (every loop turn, at most 100 ms old), read
    /// from memory without a round trip: initctl's per-request state view (WP7b).
    pub fn latest(&self) -> Option<Snapshot> {
        self.published.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    /// True while the supervisor's loop can still receive commands.
    pub fn alive(&self) -> bool {
        let (tx, _rx) = mpsc::channel();
        self.send(Cmd::Snapshot { reply: tx })
    }
    /// The services (None once the supervisor has exited).
    pub fn services(&self) -> Option<Services> {
        let (tx, rx) = mpsc::channel();
        self.send(Cmd::Services { reply: tx });
        rx.recv_timeout(Duration::from_secs(10)).ok()
    }
    /// Snapshot (None once the supervisor has exited).
    pub fn snapshot(&self) -> Option<Snapshot> {
        let (tx, rx) = mpsc::channel();
        self.send(Cmd::Snapshot { reply: tx });
        rx.recv_timeout(Duration::from_secs(5)).ok()
    }
}

/// Driver events.
enum DEv {
    Link(Inbound),
    Body { inst: String, msg: BodyMsg },
    Spawned { tag: Tag, boot: PendingBoot, result: Result<Spawned, SpawnError> },
    Cmd(Cmd),
}

/// A spawn in flight.
#[derive(Clone, Debug)]
struct PendingBoot {
    boot_id: String,
    boot_seq: u64,
    sha: String,
    reason: String,
    started: Instant,
}

/// A live body instance.
struct BodyRec {
    io: BodyIo,
    boot_id: String,
    boot_seq: u64,
    ready_at: Option<Instant>,
    pid: i32,
    started_at: String,
    stdout_eof: bool,
    raw_lines: u64,
}

/// `init-kernel supervise --under-root`: refuses to start without the flag, `INIT_HOME` and
/// fds 3 and 4 (SV-33). Returns the process exit code (§3.13.5).
pub fn main_under_root(args: Vec<String>) -> i32 {
    main_under_root_with(args, Arc::new(crate::proxyctl::NoProxy))
}

/// [`main_under_root`] with the kernel's proxy (the `init-kernel` binary passes `kernel-proxy`).
pub fn main_under_root_with(args: Vec<String>, proxy: Arc<dyn crate::proxyctl::ProxyCtl>) -> i32 {
    main_under_root_started(args, proxy, |_| {})
}

/// [`main_under_root_with`] plus `on_start`, which receives the supervisor's [`Handle`] once
/// this process holds `run/kernel.lock` and before the rest of the cold start (`init-kernel`
/// binds `run/kernel.sock` and starts the initctl server from it; WP7b, FIX6).
pub fn main_under_root_started(args: Vec<String>, proxy: Arc<dyn crate::proxyctl::ProxyCtl>, on_start: impl FnOnce(Handle)) -> i32 {
    if !args.iter().any(|a| a == "--under-root") {
        eprintln!("init-kernel supervise: only the Root starts the kernel (`supervise --under-root`, fds 3 and 4)");
        return 64;
    }
    let ctl_fd = std::env::var("INIT_ROOT_FD").ok().and_then(|v| v.parse::<i32>().ok()).unwrap_or(-1);
    let pass_fd = std::env::var("INIT_ROOT_FDPASS").ok().and_then(|v| v.parse::<i32>().ok()).unwrap_or(-1);
    if ctl_fd != 3 || pass_fd != 4 || !crate::sys::fd_is_open(3) || !crate::sys::fd_is_open(4) {
        eprintln!("init-kernel supervise: control link fds 3 and 4 missing (INIT_ROOT_FD=3, INIT_ROOT_FDPASS=4)");
        return 64;
    }
    let Some(home) = std::env::var_os("INIT_HOME").map(PathBuf::from) else {
        eprintln!("init-kernel supervise: INIT_HOME is not set");
        return 64;
    };
    // §3.9 step 1: no child (git, tar, cargo) may inherit the control link.
    let _ = crate::sys::set_cloexec(3);
    let _ = crate::sys::set_cloexec(4);
    let (Ok(ctl), Ok(pass)) = (crate::sys::own_inherited_fd(3), crate::sys::own_inherited_fd(4)) else {
        return 64;
    };
    run_with_proxy(&home, ctl, pass, proxy, on_start)
}

/// Run the supervisor on an `INIT_HOME` with the two control-link fds. `on_start` receives a
/// [`Handle`] (tests, and serve's command path) right after §3.9 step 1. Returns the kernel exit
/// code.
pub fn run(home: &Path, ctl: OwnedFd, fdpass: OwnedFd, on_start: impl FnOnce(Handle)) -> i32 {
    run_with_proxy(home, ctl, fdpass, Arc::new(crate::proxyctl::NoProxy), on_start)
}

/// [`run`] with a proxy.
pub fn run_with_proxy(home: &Path, ctl: OwnedFd, fdpass: OwnedFd, proxy: Arc<dyn crate::proxyctl::ProxyCtl>, on_start: impl FnOnce(Handle)) -> i32 {
    // §3.9 step 1 (FD_CLOEXEC is the caller's): the single-instance lock comes before `on_start`,
    // so whatever it binds (initctl's `run/kernel.sock`, FIX6) belongs to the only kernel. Everything
    // slow (hello, restores, a possible LKG rebuild) comes after it.
    let lock = match take_lock(home) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let (tx, rx) = mpsc::channel::<DEv>();
    let published = Arc::new(Mutex::new(None));
    on_start(Handle { tx: tx.clone(), published: published.clone(), home: Arc::new(home.to_path_buf()) });
    let (ltx, lrx) = mpsc::channel::<Inbound>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(i) = lrx.recv() {
                if tx.send(DEv::Link(i)).is_err() {
                    break;
                }
            }
        });
    }
    let link = match RootLink::start(ctl, fdpass, ltx, Duration::from_millis(2000)) {
        Ok(l) => l,
        Err(e) => {
            crate::klog::log("supervise", &format!("control link: {e}"));
            return 69;
        }
    };
    match Supervisor::cold_start(home, lock, link, tx, rx, proxy, published) {
        Ok(mut s) => s.run_loop(),
        Err(code) => code,
    }
}

/// §3.9 step 1: take `run/kernel.lock` (single instance) and write `run/kernel.pid`. `Err(1)`
/// when the lock cannot be opened or another kernel holds it.
fn take_lock(home: &Path) -> Result<File, i32> {
    let _ = std::fs::create_dir_all(home.join("run"));
    let lock = match std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(home.join("run/kernel.lock")) {
        Ok(f) => f,
        Err(e) => {
            crate::klog::log("supervise", &format!("run/kernel.lock: {e}"));
            return Err(1);
        }
    };
    if lock.try_lock().is_err() {
        eprintln!("kernel already running (pid in run/kernel.pid)");
        return Err(1);
    }
    let _ = fsx::write_atomic(&home.join("run/kernel.pid"), format!("{}\n", std::process::id()).as_bytes());
    Ok(lock)
}

/// The shared `hb` snapshot.
#[derive(Clone, Default)]
struct HbShared {
    data: Option<HbData>,
    stop: bool,
}

/// The supervisor.
pub struct Supervisor {
    home: PathBuf,
    cfg: KernelCfg,
    link: Arc<RootLink>,
    spawner: Arc<dyn Spawner>,
    outbox: Arc<Outbox>,
    inbox: Inbox,
    /// STEER: the inputs most recently acked by the body, and retracted (the `state` frame's
    /// `steer_seen`/`steer_retracted`, so the owner sees a steer picked up or taken back).
    steer_seen: std::collections::VecDeque<u64>,
    steer_retracted: std::collections::VecDeque<u64>,
    m: Machine,
    tx: Sender<DEv>,
    rx: Receiver<DEv>,
    boot: BootBlock,
    /// ADR-011 (X2): the Root's `extend.mode` is `"free"`; shared with the promote pipeline.
    extend_free: Arc<std::sync::atomic::AtomicBool>,
    bodies: HashMap<String, BodyRec>,
    boot_seq: u64,
    fence: Option<File>,
    hb: Arc<Mutex<HbShared>>,
    last_hb_check: Instant,
    last_disk_check: Instant,
    last_ledger: Instant,
    last_outbox_retry: Instant,
    ledger: Option<Value>,
    ceilings: Option<Value>,
    update_failed: Option<UpdateFailedFile>,
    update_failed_delivered: bool,
    handoff_reply: Option<Sender<Result<(), String>>>,
    lineage: Box<dyn crate::lineage::LineageRefs>,
    kernel_started: String,
    level: u8,
    alerts_this_hour: (Instant, u32),
    pending_inst: HashMap<Tag, PendingBoot>,
    /// `exited` events for instances not known yet, held while a spawn is in flight (K-RACE): a
    /// body that dies at once can be reported before its spawn answer gets here. The answer
    /// comes through the spawn thread and the event through the link reader, and the Root's
    /// reaper does not wait for its own spawn reply either. Replayed when that spawn lands;
    /// the rest are handled as before once no spawn is in flight.
    early_exits: Vec<ExitedData>,
    exit_code: Option<i32>,
    /// Root request ID and upgrade job associated with a pending state handoff.
    kstate_swap: Option<(String, String)>,
    staged_sha256: Option<String>,
    /// TUI-OBSERVE: when the current promote handoff began, for `promoted.handoff_ms`.
    handoff_started: Option<Instant>,
    last_tail: Option<String>,
    proxy: Arc<dyn crate::proxyctl::ProxyCtl>,
    /// An LKG commit the Root refused (`lkg_store_failed`), retried every minute.
    pending_lkg: Option<(String, String)>,
    last_lkg_retry: Instant,
    /// The LKG commit's `lkg.json.reason` and scope for `pending_lkg` (`g4_pass`, or
    /// `data_only` for a T0 promotion whose store step was refused).
    pending_lkg_reason: (pf::LkgReason, Option<String>),
    /// The source SHA `bin/init-body` was built from when it differs from `current` (after a
    /// data-only promotion, §3.6.8: `binaries.current.built_from` stays X).
    built_from: Option<String>,
    /// `initctl reload` jobs waiting for the body's `reload_done` (WP7b).
    reloads: HashMap<String, Sender<Result<ReloadOutcome, String>>>,
    /// The last `ledger.get` answer (for its age) and the last statvfs.
    ledger_at: Option<Instant>,
    free_bytes: (u64, Instant),
    published: Arc<Mutex<Option<Snapshot>>>,
    /// `run/kernel.lock`, held while this supervisor runs (§3.2 single instance).
    _lock: File,
    /// Paces `owner.out` under the Root's frame limit (K-LINE).
    pacer: Mutex<crate::ownerline::Pacer>,
    /// ADR-009: the owner's workspace and the zone's access as the Root last stated them (the
    /// `workspace` event, else the kernel's own `INIT_WORKSPACE`/`INIT_ACCESS`), and what the
    /// last live body spawn got.
    access: Option<spawnspec::Access>,
    spawned_access: Option<spawnspec::Access>,
}

fn rel(home: &Path, p: &str) -> PathBuf {
    home.join(p)
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// ADR-011: `{mode: "free"}` (the boot block's or the `ceilings` block's `extend`) → free; anything
/// else, absent included, is the gated path.
/// The owner's line for a kernel notice. F4 (demo 2026-10-25): a gate notice said only `[kernel] gate`, so the owner
/// never learned that G4 had rolled the change back, or why. It now names the stage, the verdict and the reason,
/// with what happens next for the common reasons.
pub fn notice_text(kind: &str, d: &Value) -> String {
    let sha12 = |k: &str| d[k].as_str().unwrap_or("").chars().take(12).collect::<String>();
    match kind {
        "gate" => {}
        // TUI-OBSERVE: the owner-only events.
        "gate_start" => {
            let steps: Vec<&str> = d["steps"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let scope = d["scope"].as_str().map(|s| format!(" ({s})")).unwrap_or_default();
            return format!("[kernel] gate started for {}: {}{scope}", sha12("sha"), steps.join(" "));
        }
        "rolled_back" => {
            let to = if d["to_lkg"] == true { " (LKG)" } else { "" };
            return format!("[kernel] rolled back {} → {}{to}: {}", sha12("from_sha"), sha12("to_sha"), d["reason"].as_str().unwrap_or("?"));
        }
        _ => return format!("[kernel] {kind}"),
    }
    let stage = d["stage"].as_str().unwrap_or("?");
    let verdict = match d["pass"].as_bool() {
        Some(true) => "pass",
        Some(false) => "fail",
        None => "inconclusive",
    };
    let reason = d["reason"].as_str().unwrap_or("");
    let why = match reason.rsplit('.').next().unwrap_or("") {
        "no_instrument" => " (G4 has no tasks: no sealed S drawn, empty pool and holdout; kept on G0-G3 alone)",
        "inconclusive_final" => " (still inconclusive after the escalated re-run: rolled back to LKG)",
        "insufficient" | "underpowered" | "underpowered:holdout" => " (too few tasks to decide: one escalated re-run follows)",
        "score_inferior" | "score_inferior_early" | "pool_inferior" | "holdout_inferior" => " (worse than LKG on the frozen tasks: rolled back to LKG)",
        "canary" => " (a canary scored perfectly: cheating suspected, quarantined and rolled back)",
        "no_gain" => " (not better, cheaper or smaller than LKG: rolled back to LKG)",
        "parent_probe" => " (the running version's probe failed: infra, not the candidate)",
        _ => "",
    };
    let sha: String = d["sha"].as_str().unwrap_or("").chars().take(12).collect();
    match (reason.is_empty(), sha.is_empty()) {
        (true, _) => format!("[kernel] gate {stage} {verdict}"),
        (false, true) => format!("[kernel] gate {stage} {verdict}: {reason}{why}"),
        (false, false) => format!("[kernel] gate {stage} {verdict} for {sha}: {reason}{why}"),
    }
}

/// TUI-OBSERVE: the steps of a gated promote in order, for `gate_start.steps`: the scope's stages,
/// then `handoff` (a new body binary) or `live` (a data-only move), then `G4` when it runs after the
/// version goes live (a synchronous G4 is already among the stages, before the handoff).
pub fn gate_steps(s: &kernel_gate::scope::ScopeDecision) -> Vec<&'static str> {
    let mut steps = s.stages.to_vec();
    steps.push(if s.needs_handoff { "handoff" } else { "live" });
    if s.needs_g4 == kernel_gate::scope::G4Need::Async {
        steps.push("G4");
    }
    steps
}

/// TUI-OBSERVE (tui.md §3.10): the owner's copy of a kernel notice: its level, its text and its
/// structured `data`. `d` is what the body gets; `extra` adds owner-only fields (`gate_kind`,
/// `scope`, `handoff_ms`, …). A gate stage's data uses the record's names (`spent_musd`, `wall_ms`,
/// `cache_permille`). Levels: a failed stage and a rollback are `warn`, everything else `info`.
pub fn owner_notice_view(kind: &str, d: &Value, extra: &Value) -> (AlertLevel, String, Value) {
    let mut data = serde_json::Map::new();
    data.insert("kind".into(), json!(kind));
    if kind == "gate" {
        for k in ["gate_id", "sha", "stage", "pass", "reason", "score"] {
            data.insert(k.into(), d[k].clone());
        }
        data.insert("spent_musd".into(), d["cost"].clone());
        data.insert("wall_ms".into(), d["wall"].clone());
        data.insert("cache_permille".into(), d["cache_hit"].clone());
    } else if let Some(o) = d.as_object() {
        for (k, v) in o {
            data.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    if let Some(o) = extra.as_object() {
        for (k, v) in o {
            data.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    let level = match kind {
        "gate" if d["pass"] == false => AlertLevel::Warn,
        "rolled_back" => AlertLevel::Warn,
        _ => AlertLevel::Info,
    };
    (level, notice_text(kind, d), Value::Object(data))
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)] // F4's notice tests sit next to `notice_text`
mod notice_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn f4_gate_notices_say_why() {
        let d = json!({"stage": "G4", "pass": false, "reason": "inconclusive_final", "sha": "06be87a1234567890"});
        assert_eq!(
            notice_text("gate", &d),
            "[kernel] gate G4 fail for 06be87a12345: inconclusive_final (still inconclusive after the escalated re-run: rolled back to LKG)"
        );
        let t = notice_text("gate", &json!({"stage": "G4", "pass": true, "reason": "no_instrument"}));
        assert!(t.starts_with("[kernel] gate G4 pass: no_instrument (G4 has no tasks"), "{t}");
        assert_eq!(
            notice_text("gate", &json!({"stage": "G4", "pass": null, "reason": "gate.g4.insufficient"})),
            "[kernel] gate G4 inconclusive: gate.g4.insufficient (too few tasks to decide: one escalated re-run follows)"
        );
        assert_eq!(notice_text("gate", &json!({"stage": "G2", "pass": true})), "[kernel] gate G2 pass");
        assert_eq!(notice_text("promoted", &json!({})), "[kernel] promoted");
    }

    /// The body's stage notice as `kernel_gate::record::stage_notice` builds it.
    fn stage(stage: &str, pass: Option<bool>, reason: Option<&str>, score: Option<i64>) -> Value {
        kernel_gate::record::stage_notice("j-7", "06be87a1234567890abc", stage, pass, reason, score, 1200, 9100, Some(950))
    }

    /// TUI-OBSERVE: every gate outcome reaches the owner with its data and a level that matches it.
    #[test]
    fn owner_view_carries_gate_data_for_each_outcome() {
        let extra = json!({"gate_kind": "promote", "scope": "t2"});
        let (level, text, d) = owner_notice_view("gate", &stage("G0", Some(true), None, None), &extra);
        assert_eq!((level, text.as_str()), (AlertLevel::Info, "[kernel] gate G0 pass"));
        assert_eq!(d, json!({"kind": "gate", "gate_id": "j-7", "sha": "06be87a1234567890abc", "stage": "G0", "pass": true,
            "reason": null, "score": null, "spent_musd": 1200, "wall_ms": 9100, "cache_permille": 950, "gate_kind": "promote", "scope": "t2"}));
        assert!(init_proto::icj::to_line(&d).is_ok());
        // Refused before going live (G2), and G4's rollback: warn.
        let (level, text, d) = owner_notice_view("gate", &stage("G2", Some(false), Some("probe_exit"), None), &extra);
        assert_eq!((level, d["pass"].clone(), d["reason"].clone()), (AlertLevel::Warn, json!(false), json!("probe_exit")));
        assert!(text.starts_with("[kernel] gate G2 fail for 06be87a12345: probe_exit"), "{text}");
        let (level, _, d) = owner_notice_view("gate", &stage("G4", Some(false), Some("score_inferior"), Some(412)), &json!({"gate_kind": "g4"}));
        assert_eq!((level, d["score"].clone(), d["gate_kind"].clone()), (AlertLevel::Warn, json!(412), json!("g4")));
        // Inconclusive (a re-run follows) and pass: info.
        let (level, _, d) = owner_notice_view("gate", &stage("G4", None, Some("insufficient"), None), &Value::Null);
        assert_eq!((level, d["pass"].clone()), (AlertLevel::Info, Value::Null));
        assert!(d.get("gate_kind").is_none());
        let (level, _, _) = owner_notice_view("gate", &stage("G4", Some(true), None, Some(600)), &Value::Null);
        assert_eq!(level, AlertLevel::Info);
        // An extra field never overrides the body's own.
        let (_, _, d) = owner_notice_view("gate", &stage("G1", Some(true), None, None), &json!({"stage": "G9"}));
        assert_eq!(d["stage"], "G1");
    }

    #[test]
    fn gate_steps_per_scope() {
        use kernel_gate::scope::Scope;
        assert_eq!(gate_steps(&Scope::T2.decision()), ["G0", "G1", "G2", "G3", "handoff", "G4"]);
        assert_eq!(gate_steps(&Scope::T1.decision()), ["G0", "G1", "G2", "G3", "live", "G4"]);
        assert_eq!(gate_steps(&Scope::Dep.decision()), ["G0", "G1", "G2", "G3", "G4", "handoff"]);
        assert_eq!(gate_steps(&Scope::T0.decision()), ["G0", "G1", "live"]);
        assert_eq!(gate_steps(&Scope::T0Escalated.decision()), ["G0", "G1", "G4", "live"]);
    }

    #[test]
    fn owner_view_of_lifecycle_events() {
        let (level, text, d) = owner_notice_view("gate_start", &json!({"gate_id": "j-7", "sha": "06be87a1234567890abc",
            "scope": "t2", "steps": ["G0", "G1", "G2", "G3", "handoff", "G4"]}), &Value::Null);
        assert_eq!((level, text.as_str()), (AlertLevel::Info, "[kernel] gate started for 06be87a12345: G0 G1 G2 G3 handoff G4 (t2)"));
        assert_eq!(d["kind"], "gate_start");
        assert_eq!(d["steps"].as_array().map(Vec::len), Some(6));
        let (level, text, d) = owner_notice_view("promoted", &json!({"sha": "abc", "handoff": true}), &json!({"handoff_ms": 4600, "from_sha": "f"}));
        assert_eq!((level, text.as_str()), (AlertLevel::Info, "[kernel] promoted"));
        assert_eq!(d, json!({"kind": "promoted", "sha": "abc", "handoff": true, "handoff_ms": 4600, "from_sha": "f"}));
        let (level, text, d) = owner_notice_view("rolled_back", &json!({"from_sha": "06be87a1234567890abc", "to_sha": "3f9c0a1b2c4d5e6f",
            "reason": "gate.g4.score_inferior", "data_only": false, "to_lkg": true}), &Value::Null);
        assert_eq!(level, AlertLevel::Warn);
        assert_eq!(text, "[kernel] rolled back 06be87a12345 → 3f9c0a1b2c4d (LKG): gate.g4.score_inferior");
        assert_eq!(d["to_lkg"], true);
        let (level, text, d) = owner_notice_view("lkg", &json!({"sha": "abc"}), &Value::Null);
        assert_eq!((level, text.as_str(), d), (AlertLevel::Info, "[kernel] lkg", json!({"kind": "lkg", "sha": "abc"})));
    }
}

pub fn extend_mode_free(extend: Option<&Value>) -> bool {
    extend.and_then(|e| e["mode"].as_str()) == Some("free")
}

impl Supervisor {
    /// §3.9 cold start, steps 2–12 (step 1's `lock` is taken by [`take_lock`] before
    /// `on_start`). `Err(code)` exits the process with that code.
    fn cold_start(home: &Path, lock: File, link: Arc<RootLink>, tx: Sender<DEv>, rx: Receiver<DEv>, proxy: Arc<dyn crate::proxyctl::ProxyCtl>, published: Arc<Mutex<Option<Snapshot>>>) -> Result<Supervisor, i32> {
        let home = home.to_path_buf();

        // Step 4 first in practice: the handshake must happen within 30 s, and every later halt
        // still completes it so the Root and the owner can see why.
        let hello = KernelRequest::Hello(HelloArgs { kernel_version: KERNEL_VERSION.into(), proto: init_proto::ROOT_PROTO });
        let boot: BootBlock = match link.call(&hello, Duration::from_secs(init_proto::root_link::HELLO_DEADLINE_S)).map(serde_json::from_value) {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => {
                crate::klog::log("supervise", &format!("boot block: {e}"));
                return Err(69);
            }
            Err(e) => {
                crate::klog::log("supervise", &format!("hello: {e}"));
                return Err(69);
            }
        };
        if boot.boot.reason.as_str() == crate::kupgrade::SMOKE_REASON {
            // A conformance root contains preplaced binaries, not a live installation.
            // Do not start the proxy, restore tuples, or require lineage/git here.
            return Err(crate::kupgrade::smoke::run(&home, &boot, link, |wait| {
                match rx.recv_timeout(wait) {
                    Ok(DEv::Link(inbound)) => Some(inbound),
                    Err(mpsc::RecvTimeoutError::Disconnected) => Some(Inbound::Lost),
                    _ => None,
                }
            }));
        }
        // K8: state replacement/recovery precedes proxy, job store and supervisor state reads.
        let restore = crate::kupgrade::kstate::recover(&home).and_then(|_| {
            if let Some(hash) = boot.boot.kstate.as_deref() {
                crate::kupgrade::kstate::restore(&home, &link, hash)
            } else { Ok(()) }
        });
        if let Err(e) = restore {
            crate::klog::log("kstate", &format!("restore refused: {e}"));
            return Err(70);
        }
        let cfg = match config::load(&home.join("kernel/config.toml")) {
            Ok(c) => c,
            Err(errs) => {
                for e in &errs {
                    crate::klog::log("config", &e.to_string());
                }
                return Err(init_proto::root_link::kernel_exit::CONFIG); // 78 before ready
            }
        };
        let hb = Arc::new(Mutex::new(HbShared::default()));
        start_hb_thread(link.clone(), hb.clone(), Duration::from_secs(boot.limits.hb_interval_s.max(1)));

        // The proxy (§3.2): routing and soft budgets on run/proxy.sock, forwarding to the Root's
        // egress socket from the boot block.
        let pcfg = match crate::proxyctl::proxy_config(&home.join("kernel/config.toml")) {
            Ok(v) => v,
            Err(errs) => {
                errs.iter().for_each(|e| crate::klog::log("config", e));
                return Err(init_proto::root_link::kernel_exit::CONFIG);
            }
        };
        let plabel = crate::proxyctl::ProxyLabel { sha: boot.boot.tuple_body.sha.clone(), boot_reason: "cold".into(), model: cfg.model.id.clone(), level: boot.ladder.level };
        let sink_home = home.clone();
        let sink_link = link.clone();
        let sink: crate::proxyctl::ProxyAlertSink = Arc::new(move |code: &str, message: &str| {
            let line = json!({"v": 1, "ts": crate::clock::now_rfc3339(), "source": "proxy", "code": code, "message": message});
            let _ = fsx::append_line(&sink_home.join("state/kernel/alerts.jsonl"), &init_proto::icj::to_line(&line).unwrap_or_default());
            let _ = sink_link.send(&KernelRequest::Alert(AlertArgs { code: code.to_string(), level: AlertLevel::Warn, message: crate::fsx::tail_str(message, 500) }));
        });
        if let Err(errs) = proxy.start(&pcfg, &home, Path::new(&boot.paths.egress_sock), &plabel, sink) {
            errs.iter().for_each(|e| crate::klog::log("proxy", e));
            return Err(init_proto::root_link::kernel_exit::CONFIG);
        }
        proxy.set_level(boot.ladder.level);

        // Step 2: layout (K-F5) and INIT_HOME length.
        let mut halt: Option<HaltReason> = None;
        if let Err(e) = crate::layout::check(&home, cfg.test_mode) {
            crate::klog::log("layout", &e);
            halt = Some(HaltReason::Layout);
        }
        // Step 3: tool versions.
        if halt.is_none() {
            if let Err(e) = crate::git::version_ok(&cfg.supervise.git) {
                crate::klog::log("tooling", &e);
                halt = Some(HaltReason::Tooling);
            }
        }
        for d in ["state/kernel/logs", "vault", "cache/git-home", "state/run", "state/sessions", "work/tmp", "work/home", "work/cargo-home"] {
            let _ = std::fs::create_dir_all(home.join(d));
        }

        // Step 5: state files, the outbox, the Root's instances.
        let kboot = crate::sys::random_hex(8);
        let outbox = Outbox::open(&home, &kboot);
        outbox.attach(link.clone());
        let sup: Option<SupervisorFile> = std::fs::read(rel(&home, pf::paths::SUPERVISOR)).ok().and_then(|b| pf::decode(&b).ok());
        let lkg: Option<LkgFile> = std::fs::read(rel(&home, pf::paths::LKG)).ok().and_then(|b| pf::decode(&b).ok());
        if let Ok(v) = link.call(&KernelRequest::Instances, link.request_timeout) {
            if let Ok(list) = serde_json::from_value::<Vec<InstanceInfo>>(v) {
                for i in list.into_iter().filter(|i| i.kind != InstanceKind::Kernel) {
                    let _ = link.call(&KernelRequest::Kill(init_proto::root_link::KillArgs { instance: i.instance, mode: KillMode::Kill }), link.request_timeout);
                }
            }
        }
        crate::fault::point("supervise.cold.after_sweep");

        // Versions: lkg.json, else the Root's tuple.
        let tb = &boot.boot.tuple_body;
        let lkg_sha = lkg.as_ref().map(|l| l.sha.clone()).unwrap_or_else(|| tb.sha.clone());
        let mut current = sup.as_ref().map(|s| Ver { sha: s.current.sha.clone(), provisional: s.current.provisional }).unwrap_or(Ver { sha: lkg_sha.clone(), provisional: false });
        let lineage = crate::lineage::for_home(&home, &cfg);
        let bin = home.join("bin");
        let _ = std::fs::create_dir_all(&bin);
        let mut boot_reason = BootReason::Cold;
        let mut update_failed: Option<UpdateFailedFile> = std::fs::read(rel(&home, pf::paths::UPDATE_FAILED)).ok().and_then(|b| pf::decode(&b).ok());

        // Step 6: restore from the Root's tuple.
        let rollback_class = matches!(boot.boot.reason, ZoneBootReason::RollbackSystem | ZoneBootReason::Demotion | ZoneBootReason::PanicResume);
        if halt.is_none() {
            let r = crate::recovery::restore_from_tuple(&home, &link, tb, rollback_class);
            match r {
                Ok(done) => {
                    if rollback_class {
                        if current.sha != tb.sha {
                            let stage = if boot.boot.reason == ZoneBootReason::PanicResume { UpdateFailedStage::Panic } else { UpdateFailedStage::SystemRollback };
                            let uf = new_update_failed(update_failed.as_ref(), stage, "whole-system rollback dropped the provisional chain", &current.sha, Some(tb.sha.clone()));
                            let _ = fsx::write_json(&rel(&home, pf::paths::UPDATE_FAILED), &uf);
                            update_failed = Some(uf);
                            let _ = lineage.rollback_main(&current.sha, &tb.sha);
                        }
                        let _ = lineage.set_lkg(&tb.sha);
                        current = Ver { sha: tb.sha.clone(), provisional: false };
                        let l = LkgFile {
                            v: V1,
                            sha: tb.sha.clone(),
                            binary: "bin/init-body.lkg".into(),
                            binary_sha256: tb.binary.clone(),
                            tree: format!("lineage/trees/{}", tb.sha.chars().take(12).collect::<String>()),
                            reason: pf::LkgReason::SystemRollback,
                            lineage_seq: None,
                            seq: lkg.as_ref().map(|l| l.seq + 1).unwrap_or(1),
                            prev_sha: lkg.as_ref().map(|l| l.sha.clone()),
                            gate_job: None,
                            committed_at: crate::clock::now_rfc3339(),
                            model: None,
                            ext_set: tb.ext.set.clone(),
                        };
                        let _ = fsx::write_json(&rel(&home, pf::paths::LKG), &l);
                        boot_reason = BootReason::Rollback;
                    }
                    for d in done {
                        crate::klog::log("recovery", &d);
                    }
                }
                Err(e) => {
                    crate::klog::log("recovery", &e);
                }
            }
        }
        outbox.resend_unanswered();

        // Step 7: bin repair.
        if let Ok(Some(what)) = crate::binstore::repair(&bin) {
            crate::klog::log("recovery", what);
        }
        if halt.is_none() && !bin.join("init-body.lkg").exists() && !bin.join("init-body").exists() {
            halt = Some(HaltReason::NoLkg);
        }

        // Step 9: an interrupted handoff.
        if let Some(h) = sup.as_ref().and_then(|s| s.handoff.clone()) {
            if !rollback_class {
                match h.phase.as_str() {
                    "committing" => {
                        let staged_now = crate::binstore::sha_of(&bin, "init-body");
                        if staged_now.as_deref() == Some(h.staged_sha256.as_str()) || bin.join("init-body.staged").exists() && crate::binstore::sha_of(&bin, "init-body.staged").as_deref() == Some(h.staged_sha256.as_str()) {
                            if bin.join("init-body.staged").exists() {
                                let _ = crate::binstore::commit_bin(&bin);
                            }
                            let _ = lineage.move_main(&h.to, &h.from);
                            let _ = outbox.post("promotion", json!({"sha": h.to, "from_sha": h.from, "gate_job": Value::Null, "scope": Value::Null, "lkg": false, "body_binary": h.staged_sha256, "ext_set": Value::Null, "components": []}));
                            current = Ver { sha: h.to.clone(), provisional: true };
                        } else {
                            let uf = new_update_failed(update_failed.as_ref(), UpdateFailedStage::KernelRestart, "kernel restarted during the handoff commit", &h.to, Some(h.from.clone()));
                            let _ = fsx::write_json(&rel(&home, pf::paths::UPDATE_FAILED), &uf);
                            update_failed = Some(uf);
                        }
                    }
                    "retiring" => current = Ver { sha: h.to.clone(), provisional: true },
                    _ => {
                        let uf = new_update_failed(update_failed.as_ref(), UpdateFailedStage::KernelRestart, "kernel restarted during a handoff", &h.to, Some(h.from.clone()));
                        let _ = fsx::write_json(&rel(&home, pf::paths::UPDATE_FAILED), &uf);
                        update_failed = Some(uf);
                    }
                }
            }
        }

        // Step 10: mode.
        let was_safe = sup.as_ref().is_some_and(|s| s.mode == init_proto::initctl::Mode::Safe);
        let mode = if boot.boot.mode == ZoneMode::Safe {
            Mode::Safe(SafeInfo { entered_ms: crate::clock::now_ms(), reason: "system".into(), system: true, crashes: vec![] })
        } else if current.provisional && current.sha != lkg_sha {
            Mode::Provisional(Prov { sha: current.sha.clone(), depth: 1, since_ms: crate::clock::now_ms(), probation_until: None })
        } else {
            Mode::Normal
        };

        // Step 11: what to spawn. `bin/init-body` if it matches the recorded current binary.
        let recorded = sup.as_ref().and_then(|s| s.binaries.current.as_ref().map(|b| b.sha256.clone()));
        let actual = crate::binstore::sha_of(&bin, "init-body");
        let mut m = Machine::new(cfg.supervise.clone(), current.clone(), lkg_sha.clone(), mode);
        if let Some(s) = &sup {
            m.counters = s.counters.clone();
            m.prev = s.binaries.prev.as_ref().map(|p| Ver { sha: p.built_from.clone(), provisional: p.built_from != lkg_sha });
        }
        let mut alerts = Vec::new();
        if let (Some(r), Some(a)) = (&recorded, &actual) {
            if r != a && halt.is_none() {
                alerts.push(("binary_mismatch", "bin/init-body does not match supervisor.json; spawning LKG".to_string()));
                let _ = crate::binstore::install_from(&bin, "init-body.lkg");
                m.current = Ver { sha: lkg_sha.clone(), provisional: false };
                m.mode = if m.mode.is_safe() { m.mode.clone() } else { Mode::Normal };
            }
        }
        let level = boot.ladder.level;
        let current_sha_for_built = m.current.sha.clone();
        let spawner: Arc<dyn Spawner> = link.clone();
        let access0 = boot.access.as_ref().map(spawnspec::Access::from_data).or_else(spawnspec::Access::from_env);
        let mut s = Supervisor {
            inbox: Inbox::open(&rel(&home, "state/kernel/inbox.jsonl")),
            steer_seen: std::collections::VecDeque::new(),
            steer_retracted: std::collections::VecDeque::new(),
            home,
            cfg,
            link,
            spawner,
            outbox,
            m,
            tx,
            rx,
            extend_free: Arc::new(std::sync::atomic::AtomicBool::new(extend_mode_free(boot.extend.as_ref()))),
            boot,
            bodies: HashMap::new(),
            boot_seq: sup.as_ref().and_then(|s| s.body.as_ref().map(|b| b.boot_seq)).unwrap_or(0),
            fence: None,
            hb,
            last_hb_check: Instant::now(),
            last_disk_check: Instant::now(),
            last_ledger: Instant::now(),
            last_outbox_retry: Instant::now(),
            ledger: None,
            ceilings: None,
            update_failed,
            update_failed_delivered: false,
            handoff_reply: None,
            lineage,
            kernel_started: crate::clock::now_rfc3339(),
            level,
            alerts_this_hour: (Instant::now(), 0),
            pending_inst: HashMap::new(),
            early_exits: Vec::new(),
            exit_code: None,
            kstate_swap: None,
            staged_sha256: None,
            handoff_started: None,
            last_tail: None,
            pending_lkg: None,
            last_lkg_retry: Instant::now(),
            pending_lkg_reason: (pf::LkgReason::G4Pass, None),
            built_from: sup.as_ref().and_then(|s| s.binaries.current.as_ref().map(|b| b.built_from.clone())).filter(|b| !b.is_empty() && *b != current_sha_for_built),
            reloads: HashMap::new(),
            pacer: Mutex::new(crate::ownerline::Pacer::new(Instant::now())),
            ledger_at: None,
            free_bytes: (u64::MAX, Instant::now() - Duration::from_secs(60)),
            published,
            proxy,
            _lock: lock,
            access: access0,
            spawned_access: None,
        };
        s.refresh_ledger();
        for (c, msg) in alerts {
            s.alert(c, &msg);
        }
        if was_safe && s.boot.boot.mode == ZoneMode::Normal {
            // §3.9 step 10: normal mode after a body safe mode.
            let lkg = s.m.lkg_sha.clone();
            s.outbox_post("body_safe", json!({"on": false, "sha": lkg, "crashes": []}));
        }
        s.outbox_post("kernel_start", json!({"kboot": s.outbox.kboot(), "reason": s.boot.boot.reason.as_str(), "version": KERNEL_VERSION}));
        let acts = match halt {
            Some(h) => s.m.on(Ev::Halt(h), Now::real()),
            None => s.m.on(Ev::Start { reason: boot_reason }, Now::real()),
        };
        s.exec(acts);
        // Step 12.
        if let Err(e) = s.link.call(&KernelRequest::Ready, s.link.request_timeout) {
            crate::klog::log("supervise", &format!("ready: {e}"));
        }
        s.publish_hb();
        Ok(s)
    }

    fn run_loop(&mut self) -> i32 {
        loop {
            if let Some(code) = self.exit_code {
                self.shutdown_threads();
                return code;
            }
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(DEv::Link(Inbound::Lost)) => return self.link_lost(),
                Ok(DEv::Link(Inbound::Request { id, req })) => self.on_root_request(id, req),
                Ok(DEv::Link(Inbound::Event(e))) => self.on_root_event(e),
                Ok(DEv::Body { inst, msg }) => self.on_body(inst, msg),
                Ok(DEv::Spawned { tag, boot, result }) => self.on_spawned(tag, boot, result),
                Ok(DEv::Cmd(c)) => self.on_cmd(c),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return 69,
            }
            self.periodic();
        }
    }

    fn shutdown_threads(&mut self) {
        self.hb.lock().unwrap().stop = true;
        for (_, b) in self.bodies.drain() {
            b.io.close_stdin();
        }
    }

    // ------------------------------------------------------------------------------------
    // Periodic work: deadlines, readiness, heartbeats, disk, ledger
    // ------------------------------------------------------------------------------------

    fn periodic(&mut self) {
        let now = Now::real();
        // Readiness: 100 ms while a body is starting.
        let waiting: Vec<(String, String)> = self.bodies.iter().filter(|(_, b)| b.ready_at.is_none()).map(|(i, b)| (i.clone(), b.boot_id.clone())).collect();
        if !waiting.is_empty() {
            if let Some(r) = self.read_ready() {
                for (inst, boot_id) in waiting {
                    if r.boot_id == boot_id {
                        if let Some(b) = self.bodies.get_mut(&inst) {
                            b.ready_at = Some(Instant::now());
                        }
                        let acts = self.m.on(Ev::Ready { inst }, now);
                        self.exec(acts);
                    }
                }
            }
        }
        let acts = self.m.on(Ev::Tick, now);
        self.exec(acts);
        // Heartbeats: 1 s, only for the primary outside a handoff (§3.5).
        if self.last_hb_check.elapsed() >= Duration::from_secs(1) {
            self.last_hb_check = Instant::now();
            self.check_heartbeat(now);
        }
        if self.last_disk_check.elapsed() >= Duration::from_secs(if matches!(self.m.slot, Slot::DiskFull { .. }) { 1 } else { 60 }) {
            self.last_disk_check = Instant::now();
            if matches!(self.m.slot, Slot::DiskFull { .. }) && self.free_bytes() >= 2 * self.cfg.supervise.min_free_run_bytes {
                let acts = self.m.on(Ev::DiskRecovered, now);
                self.exec(acts);
            }
        }
        if self.last_ledger.elapsed() >= Duration::from_secs(self.cfg.supervise.ledger_refresh_s) {
            self.last_ledger = Instant::now();
            self.refresh_ledger();
        }
        if self.pending_lkg.is_some() && self.last_lkg_retry.elapsed() >= Duration::from_secs(60) {
            self.last_lkg_retry = Instant::now();
            let (sha, job) = self.pending_lkg.take().unwrap();
            if let Err(e) = self.lkg_commit(&sha, &job) {
                crate::klog::log("supervise", &format!("LKG commit retry: {e}"));
                self.pending_lkg = Some((sha, job));
            }
        }
        if self.last_outbox_retry.elapsed() >= Duration::from_secs(60) {
            self.last_outbox_retry = Instant::now();
            self.outbox.resend_unanswered();
        }
        if self.free_bytes.1.elapsed() >= Duration::from_secs(5) {
            self.free_bytes = (self.free_bytes(), Instant::now());
        }
        let due = self.pacer.lock().unwrap().tick(Instant::now());
        for frame in due {
            self.link.event(&KernelEvent::OwnerOut(OwnerOutData { frame }));
        }
        self.publish_hb();
        self.publish_snapshot();
    }

    /// The snapshot every reader shares ([`Handle::latest`]).
    fn snapshot(&self) -> Snapshot {
        let mut instances: Vec<String> = self.m.primary.iter().map(|p| p.id.clone()).collect();
        if let Some(h) = &self.m.handoff {
            for i in [Some(&h.old), h.new.as_ref()].into_iter().flatten() {
                if !instances.contains(i) {
                    instances.push(i.clone());
                }
            }
        }
        let handoff = self.m.handoff.as_ref().map(|h| {
            let (kind, to) = match &h.kind {
                crate::machine::HandoffKind::Promote { sha, .. } => ("promote", sha.clone()),
                crate::machine::HandoffKind::Rollback { to, .. } => ("rollback", self.m.rollback_target(to).1),
            };
            json!({"kind": kind, "phase": h.phase.name(), "from": h.old_sha, "to": to})
        });
        Snapshot {
            slot: self.m.slot.clone(),
            mode: self.m.mode.clone(),
            current: self.m.current.clone(),
            lkg_sha: self.m.lkg_sha.clone(),
            primary: self.m.primary.as_ref().map(|p| p.id.clone()),
            counters: self.m.counters.clone(),
            busy: self.m.busy,
            level: self.level,
            outbox_unanswered: self.outbox.unanswered(),
            link_up: self.link.is_alive(),
            outbox_backlog: self.outbox.backlogged(),
            ledger: self.ledger.clone(),
            ceilings: self.ceilings.clone(),
            ledger_age_s: self.ledger_at.map(|t| t.elapsed().as_secs()),
            free_bytes: self.free_bytes.0,
            min_free_build_bytes: self.cfg.supervise.min_free_build_bytes,
            instances,
            kernel_started: self.kernel_started.clone(),
            boot_reason: self.boot.boot.reason.as_str().to_string(),
            system_safe: self.boot.boot.mode == ZoneMode::Safe,
            handoff,
            charter_sha256: self.boot.charter_sha256.clone(),
            extend_free: self.extend_free.load(std::sync::atomic::Ordering::SeqCst),
        }
    }

    fn publish_snapshot(&self) {
        let s = self.snapshot();
        *self.published.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
    }

    fn read_ready(&self) -> Option<pf::ReadyFile> {
        let b = fsx::read_nofollow(&rel(&self.home, pf::paths::READY), 64 * 1024, false).ok()?;
        pf::decode(&b).ok()
    }

    fn check_heartbeat(&mut self, now: Now) {
        if self.m.slot != Slot::Running || self.m.handoff.is_some() {
            return;
        }
        let Some(p) = self.m.primary.clone() else { return };
        let Some(rec) = self.bodies.get(&p.id) else { return };
        let Some(ready_at) = rec.ready_at else { return };
        let stale = self.cfg.supervise.heartbeat_stale();
        let mut last_age = ready_at.elapsed();
        let path = rel(&self.home, pf::paths::HEARTBEAT);
        if let Ok(b) = fsx::read_nofollow(&path, 64 * 1024, false) {
            if let Ok(h) = pf::decode::<pf::HeartbeatFile>(&b) {
                if h.boot_id == rec.boot_id {
                    let ts_age = crate::clock::age(&h.ts).unwrap_or(stale * 2);
                    let mtime_age = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).unwrap_or(stale * 2);
                    // Stale only if the timestamp is old and the file mtime agrees.
                    last_age = last_age.min(ts_age.max(mtime_age.min(ts_age)));
                }
            }
        }
        if last_age > stale {
            crate::klog::log("supervise", &format!("heartbeat stale for {} s; killing {}", last_age.as_secs(), p.id));
            let acts = self.m.on(Ev::HeartbeatStale { inst: p.id }, now);
            self.exec(acts);
        }
    }

    fn free_bytes(&self) -> u64 {
        let (first, then) = disk_probe_paths(&self.home);
        crate::sys::free_bytes(&first).or_else(|_| crate::sys::free_bytes(&then)).unwrap_or(u64::MAX)
    }

    fn refresh_ledger(&mut self) {
        if let Ok(v) = self.link.call(&KernelRequest::LedgerGet, self.link.request_timeout) {
            if let Some(l) = v.get("level").and_then(Value::as_u64) {
                self.level = l as u8;
            }
            self.ledger = Some(v);
            self.ledger_at = Some(Instant::now());
        }
        if let Ok(v) = self.link.call(&KernelRequest::CeilingsGet, self.link.request_timeout) {
            // F2 (demo 2026-10-25): the charter's `extend.mode` can change while the kernel runs; every refresh
            // re-reads it (and the Root pushes a `ceilings` event at once), so admission follows the owner's switch.
            if let Some(x) = v.get("extend") {
                self.extend_free.store(extend_mode_free(Some(x)), std::sync::atomic::Ordering::SeqCst);
            }
            self.proxy.set_ceilings(&v);
            self.ceilings = Some(v);
        }
    }

    fn publish_hb(&self) {
        let (slot, mode, busy, sha12) = self.m.hb();
        self.hb.lock().unwrap().data = Some(HbData { busy, mode, sha12, slot });
    }

    // ------------------------------------------------------------------------------------
    // The Root
    // ------------------------------------------------------------------------------------

    fn on_root_request(&mut self, id: String, req: RootRequest) {
        let now = Now::real();
        match req {
            RootRequest::Stop(a) => {
                let acts = self.m.on(Ev::RootStop { req: id, grace_ms: a.grace_ms }, now);
                self.exec(acts);
            }
            RootRequest::PrepareSwap(a) => {
                let request_id = id.clone();
                let acts = self.m.on(Ev::RootPrepareSwap { req: id, deadline_s: a.deadline_s }, now);
                if !acts.iter().any(|a| matches!(a, Act::Respond { result: Err(_), .. })) {
                    self.kstate_swap = Some((request_id, a.job));
                }
                self.exec(acts);
            }
            RootRequest::OwnerIn(a) => self.owner_in(id, a.qid, a.frame),
        }
    }

    fn on_root_event(&mut self, e: RootEvent) {
        let now = Now::real();
        match e {
            RootEvent::Exited(x) => {
                if !self.pending_inst.is_empty() && !self.bodies.contains_key(&x.instance) {
                    // Possibly a body whose spawn answer is still on its way (K-RACE).
                    self.early_exits.push(x);
                    return;
                }
                self.on_exited(x, now);
            }
            RootEvent::Level(l) => {
                self.level = l.to;
                self.proxy.set_level(l.to);
                let acts = vec![
                    Act::Report { kind: "level_applied", data: json!({"from": l.from, "to": l.to}) },
                    Act::Notice { kind: "level", data: json!({"level": l.to, "cap": l.cap, "trigger": l.trigger}) },
                ];
                if l.to < 2 {
                    crate::fleet::on_demotion(l.to);
                }
                self.exec(acts);
            }
            RootEvent::Ceilings(c) => {
                if let Some(x) = c.extra.get("extend") {
                    self.extend_free.store(extend_mode_free(Some(x)), std::sync::atomic::Ordering::SeqCst);
                }
                self.ceilings = serde_json::to_value(&c).ok();
                if let Some(v) = &self.ceilings {
                    self.proxy.set_ceilings(v);
                }
            }
            RootEvent::Mode(_) => {}
            RootEvent::Access(a) => self.on_access(a, now),
            RootEvent::Swap(s) => crate::kupgrade::on_swap_event(&s),
            RootEvent::ExtendWindow(d) => self.on_extend_window(d),
            RootEvent::ExtendRollback(d) => self.on_extend_rollback(d, now),
            RootEvent::Unknown(_) => {}
        }
    }

    /// ADR-011 §2.3 step 6 (X2): `extend.window{group, closed, outcome}`. The group's changes get
    /// their state (X0b), and on `clean`/`quiet`/`flagged` the kernel's own LKG (bin, refs,
    /// `lkg.json`, reason `change_close`) follows the Root's `lkg` move: when the group's head is
    /// the running version and no older group is open. A close that is not the newest open version
    /// leaves the local LKG for the next close; the Root's pointer stays authoritative.
    fn on_extend_window(&mut self, d: init_proto::root_link::ExtendWindowData) {
        use crate::promote::admit;
        if d.state != "closed" {
            return;
        }
        let outcome = d.outcome.clone().unwrap_or_default();
        let Some(g) = admit::record_close(&self.home, d.group, &outcome) else {
            crate::klog::log("supervise", &format!("extend.window: group {} closed ({outcome}) is not in the kernel's table", d.group));
            return;
        };
        for c in &g.changes {
            admit::change_state(&self.home, self.outbox.as_ref(), &c.sha, &outcome, json!({"job": c.job, "group": g.group, "seq": c.seq}));
        }
        if !d.lkg_eligible() {
            return;
        }
        let older_open = admit::Groups::load(&self.home).open_groups().any(|o| o.group < g.group);
        let (Some(head), Some(last)) = (g.head().map(str::to_string), g.changes.last().cloned()) else { return };
        if older_open || self.m.current.sha != head || !matches!(self.m.mode, Mode::Provisional(_)) {
            crate::klog::log("supervise", &format!("extend.window: group {} closed {outcome}; local LKG stays {} (head {}, current {})", g.group, short(&self.m.lkg_sha), short(&head), short(&self.m.current.sha)));
            return;
        }
        if let Err(e) = self.lkg_commit_as(&head, &last.job, pf::LkgReason::ChangeClose, Some(&last.scope)) {
            self.alert("lkg_local_failed", &format!("group {} closed {outcome}: {e}", g.group));
        }
    }

    /// ADR-011 §3.4 (X2): `extend.rollback{to, groups, reason}`. When the target (the parent of the
    /// oldest named group) is the kernel's LKG, the body rolls back at the next step boundary (the
    /// overlapped rollback), and its `body_rollback{to_sha: LKG}` confirms to the Root; when the body
    /// is already there, the confirmation is sent at once. ADR-013 G2: a target that is not LKG but
    /// that the running version reaches through data-only (t0) changes only (an open group's parent
    /// while an older group is still open) is a data-only rollback on the running binary, confirmed
    /// the same way. Any other target is left to the Root's whole-system rollback, which follows 120 s
    /// without a confirmation.
    fn on_extend_rollback(&mut self, d: init_proto::root_link::ExtendRollbackData, now: Now) {
        // F7: an A3 still running G3 for a group being rolled back stops (no veto, no more building).
        let stopped = crate::promote::admit::stop_a3(&self.home, &d.groups);
        if stopped > 0 {
            crate::klog::log("supervise", &format!("extend.rollback (groups {:?}): stopped {stopped} A3 run(s)", d.groups));
        }
        let target = crate::promote::admit::Groups::load(&self.home).parent_of(&d.groups);
        let reason = format!("extend_{}", d.reason.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(40).collect::<String>());
        match target {
            Some(t) if self.m.current.sha == t && !matches!(self.m.slot, Slot::RollingBack) => {
                self.outbox_post("body_rollback", json!({"from_sha": self.m.current.sha.clone(), "to_sha": t, "reason": reason}));
            }
            Some(t) if t == self.m.lkg_sha => {
                crate::klog::log("supervise", &format!("extend.rollback to {} (groups {:?}, {}): rolling back to LKG {}", d.to, d.groups, d.reason, short(&t)));
                let acts = self.m.request_rollback(&reason, true, now);
                self.exec(acts);
            }
            Some(t) if self.data_only_path(&t) => {
                crate::klog::log("supervise", &format!("extend.rollback to {} (groups {:?}, {}): data-only rollback to {}", d.to, d.groups, d.reason, short(&t)));
                let acts = self.m.request_rollback_to(&reason, Some(t), true, now);
                self.exec(acts);
            }
            other => {
                self.alert("extend_rollback_root", &format!("extend.rollback to {} (groups {:?}): the target {} is not the kernel's LKG {} and not a data-only step back; the Root's whole-system rollback follows",
                    d.to, d.groups, other.as_deref().map(short).unwrap_or("unknown"), short(&self.m.lkg_sha)));
            }
        }
    }

    /// ADR-013 G2: the running version reaches `target` back through admitted t0 changes only (the
    /// kernel's change-group table), so `bin/init-body` and the kept set are the target's too.
    fn data_only_path(&self, target: &str) -> bool {
        crate::promote::admit::Groups::load(&self.home).data_only_back(&self.m.current.sha, target)
    }

    /// ADR-009 (SBX-009 §D): the Root's `access` event. Later live spawns relay the new values;
    /// with `restart: "idle"` the running body, whose profile was fixed at its spawn, restarts
    /// gracefully at its next idle point and resumes its session (`handoff`, cause
    /// `workspace_change`).
    fn on_access(&mut self, d: init_proto::root_link::AccessData, now: Now) {
        let a = spawnspec::Access::from_data(&d);
        if self.access.as_ref() != Some(&a) {
            crate::klog::log("supervise", &format!("access {} workspace {:?}", a.access, a.workspace));
            self.access = Some(a);
        }
        if d.restart_idle() && self.m.primary.is_some() {
            let acts = self.m.on(Ev::Restart { cause: "workspace_change".into() }, now);
            self.exec(acts);
        }
    }

    /// An instance's `exited` event: the machine's exit rows, then the body's pipes.
    fn on_exited(&mut self, x: ExitedData, now: Now) {
        let tail = self.bodies.get(&x.instance).map(|b| b.io.stderr_tail()).unwrap_or_default();
        let disk_low = x.code != Some(0) && self.free_bytes() < self.cfg.supervise.min_free_run_bytes;
        let acts = self.m.on(Ev::Exited { inst: x.instance.clone(), code: x.code, signal: x.signal, disk_low, stderr_tail: tail.clone() }, now);
        if let Some(b) = self.bodies.remove(&x.instance) {
            b.io.close_stdin();
            self.last_tail = Some(tail);
        }
        self.exec(acts);
    }

    /// EOF on fd 3 (§3.13.1): stop every body, flush, exit 69 within 10 s.
    fn link_lost(&mut self) -> i32 {
        crate::klog::log("supervise", "control link lost: stopping bodies, exiting 69");
        for b in self.bodies.values() {
            b.io.send(&ToBody::Stop { reason: "root_gone".into(), grace_ms: 5000 });
        }
        let deadline = Instant::now() + Duration::from_secs(7);
        while Instant::now() < deadline && self.bodies.values().any(|b| !b.stdout_eof) {
            if let Ok(DEv::Body { inst, msg: BodyMsg::Eof }) = self.rx.recv_timeout(Duration::from_millis(100)) {
                if let Some(b) = self.bodies.get_mut(&inst) {
                    b.stdout_eof = true;
                }
            }
        }
        self.persist();
        self.proxy.shutdown(Duration::from_millis(500));
        self.shutdown_threads();
        init_proto::root_link::kernel_exit::LINK_LOST
    }

    // ------------------------------------------------------------------------------------
    // The owner relay (§3.10, §4.6)
    // ------------------------------------------------------------------------------------

    fn owner_in(&mut self, id: String, qid: String, frame: OwnerInFrame) {
        let now = Now::real();
        let text = match frame {
            OwnerInFrame::Interrupt { then } => {
                // only `continue` is known; anything else is a plain interrupt
                let then = then.filter(|t| t == "continue");
                if let Some(t) = self.m.relay_target().map(str::to_string) {
                    self.send_to(&t, &ToBody::Interrupt { then });
                }
                self.link.respond(&id, Ok(json!({})));
                return;
            }
            OwnerInFrame::Retract { seq } => {
                self.link.respond(&id, Ok(json!({})));
                return self.owner_retract(seq);
            }
            OwnerInFrame::Line { text } => text,
        };
        let trimmed = text.trim();
        let (word, rest) = match trimmed.split_once(char::is_whitespace) {
            Some((w, r)) => (w, r.trim()),
            None => (trimmed, ""),
        };
        let ev = match (word, rest.is_empty()) {
            ("/stop", true) => Some(Ev::OwnerStop),
            ("/resume", true) => Some(Ev::OwnerResume),
            ("/status", true) => {
                let s = self.status_line();
                self.owner_notice(AlertLevel::Info, &s);
                self.link.respond(&id, Ok(json!({})));
                return;
            }
            ("/kernel", true) => {
                self.owner_notice(AlertLevel::Info, "kernel commands: /stop /resume /status /accept <task> /bless <job> /reject <job> /later <text> /kernel");
                self.link.respond(&id, Ok(json!({})));
                return;
            }
            ("/accept", false) | ("/bless", false) | ("/reject", false) => {
                let kind = &word[1..];
                let key = if kind == "accept" { "task_id" } else { "job" };
                self.outbox_post("owner_signal", json!({"kind": kind, key: rest}));
                if kind == "accept" && crate::layout::valid_id(rest) {
                    self.notice("accepted", json!({"task_id": rest}));
                }
                if kind == "reject" && matches!(self.m.mode, Mode::Provisional(_)) {
                    let acts = self.m.request_rollback("owner_reject", true, now);
                    self.exec(acts);
                }
                self.link.respond(&id, Ok(json!({})));
                return;
            }
            _ => None,
        };
        if let Some(ev) = ev {
            self.link.respond(&id, Ok(json!({})));
            let acts = self.m.on(ev, now);
            self.exec(acts);
            return;
        }
        let (body_text, mode) = match trimmed.strip_prefix("/later ") {
            Some(r) => (r.to_string(), InputMode::FollowUp),
            None => (text.clone(), InputMode::Steer),
        };
        match self.inbox.offer(Some(&qid), &body_text, mode) {
            Ok(Offer::Duplicate) => self.link.respond(&id, Ok(json!({}))),
            Ok(Offer::Queued(input)) => {
                self.link.respond(&id, Ok(json!({}))); // only now is the line durable here
                self.owner_out(OwnerOutFrame::Echo { seq: input.seq, text: input.text.clone() });
                if let Some(t) = self.m.relay_target().map(str::to_string) {
                    self.send_to(&t, &input.frame());
                } else {
                    self.owner_notice(AlertLevel::Info, &format!("queued: the body is {}", self.m.hb().0));
                }
                let acts = self.m.on(Ev::OwnerLine, now);
                self.exec(acts);
            }
            Err(e) => self.link.respond(&id, Err(ErrorBody::new(ErrorCode::Internal, format!("inbox: {e}")))),
        }
    }

    /// STEER: take back input `seq`. Held here only (no body is reading it): done at once.
    /// Otherwise the body decides (`retracted{seq, ok}`), since it may already have logged it.
    fn owner_retract(&mut self, seq: u64) {
        if !self.inbox.pending_seqs().contains(&seq) {
            return self.owner_notice(AlertLevel::Info, &format!("[kernel] not taken back: message {seq} was already read by the agent"));
        }
        match self.m.relay_target().map(str::to_string) {
            Some(t) if self.send_to(&t, &ToBody::Retract { seq }) => {}
            _ => self.on_retracted(seq, true),
        }
    }

    /// The body's answer to `retract`, or the kernel's own when no body holds the input.
    fn on_retracted(&mut self, seq: u64, ok: bool) {
        if ok && self.inbox.retract(seq) {
            push_ring(&mut self.steer_retracted, seq, 16);
            self.owner_state();
            self.owner_notice(AlertLevel::Info, &format!("[kernel] taken back: message {seq} will not be read"));
        } else if !ok {
            self.owner_notice(AlertLevel::Info, &format!("[kernel] not taken back: message {seq} was already read by the agent"));
        }
    }

    fn owner_out(&self, frame: OwnerOutFrame) {
        self.owner_send(frame, false);
    }

    /// One `owner.out` event, bounded to the Root's line limit and paced under its frame limit
    /// (K-LINE, `ownerline`): a line over 1 MiB, or more than 10,000 frames a minute, would end
    /// the control link and count as a crash. `body`: a relayed body frame, which may be merged
    /// with the next or, in a flood of other kinds, dropped.
    fn owner_send(&self, frame: OwnerOutFrame, body: bool) {
        let Some(frame) = crate::ownerline::bounded(frame) else {
            return crate::klog::log("supervise", "owner.out frame over the line limit dropped");
        };
        let due = self.pacer.lock().unwrap().push(frame, body, Instant::now());
        for frame in due {
            self.link.event(&KernelEvent::OwnerOut(OwnerOutData { frame }));
        }
    }

    /// A body's display frame to the owner with the fields this build does not know (tui.md §9
    /// Q2), split or cut to fit the control link (`ownerline`). A `turn_end` is followed by a
    /// `state` frame, so the owner's run cost refreshes per turn.
    fn relay_display(&self, inst: &str, is_relay: bool, d: Display, extra: BTreeMap<String, Value>) {
        if !is_relay {
            return;
        }
        let boot_seq = self.bodies.get(inst).map(|b| b.boot_seq).unwrap_or(0);
        let turn_end = d.kind == DisplayKind::TurnEnd;
        for frame in crate::ownerline::display_frames(boot_seq, d, extra) {
            self.owner_send(frame, true);
        }
        if turn_end {
            self.owner_state();
        }
    }

    /// The `state` frame (§4.6): the primary's state, the live scope's model (`[model] id`) and
    /// today's spend booked to the live scope's current task (`x-init-task`) from the proxy's
    /// accounting; `task_musd` is null without a proxy or before the body names a task.
    fn owner_state(&self) {
        let (slot, mode, busy, sha12) = self.m.hb();
        let model = Some(self.cfg.model.id.clone()).filter(|m| !m.is_empty());
        let stats = self.proxy.stats("live");
        let task_musd = stats.as_ref().and_then(|s| s["spend"]["task"]["today_musd"].as_u64());
        let evolution_checks_today_musd = stats.as_ref().and_then(|s| s["spend"]["evolution_today_musd"].as_u64());
        let queued = self.inbox.counts().0;
        let mut pending = self.inbox.pending_seqs();
        pending.truncate(init_proto::root_link::STEER_LIST_MAX);
        self.owner_out(OwnerOutFrame::State { busy, level: self.level, members: 0, mode, model, queued, sha12, slot, task_musd,
            evolution_checks_today_musd, steer_pending: Some(pending), steer_seen: Some(self.steer_seen.iter().copied().collect()),
            steer_retracted: Some(self.steer_retracted.iter().copied().collect()) });
    }

    fn owner_notice(&self, level: AlertLevel, text: &str) {
        self.owner_notice_data(level, text, None);
    }

    /// A notice with its structured payload (TUI-OBSERVE, tui.md §3.10). `data` must be ICJ
    /// (integers only); one that is not is dropped and the text still goes out.
    fn owner_notice_data(&self, level: AlertLevel, text: &str, data: Option<Value>) {
        let data = data.filter(|d| init_proto::icj::to_line(d).is_ok());
        self.owner_out(OwnerOutFrame::Notice { level, text: text.to_string(), ts: crate::clock::now_rfc3339(), data });
    }

    /// TUI-OBSERVE: an owner-only event (`gate_start`, `rolled_back`): not in the inbox, not to the body.
    fn owner_event(&self, kind: &str, data: &Value) {
        let (level, text, owner) = owner_notice_view(kind, data, &Value::Null);
        self.owner_notice_data(level, &text, Some(owner));
    }

    fn status_line(&self) -> String {
        let (slot, mode, busy, sha12) = self.m.hb();
        format!("slot {slot} · mode {mode} · busy {busy} · sha {sha12} · lkg {} · level L{} · queued {}", self.m.lkg_sha.chars().take(12).collect::<String>(), self.level, self.inbox.counts().0)
    }

    fn send_to(&self, inst: &str, f: &ToBody) -> bool {
        self.bodies.get(inst).is_some_and(|b| b.io.send(f))
    }

    // ------------------------------------------------------------------------------------
    // Bodies
    // ------------------------------------------------------------------------------------

    fn on_body(&mut self, inst: String, msg: BodyMsg) {
        let now = Now::real();
        let is_relay = self.m.relay_target() == Some(inst.as_str()) || self.m.primary.as_ref().is_some_and(|p| p.id == inst);
        match msg {
            BodyMsg::Frame(FromBody::Ack { seq }) => {
                // STEER: the owner sees the line picked up (it is durably in the body's log)
                if self.inbox.ack(seq) {
                    push_ring(&mut self.steer_seen, seq, init_proto::root_link::STEER_LIST_MAX);
                    self.owner_state();
                }
            }
            BodyMsg::Frame(FromBody::Retracted { seq, ok }) => self.on_retracted(seq, ok),
            BodyMsg::Frame(FromBody::NoticeAck { id }) => self.inbox.notice_ack(&id),
            BodyMsg::Frame(FromBody::Display(d)) => self.relay_display(&inst, is_relay, d, BTreeMap::new()),
            BodyMsg::Display(d, extra) => self.relay_display(&inst, is_relay, d, extra),
            BodyMsg::Frame(FromBody::State { busy }) => {
                let acts = self.m.on(Ev::Busy { inst: inst.clone(), busy }, now);
                self.exec(acts);
                if is_relay {
                    self.owner_state();
                }
            }
            BodyMsg::Frame(FromBody::Paused { token, .. }) => {
                let acts = self.m.on(Ev::Paused { inst, token }, now);
                self.exec(acts);
            }
            BodyMsg::Frame(FromBody::ReloadDone { job, ok, prefix_changed, components, error }) => {
                crate::klog::log("supervise", &format!("reload_done {job} ok={ok} error={error:?}"));
                if let Some(tx) = self.reloads.remove(&job) {
                    let _ = tx.send(Ok(ReloadOutcome { components, error, ok, prefix_changed }));
                }
            }
            BodyMsg::Frame(FromBody::Unknown) => {}
            BodyMsg::Raw(text) => {
                if let Some(b) = self.bodies.get_mut(&inst) {
                    b.raw_lines += 1;
                }
                let d = Display { kind: DisplayKind::Info, text, call_id: None, name: None, ok: None };
                self.relay_display(&inst, is_relay, d, BTreeMap::new());
            }
            BodyMsg::Eof => {
                if let Some(b) = self.bodies.get_mut(&inst) {
                    b.stdout_eof = true;
                }
            }
            BodyMsg::Flood => {
                let acts = self.m.on(Ev::Flood { inst }, now);
                self.exec(acts);
            }
        }
    }

    fn on_spawned(&mut self, tag: Tag, boot: PendingBoot, result: Result<Spawned, SpawnError>) {
        let now = Now::real();
        self.pending_inst.remove(&tag);
        let r = match result {
            Ok(sp) => {
                let id = sp.instance.clone();
                let logs = rel(&self.home, "state/kernel/logs");
                let path = bodyio::stderr_log_path(&logs, "body", boot.boot_seq);
                let tx = self.tx.clone();
                let inst = id.clone();
                let io = BodyIo::start(&format!("body-{}", &id[..id.len().min(6)]), sp.stdin, sp.stdout, sp.stderr, &path, move |msg| {
                    let _ = tx.send(DEv::Body { inst: inst.clone(), msg });
                });
                // The live scope follows the relay target: a handoff's B takes it at commit.
                if self.m.handoff.as_ref().is_none_or(|h| h.new_tag != Some(tag)) {
                    let label = crate::proxyctl::ProxyLabel { sha: boot.sha.clone(), boot_reason: boot.reason.clone(), model: self.cfg.model.id.clone(), level: self.level };
                    self.proxy.set_label("live", &label);
                    self.proxy.set_body("live", &id);
                }
                self.bodies.insert(
                    id.clone(),
                    BodyRec { io, boot_id: boot.boot_id.clone(), boot_seq: boot.boot_seq, ready_at: None, pid: sp.pid, started_at: sp.started_at.clone(), stdout_eof: false, raw_lines: 0 },
                );
                Ok(id)
            }
            Err(e) => Err(e.to_string()),
        };
        let _ = boot.started;
        let landed = r.as_ref().ok().cloned();
        let acts = self.m.on(Ev::Spawned { tag, result: r }, now);
        self.exec(acts);
        // K-RACE: this instance's `exited` may already have come. Its stderr reader has only
        // just started; the pipe is at EOF (the body is gone), so the tail settles at once.
        if let Some(i) = landed.and_then(|id| self.early_exits.iter().position(|x| x.instance == id)) {
            let x = self.early_exits.remove(i);
            if let Some(b) = self.bodies.get(&x.instance) {
                b.io.wait_stderr_eof(EARLY_EXIT_TAIL_WAIT);
            }
            self.on_exited(x, Now::real());
        }
        if self.pending_inst.is_empty() {
            for x in std::mem::take(&mut self.early_exits) {
                self.on_exited(x, Now::real());
            }
        }
    }

    /// G4 failed for `sha`: `update-failed.json` (stage G4) and a rollback at the next idle point.
    fn g4_reject(&mut self, sha: String, job: String, reason: String, now: Now) {
        if self.m.current.sha == sha {
            let mut uf = new_update_failed(self.update_failed.as_ref(), UpdateFailedStage::G4, &reason, &sha, Some(self.m.lkg_sha.clone()));
            uf.job = Some(job);
            let _ = fsx::write_json(&rel(&self.home, pf::paths::UPDATE_FAILED), &uf);
            self.update_failed = Some(uf);
            self.update_failed_delivered = false;
            let acts = self.m.request_rollback(&reason, false, now);
            self.exec(acts);
        }
    }

    fn on_cmd(&mut self, c: Cmd) {
        let now = Now::real();
        match c {
            Cmd::Handoff { kind, reply } => {
                if let crate::machine::HandoffKind::Promote { staged_sha256, .. } = &kind {
                    self.staged_sha256 = Some(staged_sha256.clone());
                    self.handoff_started = Some(Instant::now());
                }
                self.handoff_reply = reply;
                let acts = self.m.on(Ev::StartHandoff(kind), now);
                self.exec(acts);
            }
            Cmd::Rollback { reason, immediate, reply } => {
                self.handoff_reply = reply;
                let acts = self.m.request_rollback(&reason, immediate, now);
                self.exec(acts);
            }
            Cmd::RollbackTo { reason, to, immediate, reply } => {
                if to != self.m.lkg_sha && !self.data_only_path(&to) {
                    if let Some(r) = reply {
                        let _ = r.send(Err(format!("{} is not LKG and the running version does not reach it through data-only changes", short(&to))));
                    }
                } else {
                    self.handoff_reply = reply;
                    let acts = self.m.request_rollback_to(&reason, Some(to), immediate, now);
                    self.exec(acts);
                }
            }
            Cmd::G4Pass { sha, job, reply } => {
                let r = self.lkg_commit_as(&sha, &job, pf::LkgReason::G4Pass, None);
                if let Err(e) = &r {
                    self.pending_lkg = Some((sha, job));
                    self.pending_lkg_reason = (pf::LkgReason::G4Pass, None);
                    self.alert("lkg_store_failed", e);
                }
                if let Some(tx) = reply {
                    let _ = tx.send(r);
                }
            }
            Cmd::G4Fail { sha, job, reason } => {
                let data = json!({"gate_id": job, "gate_kind": "g4", "sha": sha, "verdict": "reject", "reason": reason});
                self.outbox_post("gate", data);
                self.g4_reject(sha, job, reason, now);
            }
            Cmd::G4Reject { sha, job, reason } => self.g4_reject(sha, job, reason, now),
            Cmd::Services { reply } => {
                let _ = reply.send(Services {
                    home: self.home.clone(),
                    cfg: self.cfg.clone(),
                    spawner: self.spawner.clone(),
                    outbox: self.outbox.clone(),
                    level: self.level,
                    link: self.link.clone(),
                    boot: self.boot.clone(),
                    extend_free: self.extend_free.clone(),
                });
            }
            Cmd::Notice { kind, data, owner } => self.notice_with(&kind, data, &owner),
            Cmd::OwnerEvent { kind, data } => self.owner_event(&kind, &data),
            Cmd::Snapshot { reply } => {
                let _ = reply.send(self.snapshot());
            }
            Cmd::Reload { job, kind, components, reply } => {
                let target = self.m.relay_target().map(str::to_string);
                match target {
                    Some(t) if self.send_to(&t, &ToBody::Reload { job: job.clone(), kind, components }) => {
                        self.reloads.insert(job, reply);
                    }
                    _ => {
                        let _ = reply.send(Err(format!("no body takes a reload: the body slot is {}", self.m.hb().0)));
                    }
                }
            }
            Cmd::DataOnly { p, reply } => {
                let r = self.data_only(p);
                if let Some(tx) = reply {
                    let _ = tx.send(r);
                }
            }
        }
        self.publish_snapshot();
    }

    /// §3.6.8: a promotion without a handoff. `main` moves from `p.from` to `p.sha`, the kept set
    /// (if any) is swapped in, and the version is either committed as LKG at once (t0: the
    /// store-and-report steps of §3.6.6 with `reason:"data_only"`, binary unchanged) or becomes
    /// provisional (t1, or a t0 on top of a provisional chain) for the asynchronous G4.
    fn data_only(&mut self, p: DataOnly) -> Result<(), String> {
        let now = Now::real();
        if self.m.handoff.is_some() || matches!(self.m.slot, Slot::Handoff | Slot::RollingBack) {
            return Err("a handoff or rollback is in progress".into());
        }
        if self.m.current.sha != p.from {
            return Err(format!("the running version moved ({} is not {})", short(&self.m.current.sha), short(&p.from)));
        }
        if matches!(self.m.slot, Slot::Halted { .. } | Slot::Stopping { .. }) {
            return Err(format!("the body slot is {}", self.m.hb().0));
        }
        self.lineage.move_main(&p.sha, &p.from).map_err(|e| format!("update-ref main: {e}"))?;
        crate::fault::point("supervise.data_only.after_update_ref");
        let bin = self.home.join("bin");
        if self.built_from.is_none() {
            self.built_from = Some(p.from.clone());
        }
        if let Some(set) = &p.ext {
            if let Err(e) = crate::extset::swap(&bin, "ext", &set.id) {
                self.alert("ext_swap_failed", &format!("bin/ext → ext.d/{}: {e}", set.id));
            }
        }
        let body_binary = crate::binstore::sha_of(&bin, "init-body").unwrap_or_default();
        let (ext_set, components) = crate::extset::report(&bin, "ext");
        let _ = self.outbox.local_only("data_only", json!({"from": p.from, "to": p.sha, "job": p.job, "scope": p.scope}));
        let depth = match &self.m.mode {
            Mode::Provisional(pr) => pr.depth + 1,
            _ => 1,
        };
        self.m.current = Ver { sha: p.sha.clone(), provisional: true };
        if !self.m.mode.is_safe() {
            self.m.mode = Mode::Provisional(Prov { sha: p.sha.clone(), depth, since_ms: now.wall_ms, probation_until: None });
        }
        self.relabel_live(&p.sha, "promote");
        self.notice("promoted", json!({"sha": p.sha, "handoff": false}));
        if p.lkg {
            if let Err(e) = self.lkg_commit_as(&p.sha, &p.job, pf::LkgReason::DataOnly, Some(&p.scope)) {
                // The store or the report was refused: stay provisional and retry every minute
                // (§3.6.6 step 0), exactly as a G4-passed version would.
                self.alert("lkg_store_failed", &e);
                self.pending_lkg = Some((p.sha.clone(), p.job.clone()));
                self.pending_lkg_reason = (pf::LkgReason::DataOnly, Some(p.scope.clone()));
                self.outbox_post("promotion", json!({"sha": p.sha, "from_sha": p.from, "gate_job": p.job, "scope": p.scope, "lkg": false, "body_binary": body_binary, "ext_set": ext_set, "components": components}));
                self.notice("provisional", json!({"sha": p.sha, "depth": depth}));
            }
        } else {
            self.outbox_post("promotion", json!({"sha": p.sha, "from_sha": p.from, "gate_job": p.job, "scope": p.scope, "lkg": false, "body_binary": body_binary, "ext_set": ext_set, "components": components}));
            self.notice("provisional", json!({"sha": p.sha, "depth": depth}));
        }
        if let Some(set) = &p.ext {
            let names: Vec<String> = set.components.iter().map(|c| c.0.clone()).collect();
            if let Some(t) = self.m.relay_target().map(str::to_string) {
                self.send_to(&t, &ToBody::Reload { job: p.job.clone(), kind: ReloadKind::Kept, components: Some(names) });
            }
        }
        self.persist();
        Ok(())
    }

    // ------------------------------------------------------------------------------------
    // Actions
    // ------------------------------------------------------------------------------------

    fn exec(&mut self, acts: Vec<Act>) {
        let swapping = acts.iter().any(|a| matches!(a, Act::Exit(75)));
        for mut a in acts {
            if swapping {
                if let Act::Respond { req, result: Ok(value) } = &mut a {
                    if let Some((pending, job)) = self.kstate_swap.take() {
                        if &pending == req {
                            self.persist();
                            match crate::kupgrade::kstate::export(&self.home, &self.link, &job) {
                                Ok(hash) => *value = json!({"kstate": hash}),
                                Err(e) => self.alert("kstate_export_failed", &e),
                            }
                        } else { self.kstate_swap = Some((pending, job)); }
                    }
                }
            }
            self.exec_one(a);
        }
    }

    fn exec_one(&mut self, a: Act) {
        match a {
            Act::Spawn(plan) => self.spawn(plan),
            Act::Kill { inst, mode } => {
                let sp = self.spawner.clone();
                std::thread::spawn(move || {
                    if let Err(e) = sp.kill(&inst, mode) {
                        crate::klog::log("supervise", &format!("kill {inst}: {e}"));
                    }
                });
            }
            Act::Send { inst, frame } => {
                if !self.send_to(&inst, &frame) {
                    crate::klog::log("supervise", &format!("frame to {inst} not delivered"));
                }
            }
            Act::Redeliver { inst } => {
                for i in self.inbox.pending_inputs() {
                    self.send_to(&inst, &i.frame());
                }
                for n in self.inbox.pending_notices() {
                    self.send_to(&inst, &n.frame());
                }
                self.update_failed_delivered = true;
            }
            Act::InstallBin { from } => {
                if let Err(e) = crate::binstore::install_from(&self.home.join("bin"), from.file_name()) {
                    self.alert("rollback_failed", &format!("install {}: {e}", from.file_name()));
                }
            }
            Act::RollbackMain { from, to } => {
                if let Err(e) = self.lineage.rollback_main(&from, &to) {
                    self.alert("rollback_failed", &format!("lineage rollback: {e}"));
                }
                crate::fault::point("supervise.rollback.after_update_ref");
            }
            Act::RestoreMain { from, to } => {
                if let Err(e) = self.lineage.move_main(&to, &from) {
                    self.alert("rollback_failed", &format!("lineage main back to {}: {e}", short(&to)));
                }
            }
            Act::Report { kind, data } => self.outbox_post(kind, data),
            Act::LocalLineage { kind, data } => {
                if let Err(e) = self.outbox.local_only(kind, data) {
                    crate::klog::log("supervise", &format!("vault/lineage.jsonl: {e}"));
                }
            }
            Act::Alert { code, message } => self.alert(code, &message),
            Act::UpdateFailed { stage, reason, sha, from_sha, job, exit, log_tail } => {
                let mut uf = new_update_failed(self.update_failed.as_ref(), stage, &reason, &sha, from_sha);
                uf.job = job;
                if let Some((c, s)) = exit {
                    uf.exit_code = c;
                    uf.signal = s;
                }
                uf.log_tail = if log_tail.is_empty() { self.last_tail.clone().unwrap_or_default() } else { log_tail };
                uf.log_tail = fsx::tail_str(&uf.log_tail, pf::MAX_LOG_TAIL);
                if let Err(e) = fsx::write_json(&rel(&self.home, pf::paths::UPDATE_FAILED), &uf) {
                    crate::klog::log("supervise", &format!("update-failed.json: {e}"));
                }
                self.update_failed = Some(uf);
                self.update_failed_delivered = false;
            }
            Act::ResumeA { inst, token } => {
                let uf = self.update_failed.clone();
                self.update_failed_delivered = true;
                self.send_to(&inst, &ToBody::Resume { token, update_failed: uf });
            }
            Act::Notice { kind, data } => self.notice(kind, data),
            Act::FenceTry => {
                let ev = self.fence_try();
                let acts = self.m.on(ev, Now::real());
                self.exec(acts);
            }
            Act::FenceRelease => {
                if let Some(f) = self.fence.take() {
                    let _ = f.unlock();
                }
            }
            Act::Commit { job, from_sha, to_sha, staged_sha256 } => {
                self.built_from = None;
                self.relabel_live(&to_sha, "promote");
                self.commit(job, from_sha, to_sha, staged_sha256)
            }
            Act::CommitRollback { from_sha, to_sha, reason, data_only } => {
                self.relabel_live(&to_sha, "rollback");
                if !data_only {
                    if let Err(e) = crate::binstore::install_from(&self.home.join("bin"), "init-body.lkg") {
                        self.alert("rollback_failed", &format!("install LKG: {e}"));
                    }
                    self.built_from = None;
                    if let Err(e) = crate::extset::follow(&self.home.join("bin"), "ext", "ext.lkg") {
                        self.alert("rollback_failed", &format!("bin/ext → bin/ext.lkg: {e}"));
                    }
                }
                if let Err(e) = self.lineage.rollback_main(&from_sha, &to_sha) {
                    self.alert("rollback_failed", &format!("lineage rollback: {e}"));
                }
                self.outbox_post("body_rollback", json!({"from_sha": from_sha, "to_sha": to_sha, "reason": reason}));
                let to_lkg = to_sha == self.m.lkg_sha;
                self.owner_event("rolled_back", &json!({"from_sha": from_sha, "to_sha": to_sha, "reason": reason, "data_only": data_only, "to_lkg": to_lkg}));
            }
            Act::Respond { req, result } => {
                self.persist();
                self.link.respond(&req, result);
            }
            Act::Exit(code) => {
                self.proxy.shutdown(Duration::from_secs(2));
                self.exit_code = Some(code);
            }
            Act::StopFleet => crate::fleet::stop_all("safe_or_halt"),
            Act::HandoffDone { ok, reason } => {
                if let Some(r) = self.handoff_reply.take() {
                    let _ = r.send(if ok { Ok(()) } else { Err(reason.unwrap_or_default()) });
                }
                if ok {
                    self.staged_sha256 = None;
                }
            }
            Act::Persist => self.persist(),
        }
    }

    fn relabel_live(&self, sha: &str, boot_reason: &str) {
        if let Some(p) = &self.m.primary {
            self.proxy.set_label("live", &crate::proxyctl::ProxyLabel { sha: sha.into(), boot_reason: boot_reason.into(), model: self.cfg.model.id.clone(), level: self.level });
            self.proxy.set_body("live", &p.id);
        }
    }

    fn outbox_post(&self, kind: &'static str, data: Value) {
        if let Err(e) = self.outbox.post(kind, data) {
            crate::klog::log("outbox", &format!("{kind}: {e}"));
        }
    }

    fn notice(&mut self, kind: &str, data: Value) {
        self.notice_with(kind, data, &Value::Null);
    }

    /// The body gets `data` unchanged; the owner gets the text, a level that matches the outcome
    /// and the structured payload with `owner`'s extra fields (TUI-OBSERVE, [`owner_notice_view`]).
    fn notice_with(&mut self, kind: &str, data: Value, owner: &Value) {
        let shown = data.clone();
        match self.inbox.notice(kind, data) {
            Ok(n) => {
                if let Some(t) = self.m.relay_target().map(str::to_string) {
                    self.send_to(&t, &n.frame());
                }
                let (level, text, d) = owner_notice_view(kind, &shown, owner);
                self.owner_notice_data(level, &text, Some(d));
            }
            Err(e) => crate::klog::log("inbox", &format!("notice: {e}")),
        }
    }

    /// A report (§3.7.1): a line in `alerts.jsonl` and an `alert` to the Root (≤ 30 per hour).
    fn alert(&mut self, code: &str, message: &str) {
        let line = json!({"v": 1, "ts": crate::clock::now_rfc3339(), "source": "supervise", "code": code, "message": message});
        let _ = fsx::append_line(&rel(&self.home, "state/kernel/alerts.jsonl"), &init_proto::icj::to_line(&line).unwrap_or_default());
        crate::klog::log("alert", &format!("{code}: {message}"));
        if self.alerts_this_hour.0.elapsed() >= Duration::from_secs(3600) {
            self.alerts_this_hour = (Instant::now(), 0);
        }
        if self.alerts_this_hour.1 >= 30 {
            return;
        }
        self.alerts_this_hour.1 += 1;
        let level = if matches!(code, "safe_entered" | "halted" | "config_error" | "disk_low") { AlertLevel::Alert } else { AlertLevel::Warn };
        let req = KernelRequest::Alert(AlertArgs { code: code.to_string(), level, message: crate::fsx::tail_str(message, 500) });
        let _ = self.link.send(&req); // fire and forget
    }

    fn fence_try(&mut self) -> Ev {
        if self.fence.is_some() {
            return Ev::FenceOk;
        }
        let Some(sid) = crate::layout::read_session(&self.home) else { return Ev::FenceOk };
        let path = self.home.join("state/sessions").join(format!("{sid}.jsonl"));
        use std::os::unix::fs::OpenOptionsExt;
        let f = match std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&path) {
            Ok(f) => f,
            Err(_) => return Ev::FenceOk, // no log yet: nothing to fence
        };
        match f.try_lock() {
            Ok(()) => {
                self.fence = Some(f);
                Ev::FenceOk
            }
            Err(_) => Ev::FenceBusy,
        }
    }

    fn commit(&mut self, job: String, from_sha: String, to_sha: String, staged_sha256: String) {
        self.persist_with_phase(Some("committing"));
        crate::fault::point("supervise.commit.after_phase_write");
        let bin = self.home.join("bin");
        if bin.join("init-body.staged").exists() {
            if let Err(e) = crate::binstore::commit_bin(&bin) {
                self.alert("handoff_commit", &format!("bin renames: {e}"));
            }
        }
        if let Err(e) = self.lineage.move_main(&to_sha, &from_sha) {
            self.alert("lineage_cas", &format!("update-ref main: {e}"));
        }
        crate::fault::point("supervise.commit.after_update_ref");
        crate::fault::point("supervise.root.after_ref_before_outbox");
        let _ = self.outbox.local_only("handoff", json!({"from": from_sha, "to": to_sha, "job": job}));
        self.outbox_post(
            "promotion",
            json!({"sha": to_sha, "from_sha": from_sha, "gate_job": job, "scope": Value::Null, "lkg": false, "body_binary": staged_sha256, "ext_set": Value::Null, "components": []}),
        );
        let handoff_ms = self.handoff_started.take().map(|t| t.elapsed().as_millis() as u64);
        self.notice_with("promoted", json!({"sha": to_sha, "handoff": true}), &json!({"from_sha": from_sha, "handoff_ms": handoff_ms}));
    }

    /// §3.6.6 LKG commit: `store.put` the running binary, report `promotion{lkg:true}` and wait
    /// for the Root's `seq`, then the local install, `lkg.json`, `refs/heads/lkg`, the exported
    /// tree, `Mode=Normal`, the local `lkg` record and the body notice. A Root refusal keeps the
    /// version provisional (`lkg_store_failed`), retried every minute.
    /// The retried LKG commit of `pending_lkg`, with the reason it was first attempted for.
    fn lkg_commit(&mut self, sha: &str, job: &str) -> Result<(), String> {
        let (reason, scope) = self.pending_lkg_reason.clone();
        let r = self.lkg_commit_as(sha, job, reason, scope.as_deref());
        if r.is_ok() {
            self.pending_lkg_reason = (pf::LkgReason::G4Pass, None);
        }
        r
    }

    fn lkg_commit_as(&mut self, sha: &str, job: &str, reason: pf::LkgReason, scope: Option<&str>) -> Result<(), String> {
        if self.m.current.sha != sha || !matches!(self.m.mode, Mode::Provisional(_)) {
            return Err(format!("{} is not the current provisional version", &sha[..sha.len().min(12)]));
        }
        let bin = self.home.join("bin");
        let path = bin.join("init-body");
        let sha256 = fsx::sha256_file(&path).map_err(|e| format!("bin/init-body: {e}"))?;
        // ADR-011: after the Root's `change_close` the blobs are already stored (A0) and the Root moved
        // `lkg` itself; the kernel reports no `promotion{lkg:true}` (the Root refuses one that matches
        // no closed group), it only mirrors the pointer locally.
        let change_close = reason == pf::LkgReason::ChangeClose;
        let (ext_set, components) = crate::extset::report(&bin, "ext");
        if !change_close {
            let f = File::open(&path).map_err(|e| format!("bin/init-body: {e}"))?;
            let bytes = f.metadata().map(|m| m.len()).unwrap_or(0);
            use std::os::fd::AsFd;
            let put = KernelRequest::StorePut(init_proto::root_link::StorePutArgs { kind: init_proto::root_link::StoreKind::InitBody, name: None, sha256: sha256.clone(), bytes, job: None });
            self.link.call_with_fd(&put, f.as_fd(), crate::link::STORE_TIMEOUT).map_err(|e| format!("store.put: {e}"))?;
            crate::fault::point("supervise.lkg.after_store_put");
            // Kept components (§3.6.6 step 0): each file of the set `bin/ext` points at, one fd each.
            if let Some(set) = ext_set.as_str() {
                for c in components.as_array().cloned().unwrap_or_default() {
                    let (name, csha) = (c["name"].as_str().unwrap_or(""), c["sha256"].as_str().unwrap_or(""));
                    let p = bin.join("ext.d").join(set).join(format!("{name}.wasm"));
                    let f = File::open(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                    let bytes = f.metadata().map(|m| m.len()).unwrap_or(0);
                    let put = KernelRequest::StorePut(init_proto::root_link::StorePutArgs { kind: init_proto::root_link::StoreKind::Ext, name: Some(name.to_string()), sha256: csha.to_string(), bytes, job: None });
                    self.link.call_with_fd(&put, f.as_fd(), crate::link::STORE_TIMEOUT).map_err(|e| format!("store.put {name}: {e}"))?;
                }
            }
        }
        let from = self.m.lkg_sha.clone();
        let mut data = json!({"sha": sha, "from_sha": from, "gate_job": job, "scope": scope, "lkg": true, "body_binary": sha256, "ext_set": ext_set, "components": components});
        // M7 (kernel/gate.md §3.9, root/root.md §3.10.5): a `lineage_gain` accept carries
        // `milestone_required`; the Root counts these promotions as the ratchet span's unconfirmed
        // accepts (build/status/ROOT-S1a.md). The flag is read from the job's own gate record.
        if crate::promote::g4v2::milestone_required(&self.home, job) {
            data["milestone_required"] = json!(true);
        }
        let lineage_seq = if change_close {
            None
        } else {
            Some(self.outbox.post_wait("promotion", data).map_err(|e| format!("promotion{{lkg:true}}: {e}"))?.seq)
        };
        crate::binstore::install_lkg(&bin).map_err(|e| format!("LKG install: {e}"))?;
        if let Some(set) = ext_set.as_str() {
            if let Err(e) = crate::extset::point(&bin, "ext.lkg", set) {
                self.alert("lineage_cas", &format!("bin/ext.lkg: {e}"));
            }
        }
        let prev: Option<LkgFile> = std::fs::read(rel(&self.home, pf::paths::LKG)).ok().and_then(|b| pf::decode(&b).ok());
        let sha12: String = sha.chars().take(12).collect();
        let l = LkgFile {
            v: V1,
            sha: sha.to_string(),
            binary: "bin/init-body.lkg".into(),
            binary_sha256: sha256,
            tree: format!("lineage/trees/{sha12}"),
            reason,
            lineage_seq,
            seq: prev.as_ref().map(|p| p.seq + 1).unwrap_or(1),
            prev_sha: Some(from.clone()),
            gate_job: Some(job.to_string()),
            committed_at: crate::clock::now_rfc3339(),
            model: Some(pf::ModelRef { id: self.cfg.model.id.clone(), provider: self.cfg.model.provider.clone() }),
            ext_set: ext_set.as_str().map(str::to_string),
        };
        fsx::write_json(&rel(&self.home, pf::paths::LKG), &l).map_err(|e| format!("lkg.json: {e}"))?;
        if let Err(e) = self.lineage.set_lkg(sha) {
            self.alert("lineage_cas", &format!("update-ref lkg: {e}"));
        }
        if self.home.join("lineage/body.git").is_dir() {
            let git = crate::git::Git::body(&self.cfg.supervise.git, &self.home);
            let trees = self.home.join("lineage/trees");
            let dir = trees.join(&sha12);
            if !dir.exists() {
                if let Err(e) = git.export_tree(sha, &dir) {
                    crate::klog::log("supervise", &format!("export lineage/trees/{sha12}: {e}"));
                }
            }
            let old12: String = from.chars().take(12).collect();
            if old12 != sha12 && old12.len() == 12 && old12.bytes().all(|b| b.is_ascii_hexdigit()) {
                let _ = std::fs::remove_dir_all(trees.join(&old12)); // exact path: the old LKG tree
            }
        }
        let acts = self.m.on(Ev::LkgCommitted { sha: sha.to_string() }, Now::real());
        self.exec(acts);
        let _ = self.outbox.local_only("lkg", json!({"sha": sha, "job": job, "lineage_seq": lineage_seq, "reason": reason}));
        self.notice("lkg", json!({"sha": sha}));
        self.persist();
        Ok(())
    }

    fn spawn(&mut self, plan: SpawnPlan) {
        self.boot_seq += 1;
        let boot_id = crate::sys::random_hex(8);
        let session = crate::layout::read_session(&self.home).unwrap_or_else(crate::layout::new_session_id);
        let bf = self.boot_file(&plan, &boot_id, self.boot_seq, &session);
        if let Err(e) = fsx::write_json(&rel(&self.home, pf::paths::BOOT), &bf) {
            crate::klog::log("supervise", &format!("boot.json: {e}"));
        }
        let mode = match plan.mode {
            InitMode::Normal => "normal",
            InitMode::Probe => "probe",
            InitMode::Safe => "safe",
        };
        let reason = serde_json::to_value(plan.reason).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let test_vars = spawnspec::own_test_vars();
        let run_dir = self.home.join("run");
        let env = spawnspec::body_env(&spawnspec::BodyEnv {
            root: &self.home,
            run_dir: &run_dir,
            session: &session,
            sha: &plan.sha,
            boot_id: &boot_id,
            boot_reason: &reason,
            body_id: "b0",
            mode,
            kernel_version: KERNEL_VERSION,
            sandbox: &self.cfg.sandbox,
            test_mode: self.cfg.test_mode,
            test_vars: &test_vars,
            access: self.access.as_ref(),
        });
        self.spawned_access = self.access.clone();
        let req = SpawnRequest {
            kind: InstanceKind::Body,
            profile: "body".into(),
            params: spawnspec::body_params(&self.home, &BodyLane::Live, &self.cfg.sandbox),
            program: self.home.join("bin").join(plan.bin.file_name()),
            args: vec![],
            env,
            cwd: self.home.join("work"),
            lane: Some("b0".into()),
            replaces: plan.replaces.clone(),
        };
        let pb = PendingBoot { boot_id, boot_seq: self.boot_seq, sha: plan.sha.clone(), reason: reason.clone(), started: Instant::now() };
        self.pending_inst.insert(plan.tag, pb.clone());
        let sp = self.spawner.clone();
        let tx = self.tx.clone();
        let tag = plan.tag;
        std::thread::spawn(move || {
            let result = sp.spawn(&req);
            let _ = tx.send(DEv::Spawned { tag, boot: pb, result });
        });
    }

    fn boot_file(&self, plan: &SpawnPlan, boot_id: &str, boot_seq: u64, session: &str) -> BootFile {
        let (inputs, notices) = self.inbox.counts();
        let lkg12: String = self.m.lkg_sha.chars().take(12).collect();
        let (release, arch) = crate::sys::uname();
        let money = self.ceilings.as_ref().and_then(|c| c.get("money")).cloned().unwrap_or(Value::Null);
        let budget = pf::EnvBudget {
            cap_musd: money.get("day_limit_musd").and_then(Value::as_u64).unwrap_or(0),
            day_starts_at: money.get("day_starts_at").and_then(Value::as_str).unwrap_or("").to_string(),
            today_musd: money.get("day_spent_musd").and_then(Value::as_u64).unwrap_or(0),
        };
        let root = json!({
            "level": self.level,
            "max_level": self.ledger.as_ref().and_then(|l| l.get("max_level")).cloned().unwrap_or(json!(self.boot.ladder.cap)),
            "next": self.ledger.as_ref().and_then(|l| l.get("next")).cloned().unwrap_or(Value::Null),
            "ceilings": self.ceilings.clone().unwrap_or(Value::Null),
            "classes": self.ceilings.as_ref().and_then(|c| c.get("classes")).cloned().unwrap_or(json!([])),
            "audited_good": self.boot.boot.tuple.audited.clone(),
        });
        let provisional = match &self.m.mode {
            Mode::Provisional(p) => Some(pf::ProvisionalInfo {
                depth: p.depth,
                g4: Some("queued".into()),
                probation_until: p.probation_until.map(|u| crate::clock::format_ms(crate::clock::now_ms() + u.saturating_duration_since(Instant::now()).as_millis() as i64)),
                since: crate::clock::format_ms(p.since_ms),
            }),
            _ => None,
        };
        let safe = match &self.m.mode {
            Mode::Safe(s) => Some(json!({"entered_at": crate::clock::format_ms(s.entered_ms), "reason": s.reason, "system": s.system, "crashes": s.crashes.iter().map(|t| crate::clock::format_ms(*t)).collect::<Vec<_>>()})),
            _ => None,
        };
        let zone_reason = self.boot.boot.reason.as_str().to_string();
        let cause = boot_cause(plan, &self.boot.boot.reason, self.boot.boot.cause.as_ref());
        BootFile {
            v: V1,
            body_id: "b0".into(),
            boot_id: boot_id.into(),
            boot_seq,
            branch: plan.branch.clone(),
            cause,
            crash: plan.crash.clone(),
            env: pf::BootEnv {
                budget,
                init_home: self.home.to_string_lossy().into_owned(),
                kernel_version: KERNEL_VERSION.into(),
                mem_total_mb: crate::sys::mem_total_mb(),
                model: pf::EnvModel { context_window: self.cfg.model.context_window, id: self.cfg.model.id.clone(), max_output: self.cfg.model.max_output, provider: self.cfg.model.provider.clone() },
                network: pf::EnvNetwork { egress: vec![], mode: "proxy-only".into() },
                os: pf::EnvOs { arch: if arch == "arm64" || arch == "aarch64" { "arm64".into() } else { arch }, family: if cfg!(target_os = "macos") { "macos".into() } else { "linux".into() }, version: release },
                root,
                sandbox: sandbox_status(),
            },
            // EVO-LITE (evolution-v2 §3.4.4, §4.6): the engine line's counts, from the last mine.
            evolution: kernel_evo::mine::boot_summary(&self.home),
            from_sha: plan.from_sha.clone(),
            kernel: pf::BootKernel { proto: pf::ProtoVersions { initctl: init_proto::PROTO, root: init_proto::ROOT_PROTO, ui: 1 }, version: KERNEL_VERSION.into() },
            kernel_upgrade: None,
            lkg_sha: self.m.lkg_sha.clone(),
            lkg_tree: format!("lineage/trees/{lkg12}"),
            mode: plan.mode,
            model: pf::BootModel { descriptor: self.cfg.model.descriptor.clone(), effort: self.cfg.model.effort.clone(), model: self.cfg.model.id.clone(), provider: self.cfg.model.provider.clone() },
            panic: self.boot.boot.panic.as_ref().and_then(|p| serde_json::from_value(p.clone()).ok()),
            pending: pf::Pending { inputs, notices },
            provisional,
            reason: plan.reason,
            restarts_since_lkg: self.m.counters.restarts_since_lkg,
            safe,
            selfmap: json!({}),
            session: session.into(),
            sha: plan.sha.clone(),
            spawned_at: crate::clock::now_rfc3339(),
            update_failed: if self.update_failed_delivered { None } else { self.update_failed.clone() },
            zone: pf::ZoneInfo { boot_reason: zone_reason, kernel_sha256: self.boot.boot.tuple_kernel.init_kernel.clone() },
        }
    }

    fn persist(&self) {
        self.persist_with_phase(None);
    }

    fn persist_with_phase(&self, phase_override: Option<&str>) {
        let bin = self.home.join("bin");
        let bref = |name: &str, built_from: &str| crate::binstore::sha_of(&bin, name).map(|sha256| pf::BinaryRef { built_from: built_from.to_string(), sha256 });
        let handoff = self.m.handoff.as_ref().map(|h| {
            let to = match &h.kind {
                crate::machine::HandoffKind::Promote { sha, .. } => sha.clone(),
                crate::machine::HandoffKind::Rollback { to, .. } => self.m.rollback_target(to).1,
            };
            pf::HandoffRecord {
                bid: "b0".into(),
                from: h.old_sha.clone(),
                new_instance: h.new.clone(),
                old_instance: h.old.clone(),
                phase: phase_override.unwrap_or(h.phase.name()).to_string(),
                staged_sha256: self.staged_sha256.clone().unwrap_or_default(),
                started_at: crate::clock::format_ms(h.started_ms),
                to,
                token: h.token.clone(),
            }
        });
        let body = self.m.primary.as_ref().and_then(|p| {
            self.bodies.get(&p.id).map(|b| pf::SupervisedBody {
                boot_id: b.boot_id.clone(),
                boot_seq: b.boot_seq,
                instance: p.id.clone(),
                pid: b.pid,
                ready_at: b.ready_at.map(|r| crate::clock::format_ms(crate::clock::now_ms() - r.elapsed().as_millis() as i64)),
                started_at: b.started_at.clone(),
            })
        });
        let body = body.or_else(|| {
            // Keep boot_seq monotonic across restarts even with no body.
            Some(pf::SupervisedBody { boot_id: String::new(), boot_seq: self.boot_seq, instance: String::new(), pid: 0, ready_at: None, started_at: String::new() })
        });
        let instances = self
            .bodies
            .iter()
            .map(|(id, b)| pf::InstanceRecord { id: id.clone(), kind: InstanceKind::Body, lane: Some("b0".into()), mark: None, pgid: Some(b.pid), pid: b.pid, pidns_ino: None, started_at: b.started_at.clone() })
            .collect();
        let c = &self.m.crash;
        let f = SupervisorFile {
            v: V1,
            binaries: pf::Binaries {
                current: bref("init-body", self.built_from.as_deref().unwrap_or(&self.m.current.sha)),
                ext: None,
                lkg: bref("init-body.lkg", &self.m.lkg_sha),
                prev: self.m.prev.as_ref().and_then(|p| bref("init-body.prev", &p.sha)),
                staged: self.staged_sha256.as_ref().and_then(|_| bref("init-body.staged", "")),
            },
            body,
            counters: self.m.counters.clone(),
            crash: pf::CrashState {
                backoff_until: match &self.m.slot {
                    Slot::Backoff { until, .. } => Some(crate::clock::format_ms(crate::clock::now_ms() + until.saturating_duration_since(Instant::now()).as_millis() as i64)),
                    _ => None,
                },
                consecutive: c.consecutive,
                last_exit: c.last_exit.map(|(at, code, signal)| pf::LastExit { at: crate::clock::format_ms(at), code, signal }),
                lkg_window: c.lkg_window.iter().map(|t| crate::clock::format_ms(*t)).collect(),
            },
            current: pf::SupervisorCurrent { provisional: self.m.current.provisional, sha: self.m.current.sha.clone(), since: self.kernel_started.clone() },
            fleet: None,
            handoff,
            instances,
            kernel: pf::SupervisorKernel { pid: std::process::id() as i32, started_at: self.kernel_started.clone(), version: KERNEL_VERSION.into() },
            lkg: pf::SupervisorLkg { sha: self.m.lkg_sha.clone() },
            mode: self.m.mode.name(),
            pipeline: pf::PipelineState { job: None, state: "idle".into() },
            root: pf::SupervisorRoot {
                boot_reason: self.boot.boot.reason.as_str().to_string(),
                last_lineage_seq: self.outbox.last_root_seq(),
                level: self.level,
                mode: if self.boot.boot.mode == ZoneMode::Safe { "safe".into() } else { "normal".into() },
                outbox_unanswered: self.outbox.unanswered() as u64,
            },
            slot: self.m.slot.name(),
            sweep: Sweep::Full,
        };
        if let Err(e) = fsx::write_json(&rel(&self.home, pf::paths::SUPERVISOR), &f) {
            crate::klog::log("supervise", &format!("supervisor.json: {e}"));
        }
    }
}

fn new_update_failed(prev: Option<&UpdateFailedFile>, stage: UpdateFailedStage, reason: &str, sha: &str, from_sha: Option<String>) -> UpdateFailedFile {
    UpdateFailedFile {
        v: V1,
        at: crate::clock::now_rfc3339(),
        exit_code: None,
        from_sha,
        job: None,
        log_path: None,
        log_tail: String::new(),
        reason: reason.to_string(),
        report: None,
        seq: prev.map(|p| p.seq + 1).unwrap_or(1),
        sha: sha.to_string(),
        signal: None,
        stage,
    }
}

/// `boot.json.env.sandbox` (kernel/sandbox.md §4.2); `initctl status` carries the same object.
pub fn sandbox_status() -> pf::SandboxStatus {
    let mac = cfg!(target_os = "macos");
    pf::SandboxStatus {
        hidden: vec!["cache/".into(), "fleet/".into(), "probe/".into(), "vault/".into()],
        kind: if mac { "seatbelt".into() } else { "bwrap".into() },
        landlock: None,
        landlock_abi: None,
        network: "kernel-sockets-only".into(),
        read_only: vec!["bin/".into(), "kernel/".into(), "lineage/".into(), "root/".into(), "state/kernel/".into()],
        seccomp: !mac,
        sweep: Sweep::Full,
        write: vec!["body/".into(), "state/".into(), "work/".into()],
        zone: pf::SandboxZone { launcher: "root".into(), network: "root-egress-only".into() },
    }
}

/// A fixed `hb` (install mode): `{slot, mode:"normal", busy:true}` every `every`.
pub(crate) fn start_hb(link: Arc<RootLink>, every: Duration, slot: &str) {
    let hb = Arc::new(Mutex::new(HbShared { data: Some(HbData { busy: true, mode: "normal".into(), sha12: String::new(), slot: slot.into() }), stop: false }));
    start_hb_thread(link, hb, every);
}

fn start_hb_thread(link: Arc<RootLink>, hb: Arc<Mutex<HbShared>>, every: Duration) {
    std::thread::Builder::new()
        .name("hb".into())
        .spawn(move || {
            loop {
                let (data, stop) = {
                    let g = hb.lock().unwrap();
                    (g.data.clone(), g.stop)
                };
                if stop || !link.is_alive() {
                    return;
                }
                let data = data.unwrap_or(HbData { busy: false, mode: "normal".into(), sha12: String::new(), slot: "cold_start".into() });
                link.event(&KernelEvent::Hb(data));
                std::thread::sleep(every);
            }
        })
        .expect("spawn hb thread");
}

/// A bounded ring of recent seqs, oldest first (the `state` frame's STEER lists).
fn push_ring(r: &mut std::collections::VecDeque<u64>, seq: u64, max: usize) {
    r.push_back(seq);
    while r.len() > max {
        r.pop_front();
    }
}

/// `boot.json.cause` of a spawn: the plan's own cause (a restart, or the Root's extend.rollback as
/// `extend_<signal>`); else the zone's demotion or whole-system rollback, but only on a spawn that
/// replaces no running body (the zone's first body, or a crash restart). A later handoff (a G4 or
/// `initctl` rollback, a promote) used to repeat that zone cause, which by then was stale (demo
/// 2026-10-25, F4: "Cause: ceiling_hit: ?. Lineage record 21" after the owner's `/rollback`).
fn boot_cause(plan: &SpawnPlan, zone: &ZoneBootReason, zone_cause: Option<&Value>) -> Option<pf::BootCause> {
    if let Some(kind) = &plan.cause {
        return Some(pf::BootCause { kind: kind.clone(), trigger: None, seq: None, evidence: None });
    }
    match zone {
        ZoneBootReason::Demotion | ZoneBootReason::RollbackSystem if plan.replaces.is_none() => Some(pf::BootCause {
            kind: zone.as_str().to_string(),
            trigger: zone_cause.and_then(|c| c.get("trigger")).cloned(),
            seq: zone_cause.and_then(|c| c.get("seq")).and_then(Value::as_u64),
            evidence: None,
        }),
        _ => None,
    }
}


/// Where the kernel measures free space for `disk.min_free_build_gib` and `min_free_run_gib`: `INIT_HOME/cache`, where
/// its builds write, then INIT_HOME. On Linux the kernel's zone binds INIT_HOME's subtrees (cache among them), not
/// INIT_HOME itself, which sits on the zone's own tmpfs, so a statvfs of INIT_HOME measured that tmpfs (REL-050: with
/// INIT_HOME under /tmp, a 64 MiB tmpfs, every promote was refused `E_DISK` with 32 GB free on the disk).
pub(crate) fn disk_probe_paths(home: &Path) -> (PathBuf, PathBuf) {
    (home.join("cache"), home.to_path_buf())
}

#[cfg(test)]
mod boot_cause_tests {
    use super::*;
    use crate::machine::BinSel;

    fn plan(replaces: Option<&str>, cause: Option<&str>) -> SpawnPlan {
        SpawnPlan { tag: 2, bin: BinSel::Lkg, sha: "a".repeat(40), reason: BootReason::Rollback, mode: InitMode::Normal, replaces: replaces.map(str::to_string),
                    crash: None, branch: None, from_sha: Some("b".repeat(40)), cause: cause.map(str::to_string) }
    }

    /// F4: the zone booted after a demotion (ceiling_hit, lineage record 21); later the owner's
    /// `/rollback` (H3) rolls a change group back. The rollback's boot names H3, not the demotion.
    #[test]
    fn f4_a_later_rollback_never_repeats_the_zone_cause() {
        let zone = json!({"trigger": "ceiling_hit", "seq": 21});
        let first = boot_cause(&plan(None, None), &ZoneBootReason::Demotion, Some(&zone)).unwrap();
        assert_eq!((first.kind.as_str(), first.trigger, first.seq), ("demotion", Some(json!("ceiling_hit")), Some(21)));
        let h3 = boot_cause(&plan(Some("b0-1"), Some("extend_H3")), &ZoneBootReason::Demotion, Some(&zone)).unwrap();
        assert_eq!((h3.kind.as_str(), h3.trigger, h3.seq), ("extend_H3", None, None));
        assert_eq!(boot_cause(&plan(Some("b0-1"), None), &ZoneBootReason::Demotion, Some(&zone)), None, "a G4 or initctl rollback after it");
        assert_eq!(boot_cause(&plan(Some("b0-1"), None), &ZoneBootReason::RollbackSystem, Some(&zone)), None);
        assert_eq!(boot_cause(&plan(None, None), &ZoneBootReason::Cold, None), None);
        let restart = boot_cause(&plan(None, Some("workspace_change")), &ZoneBootReason::Cold, None).unwrap();
        assert_eq!(restart.kind, "workspace_change");
    }
}

#[cfg(test)]
mod disk_probe_tests {
    use super::*;

    /// REL-050: free space is read where the builds write (INIT_HOME/cache, a bound subtree of the Linux kernel zone),
    /// with INIT_HOME only as the fallback.
    #[test]
    fn free_space_is_measured_where_builds_write() {
        let (first, then) = disk_probe_paths(Path::new("/home/o/.init"));
        assert_eq!(first, Path::new("/home/o/.init/cache"));
        assert_eq!(then, Path::new("/home/o/.init"));
    }
}
