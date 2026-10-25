//! initctl commands, lanes, state gating and the job model (kernel/initctl.md §3.4–§4.4).
//!
//! Wire command names: single words as in the doc (`ping`, `promote`, …); the two-word commands
//! use dotted names like `job.status` (`task.submit`, `fleet.spawn`, `kernel.promote`, …).
//! [`CommandName::parse`] also accepts the space-separated form (`"task submit"`) and the aliases
//! `wait` (= `job.wait`) and `propose-kernel` (= `kernel.propose`).

use crate::envelope::{ErrorBody, Request};
use crate::errors::ErrorCode;
use crate::files::SandboxStatus;
use crate::icj::{self, V1};
use crate::validate;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------------------------
// Lanes and the command matrix (§3.4)
// ---------------------------------------------------------------------------------------------

/// A kernel socket lane (§3.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    /// `run/kernel.sock`: the primary body `b0`.
    Live,
    /// `run/fleet/<bid>/kernel.sock`: an L2 fleet member.
    Member,
    /// `probe/<id>/run/kernel.sock`: a probe instance.
    Probe,
}

impl Lane {
    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Live => "live",
            Lane::Member => "member",
            Lane::Probe => "probe",
        }
    }
}

/// The purpose of a probe lane: `probe` (G2, G3) or `run` (a G4 task run).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbePurpose {
    Probe,
    Run,
}

/// Every initctl command (§4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CommandName {
    Ping,
    Status,
    Level,
    Reload,
    Gate,
    Promote,
    Rollback,
    Vendor,
    TaskTemplate,
    TaskSubmit,
    TaskList,
    TaskShow,
    BudgetSet,
    ModelSet,
    FleetList,
    FleetSpawn,
    FleetSend,
    FleetStop,
    KernelPropose,
    KernelPromote,
    KernelStatus,
    JobStatus,
    JobWait,
    JobList,
    JobCancel,
    EvoFrictions,
    EvoPropose,
    EvoVariant,
    EvoScreen,
    EvoStatus,
    EvoAbandon,
    Grant,
    Goal,
}

impl CommandName {
    /// Every command, in §3.4 table order.
    pub const ALL: &'static [CommandName] = &[
        CommandName::Ping,
        CommandName::Status,
        CommandName::Level,
        CommandName::Reload,
        CommandName::Gate,
        CommandName::Promote,
        CommandName::Rollback,
        CommandName::Vendor,
        CommandName::TaskTemplate,
        CommandName::TaskSubmit,
        CommandName::TaskList,
        CommandName::TaskShow,
        CommandName::BudgetSet,
        CommandName::ModelSet,
        CommandName::FleetList,
        CommandName::FleetSpawn,
        CommandName::FleetSend,
        CommandName::FleetStop,
        CommandName::KernelPropose,
        CommandName::KernelPromote,
        CommandName::KernelStatus,
        CommandName::JobStatus,
        CommandName::JobWait,
        CommandName::JobList,
        CommandName::JobCancel,
        CommandName::EvoFrictions,
        CommandName::EvoPropose,
        CommandName::EvoVariant,
        CommandName::EvoScreen,
        CommandName::EvoStatus,
        CommandName::EvoAbandon,
        CommandName::Grant,
        CommandName::Goal,
    ];

    /// The canonical wire name.
    pub fn as_str(self) -> &'static str {
        use CommandName::*;
        match self {
            Ping => "ping",
            Status => "status",
            Level => "level",
            Reload => "reload",
            Gate => "gate",
            Promote => "promote",
            Rollback => "rollback",
            Vendor => "vendor",
            TaskTemplate => "task.template",
            TaskSubmit => "task.submit",
            TaskList => "task.list",
            TaskShow => "task.show",
            BudgetSet => "budget.set",
            ModelSet => "model.set",
            FleetList => "fleet.list",
            FleetSpawn => "fleet.spawn",
            FleetSend => "fleet.send",
            FleetStop => "fleet.stop",
            KernelPropose => "kernel.propose",
            KernelPromote => "kernel.promote",
            KernelStatus => "kernel.status",
            JobStatus => "job.status",
            JobWait => "job.wait",
            JobList => "job.list",
            JobCancel => "job.cancel",
            EvoFrictions => "evo.frictions",
            EvoPropose => "evo.propose",
            EvoVariant => "evo.variant",
            EvoScreen => "evo.screen",
            EvoStatus => "evo.status",
            EvoAbandon => "evo.abandon",
            Grant => "grant",
            Goal => "goal",
        }
    }

    /// Wire aliases (§4.1).
    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            CommandName::JobWait => &["wait"],
            CommandName::KernelPropose => &["propose-kernel"],
            _ => &[],
        }
    }

    /// Parse a wire name, an alias, or the space-separated form of a dotted name.
    pub fn parse(s: &str) -> Option<CommandName> {
        let norm = s.split_whitespace().collect::<Vec<_>>().join(".");
        CommandName::ALL
            .iter()
            .copied()
            .find(|c| c.as_str() == norm || c.aliases().contains(&norm.as_str()))
    }

    /// The minimum autonomy level (§3.4). `fleet.stop` is "any" and reports 0.
    pub fn min_level(self) -> u8 {
        use CommandName::*;
        match self {
            TaskSubmit | TaskList | TaskShow | BudgetSet | ModelSet => 1,
            FleetSpawn | FleetSend => 2,
            KernelPromote | KernelStatus => 3,
            _ => 0,
        }
    }

    /// True when the command needs a level above L0: refused locally with `E_LEVEL` below it,
    /// and with `E_ROOT` when the cached ledger is stale or the link is down (§3.5).
    pub fn level_gated(self) -> bool {
        self.min_level() > 0
    }

    /// The lane columns of §3.4. `true` may still be scoped (member-scoped `status`, a probe's
    /// `gate` limited to G0–G2); the server applies the scope.
    pub fn allowed_on(self, lane: Lane, purpose: Option<ProbePurpose>) -> bool {
        use CommandName::*;
        match lane {
            Lane::Live => true,
            Lane::Member => matches!(
                self,
                Ping | Status | Level | Reload | Gate | Promote | Vendor | JobStatus | JobWait | JobList | JobCancel
            ),
            Lane::Probe => match self {
                Ping | Status | Level | Reload | JobStatus | JobWait | JobList | JobCancel => true,
                Gate => purpose != Some(ProbePurpose::Run),
                _ => false,
            },
        }
    }

    /// Commands that need a Root request (§3.5 row 1): refused with `E_ROOT` while the control
    /// link is down. (`level` answers from its cache and returns `E_ROOT` itself when it can't.)
    pub fn needs_root(self) -> bool {
        use CommandName::*;
        matches!(self, Vendor | TaskSubmit | FleetSpawn | KernelPromote | KernelPropose | Grant | Goal)
    }

    /// Refused with `E_ROOT{reason:"outbox_backlog"}` while the outbox is backed up (§3.5 row 2).
    pub fn refused_on_outbox_backlog(self) -> bool {
        use CommandName::*;
        matches!(self, Promote | Vendor | TaskSubmit | FleetSpawn | KernelPromote)
    }

    /// Allowed while the kernel is `Halted` (§3.5 row 3); everything else gets `E_HALTED`.
    pub fn allowed_when_halted(self) -> bool {
        use CommandName::*;
        matches!(self, Ping | Status | Level | JobStatus | JobWait | JobList | JobCancel)
    }

    /// Refused with `E_SAFE_MODE` in body or system safe mode (§3.5 row 5).
    pub fn refused_in_safe_mode(self) -> bool {
        use CommandName::*;
        matches!(
            self,
            Promote | Gate | Reload | Vendor | Rollback | TaskSubmit | BudgetSet | ModelSet | FleetSpawn | FleetSend | KernelPromote | EvoPropose | EvoVariant | EvoScreen
        )
    }

    /// Refused with `E_PROVISIONAL` while the primary is provisional (§3.5 row 6).
    pub fn refused_when_provisional(self) -> bool {
        matches!(self, CommandName::Vendor | CommandName::KernelPromote)
    }

    /// Refused with `E_DISK` below `disk.min_free_build_gib` (§3.5 row 8).
    pub fn refused_on_low_disk(self) -> bool {
        use CommandName::*;
        matches!(self, Gate | Promote | Vendor | TaskSubmit | FleetSpawn | KernelPromote | EvoScreen)
    }
}

impl std::fmt::Display for CommandName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for CommandName {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CommandName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        CommandName::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("unknown command `{s}`")))
    }
}

// ---------------------------------------------------------------------------------------------
// The job model (§3.6, §4.3, §4.4)
// ---------------------------------------------------------------------------------------------

open_enum! {
    /// Job and progress states. Record states: `queued` `running` `awaiting_boundary`
    /// `awaiting_root` `awaiting_swap` `succeeded` `failed` `cancelled`; progress frames also use
    /// `gating` and `handing_off` (§4.3).
    pub enum JobState {
        Queued = "queued",
        Running = "running",
        Gating = "gating",
        AwaitingBoundary = "awaiting_boundary",
        HandingOff = "handing_off",
        AwaitingRoot = "awaiting_root",
        AwaitingSwap = "awaiting_swap",
        Succeeded = "succeeded",
        Failed = "failed",
        Cancelled = "cancelled",
    }
}

impl JobState {
    /// `succeeded`, `failed` or `cancelled`.
    pub fn is_terminal(&self) -> bool {
        matches!(self, JobState::Succeeded | JobState::Failed | JobState::Cancelled)
    }

    /// `wait` returns here as if the job were finished: terminal, `awaiting_boundary`, or
    /// `awaiting_swap` (§3.6 milestones).
    pub fn wait_returns(&self) -> bool {
        self.is_terminal() || matches!(self, JobState::AwaitingBoundary | JobState::AwaitingSwap)
    }

    /// `job.cancel` is allowed in `queued` and `running` (before `awaiting_boundary` or
    /// `awaiting_root`); `gating` is a running sub-state.
    pub fn cancellable(&self) -> bool {
        matches!(self, JobState::Queued | JobState::Running | JobState::Gating)
    }
}

open_enum! {
    /// Progress and stage-result stage names (§4.3).
    pub enum Stage {
        Fetch = "fetch",
        Validate = "validate",
        Build = "build",
        G0 = "G0",
        G1 = "G1",
        G2 = "G2",
        G3 = "G3",
        G4 = "G4",
        Vendor = "vendor",
        Registry = "registry",
        Pause = "pause",
        Handoff = "handoff",
        Admission = "admission",
        Register = "register",
        K0 = "K0",
        K1 = "K1",
        K2 = "K2",
        K3 = "K3",
        Store = "store",
        GateOfGates = "gate_of_gates",
        Prepare = "prepare",
        Installed = "installed",
        Ready = "ready",
        Probation = "probation",
        Member = "member",
    }
}

/// `msg` in a progress frame is at most 200 bytes.
pub const MAX_PROGRESS_MSG: usize = 200;

/// The `progress` object of a progress frame and of a job record (§4.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// Integer seconds or null.
    pub eta_s: Option<u64>,
    pub job: String,
    pub msg: String,
    /// 0–100, never decreasing within a job.
    pub pct: u8,
    pub stage: Option<Stage>,
    pub state: JobState,
    /// RFC 3339 UTC with milliseconds.
    pub ts: String,
}

impl Progress {
    /// A progress object; `msg` is cut to one line of 200 bytes and `pct` clamped to 100.
    pub fn new(job: &str, state: JobState, stage: Option<Stage>, pct: u8, eta_s: Option<u64>, msg: &str, ts: &str) -> Progress {
        Progress {
            eta_s,
            job: job.to_string(),
            msg: validate::one_line(msg, MAX_PROGRESS_MSG),
            pct: pct.min(100),
            stage,
            state,
            ts: ts.to_string(),
        }
    }
}

/// The `progress` object inside a job record (§4.4): a progress frame's fields without `job`,
/// which is the record's own `id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordProgress {
    pub eta_s: Option<u64>,
    pub msg: String,
    pub pct: u8,
    pub stage: Option<Stage>,
    pub state: JobState,
    pub ts: String,
}

impl From<Progress> for RecordProgress {
    fn from(p: Progress) -> RecordProgress {
        RecordProgress { eta_s: p.eta_s, msg: p.msg, pct: p.pct, stage: p.stage, state: p.state, ts: p.ts }
    }
}

impl RecordProgress {
    /// The progress frame object for job `job`.
    pub fn with_job(self, job: &str) -> Progress {
        Progress { eta_s: self.eta_s, job: job.to_string(), msg: self.msg, pct: self.pct, stage: self.stage, state: self.state, ts: self.ts }
    }
}

/// One gate stage's outcome (`stages[]` in `gate`, `promote`, job records). Durations are
/// integer ms; `code` is the stage's `gate.*` code when it failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageResult {
    pub stage: Stage,
    pub pass: bool,
    pub wall_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A job record, `state/kernel/jobs/<id>.json` (§4.4); also the result of `job.status`,
/// `job.wait`, `job.cancel` and the items of `job.list`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobRecord {
    pub v: V1,
    /// `j-` + 6-digit zero-padded sequence.
    pub id: String,
    /// The command that created the job (an initctl wire name, or a kernel-internal job kind
    /// such as an asynchronous G4).
    pub cmd: String,
    pub lane: Lane,
    pub state: JobState,
    pub args: Value,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub progress: Option<RecordProgress>,
    /// The command's result object once terminal.
    pub result: Option<Value>,
    /// Shape of §3.2's `error`.
    pub error: Option<ErrorBody>,
    pub sha: Option<String>,
    #[serde(default)]
    pub stages: Vec<StageResult>,
    /// `kernel promote` only: the Root's `s-NNNNNN`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_job: Option<String>,
    /// Why the job ended when no command error says it, e.g. `kernel_restart` (§3.7, §5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `job.wait` only: true when the wait timed out and this is the current record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timed_out: Option<bool>,
}

/// Kept job records (older ones are deleted by exact path).
pub const JOB_RECORDS_KEPT: usize = 200;

/// Format a job id: `j-` + 6-digit zero-padded sequence.
pub fn job_id(seq: u64) -> String {
    format!("j-{seq:06}")
}

// ---------------------------------------------------------------------------------------------
// Command arguments (§4.1)
// ---------------------------------------------------------------------------------------------

fn args_err(msg: impl AsRef<str>) -> ErrorBody {
    ErrorBody::new(ErrorCode::Args, msg)
}

fn cap(field: &str, s: &str, max: usize) -> Result<(), ErrorBody> {
    if s.len() > max {
        return Err(args_err(format!("`{field}` is {} bytes; the cap is {max}", s.len())));
    }
    Ok(())
}

fn non_empty(field: &str, s: &str, max: usize) -> Result<(), ErrorBody> {
    if s.is_empty() {
        return Err(args_err(format!("`{field}` must not be empty")));
    }
    cap(field, s, max)
}

/// Path arguments are at most 4096 bytes.
pub const MAX_PATH_BYTES: usize = 4096;
/// `rollback.reason` is at most 500 bytes.
pub const MAX_REASON_BYTES: usize = 500;
/// `kernel propose` title ≤ 200 bytes, rationale ≤ 8 KiB.
pub const MAX_TITLE_BYTES: usize = 200;
pub const MAX_RATIONALE_BYTES: usize = 8 * 1024;
/// `fleet send` text (and the `fleet spawn` prompt) ≤ 64 KiB.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// `job.wait` default and maximum timeout.
pub const WAIT_DEFAULT_S: u64 = 600;
pub const WAIT_MAX_S: u64 = 3600;
/// `job.list` default and maximum limit.
pub const JOB_LIST_DEFAULT: u32 = 20;
pub const JOB_LIST_MAX: u32 = 200;

/// `{}`: the args of `ping`, `status`, `level`, `task.template`, `task.list`, `fleet.list`,
/// `kernel.status`. Unknown fields are ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoArgs {}

/// `reload`: `{"wait"?: bool}` (default false; true on the live lane → `E_WOULD_DEADLOCK`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,
}

/// `gate` (quick only).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateArgs {
    /// Must be true.
    pub quick: bool,
    /// Relative to `INIT_HOME`, or absolute inside it.
    pub repo: String,
    /// A ref name or a 40-hex SHA.
    pub rev: String,
    /// Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,
}

/// `promote`: `{"ref": "evo/<id>"}` or (L2, live lane) `{"sha": "<40 hex>"}`, plus `wait`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromoteArgs {
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,
    /// `promote --check` (GBAL G7): run promote's commit checks (fetch, candidate checks, the validator, the scope) and
    /// answer at once; nothing is promoted and no job starts. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<bool>,
}

/// `rollback`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackArgs {
    /// ≤ 500 bytes.
    pub reason: String,
    /// Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,
}

/// `vendor.workspace`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VendorWorkspace {
    Body,
    Ext,
}

/// `vendor`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorArgs {
    /// `^[A-Za-z0-9_-]{1,64}$`.
    #[serde(rename = "crate")]
    pub krate: String,
    /// An exact semver.
    pub version: String,
    /// Default `body`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<VendorWorkspace>,
    /// Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,
}

/// `kernel.propose` (alias `propose-kernel`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelProposeArgs {
    /// ≤ 200 bytes.
    pub title: String,
    /// A path under `body/` or `work/` (the kernel reads it with `O_NOFOLLOW`).
    pub patch: String,
    /// ≤ 8 KiB.
    pub rationale: String,
}

/// `kernel.promote`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelPromoteArgs {
    /// `^kevo/[a-z0-9][a-z0-9._-]{0,62}$`.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,
}

/// The engine's `k_variants` (evolution-v2 §4.5).
pub const EVO_K_MAX: u32 = 3;

/// `p-` + 6 digits (evolution-v2 §4.2).
fn evo_proposal_id(s: &str) -> Result<(), ErrorBody> {
    if s.len() == 8 && s.starts_with("p-") && s[2..].bytes().all(|b| b.is_ascii_digit()) {
        Ok(())
    } else {
        Err(args_err("`proposal` must be a proposal id such as p-000007"))
    }
}

/// `evo.frictions` (evolution-v2 §4.1): the digest, or with `all` the full index.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvoFrictionsArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<bool>,
}

/// `evo.propose`: a JSON draft (evolution-v2 §3.4.7) under `body/` or `work/` (read `O_NOFOLLOW`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvoProposeArgs {
    pub file: String,
}

/// `evo.variant`: a unified diff against lineage `main`, under `body/` or `work/`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvoVariantArgs {
    pub proposal: String,
    pub k: u32,
    pub patch: String,
}

/// `evo.screen` (a job; default `wait` false).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvoScreenArgs {
    /// The proposal to screen; empty with `pool_repeats` (pool calibration).
    pub proposal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,
    /// REAL-M7 (M7 line C): set → no proposal screen; the incumbent runs `pool_repeats` times on the pool
    /// (`pool_limit` tasks, 0 = all eligible) to seed the pool cache and the miner (`initctl evo calibrate`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_repeats: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_limit: Option<u32>,
}

/// `evo.status`: one proposal, or all.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvoStatusArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<String>,
}

/// `evo.abandon`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvoAbandonArgs {
    pub proposal: String,
    /// ≤ 500 bytes; its alphanumeric head names the state (`abandoned:<reason>`).
    pub reason: String,
}

/// `grant` (ADR-011 §7.4): ask the owner for a grant. The kernel relays it to the Root as
/// `grant.request`; the Root stores it, alerts the owner and answers `{id, status:"pending"}`.
/// Nothing changes until the owner types `/grant <id>` (typed confirmation) or `/deny <id>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantArgs {
    /// A §7.4 kind: `money.day_usd`, `extend.open_groups`, `dep`, `actions.<class>`, …
    pub kind: String,
    /// The value in the kind's shape (a decimal string for money, an integer, or `dep`'s crate list).
    pub value: Value,
    /// Hours, 1 to 8,760; the Root's default is 30 days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_h: Option<u64>,
    /// Why (≤ 4,096 bytes): the task, the limit hit, the evidence. Kept off the lineage.
    pub why: String,
}

/// `goal <op> [key=value …]` (ADR-013, GBAL): the agent's side of an owner goal, relayed to the Root as
/// `goal.status`, `goal.propose`, `goal.predict`, `goal.stop`, `card.read` or `card.measure`. The Root validates the
/// arguments; the agent can never set a goal (only the owner's `/goal` line does).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalArgs {
    /// `status`, `propose`, `predict`, `stop`, `read` or `measure`.
    pub op: String,
    #[serde(default)]
    pub args: Value,
}

/// The Root command of a `goal` op.
pub fn goal_cmd(op: &str) -> Option<&'static str> {
    Some(match op {
        "status" => "goal.status",
        "propose" => "goal.propose",
        "predict" => "goal.predict",
        "stop" => "goal.stop",
        "read" => "card.read",
        "measure" => "card.measure",
        _ => return None,
    })
}

/// `grant`: the Root's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantResult {
    pub id: String,
    pub status: String,
}

/// `task.submit`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSubmitArgs {
    /// A path under `work/` or `body/`.
    pub dir: String,
    /// Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,
}

/// `task.show`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskShowArgs {
    pub family: String,
}

/// A `budget set` value: an integer (micro-USD, permille, counts) or a decimal string as in
/// `config.toml` (ICJ forbids floats on the wire).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BudgetValue {
    Int(i64),
    Text(String),
}

/// `budget.set`: `{"set": {<key>: <value>…}}`, keys and bounds per evolution.md §3.14.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetSetArgs {
    pub set: BTreeMap<String, BudgetValue>,
}

/// `model.set`: `{"model": "<provider>/<id>"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSetArgs {
    pub model: String,
}

/// `fleet.spawn`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetSpawnArgs {
    /// `main` (default) or a 40-hex SHA under `refs/init/archive/` or `refs/init/qualified/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_share_musd_day: Option<u64>,
    /// ADR-011 §1.1: the sub-agent spec `agents/<agent>/AGENT.md` (role prompt, tool subset, model
    /// route, budget share) the member starts with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// `fleet.send`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetSendArgs {
    pub bid: String,
    /// ≤ 64 KiB.
    pub text: String,
}

/// `fleet.stop`: `bid` is a member id or `"all"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetStopArgs {
    pub bid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `job.status`, `job.cancel`: `{"job"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobArgs {
    pub job: String,
}

/// `job.wait` (alias `wait`): exactly one of `job` or `last:true`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobWaitArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<bool>,
    /// With `last`: the newest job of this command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    /// Default 600, max 3600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u64>,
}

impl JobWaitArgs {
    /// The effective timeout.
    pub fn timeout(&self) -> u64 {
        self.timeout_s.unwrap_or(WAIT_DEFAULT_S)
    }
}

/// `job.list` `state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobListState {
    Active,
    All,
}

/// `job.list`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobListArgs {
    /// Default `active`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<JobListState>,
    /// Default 20, ≤ 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A parsed, validated initctl command.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Ping,
    Status,
    Level,
    Reload(ReloadArgs),
    Gate(GateArgs),
    Promote(PromoteArgs),
    Rollback(RollbackArgs),
    Vendor(VendorArgs),
    TaskTemplate,
    TaskSubmit(TaskSubmitArgs),
    TaskList,
    TaskShow(TaskShowArgs),
    BudgetSet(BudgetSetArgs),
    ModelSet(ModelSetArgs),
    FleetList,
    FleetSpawn(FleetSpawnArgs),
    FleetSend(FleetSendArgs),
    FleetStop(FleetStopArgs),
    KernelPropose(KernelProposeArgs),
    KernelPromote(KernelPromoteArgs),
    KernelStatus,
    JobStatus(JobArgs),
    JobWait(JobWaitArgs),
    JobList(JobListArgs),
    JobCancel(JobArgs),
    EvoFrictions(EvoFrictionsArgs),
    EvoPropose(EvoProposeArgs),
    EvoVariant(EvoVariantArgs),
    EvoScreen(EvoScreenArgs),
    EvoStatus(EvoStatusArgs),
    EvoAbandon(EvoAbandonArgs),
    Grant(GrantArgs),
    Goal(GoalArgs),
}

fn decode<T: DeserializeOwned>(args: &Value) -> Result<T, ErrorBody> {
    serde_json::from_value(args.clone()).map_err(|e| args_err(format!("invalid args: {e}")))
}

impl Command {
    /// Parse `cmd` + `args`: `E_UNKNOWN_CMD` for an unknown name, `E_ARGS` for bad args.
    /// Unknown fields in `args` are ignored.
    pub fn parse(cmd: &str, args: &Value) -> Result<Command, ErrorBody> {
        let name = CommandName::parse(cmd)
            .ok_or_else(|| ErrorBody::new(ErrorCode::UnknownCmd, format!("unknown command `{}`", validate::one_line(cmd, 64))))?;
        if !args.is_object() {
            return Err(args_err("`args` must be an object"));
        }
        use CommandName as N;
        let c = match name {
            N::Ping => Command::Ping,
            N::Status => Command::Status,
            N::Level => Command::Level,
            N::Reload => Command::Reload(decode(args)?),
            N::Gate => Command::Gate(decode(args)?),
            N::Promote => Command::Promote(decode(args)?),
            N::Rollback => Command::Rollback(decode(args)?),
            N::Vendor => Command::Vendor(decode(args)?),
            N::TaskTemplate => Command::TaskTemplate,
            N::TaskSubmit => Command::TaskSubmit(decode(args)?),
            N::TaskList => Command::TaskList,
            N::TaskShow => Command::TaskShow(decode(args)?),
            N::BudgetSet => Command::BudgetSet(decode(args)?),
            N::ModelSet => Command::ModelSet(decode(args)?),
            N::FleetList => Command::FleetList,
            N::FleetSpawn => Command::FleetSpawn(decode(args)?),
            N::FleetSend => Command::FleetSend(decode(args)?),
            N::FleetStop => Command::FleetStop(decode(args)?),
            N::KernelPropose => Command::KernelPropose(decode(args)?),
            N::KernelPromote => Command::KernelPromote(decode(args)?),
            N::KernelStatus => Command::KernelStatus,
            N::JobStatus => Command::JobStatus(decode(args)?),
            N::JobWait => Command::JobWait(decode(args)?),
            N::JobList => Command::JobList(decode(args)?),
            N::JobCancel => Command::JobCancel(decode(args)?),
            N::EvoFrictions => Command::EvoFrictions(decode(args)?),
            N::EvoPropose => Command::EvoPropose(decode(args)?),
            N::EvoVariant => Command::EvoVariant(decode(args)?),
            N::EvoScreen => Command::EvoScreen(decode(args)?),
            N::EvoStatus => Command::EvoStatus(decode(args)?),
            N::EvoAbandon => Command::EvoAbandon(decode(args)?),
            N::Grant => Command::Grant(decode(args)?),
            N::Goal => Command::Goal(decode(args)?),
        };
        c.validate()?;
        Ok(c)
    }

    /// Parse a request's command.
    pub fn from_request(r: &Request) -> Result<Command, ErrorBody> {
        Command::parse(&r.cmd, &r.args)
    }

    /// The command's name.
    pub fn name(&self) -> CommandName {
        use CommandName as N;
        match self {
            Command::Ping => N::Ping,
            Command::Status => N::Status,
            Command::Level => N::Level,
            Command::Reload(_) => N::Reload,
            Command::Gate(_) => N::Gate,
            Command::Promote(_) => N::Promote,
            Command::Rollback(_) => N::Rollback,
            Command::Vendor(_) => N::Vendor,
            Command::TaskTemplate => N::TaskTemplate,
            Command::TaskSubmit(_) => N::TaskSubmit,
            Command::TaskList => N::TaskList,
            Command::TaskShow(_) => N::TaskShow,
            Command::BudgetSet(_) => N::BudgetSet,
            Command::ModelSet(_) => N::ModelSet,
            Command::FleetList => N::FleetList,
            Command::FleetSpawn(_) => N::FleetSpawn,
            Command::FleetSend(_) => N::FleetSend,
            Command::FleetStop(_) => N::FleetStop,
            Command::KernelPropose(_) => N::KernelPropose,
            Command::KernelPromote(_) => N::KernelPromote,
            Command::KernelStatus => N::KernelStatus,
            Command::JobStatus(_) => N::JobStatus,
            Command::JobWait(_) => N::JobWait,
            Command::JobList(_) => N::JobList,
            Command::JobCancel(_) => N::JobCancel,
            Command::EvoFrictions(_) => N::EvoFrictions,
            Command::EvoPropose(_) => N::EvoPropose,
            Command::EvoVariant(_) => N::EvoVariant,
            Command::EvoScreen(_) => N::EvoScreen,
            Command::EvoStatus(_) => N::EvoStatus,
            Command::EvoAbandon(_) => N::EvoAbandon,
            Command::Grant(_) => N::Grant,
            Command::Goal(_) => N::Goal,
        }
    }

    /// The `args` object.
    pub fn args(&self) -> Value {
        fn v<T: Serialize>(x: &T) -> Value {
            icj::to_value(x).expect("command args are ICJ")
        }
        match self {
            Command::Ping
            | Command::Status
            | Command::Level
            | Command::TaskTemplate
            | Command::TaskList
            | Command::FleetList
            | Command::KernelStatus => v(&NoArgs {}),
            Command::Reload(a) => v(a),
            Command::Gate(a) => v(a),
            Command::Promote(a) => v(a),
            Command::Rollback(a) => v(a),
            Command::Vendor(a) => v(a),
            Command::TaskSubmit(a) => v(a),
            Command::TaskShow(a) => v(a),
            Command::BudgetSet(a) => v(a),
            Command::ModelSet(a) => v(a),
            Command::FleetSpawn(a) => v(a),
            Command::FleetSend(a) => v(a),
            Command::FleetStop(a) => v(a),
            Command::KernelPropose(a) => v(a),
            Command::KernelPromote(a) => v(a),
            Command::JobStatus(a) | Command::JobCancel(a) => v(a),
            Command::JobWait(a) => v(a),
            Command::JobList(a) => v(a),
            Command::EvoFrictions(a) => v(a),
            Command::EvoPropose(a) => v(a),
            Command::EvoVariant(a) => v(a),
            Command::EvoScreen(a) => v(a),
            Command::EvoStatus(a) => v(a),
            Command::EvoAbandon(a) => v(a),
            Command::Grant(a) => v(a),
            Command::Goal(a) => v(a),
        }
    }

    /// The request frame for this command.
    pub fn to_request(&self, id: impl Into<String>) -> Request {
        Request { v: V1, id: id.into(), cmd: self.name().as_str().to_string(), args: self.args() }
    }

    /// The effective `wait` flag (the per-command defaults of §4.1).
    pub fn wait(&self) -> bool {
        match self {
            Command::Reload(a) => a.wait.unwrap_or(false),
            Command::Gate(a) => a.wait.unwrap_or(true),
            Command::Promote(a) => a.wait.unwrap_or(false),
            Command::Rollback(a) => a.wait.unwrap_or(false),
            Command::Vendor(a) => a.wait.unwrap_or(true),
            Command::TaskSubmit(a) => a.wait.unwrap_or(true),
            Command::KernelPromote(a) => a.wait.unwrap_or(false),
            Command::EvoScreen(a) => a.wait.unwrap_or(false),
            Command::JobWait(_) => true,
            _ => false,
        }
    }

    /// The argument rules of §4.1 (lane- and state-independent).
    pub fn validate(&self) -> Result<(), ErrorBody> {
        match self {
            Command::Gate(a) => {
                if !a.quick {
                    return Err(args_err("`quick` must be true: a full gate runs only through promote"));
                }
                non_empty("repo", &a.repo, MAX_PATH_BYTES)?;
                non_empty("rev", &a.rev, 255)
            }
            Command::Promote(a) if a.check == Some(true) && (a.sha.is_some() || a.wait.is_some()) => {
                Err(args_err("`check` takes only `ref` (evo/<id>): no `sha`, no `wait`"))
            }
            Command::Promote(a) => match (&a.git_ref, &a.sha) {
                (Some(r), None) if validate::is_evo_ref(r) => Ok(()),
                (Some(r), None) => Err(args_err(format!("`ref` must be evo/<id>, got `{}`", validate::one_line(r, 100)))),
                (None, Some(s)) if validate::is_sha40(s) => Ok(()),
                (None, Some(_)) => Err(args_err("`sha` must be 40 lowercase hex")),
                _ => Err(args_err("exactly one of `ref` or `sha` is required")),
            },
            Command::Rollback(a) => cap("reason", &a.reason, MAX_REASON_BYTES),
            Command::Vendor(a) => {
                if !validate::is_crate_name(&a.krate) {
                    return Err(args_err("`crate` must match ^[A-Za-z0-9_-]{1,64}$"));
                }
                if !validate::is_exact_semver(&a.version) {
                    return Err(args_err("`version` must be an exact semver such as 1.2.3"));
                }
                Ok(())
            }
            Command::TaskSubmit(a) => non_empty("dir", &a.dir, MAX_PATH_BYTES),
            Command::TaskShow(a) => non_empty("family", &a.family, 255),
            Command::BudgetSet(a) => {
                if a.set.is_empty() {
                    return Err(args_err("`set` must name at least one key"));
                }
                Ok(())
            }
            Command::ModelSet(a) => {
                if validate::is_model_ref(&a.model) {
                    Ok(())
                } else {
                    Err(args_err("`model` must be <provider>/<id>"))
                }
            }
            Command::FleetSpawn(a) => {
                if let Some(p) = &a.parent
                    && p != "main"
                    && !validate::is_sha40(p)
                {
                    return Err(args_err("`parent` must be `main` or a 40-hex SHA"));
                }
                non_empty("prompt", &a.prompt, MAX_TEXT_BYTES)
            }
            Command::FleetSend(a) => {
                non_empty("bid", &a.bid, 64)?;
                non_empty("text", &a.text, MAX_TEXT_BYTES)
            }
            Command::FleetStop(a) => {
                non_empty("bid", &a.bid, 64)?;
                a.reason.as_deref().map_or(Ok(()), |r| cap("reason", r, MAX_REASON_BYTES))
            }
            Command::KernelPropose(a) => {
                non_empty("title", &a.title, MAX_TITLE_BYTES)?;
                non_empty("patch", &a.patch, MAX_PATH_BYTES)?;
                cap("rationale", &a.rationale, MAX_RATIONALE_BYTES)
            }
            Command::KernelPromote(a) => {
                if validate::is_kevo_ref(&a.git_ref) {
                    Ok(())
                } else {
                    Err(args_err("`ref` must match ^kevo/[a-z0-9][a-z0-9._-]{0,62}$"))
                }
            }
            Command::JobStatus(a) | Command::JobCancel(a) => {
                if validate::is_job_id(&a.job) {
                    Ok(())
                } else {
                    Err(args_err("`job` must be a job id such as j-000043"))
                }
            }
            Command::JobWait(a) => {
                match (&a.job, a.last.unwrap_or(false)) {
                    (Some(j), false) if validate::is_job_id(j) => {}
                    (Some(_), false) => return Err(args_err("`job` must be a job id such as j-000043")),
                    (None, true) => {}
                    _ => return Err(args_err("exactly one of `job` or `last:true` is required")),
                }
                if a.cmd.is_some() && a.job.is_some() {
                    return Err(args_err("`cmd` filters `last` only"));
                }
                if let Some(c) = &a.cmd
                    && CommandName::parse(c).is_none()
                {
                    return Err(args_err(format!("unknown command `{}` in `cmd`", validate::one_line(c, 64))));
                }
                if a.timeout() > WAIT_MAX_S {
                    return Err(args_err(format!("`timeout_s` max is {WAIT_MAX_S}")));
                }
                Ok(())
            }
            Command::JobList(a) => {
                if a.limit.unwrap_or(JOB_LIST_DEFAULT) > JOB_LIST_MAX {
                    return Err(args_err(format!("`limit` max is {JOB_LIST_MAX}")));
                }
                Ok(())
            }
            Command::EvoPropose(a) => non_empty("file", &a.file, MAX_PATH_BYTES),
            Command::EvoVariant(a) => {
                evo_proposal_id(&a.proposal)?;
                if !(1..=EVO_K_MAX).contains(&a.k) {
                    return Err(args_err(format!("`k` must be 1 to {EVO_K_MAX}")));
                }
                non_empty("patch", &a.patch, MAX_PATH_BYTES)
            }
            Command::EvoScreen(a) => match a.pool_repeats {
                Some(r) if !(1..=6).contains(&r) => Err(args_err("`pool_repeats` must be 1 to 6")),
                Some(_) if !a.proposal.is_empty() => Err(args_err("`proposal` must be empty for a pool calibration")),
                Some(_) => Ok(()),
                None => evo_proposal_id(&a.proposal),
            },
            Command::EvoStatus(a) => a.proposal.as_deref().map_or(Ok(()), evo_proposal_id),
            Command::EvoAbandon(a) => {
                evo_proposal_id(&a.proposal)?;
                non_empty("reason", &a.reason, MAX_REASON_BYTES)
            }
            Command::Goal(a) if goal_cmd(&a.op).is_none() => Err(args_err("`op` is status, propose, predict, stop, read or measure")),
            Command::Goal(a) if !(a.args.is_object() || a.args.is_null()) => Err(args_err("`args` is an object")),
            Command::Goal(_) => Ok(()),
            Command::Grant(a) => {
                non_empty("kind", &a.kind, 64)?;
                if !a.kind.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b)) {
                    return Err(args_err("`kind` must be a grant kind like `money.day_usd` or `dep`"));
                }
                if a.value.is_null() {
                    return Err(args_err("`value` is required"));
                }
                if a.duration_h.is_some_and(|h| !(1..=8760).contains(&h)) {
                    return Err(args_err("`duration_h` must be 1 to 8760"));
                }
                non_empty("why", &a.why, 4096)
            }
            Command::Ping
            | Command::Status
            | Command::Level
            | Command::Reload(_)
            | Command::TaskTemplate
            | Command::TaskList
            | Command::FleetList
            | Command::KernelStatus
            | Command::EvoFrictions(_) => Ok(()),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Command results (§4.1)
// ---------------------------------------------------------------------------------------------

/// `ping`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingResult {
    pub kernel_version: String,
    pub lane: Lane,
    pub proto: u32,
    pub time: String,
}

/// `status.kernel`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusKernel {
    pub proto: u32,
    pub uptime_s: u64,
    pub version: String,
}

/// The supervisor slot (supervise.md §4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    ColdStart,
    Starting,
    Running,
    Handoff,
    RollingBack,
    Backoff,
    Stopped,
    DiskFull,
    Halted,
    Stopping,
}

/// The body mode (supervise.md §4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Normal,
    Provisional,
    Safe,
}

/// Instance sweep quality (supervise.md §3.8.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sweep {
    Full,
    Degraded,
}

/// `status.current`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusCurrent {
    pub depth: u32,
    pub provisional: bool,
    pub sha: String,
    pub since: String,
}

/// `status.lkg`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusLkg {
    pub seq: u64,
    pub sha: String,
    pub since: String,
}

/// Supervisor counters (`status.counters`, `supervisor.json.counters`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    pub crashes_total: u64,
    pub restarts_since_lkg: u64,
    pub rollbacks: u64,
    pub swaps_failed: u64,
    pub swaps_ok: u64,
}

/// `status.disk`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusDisk {
    pub free_mib: u64,
}

/// One active job in `status.jobs`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSummary {
    pub cmd: String,
    pub id: String,
    pub stage: Option<Stage>,
    pub state: JobState,
}

/// `status.budget`: integers, micro-USD. `day_*` are the Root's; `gate_*` are null when no gate
/// runs; `soft` is the proxy's soft budgets (`ProxyStatus`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusBudget {
    pub day_cap_musd: u64,
    pub day_spent_musd: u64,
    pub gate_cap_musd: Option<u64>,
    pub gate_spent_musd: Option<u64>,
    pub soft: BTreeMap<String, u64>,
}

/// `status.body`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusBody {
    /// `b0` or the member's `bid`.
    pub id: String,
    pub lane: Lane,
}

/// One member in `status.fleet.members`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusFleetMember {
    pub bid: String,
    pub busy: bool,
    pub line: String,
    pub qualified: u32,
    pub share_musd_day: u64,
    pub slot: Slot,
    pub spent_today_musd: u64,
    pub submissions: u32,
}

/// `status.fleet` (null when no member runs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusFleet {
    pub campaign: String,
    pub members: Vec<StatusFleetMember>,
    pub parallel_bodies: u32,
}

/// `status.link`: the control link's health.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusLink {
    pub boot_reason: String,
    pub ceilings_age_s: Option<u64>,
    pub connected: bool,
    pub outbox_unanswered: u64,
}

/// `status` (§4.1). `evolution`, `handoff`, `safe` and `root` are owned by other docs and
/// carried as JSON; `root` is the Root's published status relayed verbatim.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusResult {
    pub body: StatusBody,
    pub budget: StatusBudget,
    pub cache_hit_permille: Option<u32>,
    /// `sha256:<64 hex>` of `kernel/CONSTITUTION.md`.
    pub constitution_sha256: String,
    pub counters: Counters,
    pub current: StatusCurrent,
    pub disk: StatusDisk,
    pub evolution: Value,
    pub fleet: Option<StatusFleet>,
    pub handoff: Option<Value>,
    pub jobs: Vec<JobSummary>,
    pub kernel: StatusKernel,
    pub lane: Lane,
    pub link: StatusLink,
    pub lkg: StatusLkg,
    pub mode: Mode,
    pub root: Value,
    pub safe: Option<Value>,
    /// kernel/sandbox.md §4.2: "`initctl status` carries the same object as `sandbox`".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxStatus>,
    pub slot: Slot,
    pub sweep: Sweep,
}

/// `level`: the Root's `ledger.get` and `ceilings.get` results passed through unchanged, plus
/// the cache age. Typed views: [`crate::root_link::Ledger`], [`crate::root_link::Ceilings`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LevelResult {
    pub age_s: u64,
    pub ceilings: Value,
    pub ledger: Value,
}

/// `reload`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadResult {
    pub job: String,
    pub state: JobState,
}

/// The `result` of a finished reload job (from the body's `reload_done`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadOutcome {
    pub components: Vec<String>,
    pub error: Option<String>,
    pub ok: bool,
    pub prefix_changed: bool,
}

/// `gate` (quick). `pass:false` is still `ok:true` (CLI exit 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateResult {
    pub job: String,
    pub pass: bool,
    /// `state/kernel/gate/<job>/report.json`.
    pub report: String,
    pub sha: String,
    pub stages: Vec<StageResult>,
    pub warnings: Vec<String>,
}

/// `promote`: `{job, sha, state}`; with `wait:true` also `stages`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromoteResult {
    pub job: String,
    pub sha: String,
    pub state: JobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stages: Option<Vec<StageResult>>,
}

/// `rollback`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackResult {
    pub from: String,
    pub job: String,
    pub state: JobState,
    pub to: String,
}

/// One crate added by `vendor`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendoredCrate {
    pub checksum: String,
    pub name: String,
    pub version: String,
}

/// `vendor`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorResult {
    pub added: Vec<VendoredCrate>,
    #[serde(rename = "crate")]
    pub krate: String,
    pub job: String,
    pub lock: String,
    pub note: String,
    pub vendor_dir: String,
    pub version: String,
}

/// `kernel.propose`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelProposeResult {
    pub path: String,
    /// `kp-0007`.
    pub proposal: String,
    pub root_id: String,
    /// Always `filed`.
    pub status: String,
}

/// `kernel.promote`: `{job, sha, state}`; with `wait:true` also `root_job` and `stages`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelPromoteResult {
    pub job: String,
    pub sha: String,
    pub state: JobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_job: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stages: Option<Vec<StageResult>>,
}

/// `kernel.status.candidate`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelCandidate {
    pub job: String,
    pub root_job: Option<String>,
    pub sha: String,
    pub stages: Vec<StageResult>,
    pub state: JobState,
}

/// `kernel.status.running`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelRunning {
    pub built_from: String,
    /// `agent` or `maintainer`.
    pub source: String,
    pub version: String,
}

/// `kernel.status.last_audit`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastAudit {
    pub at: String,
    pub regression: bool,
}

/// `kernel.status`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelStatusResult {
    pub candidate: Option<KernelCandidate>,
    pub last_audit: Option<LastAudit>,
    pub running: KernelRunning,
}

/// One admission rule outcome in `task.submit`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCheck {
    pub detail: String,
    pub ok: bool,
    pub rule: String,
}

/// `task.submit` (kernel/gate.md Q-IC2). `sealed` is a count, never the ids. `admitted:false`
/// is still `ok:true` (CLI exit 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSubmitResult {
    pub admitted: bool,
    pub checks: Vec<TaskCheck>,
    pub family: String,
    pub job: String,
    /// The Root lineage `seq` of the registration; null when not admitted.
    pub seq: Option<u64>,
    pub sealed: u32,
    pub visible: Vec<String>,
}

/// One family in `task.list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFamilySummary {
    pub admitted_at: String,
    pub eligible_from_lkg_seq: u64,
    pub family: String,
    pub sealed: u32,
    pub seq: u64,
    pub size: u32,
    pub suspended: Option<String>,
}

/// `task.list`: the result is the array itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskListResult(pub Vec<TaskFamilySummary>);

/// `budget.set`: the budgets after `ProxyHandle::set_budgets` (kernel/proxy.md owns the keys).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BudgetSetResult {
    pub budgets: Value,
}

/// `model.set`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSetResult {
    pub model: String,
    pub thinning_job: Option<String>,
}

/// One member in `fleet.list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetMemberSummary {
    pub bid: String,
    pub busy: bool,
    pub line: String,
    pub qualified: u32,
    pub share_musd_day: u64,
    pub spent_today_musd: u64,
    pub submissions: u32,
}

/// `fleet.list` candidate kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    Archive,
    Qualified,
}

/// One candidate parent in `fleet.list` with its HGM score (integers only).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetCandidate {
    pub cmp_permille: Option<u32>,
    pub kind: CandidateKind,
    pub n_f: u32,
    pub n_s: u32,
    pub sha: String,
}

/// `fleet.list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetListResult {
    pub candidates: Vec<FleetCandidate>,
    pub members: Vec<FleetMemberSummary>,
    pub suggested_parent: Option<String>,
}

/// `fleet.spawn`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetSpawnResult {
    pub bid: String,
    pub job: String,
}

/// `fleet.send`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetSendResult {
    pub queued: bool,
}

/// `fleet.stop`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetStopResult {
    pub stopping: Vec<String>,
}

/// `job.list`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobListResult {
    pub jobs: Vec<JobRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_aliases_and_space_form() {
        for c in CommandName::ALL {
            assert_eq!(CommandName::parse(c.as_str()), Some(*c));
            assert_eq!(CommandName::parse(&c.as_str().replace('.', " ")), Some(*c));
        }
        assert_eq!(CommandName::parse("wait"), Some(CommandName::JobWait));
        assert_eq!(CommandName::parse("propose-kernel"), Some(CommandName::KernelPropose));
        assert_eq!(CommandName::parse("approve"), None);
        assert_eq!(CommandName::parse("panic"), None);
        assert_eq!(CommandName::ALL.len(), 33);
    }

    #[test]
    fn level_column() {
        use CommandName::*;
        let expect = [
            (Ping, 0),
            (Status, 0),
            (Level, 0),
            (Reload, 0),
            (Gate, 0),
            (Promote, 0),
            (Rollback, 0),
            (Vendor, 0),
            (TaskTemplate, 0),
            (TaskSubmit, 1),
            (TaskList, 1),
            (TaskShow, 1),
            (BudgetSet, 1),
            (ModelSet, 1),
            (FleetList, 0),
            (FleetSpawn, 2),
            (FleetSend, 2),
            (FleetStop, 0),
            (KernelPropose, 0),
            (KernelPromote, 3),
            (KernelStatus, 3),
            (JobStatus, 0),
            (JobWait, 0),
            (JobList, 0),
            (JobCancel, 0),
            (EvoFrictions, 0),
            (EvoPropose, 0),
            (EvoVariant, 0),
            (EvoScreen, 0),
            (EvoStatus, 0),
            (EvoAbandon, 0),
            (Grant, 0),
            (Goal, 0),
        ];
        assert_eq!(expect.len(), CommandName::ALL.len());
        for (c, l) in expect {
            assert_eq!(c.min_level(), l, "{c}");
        }
    }

    #[test]
    fn lane_columns() {
        use CommandName::*;
        let member_ok = [Ping, Status, Level, Reload, Gate, Promote, Vendor, JobStatus, JobWait, JobList, JobCancel];
        let probe_ok = [Ping, Status, Level, Reload, Gate, JobStatus, JobWait, JobList, JobCancel];
        for c in CommandName::ALL {
            assert!(c.allowed_on(Lane::Live, None), "{c} live");
            assert_eq!(c.allowed_on(Lane::Member, None), member_ok.contains(c), "{c} member");
            assert_eq!(c.allowed_on(Lane::Probe, Some(ProbePurpose::Probe)), probe_ok.contains(c), "{c} probe");
        }
        // IC-20: a task run must not start gates.
        assert!(!Gate.allowed_on(Lane::Probe, Some(ProbePurpose::Run)));
        assert!(Reload.allowed_on(Lane::Probe, Some(ProbePurpose::Run)));
    }

    #[test]
    fn state_gating_rows() {
        use CommandName::*;
        let safe: Vec<_> = CommandName::ALL.iter().filter(|c| c.refused_in_safe_mode()).copied().collect();
        assert_eq!(safe, vec![Reload, Gate, Promote, Rollback, Vendor, TaskSubmit, BudgetSet, ModelSet, FleetSpawn, FleetSend, KernelPromote, EvoPropose, EvoVariant, EvoScreen]);
        assert!(EvoScreen.refused_on_low_disk() && !EvoPropose.refused_on_low_disk());
        assert!(!EvoFrictions.needs_root() && !EvoPropose.level_gated());
        let halted_ok: Vec<_> = CommandName::ALL.iter().filter(|c| c.allowed_when_halted()).copied().collect();
        assert_eq!(halted_ok, vec![Ping, Status, Level, JobStatus, JobWait, JobList, JobCancel]);
        assert!(Vendor.refused_when_provisional() && KernelPromote.refused_when_provisional() && !Promote.refused_when_provisional());
        assert!(Gate.refused_on_low_disk() && !Rollback.refused_on_low_disk());
        assert!(Promote.refused_on_outbox_backlog() && !Gate.refused_on_outbox_backlog());
        assert!(FleetSpawn.needs_root() && !Promote.needs_root());
        assert!(!FleetStop.level_gated() && FleetSpawn.level_gated());
    }

    /// ADR-011 §7.4: `grant` relays a request to the Root (live lane, any level).
    #[test]
    fn grant_args() {
        let c = Command::parse("grant", &json!({"kind": "money.day_usd", "value": "2.00", "duration_h": 48, "why": "w"})).unwrap();
        assert_eq!(c.name(), CommandName::Grant);
        assert_eq!(c.args(), json!({"duration_h": 48, "kind": "money.day_usd", "value": "2.00", "why": "w"}));
        for bad in [
            json!({"kind": "Money", "value": 1, "why": "w"}),
            json!({"kind": "dep", "value": null, "why": "w"}),
            json!({"kind": "dep", "why": "w"}),
            json!({"kind": "dep", "value": 1, "why": ""}),
            json!({"kind": "dep", "value": 1, "why": "w", "duration_h": 0}),
        ] {
            assert_eq!(Command::parse("grant", &bad).unwrap_err().code, ErrorCode::Args, "{bad}");
        }
        assert!(CommandName::Grant.needs_root() && !CommandName::Grant.allowed_on(Lane::Member, None));
    }

    #[test]
    fn parse_and_validate() {
        let c = Command::parse("promote", &json!({"ref": "evo/pty-tool"})).unwrap();
        assert!(!c.wait());
        assert_eq!(c.to_request("5").to_line(), b"{\"args\":{\"ref\":\"evo/pty-tool\"},\"cmd\":\"promote\",\"id\":\"5\",\"v\":1}\n".to_vec());
        let e = |cmd: &str, a: Value| Command::parse(cmd, &a).unwrap_err().code;
        assert_eq!(e("approve", json!({})), ErrorCode::UnknownCmd);
        assert_eq!(e("promote", json!({})), ErrorCode::Args);
        assert_eq!(e("promote", json!({"ref": "evo/x", "sha": "4b1e0c2d9a7f3e5d1c0b9a8f7e6d5c4b3a291807"})), ErrorCode::Args);
        assert_eq!(e("promote", json!({"ref": "main"})), ErrorCode::Args);
        assert_eq!(e("gate", json!({"quick": false, "repo": "work/wt", "rev": "HEAD"})), ErrorCode::Args);
        assert_eq!(e("vendor", json!({"crate": "vt100", "version": "^0.16"})), ErrorCode::Args);
        assert_eq!(e("vendor", json!({"crate": "vt100", "version": "0.16.2", "workspace": "kernel"})), ErrorCode::Args);
        assert_eq!(e("kernel.promote", json!({"ref": "kevo/Bad"})), ErrorCode::Args);
        assert_eq!(e("evo.variant", json!({"proposal": "p-000001", "k": 4, "patch": "/h/work/v.diff"})), ErrorCode::Args);
        assert_eq!(e("evo.screen", json!({"proposal": "p-1"})), ErrorCode::Args);
        assert_eq!(e("evo.abandon", json!({"proposal": "p-000001", "reason": ""})), ErrorCode::Args);
        assert!(Command::parse("evo frictions", &json!({})).is_ok());
        assert_eq!(e("wait", json!({})), ErrorCode::Args);
        assert_eq!(e("wait", json!({"job": "j-000001", "last": true})), ErrorCode::Args);
        assert_eq!(e("wait", json!({"job": "j-000001", "timeout_s": 3601})), ErrorCode::Args);
        assert_eq!(e("job.list", json!({"limit": 201})), ErrorCode::Args);
        assert_eq!(e("rollback", json!({"reason": "x".repeat(501)})), ErrorCode::Args);
        assert_eq!(e("kernel propose", json!({"title": "t", "patch": "work/p.diff", "rationale": "x".repeat(8193)})), ErrorCode::Args);
        assert_eq!(e("model.set", json!({"model": "flash"})), ErrorCode::Args);
        assert_eq!(e("fleet.spawn", json!({"parent": "HEAD", "prompt": "go"})), ErrorCode::Args);
        // Unknown fields are ignored.
        assert!(Command::parse("ping", &json!({"extra": 1})).is_ok());
        assert!(Command::parse("gate", &json!({"quick": true, "repo": "work/wt", "rev": "HEAD", "x": 1})).unwrap().wait());
        let w = Command::parse("wait", &json!({"last": true, "cmd": "promote"})).unwrap();
        assert_eq!(w.name(), CommandName::JobWait);
        assert_eq!(w.to_request("9").cmd, "job.wait");
    }

    #[test]
    fn job_states() {
        assert!(JobState::AwaitingBoundary.wait_returns() && JobState::AwaitingSwap.wait_returns());
        assert!(!JobState::AwaitingRoot.wait_returns() && !JobState::Running.wait_returns());
        assert!(JobState::Cancelled.is_terminal());
        assert!(!JobState::AwaitingBoundary.cancellable() && JobState::Queued.cancellable());
        assert_eq!(serde_json::to_string(&JobState::parse("new_state")).unwrap(), "\"new_state\"");
        assert_eq!(job_id(43), "j-000043");
        let p = Progress::new("j-000001", JobState::Running, Some(Stage::G2), 140, None, &"m".repeat(300), "2026-10-19T12:00:00.000Z");
        assert_eq!((p.pct, p.msg.len()), (100, 200));
    }
}
