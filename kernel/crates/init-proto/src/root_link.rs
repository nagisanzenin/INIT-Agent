//! The Root↔kernel control link (root/root.md §4.1; kernel side: kernel/supervise.md §3.13).
//!
//! fd 3 carries ICJ lines (≤ 1 MiB): requests `{"args","cmd","id","v":1}` (ids `k-<n>` from the
//! kernel, `r-<n>` from the Root; pipelined, answered out of order), responses
//! `{"id","ok",…,"v":1}`, and events `{"data","ev","v":1}` (no id, no response). fd 4 carries
//! passed fds, each datagram tagged [`FdTag`] and sent before the response line with that id.
//!
//! root/root.md owns this protocol. Where kernel/supervise.md §3.13 disagrees (field names of
//! `lineage.append`, `system.swap` and the `level` event), root/root.md's names are used.

use crate::envelope::{ErrorBody, Event, Request};
use crate::errors::ErrorCode;
use crate::icj::{self, IcjError, V1};
use crate::stdio::Display;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// `hello` must be the first frame, within 30 s of spawn.
pub const HELLO_DEADLINE_S: u64 = 30;
/// The Root treats 30 s without `hb` as a kernel crash.
pub const HB_TIMEOUT_S: u64 = 30;
/// Unmatched fd datagrams are kept 60 s, at most 32.
pub const FD_HOLD_S: u64 = 60;
pub const FD_HOLD_MAX: usize = 32;
/// Kernel exit codes read by the Root (supervise.md §3.13.5).
pub mod kernel_exit {
    /// After `stop`.
    pub const STOPPED: i32 = 0;
    /// After `prepare_swap`.
    pub const SWAP: i32 = 75;
    /// Control link lost.
    pub const LINK_LOST: i32 = 69;
    /// `config.toml` invalid at start (`EX_CONFIG`).
    pub const CONFIG: i32 = 78;
}

/// The tag of an fd-channel datagram: `{"id","n"}` plus `SCM_RIGHTS`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FdTag {
    pub id: String,
    pub n: u32,
}

open_enum! {
    /// `root.*` error codes (root/root.md §4.1.2).
    pub enum RootErrorCode {
        Proto = "root.proto",
        Version = "root.version",
        UnknownCmd = "root.unknown_cmd",
        Args = "root.args",
        /// The level does not grant this; also a demoting ceiling hit.
        Level = "root.level",
        Mode = "root.mode",
        Quota = "root.quota",
        Busy = "root.busy",
        Rate = "root.rate",
        Store = "root.store",
        NotFound = "root.not_found",
        Lineage = "root.lineage",
        Internal = "root.internal",
    }
}

impl RootErrorCode {
    /// The `root.*` code carried by an error body, if it is one.
    pub fn of(e: &ErrorBody) -> RootErrorCode {
        RootErrorCode::parse(e.code.as_str())
    }

    /// An error body with this code (codes travel in the same envelope as initctl's).
    pub fn error(&self, message: impl AsRef<str>) -> ErrorBody {
        ErrorBody::new(ErrorCode::parse(self.as_str()), message)
    }
}

open_enum! {
    /// Root instance kinds (`spawn.kind`, `instances`, `supervisor.json.instances`).
    pub enum InstanceKind {
        Kernel = "kernel",
        Body = "body",
        Probe = "probe",
        Build = "build",
        Check = "check",
        GitRead = "git_read",
    }
}

// ---------------------------------------------------------------------------------------------
// Kernel → Root requests (root/root.md §4.1.3)
// ---------------------------------------------------------------------------------------------

/// `hello`: the first frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloArgs {
    pub kernel_version: String,
    pub proto: u32,
}

/// `spawn`. The Root answers `{instance, pid, started_at}` and passes 3 fds
/// `[stdin_w, stdout_r, stderr_r]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnArgs {
    pub kind: InstanceKind,
    /// A Root profile name (`body`, `build`, `git-read`, …).
    pub profile: String,
    pub params: BTreeMap<String, String>,
    pub program: String,
    pub args: Vec<String>,
    /// The Root refuses `INIT_ROOT_*` keys here (V4).
    pub env: BTreeMap<String, String>,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    /// The instance an overlapped-handoff B replaces (allowed one over `parallel_bodies`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
}

/// `spawn` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnResult {
    /// 12 hex.
    pub instance: String,
    pub pid: i32,
    pub started_at: String,
}

/// `kill.mode`: `term` = SIGTERM to the process group; `kill` = full sweep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillMode {
    Term,
    Kill,
}

/// `kill`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillArgs {
    pub instance: String,
    pub mode: KillMode,
}

/// `kill` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillResult {
    pub processes: u32,
}

/// One entry of the `instances` result (an array).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub instance: String,
    pub kind: InstanceKind,
    pub lane: Option<String>,
    pub pid: i32,
    pub started_at: String,
}

/// `store.put.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoreKind {
    #[serde(rename = "init-kernel")]
    InitKernel,
    #[serde(rename = "initctl")]
    Initctl,
    #[serde(rename = "init-body")]
    InitBody,
    #[serde(rename = "ext")]
    Ext,
    #[serde(rename = "kstate")]
    Kstate,
    /// Tuple v2 (M7; root.md §3.7.1): a pax tar of the body's T0 paths (ADR-011 adds `agents/**`).
    #[serde(rename = "t0")]
    T0,
}

/// `store.put` (+ 1 read-only fd).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorePutArgs {
    pub kind: StoreKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub sha256: String,
    pub bytes: u64,
    /// For `kstate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
}

/// `store.put` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorePutResult {
    pub sha256: String,
}

/// `store.get`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreGetArgs {
    pub sha256: String,
}

/// `store.get` result (+ 1 read-only fd).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreGetResult {
    pub bytes: u64,
}

/// `tuple.get` result: `current`, `lkg`, `audited` (ids and objects, root/root.md §3.7.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TupleGetResult {
    pub audited: Value,
    pub current: Value,
    pub lkg: Value,
}

/// `lineage.append`. `idem` = `"<boot_id>:<outbox_seq>"` (supervise.md calls it `dedup_key`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineageAppendArgs {
    pub kind: String,
    pub data: Value,
    pub idem: String,
}

/// `lineage.append` result. `dup` is true when `idem` was seen before (original `seq`/`hash`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageAppendResult {
    pub dup: bool,
    /// `sha256:<64 hex>`.
    pub hash: String,
    pub seq: u64,
}

/// `system.swap` (L3; root/root.md §3.8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SystemSwapArgs {
    /// Explicit admitted free-mode kernel group; absent for the legacy gated path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u64>,
    pub version: String,
    /// A bare repo under `INIT_HOME`.
    pub source_repo: String,
    pub source_sha: String,
    /// sha256 of the stored `init-kernel`.
    pub init_kernel: String,
    /// sha256 of the stored `initctl`.
    pub initctl: String,
    /// The K-stage summary (kernel/gate.md).
    pub report: Value,
}

/// `system.swap` result: the Root's job `s-NNNNNN`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemSwapResult {
    pub job: String,
}

/// `proposal.file`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalFileArgs {
    /// ≤ 200 bytes.
    pub title: String,
    /// ≤ 64 KiB.
    pub text: String,
}

/// `proposal.file` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalFileResult {
    pub id: String,
}

/// `sealed.holdout_rotate` result (+ one read-only fd per task bundle).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldoutRotateResult {
    pub count: u32,
}

/// Alert level (`alert.level`, `owner.out` notices).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertLevel {
    Info,
    Warn,
    Alert,
}

/// `alert` (≤ 30 per hour).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertArgs {
    pub code: String,
    pub level: AlertLevel,
    pub message: String,
}

/// `{}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

/// A kernel → Root request.
#[derive(Clone, Debug, PartialEq)]
pub enum KernelRequest {
    Hello(HelloArgs),
    Ready,
    Spawn(SpawnArgs),
    Kill(KillArgs),
    Instances,
    StorePut(StorePutArgs),
    StoreGet(StoreGetArgs),
    TupleGet,
    LineageAppend(LineageAppendArgs),
    LedgerGet,
    CeilingsGet,
    SystemSwap(SystemSwapArgs),
    SystemSwapStatus(SystemSwapStatusArgs),
    ProposalFile(ProposalFileArgs),
    /// root/sealed-eval.md §4.2 `FamilyEvalReq`, carried as JSON.
    SealedFamilyEval(Value),
    SealedHoldoutRotate,
    /// M7 G4 v2 (m7-design.md §4.1; pinned in build/status/ROOT-S1a.md): the Root-run sealed slice.
    /// Args and result are `kernel_gate::g4v2::{SliceArgs, SliceAnswer}`, carried as JSON here so
    /// this crate stays free of gate types. The answer comes back when the slice ends (≤ `wall_s`
    /// + about 60 s); other frames interleave meanwhile.
    SealedG4Slice(Value),
    Alert(AlertArgs),
    /// ADR-011 §2.3 step 2 (X2): `change.open{sha, from_sha, scope, candidate{body{binary, sha, t0},
    /// ext{components, set}, kernel?}, caps, prefix_change}` → `{seq, group, snapshot, window}`.
    /// Carried as JSON (the Root owns the shape; root/crates/init-root/src/sentinel/mod.rs).
    ChangeOpen(Value),
    /// ADR-011 H12: `change.veto{group, reason}` → `{}` (always honoured, never counted).
    ChangeVeto(ChangeVetoArgs),
    /// ADR-011 §7.4 (X7): `grant.request{kind, value, duration_h?, why}` → `{id, status:"pending"}`.
    /// The Root stores and alerts it; only the owner decides (`/grant <id>`, `/deny <id>`).
    /// Carried as JSON (the Root owns the kinds and value shapes: root src/grant.rs).
    GrantRequest(Value),
}

/// `change.veto` (ADR-011 §3.3 H12; A3 failures, tool-error monitors, the agent's own judgment).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemSwapStatusArgs { pub job: String }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeVetoArgs {
    pub group: u64,
    /// ≤ 200 characters; the Root strips control characters.
    pub reason: String,
}

fn link_args<T: DeserializeOwned>(cmd: &str, args: &Value) -> Result<T, ErrorBody> {
    serde_json::from_value(args.clone()).map_err(|e| RootErrorCode::Args.error(format!("{cmd}: {e}")))
}

fn to_args<T: Serialize>(x: &T) -> Value {
    icj::to_value(x).expect("link args are ICJ")
}

impl KernelRequest {
    /// The wire `cmd`.
    pub fn cmd(&self) -> &'static str {
        match self {
            KernelRequest::Hello(_) => "hello",
            KernelRequest::Ready => "ready",
            KernelRequest::Spawn(_) => "spawn",
            KernelRequest::Kill(_) => "kill",
            KernelRequest::Instances => "instances",
            KernelRequest::StorePut(_) => "store.put",
            KernelRequest::StoreGet(_) => "store.get",
            KernelRequest::TupleGet => "tuple.get",
            KernelRequest::LineageAppend(_) => "lineage.append",
            KernelRequest::LedgerGet => "ledger.get",
            KernelRequest::CeilingsGet => "ceilings.get",
            KernelRequest::SystemSwap(_) => "system.swap",
            KernelRequest::SystemSwapStatus(_) => "system.swap_status",
            KernelRequest::ProposalFile(_) => "proposal.file",
            KernelRequest::SealedFamilyEval(_) => "sealed.family_eval",
            KernelRequest::SealedHoldoutRotate => "sealed.holdout_rotate",
            KernelRequest::SealedG4Slice(_) => "sealed.g4_slice",
            KernelRequest::Alert(_) => "alert",
            KernelRequest::ChangeOpen(_) => "change.open",
            KernelRequest::ChangeVeto(_) => "change.veto",
            KernelRequest::GrantRequest(_) => "grant.request",
        }
    }

    /// The `args` object.
    pub fn args(&self) -> Value {
        match self {
            KernelRequest::Hello(a) => to_args(a),
            KernelRequest::Spawn(a) => to_args(a),
            KernelRequest::Kill(a) => to_args(a),
            KernelRequest::StorePut(a) => to_args(a),
            KernelRequest::StoreGet(a) => to_args(a),
            KernelRequest::LineageAppend(a) => to_args(a),
            KernelRequest::SystemSwap(a) => to_args(a),
            KernelRequest::SystemSwapStatus(a) => to_args(a),
            KernelRequest::ProposalFile(a) => to_args(a),
            KernelRequest::SealedFamilyEval(v) => v.clone(),
            KernelRequest::SealedG4Slice(v) => v.clone(),
            KernelRequest::Alert(a) => to_args(a),
            KernelRequest::ChangeOpen(v) => v.clone(),
            KernelRequest::ChangeVeto(a) => to_args(a),
            KernelRequest::GrantRequest(v) => v.clone(),
            KernelRequest::Ready
            | KernelRequest::Instances
            | KernelRequest::TupleGet
            | KernelRequest::LedgerGet
            | KernelRequest::CeilingsGet
            | KernelRequest::SealedHoldoutRotate => to_args(&Empty {}),
        }
    }

    /// Parse a request the Root received: `root.unknown_cmd` or `root.args` on failure.
    pub fn parse(cmd: &str, args: &Value) -> Result<KernelRequest, ErrorBody> {
        Ok(match cmd {
            "hello" => KernelRequest::Hello(link_args(cmd, args)?),
            "ready" => KernelRequest::Ready,
            "spawn" => KernelRequest::Spawn(link_args(cmd, args)?),
            "kill" => KernelRequest::Kill(link_args(cmd, args)?),
            "instances" => KernelRequest::Instances,
            "store.put" => KernelRequest::StorePut(link_args(cmd, args)?),
            "store.get" => KernelRequest::StoreGet(link_args(cmd, args)?),
            "tuple.get" => KernelRequest::TupleGet,
            "lineage.append" => KernelRequest::LineageAppend(link_args(cmd, args)?),
            "ledger.get" => KernelRequest::LedgerGet,
            "ceilings.get" => KernelRequest::CeilingsGet,
            "system.swap" => KernelRequest::SystemSwap(link_args(cmd, args)?),
            "system.swap_status" => KernelRequest::SystemSwapStatus(link_args(cmd, args)?),
            "proposal.file" => KernelRequest::ProposalFile(link_args(cmd, args)?),
            "sealed.family_eval" => KernelRequest::SealedFamilyEval(args.clone()),
            "sealed.holdout_rotate" => KernelRequest::SealedHoldoutRotate,
            "sealed.g4_slice" => KernelRequest::SealedG4Slice(args.clone()),
            "alert" => KernelRequest::Alert(link_args(cmd, args)?),
            "change.open" => KernelRequest::ChangeOpen(args.clone()),
            "change.veto" => KernelRequest::ChangeVeto(link_args(cmd, args)?),
            "grant.request" => KernelRequest::GrantRequest(args.clone()),
            other => {
                return Err(RootErrorCode::UnknownCmd.error(format!("unknown command `{}`", crate::validate::one_line(other, 64))));
            }
        })
    }

    /// The request frame with kernel id `k-<n>`.
    pub fn to_request(&self, n: u64) -> Request {
        Request { v: V1, id: format!("k-{n}"), cmd: self.cmd().to_string(), args: self.args() }
    }
}

// ---------------------------------------------------------------------------------------------
// The boot block (`hello` result)
// ---------------------------------------------------------------------------------------------

/// `boot.tuple`: tuple ids.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TupleIds {
    pub audited: Option<String>,
    pub current: Option<String>,
    pub lkg: Option<String>,
}

/// `{name, sha256}` of a component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentRef {
    pub name: String,
    pub sha256: String,
}

/// `tuple_body.ext`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TupleExt {
    pub components: Vec<ComponentRef>,
    pub set: Option<String>,
}

/// `boot.tuple_body`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TupleBody {
    /// sha256 of the body binary.
    pub binary: String,
    pub ext: TupleExt,
    pub sha: String,
}

/// `boot.tuple_kernel`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TupleKernel {
    pub built_from: String,
    /// The admitted build digest (selfmap's `live.kernel.verified`).
    pub init_kernel: String,
    /// `maintainer` | `agent`.
    pub source: String,
    pub version: String,
}

/// The zone mode in the boot block and the `mode` event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneMode {
    Normal,
    Safe,
}

open_enum! {
    /// Why the Root started the kernel (`boot.reason`).
    pub enum ZoneBootReason {
        Cold = "cold",
        Crash = "crash",
        RollbackSystem = "rollback_system",
        Demotion = "demotion",
        PanicResume = "panic_resume",
        SafeClear = "safe_clear",
        KernelUpgrade = "kernel_upgrade",
        Install = "install",
    }
}

/// `boot` in the boot block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BootInfo {
    /// `{trigger, seq}` for `demotion` and `rollback_system`; else null.
    pub cause: Option<Value>,
    /// sha256 of a state snapshot to restore first; else null.
    pub kstate: Option<String>,
    pub level: u8,
    pub mode: ZoneMode,
    /// `{at, channel}` after a panic resume; else null.
    pub panic: Option<Value>,
    pub reason: ZoneBootReason,
    pub tuple: TupleIds,
    pub tuple_body: TupleBody,
    pub tuple_kernel: TupleKernel,
}

/// `ladder` in the boot block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LadderInfo {
    pub cap: u8,
    pub evolution_share_max_permille: Option<u32>,
    pub level: u8,
}

/// `limits` in the boot block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub hb_interval_s: u64,
    pub max_instances: u32,
    pub parallel_bodies: u32,
}

/// `paths` in the boot block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootPaths {
    pub egress_sock: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_git: Option<String>,
}

/// `root` in the boot block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootVersion {
    pub proto: u32,
    pub version: String,
}

/// The `hello` result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BootBlock {
    /// ADR-009 (SBX-009 §D): `{access, full, workspace}`; absent from an older Root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<AccessData>,
    /// ADR-011 (X2): `{mode: "free"|"gated"}`, the effective charter `extend.mode`; absent from an
    /// older Root, which means `"gated"`. The `ceilings` block carries the same object after a
    /// charter reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extend: Option<Value>,
    pub boot: BootInfo,
    pub charter_sha256: String,
    pub ladder: LadderInfo,
    pub limits: Limits,
    pub paths: RootPaths,
    pub root: RootVersion,
}

// ---------------------------------------------------------------------------------------------
// ledger.json and ceilings (root/root.md §4.5, root/egress.md §4.2)
// ---------------------------------------------------------------------------------------------

/// One criterion of `ledger.next`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Criterion {
    pub have: i64,
    pub id: String,
    pub met: bool,
    pub need: i64,
}

/// `ledger.next` (null at the cap).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerNext {
    pub criteria: Vec<Criterion>,
    pub level: u8,
}

/// `ledger.json` / the `ledger.get` result. Unknown fields are kept in `extra`, so the object
/// relays verbatim.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    pub v: V1,
    pub cap: u8,
    pub entered_at: String,
    pub entered_seq: u64,
    pub evolution_share_max_permille: Option<u32>,
    pub incident: Option<Value>,
    /// `{from, to, direction, cause, trigger, seq, at}` of the newest `ladder` record.
    pub last_change: Option<Value>,
    pub level: u8,
    pub max_level: u8,
    pub next: Option<LedgerNext>,
    pub seq: u64,
    pub supported_max_level: u8,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// The money block (root/egress.md §4.2), integers in micro-USD.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MoneyBlock {
    pub day_limit_musd: u64,
    pub day_reserved_musd: u64,
    pub day_resets_at: String,
    pub day_spent_musd: u64,
    pub day_starts_at: String,
    pub sealed_reserve_musd: u64,
    pub total_limit_musd: u64,
    pub total_spent_musd: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `{active, limit}` counts from the broker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveLimit {
    pub active: u32,
    pub limit: u32,
}

/// `ceilings.json` / the `ceilings.get` result / the `ceilings` event: egress's block
/// (`epoch`, `level`, `money`, `models`, `classes`, …) plus the broker's `instances` and
/// `parallel_bodies`. Every field is optional and unknown fields are kept, so it relays
/// verbatim.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ceilings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v: Option<V1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub money: Option<MoneyBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classes: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instances: Option<ActiveLimit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_bodies: Option<ActiveLimit>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

// ---------------------------------------------------------------------------------------------
// Root → kernel requests and events (root/root.md §4.1.4)
// ---------------------------------------------------------------------------------------------

/// `stop`: stop the bodies, flush, answer `ok`, exit 0.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopArgs {
    pub reason: String,
    pub grace_ms: u64,
}

/// `prepare_swap`: pause and stop the live body, flush, answer `ok`, exit 75 within 10 s.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareSwapArgs {
    pub job: String,
    pub deadline_s: u64,
}

/// `owner.in.frame` (supervise.md §4.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum OwnerInFrame {
    Line { text: String },
    /// `then: "continue"` (STEER): interrupt and steer: the body aborts as for a plain interrupt,
    /// then continues the run with the queued steer instead of going idle. Absent: a plain stop.
    Interrupt {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        then: Option<String>,
    },
    /// STEER: take back owner input `seq` (the kernel echo's) if the body has not consumed it.
    Retract { seq: u64 },
}

/// `owner.in`: append durably to the inbox (dedup by `qid`), then answer `ok`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerInArgs {
    pub qid: String,
    pub frame: OwnerInFrame,
}

/// A Root → kernel request.
#[derive(Clone, Debug, PartialEq)]
pub enum RootRequest {
    Stop(StopArgs),
    PrepareSwap(PrepareSwapArgs),
    OwnerIn(OwnerInArgs),
}

impl RootRequest {
    pub fn cmd(&self) -> &'static str {
        match self {
            RootRequest::Stop(_) => "stop",
            RootRequest::PrepareSwap(_) => "prepare_swap",
            RootRequest::OwnerIn(_) => "owner.in",
        }
    }

    pub fn args(&self) -> Value {
        match self {
            RootRequest::Stop(a) => to_args(a),
            RootRequest::PrepareSwap(a) => to_args(a),
            RootRequest::OwnerIn(a) => to_args(a),
        }
    }

    /// Parse a request the kernel received.
    pub fn parse(cmd: &str, args: &Value) -> Result<RootRequest, ErrorBody> {
        Ok(match cmd {
            "stop" => RootRequest::Stop(link_args(cmd, args)?),
            "prepare_swap" => RootRequest::PrepareSwap(link_args(cmd, args)?),
            "owner.in" => RootRequest::OwnerIn(link_args(cmd, args)?),
            other => {
                return Err(RootErrorCode::UnknownCmd.error(format!("unknown command `{}`", crate::validate::one_line(other, 64))));
            }
        })
    }

    /// The request frame with Root id `r-<n>`.
    pub fn to_request(&self, n: u64) -> Request {
        Request { v: V1, id: format!("r-{n}"), cmd: self.cmd().to_string(), args: self.args() }
    }
}

/// `exited`: an instance ended (the Root has swept it and deleted its mark).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitedData {
    pub at: String,
    pub code: Option<i32>,
    pub instance: String,
    pub rusage_ms: u64,
    pub signal: Option<i32>,
}

/// `level.cause`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LevelCause {
    Unlock,
    Demotion,
    CharterMax,
}

/// `level`: a ladder change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LevelData {
    pub cap: u8,
    pub cause: LevelCause,
    pub evolution_share_max_permille: Option<u32>,
    pub from: u8,
    /// The R4 `ladder` record's `seq`.
    pub r4_seq: u64,
    pub to: u8,
    /// `{kind, seq}` for a demotion or cap; else null.
    pub trigger: Option<Value>,
}

/// `mode`: arrives only together with a restart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeData {
    pub mode: ZoneMode,
}

/// `swap`: progress of a kernel upgrade.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapData {
    pub job: String,
    pub msg: String,
    /// `gog_pass`, `gog_fail`, `prepare`, `installed`, `ready`, `probation`, …
    pub stage: String,
}

/// `access` (ADR-009; root SBX-009 protocol §D): the owner's workspace and the live body's grant,
/// sent after every `ready` (`restart: null`) and on every change (`restart: "idle"` when the live
/// body's grant changed: restart it gracefully at its next idle point; a body's sandbox profile is
/// fixed at spawn). The same object without `restart` is the `hello` result's `access`. The Root
/// itself sets `INIT_WORKSPACE`/`INIT_ACCESS` on body spawns; the kernel relays the same values.
/// Unknown fields are kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessData {
    /// `none` | `read_home` | `workspace` | `full`.
    pub access: String,
    #[serde(default)]
    pub full: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<String>,
    /// The canonical workspace path; None when there is none.
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl AccessData {
    /// `restart: "idle"`: restart the live body at its next idle point.
    pub fn restart_idle(&self) -> bool {
        self.restart.as_deref() == Some("idle")
    }
}

/// `extend.window` (ADR-011 Records): a change group's window opened or closed. `outcome` (closed
/// only): `clean`|`quiet`|`flagged`|`rolled_back`|`collateral`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtendWindowData {
    pub group: u64,
    /// `open` | `closed`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

impl ExtendWindowData {
    /// A close after which the Root may move `lkg` to the group's tuple (ADR-011 §2.3 step 6).
    pub fn lkg_eligible(&self) -> bool {
        self.state == "closed" && matches!(self.outcome.as_deref(), Some("clean" | "quiet" | "flagged"))
    }
}

/// `extend.rollback` (ADR-011 §3.4): roll the body back to tuple `to` at the next step boundary and
/// confirm with `lineage.append body_rollback{from_sha, to_sha, reason}` (`to_sha` = that tuple's
/// `body.sha`); no confirmation within 120 s → the Root's whole-system rollback.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtendRollbackData {
    pub to: String,
    pub groups: Vec<u64>,
    pub reason: String,
}

/// A Root → kernel event.
#[derive(Clone, Debug, PartialEq)]
pub enum RootEvent {
    Exited(ExitedData),
    Level(LevelData),
    Mode(ModeData),
    Ceilings(Ceilings),
    Swap(SwapData),
    Access(AccessData),
    /// ADR-011 (X2).
    ExtendWindow(ExtendWindowData),
    ExtendRollback(ExtendRollbackData),
    /// An event this build does not know (ignored).
    Unknown(Event),
}

/// `hb` (every 5 s): the primary's state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HbData {
    pub busy: bool,
    pub mode: String,
    pub sha12: String,
    pub slot: String,
}

/// `owner.out` display: a body display frame plus `boot_seq`, and the fields the body added that
/// this build does not know, relayed unchanged (tui.md §3.15, §9 Q2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerDisplay {
    pub boot_seq: u64,
    #[serde(flatten)]
    pub display: Display,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Keys of a relayed display frame that only the relay sets (the Root sets `rseq` and `src`,
/// tui.md §9 Q1); a body's own copies are dropped.
const RELAY_KEYS: [&str; 5] = ["boot_seq", "rseq", "src", "t", "v"];
/// Room left on the 1 MiB line for the fields the Root adds when it relays the frame.
const RELAY_HEADROOM: usize = 1024;

impl OwnerDisplay {
    /// The relayed frame. `extra` loses the relay's own keys, and is dropped whole (the known
    /// fields kept) when the `owner.out` line would not fit in 1 MiB.
    pub fn new(boot_seq: u64, display: Display, mut extra: BTreeMap<String, Value>) -> OwnerDisplay {
        extra.retain(|k, _| !RELAY_KEYS.contains(&k.as_str()));
        let mut d = OwnerDisplay { boot_seq, display, extra };
        if !d.extra.is_empty() {
            let ev = KernelEvent::OwnerOut(OwnerOutData { frame: OwnerOutFrame::Display(d.clone()) });
            let fits = ev.to_event().and_then(|e| icj::to_line(&e)).is_ok_and(|l| l.len() <= crate::envelope::MAX_RESPONSE_LINE - RELAY_HEADROOM);
            if !fits {
                d.extra.clear();
            }
        }
        d
    }
}

/// `owner.out.frame` (supervise.md §4.6). `state.model` is the live scope's model and
/// `state.task_musd` today's spend booked to the current task, null when unknown (tui.md §9 Q2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum OwnerOutFrame {
    Display(OwnerDisplay),
    Echo { seq: u64, text: String },
    Notice { level: AlertLevel, text: String, ts: String },
    State {
        busy: bool,
        level: u8,
        members: u32,
        mode: String,
        #[serde(default)]
        model: Option<String>,
        queued: u64,
        sha12: String,
        slot: String,
        #[serde(default)]
        task_musd: Option<u64>,
        /// Proxy-attributed gate/probe costs today; not all free-evolution reasoning.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        evolution_checks_today_musd: Option<u64>,
        /// STEER: the seqs of owner inputs the body has not acked yet (steers and follow-ups,
        /// ascending, at most [`STEER_LIST_MAX`]). Absent from an older kernel.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steer_pending: Option<Vec<u64>>,
        /// STEER: the most recently acked input seqs (durably in the body's log), oldest first.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steer_seen: Option<Vec<u64>>,
        /// STEER: the most recently retracted input seqs (never logged), oldest first.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steer_retracted: Option<Vec<u64>>,
    },
}

/// The longest `steer_pending` / `steer_seen` / `steer_retracted` list in a `state` frame.
pub const STEER_LIST_MAX: usize = 32;

/// `owner.out`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerOutData {
    pub frame: OwnerOutFrame,
}

/// A kernel → Root event.
#[derive(Clone, Debug, PartialEq)]
pub enum KernelEvent {
    Hb(HbData),
    OwnerOut(OwnerOutData),
}

fn event<T: Serialize>(ev: &str, data: &T) -> Result<Event, IcjError> {
    Ok(Event { v: V1, ev: ev.to_string(), data: icj::to_value(data)? })
}

fn event_data<T: DeserializeOwned>(e: &Event) -> Result<T, ErrorBody> {
    serde_json::from_value(e.data.clone()).map_err(|x| RootErrorCode::Args.error(format!("event {}: {x}", e.ev)))
}

impl RootEvent {
    pub fn to_event(&self) -> Result<Event, IcjError> {
        match self {
            RootEvent::Exited(d) => event("exited", d),
            RootEvent::Level(d) => event("level", d),
            RootEvent::Mode(d) => event("mode", d),
            RootEvent::Ceilings(d) => event("ceilings", d),
            RootEvent::Swap(d) => event("swap", d),
            RootEvent::Access(d) => event("access", d),
            RootEvent::ExtendWindow(d) => event("extend.window", d),
            RootEvent::ExtendRollback(d) => event("extend.rollback", d),
            RootEvent::Unknown(e) => Ok(e.clone()),
        }
    }

    pub fn from_event(e: &Event) -> Result<RootEvent, ErrorBody> {
        Ok(match e.ev.as_str() {
            "exited" => RootEvent::Exited(event_data(e)?),
            "level" => RootEvent::Level(event_data(e)?),
            "mode" => RootEvent::Mode(event_data(e)?),
            "ceilings" => RootEvent::Ceilings(event_data(e)?),
            "swap" => RootEvent::Swap(event_data(e)?),
            "access" => RootEvent::Access(event_data(e)?),
            "extend.window" => RootEvent::ExtendWindow(event_data(e)?),
            "extend.rollback" => RootEvent::ExtendRollback(event_data(e)?),
            _ => RootEvent::Unknown(e.clone()),
        })
    }
}

impl KernelEvent {
    pub fn to_event(&self) -> Result<Event, IcjError> {
        match self {
            KernelEvent::Hb(d) => event("hb", d),
            KernelEvent::OwnerOut(d) => event("owner.out", d),
        }
    }

    pub fn from_event(e: &Event) -> Result<KernelEvent, ErrorBody> {
        Ok(match e.ev.as_str() {
            "hb" => KernelEvent::Hb(event_data(e)?),
            "owner.out" => KernelEvent::OwnerOut(event_data(e)?),
            other => return Err(RootErrorCode::UnknownCmd.error(format!("unknown event `{other}`"))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// ADR-011 X2: the new requests, events, the boot block's `extend` and `store.put kind:"t0"`.
    #[test]
    fn extend_link_round_trip() {
        let r = KernelRequest::ChangeVeto(ChangeVetoArgs { group: 3, reason: "g3 failed".into() });
        let req = r.to_request(9);
        assert_eq!(String::from_utf8(req.to_line()).unwrap(), "{\"args\":{\"group\":3,\"reason\":\"g3 failed\"},\"cmd\":\"change.veto\",\"id\":\"k-9\",\"v\":1}\n");
        assert_eq!(KernelRequest::parse(&req.cmd, &req.args).unwrap(), r);
        let open = KernelRequest::ChangeOpen(json!({"scope": "t2", "sha": "ab12"}));
        assert_eq!((open.cmd(), KernelRequest::parse("change.open", &open.args()).unwrap()), ("change.open", open.clone()));
        let grant = KernelRequest::GrantRequest(json!({"kind": "extend.open_groups", "value": 6, "why": "w"}));
        assert_eq!((grant.cmd(), KernelRequest::parse("grant.request", &grant.args()).unwrap()), ("grant.request", grant.clone()));
        let w = Event { v: crate::icj::V1, ev: "extend.window".into(), data: json!({"group": 4, "outcome": "clean", "state": "closed"}) };
        let Ok(RootEvent::ExtendWindow(d)) = RootEvent::from_event(&w) else { panic!("extend.window") };
        assert!(d.lkg_eligible());
        assert_eq!(RootEvent::ExtendWindow(d).to_event().unwrap().data, w.data);
        let o = ExtendWindowData { group: 4, state: "open".into(), outcome: None };
        assert!(!o.lkg_eligible());
        assert!(!ExtendWindowData { group: 4, state: "closed".into(), outcome: Some("rolled_back".into()) }.lkg_eligible());
        let rb = Event { v: crate::icj::V1, ev: "extend.rollback".into(), data: json!({"groups": [4, 5], "reason": "H1", "to": "t0a1b2c3d4e5f"}) };
        let Ok(RootEvent::ExtendRollback(d)) = RootEvent::from_event(&rb) else { panic!("extend.rollback") };
        assert_eq!((d.groups.as_slice(), d.to.as_str()), (&[4u64, 5][..], "t0a1b2c3d4e5f"));
        assert_eq!(serde_json::to_value(StoreKind::T0).unwrap(), json!("t0"));
    }

    #[test]
    fn request_round_trip() {
        let r = KernelRequest::Kill(KillArgs { instance: "7c2e91d04b3a".into(), mode: KillMode::Term });
        let req = r.to_request(7);
        assert_eq!(
            String::from_utf8(req.to_line()).unwrap(),
            "{\"args\":{\"instance\":\"7c2e91d04b3a\",\"mode\":\"term\"},\"cmd\":\"kill\",\"id\":\"k-7\",\"v\":1}\n"
        );
        assert_eq!(KernelRequest::parse(&req.cmd, &req.args).unwrap(), r);
        let e = KernelRequest::parse("approve", &json!({})).unwrap_err();
        assert_eq!(RootErrorCode::of(&e), RootErrorCode::UnknownCmd);
        let e = KernelRequest::parse("kill", &json!({"instance": 1})).unwrap_err();
        assert_eq!(RootErrorCode::of(&e), RootErrorCode::Args);
        assert_eq!(KernelRequest::Ready.to_request(1).args, json!({}));
    }

    #[test]
    fn unknown_events_tolerated() {
        let e = Event { v: V1, ev: "future".into(), data: json!({"x": 1}) };
        assert!(matches!(RootEvent::from_event(&e), Ok(RootEvent::Unknown(_))));
    }

    /// ADR-009: the `access` event (SBX-009 §D); a null workspace is None, unknown fields are kept,
    /// and the hello result's `access` is optional.
    #[test]
    fn access_event() {
        let data = json!({"access": "workspace", "full": false, "restart": "idle", "workspace": "/Users/o/proj", "x": 1});
        let e = Event { v: V1, ev: "access".into(), data: data.clone() };
        let Ok(RootEvent::Access(a)) = RootEvent::from_event(&e) else { panic!("not an access event") };
        assert_eq!((a.access.as_str(), a.workspace.as_deref(), a.restart_idle()), ("workspace", Some("/Users/o/proj"), true));
        assert_eq!(RootEvent::Access(a).to_event().unwrap().data, data);
        let e = Event { v: V1, ev: "access".into(), data: json!({"access": "read_home", "full": false, "restart": null, "workspace": null}) };
        assert!(matches!(RootEvent::from_event(&e), Ok(RootEvent::Access(a)) if a.workspace.is_none() && !a.restart_idle()));
        let e = Event { v: V1, ev: "access".into(), data: json!({"workspace": "/x"}) };
        assert!(RootEvent::from_event(&e).is_err(), "access is required");
    }

    #[test]
    fn root_codes() {
        assert_eq!(RootErrorCode::ALL.len(), 13);
        let b = RootErrorCode::Level.error("needs L3");
        assert_eq!(b.code.as_str(), "root.level");
        assert!(b.code.is_dotted());
    }

    #[test]
    fn owner_display_relays_unknown_fields_within_the_line() {
        use crate::stdio::{decode_from_body, display_extra, DisplayKind, FromBody};
        let line = br#"{"boot_seq":9,"detail":{"argv":["ls"]},"kind":"tool_start","name":"bash","src":"root","t":"display","text":"ls","v":1}"#;
        let FromBody::Display(d) = decode_from_body(line).unwrap() else { panic!() };
        let od = OwnerDisplay::new(3, d.clone(), display_extra(line));
        let out = icj::to_value(&OwnerOutData { frame: OwnerOutFrame::Display(od.clone()) }).unwrap();
        // The body's extra field reaches the Root as sent; the relay's own keys are the kernel's.
        assert_eq!(out, json!({"frame": {"boot_seq": 3, "detail": {"argv": ["ls"]}, "kind": "tool_start", "name": "bash", "t": "display", "text": "ls"}}));
        let back: OwnerOutData = serde_json::from_value(out).unwrap();
        assert_eq!(back.frame, OwnerOutFrame::Display(od));
        // Too big with the extras: they are dropped, the known fields kept.
        let big = BTreeMap::from([("detail".to_string(), json!("x".repeat(crate::envelope::MAX_RESPONSE_LINE)))]);
        let od = OwnerDisplay::new(3, d.clone(), big);
        assert!(od.extra.is_empty());
        assert_eq!((od.display.kind, od.display.text.as_str()), (DisplayKind::ToolStart, "ls"));
        let small = BTreeMap::from([("detail".to_string(), json!("x".repeat(1000)))]);
        assert_eq!(OwnerDisplay::new(3, d, small).extra.len(), 1);
    }

    #[test]
    fn owner_state_has_model_and_task_musd() {
        let f = OwnerOutFrame::State {
            busy: false,
            level: 0,
            members: 0,
            mode: "normal".into(),
            model: Some("deepseek-flash".into()),
            queued: 0,
            sha12: "3f9c0a1b2c4d".into(),
            slot: "running".into(),
            task_musd: None,
            evolution_checks_today_musd: None,
            steer_pending: Some(vec![4]),
            steer_seen: Some(vec![2, 3]),
            steer_retracted: None,
        };
        let v = icj::to_value(&f).unwrap();
        assert_eq!((v["steer_pending"].clone(), v["steer_seen"].clone()), (json!([4]), json!([2, 3])));
        assert!(v.get("steer_retracted").is_none());
        assert_eq!(v["model"], "deepseek-flash");
        assert!(v.get("evolution_checks_today_musd").is_none());
        assert!(v.as_object().unwrap().contains_key("task_musd") && v["task_musd"].is_null());
        assert_eq!(serde_json::from_value::<OwnerOutFrame>(v).unwrap(), f);
        // A state frame without them (an older kernel) still decodes.
        let old = json!({"busy": true, "level": 0, "members": 0, "mode": "normal", "queued": 1, "sha12": "x", "slot": "running", "t": "state"});
        assert!(matches!(serde_json::from_value::<OwnerOutFrame>(old).unwrap(), OwnerOutFrame::State { model: None, task_musd: None, steer_pending: None, .. }));
    }

    #[test]
    fn owner_in_interrupt_and_retract_frames() {
        let f = |v: Value| serde_json::from_value::<OwnerInFrame>(v).unwrap();
        assert_eq!(f(json!({"t": "interrupt"})), OwnerInFrame::Interrupt { then: None });
        assert_eq!(f(json!({"t": "interrupt", "then": "continue"})), OwnerInFrame::Interrupt { then: Some("continue".into()) });
        assert_eq!(f(json!({"seq": 9, "t": "retract"})), OwnerInFrame::Retract { seq: 9 });
        assert_eq!(icj::to_value(&OwnerInFrame::Interrupt { then: None }).unwrap(), json!({"t": "interrupt"}));
    }

    #[test]
    fn owner_state_round_trips_known_evolution_checks_cost() {
        let v = json!({"busy": true, "level": 0, "members": 0, "mode": "normal", "model": null,
            "queued": 0, "sha12": "x", "slot": "running", "t": "state", "task_musd": null,
            "evolution_checks_today_musd": 0});
        let frame: OwnerOutFrame = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(icj::to_value(&frame).unwrap(), v, "known zero is preserved, unlike unavailable attribution");
    }

    #[test]
    fn ceilings_keep_unknown_fields() {
        let src = json!({"compute": {"cpu_core_hours_day": 48}, "parallel_bodies": {"active": 1, "limit": 1}, "v": 1});
        let c: Ceilings = serde_json::from_value(src.clone()).unwrap();
        assert_eq!(c.parallel_bodies, Some(ActiveLimit { active: 1, limit: 1 }));
        assert_eq!(icj::to_value(&c).unwrap(), src);
    }
}
