//! The seam between the initctl server and the rest of the kernel.
//!
//! kernel-serve owns the wire (framing, envelope, peer check, lanes, limits), the job model, the
//! state gating of initctl.md §3.5, the level checks, the serve-side quotas, path hardening and
//! `kernel propose`'s files. Everything that touches the supervisor, lineage, the gate or the
//! Root is behind [`Backend`], which kernel-supervise (WP4) and kernel-gate (WP8) implement.
//!
//! Contract for implementors:
//! - Every method is called on a connection thread; it may block (a Root request, a fetch), but
//!   long work belongs in a job.
//! - A job command (`reload`, `gate`, `promote`, `rollback`, `vendor`, `task_submit`,
//!   `fleet_spawn`, `kernel_promote`) receives a [`JobCtl`] for a job that serve already reserved.
//!   The method runs the **synchronous part** (initctl.md §4.1: fetch, kernel checks, validator)
//!   and returns `Err` for any failure there, which becomes the response; the job is then
//!   discarded. On `Ok(Begun::Started(..))` the backend keeps the `JobCtl` and drives the job from
//!   its own threads: progress, stage results, the milestones, and exactly one terminal call.
//! - Serve has already applied the lane matrix (§3.4), every row of §3.5 it can see in
//!   [`KernelView`], the level column, the serve-side quotas and path hardening when a backend
//!   method runs. A level-gated method is never called below its level, so nothing below the
//!   level reaches the Root (§3.5, supervise.md §3.16).
//! - Default methods answer `E_INTERNAL` ("not implemented in this kernel build"), so a backend
//!   can grow command by command.

use crate::jobs::JobCtl;
use init_proto::ErrorBody;
use init_proto::errors::{BusyReason, ErrorCode};
use init_proto::initctl::*;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Which lane instance a request came from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LaneCtx {
    pub lane: Lane,
    /// `b0` on the live lane, the member's `bid`, or the probe id.
    pub id: String,
    /// Probe lanes only: `probe` (G2, G3) or `run` (a G4 task run).
    pub purpose: Option<ProbePurpose>,
}

impl LaneCtx {
    /// The primary's lane (`b0`).
    pub fn live() -> LaneCtx {
        LaneCtx { lane: Lane::Live, id: "b0".into(), purpose: None }
    }

    /// A fleet member's lane.
    pub fn member(bid: &str) -> LaneCtx {
        LaneCtx { lane: Lane::Member, id: bid.into(), purpose: None }
    }

    /// A probe instance's lane.
    pub fn probe(id: &str, purpose: ProbePurpose) -> LaneCtx {
        LaneCtx { lane: Lane::Probe, id: id.into(), purpose: Some(purpose) }
    }

    /// The key that owns jobs, limits and the socket: `live:b0`, `member:<bid>`, `probe:<id>`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.lane.as_str(), self.id)
    }
}

/// A lane socket to bind (initctl.md §3.1, §3.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneSpec {
    pub ctx: LaneCtx,
    /// `run/kernel.sock`, `run/fleet/<bid>/kernel.sock` or `probe/<id>/run/kernel.sock`.
    pub socket: PathBuf,
    /// The base of relative path arguments: `INIT_HOME`, or the probe root.
    pub home: PathBuf,
    /// Path arguments (`gate.repo`, `task.submit.dir`, `kernel.propose.patch`) must resolve, with
    /// no symlink, under one of these: `body/` and `work/`, or the probe root.
    pub allowed: Vec<PathBuf>,
}

impl LaneSpec {
    /// The live lane of `home`: `run/kernel.sock`, paths under `body/` or `work/`.
    pub fn live(home: &Path) -> LaneSpec {
        LaneSpec {
            ctx: LaneCtx::live(),
            socket: home.join("run/kernel.sock"),
            home: home.to_path_buf(),
            allowed: vec![home.join("body"), home.join("work")],
        }
    }

    /// A member lane: `run/fleet/<bid>/kernel.sock`; paths inside the member's root.
    pub fn member(home: &Path, bid: &str, member_root: &Path) -> LaneSpec {
        LaneSpec {
            ctx: LaneCtx::member(bid),
            socket: home.join("run/fleet").join(bid).join("kernel.sock"),
            home: home.to_path_buf(),
            allowed: vec![member_root.to_path_buf()],
        }
    }

    /// A probe lane: `probe/<id>/run/kernel.sock`; paths inside the probe root.
    pub fn probe(home: &Path, id: &str, purpose: ProbePurpose) -> LaneSpec {
        let root = home.join("probe").join(id);
        LaneSpec {
            ctx: LaneCtx::probe(id, purpose),
            socket: root.join("run/kernel.sock"),
            home: root.clone(),
            allowed: vec![root],
        }
    }
}

/// A handoff, rollback or pipeline that blocks `promote` and `rollback` (§3.5 `E_BUSY`), when
/// it is not an initctl job serve already knows about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusyInfo {
    pub job: Option<String>,
    pub reason: BusyReason,
}

/// The `parallel_bodies` ceiling and the bodies counted against it (the primary counts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyCount {
    pub limit: u32,
    pub used: u32,
}

/// What serve needs to know to apply initctl.md §3.5 before any work, for one lane. A snapshot,
/// taken once per request from memory (no disk, no Root round trip).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelView {
    /// The control link to the Root is up.
    pub link_up: bool,
    /// Age of the cached level and ceilings in seconds (boot block, `level`/`ceilings` events,
    /// `ledger.get`/`ceilings.get`); `None` when none was ever received. Stale after 120 s.
    pub ledger_age_s: Option<u64>,
    /// The Root's level from that cache.
    pub level: Option<u8>,
    /// The Root outbox is backed up (supervise.md §3.13.3).
    pub outbox_backlog: bool,
    /// The kernel's own `Halted{reason}`.
    pub halted: Option<String>,
    /// `Stopping` (Root `stop` or `prepare_swap`), or shutting down.
    pub stopping: bool,
    /// This lane's body is in safe mode, or the system is (boot block).
    pub safe_mode: bool,
    /// The primary is provisional.
    pub primary_provisional: bool,
    /// This lane's provisional depth and `max_provisional_depth` (3): `promote` is refused at
    /// `depth >= max`.
    pub provisional_depth: u32,
    pub max_provisional_depth: u32,
    /// Free space below `disk.min_free_build_gib`.
    pub disk_low: bool,
    /// A handoff, rollback or pipeline serve does not see as an initctl job (an owner `/reject`,
    /// a G4 failure at idle, a thinning baseline). Active `promote`/`rollback` jobs are counted
    /// by serve itself.
    pub busy: Option<BusyInfo>,
    /// A swapped kernel awaits its sealed audit, or a kernel candidate is in flight outside an
    /// initctl job (evolution.md §3.15.6). Active `kernel.promote` jobs are counted by serve.
    pub kernel_candidate: Option<String>,
    /// `parallel_bodies`; `None` when unknown (then `fleet spawn` is refused with `E_ROOT`).
    pub bodies: Option<BodyCount>,
    /// ADR-011: the Root runs charter `extend.mode = "free"` (members from L0, §4.5).
    pub extend_free: bool,
}

impl Default for KernelView {
    /// A healthy kernel at L0 with a fresh ledger.
    fn default() -> KernelView {
        KernelView {
            link_up: true,
            ledger_age_s: Some(0),
            level: Some(0),
            outbox_backlog: false,
            halted: None,
            stopping: false,
            safe_mode: false,
            primary_provisional: false,
            provisional_depth: 0,
            max_provisional_depth: 3,
            disk_low: false,
            busy: None,
            kernel_candidate: None,
            bodies: Some(BodyCount { limit: 1, used: 1 }),
            extend_free: false,
        }
    }
}

/// The Root's `ledger.get` and `ceilings.get` results, passed through unchanged (initctl.md
/// `level`), with the age of the copy.
#[derive(Clone, Debug, PartialEq)]
pub struct LedgerSnapshot {
    pub ledger: Value,
    pub ceilings: Value,
    pub age_s: u64,
}

/// The outcome of a job command's synchronous part.
#[derive(Clone, Debug, PartialEq)]
pub enum Begun {
    /// The job was accepted. The fields go into the immediate result next to `job` and `state`:
    /// `sha` for `promote`, `gate` and `kernel.promote`; `from` and `to` for `rollback`; `bid`
    /// for `fleet.spawn`; nothing for `reload`.
    Started(Map<String, Value>),
    /// The request duplicates an active job (idempotency, initctl.md §4.1 `promote`): answer with
    /// that job instead. The reserved job is discarded.
    Existing(String),
}

impl Begun {
    /// `Started` with no fields.
    pub fn started() -> Begun {
        Begun::Started(Map::new())
    }

    /// Add a result field (only on `Started`).
    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Begun {
        if let Begun::Started(m) = &mut self {
            m.insert(key.to_string(), value.into());
        }
        self
    }
}

/// `E_INTERNAL` for a command this backend does not implement yet.
pub fn not_implemented(cmd: CommandName) -> ErrorBody {
    ErrorBody::new(ErrorCode::Internal, format!("`{cmd}` is not implemented in this kernel build"))
}

/// The kernel behind the initctl server. See the module docs for the contract.
#[allow(unused_variables)]
pub trait Backend: Send + Sync + 'static {
    // ---- state ------------------------------------------------------------------------------

    /// The §3.5 snapshot for `lane` (member-scoped and probe-scoped where it matters: `safe_mode`
    /// and `provisional_depth` are the lane body's).
    fn view(&self, lane: &LaneCtx) -> KernelView;

    /// Instance marks of the lane's current instances, `INIT_HOME/run/mark/<instance id>` from the
    /// Root's `spawn` responses (macOS peer attribution, §3.3). During an overlapped handoff both
    /// the old and the new body's marks.
    fn marks(&self, lane: &LaneCtx) -> Vec<PathBuf> {
        Vec::new()
    }

    // ---- read commands ----------------------------------------------------------------------

    /// `status`. Serve overwrites `lane`, `jobs` (this lane's active jobs) and
    /// `constitution_sha256` (computed at server start); the backend fills everything else.
    fn status(&self, lane: &LaneCtx) -> Result<StatusResult, ErrorBody>;

    /// The cached ledger and ceilings with their age, if any. `level` answers from it when it is
    /// younger than 60 s.
    fn ledger(&self) -> Option<LedgerSnapshot>;

    /// Ask the Root (`ledger.get` + `ceilings.get`) and return the fresh copy (age 0). Called only
    /// with the link up. Any failure becomes `E_ROOT`.
    fn refresh_ledger(&self) -> Result<LedgerSnapshot, ErrorBody>;

    // ---- job commands (synchronous part; the backend then drives `job`) ---------------------

    /// `reload`: send the body `{"t":"reload","job","kind":"request"}`, call
    /// `job.awaiting_boundary()`, and finish on `reload_done` (or `E_TIMEOUT` after 1800 s).
    /// `E_NO_BODY` when no body runs. Serve already returns an active reload job of the same lane
    /// (idempotent) and refuses `wait:true` on the live lane.
    fn reload(&self, lane: &LaneCtx, args: &ReloadArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        Err(not_implemented(CommandName::Reload))
    }

    /// `gate --quick`; `repo` is resolved and checked by serve. The terminal result is a
    /// [`GateResult`] (`pass:false` is a success).
    fn gate(&self, lane: &LaneCtx, args: &GateArgs, repo: &Path, job: JobCtl) -> Result<Begun, ErrorBody> {
        Err(not_implemented(CommandName::Gate))
    }

    /// `promote` (`ref`, or `sha` from L2 on the live lane). Synchronous part: fetch, kernel
    /// checks, evolution validator; return `Begun::Started` with `sha`. Then build, G0–G3[, G4],
    /// `job.awaiting_boundary()` (send `pause` only after it returns), handoff, terminal.
    fn promote(&self, lane: &LaneCtx, args: &PromoteArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        Err(not_implemented(CommandName::Promote))
    }

    /// `promote --check` (`check: true`): promote's synchronous commit checks only (fetch, candidate checks, the
    /// validator under the current `extend.mode`, the scope); the answer is the result, no job starts.
    fn promote_check(&self, lane: &LaneCtx, args: &PromoteArgs) -> Result<Value, ErrorBody> {
        Err(not_implemented(CommandName::Promote))
    }

    /// `rollback`: return `from`/`to` (or `E_ALREADY_LKG`), then `job.awaiting_boundary()`.
    fn rollback(&self, lane: &LaneCtx, args: &RollbackArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        Err(not_implemented(CommandName::Rollback))
    }

    /// `vendor` (initctl.md §4.1 steps 1–8; [`crate::vendor`] has the manifest checks). The
    /// terminal result is a [`VendorResult`].
    fn vendor(&self, lane: &LaneCtx, args: &VendorArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        Err(not_implemented(CommandName::Vendor))
    }

    /// `task submit` (an `admit` job); `dir` is resolved and checked by serve. The terminal result
    /// is a [`TaskSubmitResult`] (`admitted:false` is a success).
    fn task_submit(&self, lane: &LaneCtx, args: &TaskSubmitArgs, dir: &Path, job: JobCtl) -> Result<Begun, ErrorBody> {
        Err(not_implemented(CommandName::TaskSubmit))
    }

    /// `fleet spawn`; return `bid`. Serve has checked the level, `fleet_spawns_day` and
    /// `parallel_bodies` (no `spawn` reaches the Root past the ceiling).
    fn fleet_spawn(&self, lane: &LaneCtx, args: &FleetSpawnArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        Err(not_implemented(CommandName::FleetSpawn))
    }

    /// `kernel promote` (L3): K0–K3, `store.put`, `system.swap`, `job.awaiting_root(..)`, then
    /// `job.awaiting_swap()` on `gog_pass` or `job.fail(E_GATE_FAILED{stage:"gate_of_gates"})`.
    fn kernel_promote(&self, lane: &LaneCtx, args: &KernelPromoteArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        Err(not_implemented(CommandName::KernelPromote))
    }

    /// `job.cancel` was accepted: serve has already marked the record `cancelled`; kill the
    /// job's instances through the Root's `kill`. Later `JobCtl` calls are ignored.
    fn cancel(&self, lane: &LaneCtx, job: &str) {}

    // ---- immediate commands -----------------------------------------------------------------

    /// `task template`: a skeleton family.
    fn task_template(&self, lane: &LaneCtx) -> Result<Value, ErrorBody> {
        Err(not_implemented(CommandName::TaskTemplate))
    }

    /// `task list` (L1).
    fn task_list(&self, lane: &LaneCtx) -> Result<TaskListResult, ErrorBody> {
        Err(not_implemented(CommandName::TaskList))
    }

    /// `task show` (L1): visible instances only.
    fn task_show(&self, lane: &LaneCtx, args: &TaskShowArgs) -> Result<Value, ErrorBody> {
        Err(not_implemented(CommandName::TaskShow))
    }

    /// `budget set` (L1) → `ProxyHandle::set_budgets`; `E_BUDGET` on a bound violation.
    fn budget_set(&self, lane: &LaneCtx, args: &BudgetSetArgs) -> Result<BudgetSetResult, ErrorBody> {
        Err(not_implemented(CommandName::BudgetSet))
    }

    /// `model set` (L1); `E_ARGS` when the charter does not allow the model.
    fn model_set(&self, lane: &LaneCtx, args: &ModelSetArgs) -> Result<ModelSetResult, ErrorBody> {
        Err(not_implemented(CommandName::ModelSet))
    }

    /// `fleet list`.
    fn fleet_list(&self, lane: &LaneCtx) -> Result<FleetListResult, ErrorBody> {
        Err(not_implemented(CommandName::FleetList))
    }

    /// `fleet send` (L2).
    fn fleet_send(&self, lane: &LaneCtx, args: &FleetSendArgs) -> Result<FleetSendResult, ErrorBody> {
        Err(not_implemented(CommandName::FleetSend))
    }

    /// `fleet stop` (any level).
    fn fleet_stop(&self, lane: &LaneCtx, args: &FleetStopArgs) -> Result<FleetStopResult, ErrorBody> {
        Err(not_implemented(CommandName::FleetStop))
    }

    /// `kernel status` (L3). Serve fills nothing; the backend reads its job records through
    /// [`crate::JobStore`] if it wants the candidate.
    fn kernel_status(&self, lane: &LaneCtx) -> Result<KernelStatusResult, ErrorBody> {
        Err(not_implemented(CommandName::KernelStatus))
    }

    /// `kernel propose`: send `proposal.file{title, text}` to the Root and return its id. Serve
    /// has read the patch (`O_NOFOLLOW`, ≤ 1 MiB, UTF-8), built `text` (≤ 64 KiB), checked the
    /// 5-per-day quota, and writes `state/kernel/proposals/kp-NNNN/` after this returns.
    fn file_proposal(&self, lane: &LaneCtx, title: &str, text: &str) -> Result<String, ErrorBody> {
        Err(not_implemented(CommandName::KernelPropose))
    }

    /// `grant` (ADR-011 §7.4): relay `grant.request{kind, value, duration_h?, why}` to the Root
    /// and return its answer (`{id, status:"pending"}`). Only the owner decides.
    /// `goal` (ADR-013): relay `goal.*` / `card.*` to the Root and return its answer.
    fn goal(&self, lane: &LaneCtx, args: &init_proto::initctl::GoalArgs) -> Result<Value, ErrorBody> {
        let _ = (lane, args);
        Err(not_implemented(CommandName::Goal))
    }

    fn grant_request(&self, lane: &LaneCtx, args: &GrantArgs) -> Result<GrantResult, ErrorBody> {
        Err(not_implemented(CommandName::Grant))
    }

    // ---- the evolution engine (evolution-v2 §4.1; lane EVO-LITE) -----------------------------

    /// `evo frictions`, `evo propose`, `evo variant`, `evo status`, `evo abandon`: immediate
    /// engine commands. For `propose` and `variant`, serve has resolved the file argument (§7)
    /// and read it (`O_NOFOLLOW`, UTF-8, ≤ 64 KiB draft / ≤ 200 KiB patch) into `text`.
    fn evo(&self, lane: &LaneCtx, cmd: &Command, text: Option<&str>) -> Result<Value, ErrorBody> {
        Err(not_implemented(cmd.name()))
    }

    /// `evo screen`: the reject-only screen of a proposal's variants and the go/no-go, as a job.
    fn evo_screen(&self, lane: &LaneCtx, args: &EvoScreenArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        Err(not_implemented(CommandName::EvoScreen))
    }
}
