//! Kernel state files (kernel/supervise.md §4.3, §4.4, §4.7).
//!
//! All carry `"v":1`, sorted keys (ICJ), RFC 3339 UTC timestamps with milliseconds, and SHAs as
//! 40 lowercase hex. A file with another `v` fails to parse (error text starts with
//! [`crate::icj::UNSUPPORTED_VERSION`]). Unknown fields are ignored on read.
//!
//! Objects owned by other docs and copied verbatim (`evolution`, `selfmap`, `env.root`, `safe`)
//! are carried as `serde_json::Value`.

use crate::icj::{self, IcjError, V1};
use crate::initctl::{Counters, Mode, Slot, Sweep};
use crate::root_link::InstanceKind;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Paths relative to `INIT_HOME` (CONVENTIONS §3, supervise.md §3.1).
pub mod paths {
    pub const LKG: &str = "state/kernel/lkg.json";
    pub const BOOT: &str = "state/kernel/boot.json";
    pub const UPDATE_FAILED: &str = "state/kernel/update-failed.json";
    pub const SUPERVISOR: &str = "state/kernel/supervisor.json";
    pub const JOBS_DIR: &str = "state/kernel/jobs";
    pub const READY: &str = "state/run/ready";
    pub const HEARTBEAT: &str = "state/run/heartbeat";
    pub const LINEAGE_LOCAL: &str = "vault/lineage.jsonl";
    pub const ROOT_OUTBOX: &str = "state/kernel/root-outbox.jsonl";
    pub const KERNEL_SOCK: &str = "run/kernel.sock";
    pub const PROXY_SOCK: &str = "run/proxy.sock";
}

/// The bytes written to disk: one ICJ line plus `\n`.
pub fn encode<T: Serialize>(x: &T) -> Result<Vec<u8>, IcjError> {
    icj::to_line(x)
}

/// Parse a state file (ICJ rule 4 and the `v` check apply).
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, IcjError> {
    let trimmed = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    icj::from_slice(trimmed)
}

// ---------------------------------------------------------------------------------------------
// lkg.json
// ---------------------------------------------------------------------------------------------

/// Why an LKG was committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LkgReason {
    Seed,
    G4Pass,
    DataOnly,
    SystemRollback,
    /// ADR-011 §2.3 step 6: the Root closed the version's change group and moved its `lkg`.
    ChangeClose,
}

/// `{provider, id}` of a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub id: String,
    pub provider: String,
}

/// `state/kernel/lkg.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LkgFile {
    pub v: V1,
    pub sha: String,
    /// `bin/init-body.lkg`.
    pub binary: String,
    pub binary_sha256: String,
    /// `lineage/trees/<sha12>` (relative to `INIT_HOME`).
    pub tree: String,
    pub reason: LkgReason,
    /// The Root lineage `seq` of this LKG's `promotion{lkg:true}`; null before the genesis tuple.
    pub lineage_seq: Option<u64>,
    /// +1 per LKG commit.
    pub seq: u64,
    pub prev_sha: Option<String>,
    pub gate_job: Option<String>,
    pub committed_at: String,
    /// The model G4 judged this LKG with; null for `seed`.
    pub model: Option<ModelRef>,
    /// The kept-component set installed as `bin/ext.lkg`.
    pub ext_set: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// update-failed.json
// ---------------------------------------------------------------------------------------------

open_enum! {
    /// `update-failed.json.stage` (supervise.md §4.3).
    pub enum UpdateFailedStage {
        Fetch = "fetch",
        Validate = "validate",
        Build = "build",
        G0 = "G0",
        G1 = "G1",
        G2 = "G2",
        G3 = "G3",
        G4 = "G4",
        Pause = "pause",
        Fence = "fence",
        Handoff = "handoff",
        Probation = "probation",
        CrashProvisional = "crash_provisional",
        Rollback = "rollback",
        KernelRestart = "kernel_restart",
        Panic = "panic",
        SystemRollback = "system_rollback",
        Demotion = "demotion",
        GateOfGates = "gate_of_gates",
    }
}

/// `state/kernel/update-failed.json`: the latest failure of a promotion, handoff, probation or
/// G4. Also embedded in `boot.json.update_failed` and the stdio `resume` frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateFailedFile {
    pub v: V1,
    pub at: String,
    /// Null when no process exited (e.g. `fence`).
    pub exit_code: Option<i32>,
    pub from_sha: Option<String>,
    pub job: Option<String>,
    pub log_path: Option<String>,
    /// ≤ 4 KiB of the failed process's stderr.
    pub log_tail: String,
    pub reason: String,
    pub report: Option<String>,
    /// +1 per failure; bodies dedup by `seq`.
    pub seq: u64,
    pub sha: String,
    pub signal: Option<i32>,
    pub stage: UpdateFailedStage,
}

/// `log_tail` cap.
pub const MAX_LOG_TAIL: usize = 4 * 1024;

// ---------------------------------------------------------------------------------------------
// boot.json
// ---------------------------------------------------------------------------------------------

/// `INIT_BOOT_REASON` / `boot.json.reason`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootReason {
    Cold,
    Promote,
    Rollback,
    Crash,
    Handoff,
}

/// `INIT_MODE` / `boot.json.mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitMode {
    Normal,
    Probe,
    Safe,
}

/// `boot.json.cause`: `{kind: "model_change"|"owner_restart"}` on `handoff`;
/// `{kind: "demotion"|"rollback_system", trigger, seq, evidence}` after such a kernel start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BootCause {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Value>,
}

/// `boot.json.crash`: the exit that caused this spawn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrashInfo {
    pub exit_code: Option<i32>,
    /// ≤ 4 KiB of stderr.
    pub log_tail: String,
    pub signal: Option<i32>,
}

/// `boot.json.env.budget` (integers, micro-USD).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvBudget {
    pub cap_musd: u64,
    pub day_starts_at: String,
    pub today_musd: u64,
}

/// `boot.json.env.model`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvModel {
    pub context_window: u64,
    pub id: String,
    pub max_output: u64,
    pub provider: String,
}

/// `boot.json.env.network`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvNetwork {
    pub egress: Vec<Value>,
    pub mode: String,
}

/// `boot.json.env.os`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvOs {
    pub arch: String,
    pub family: String,
    pub version: String,
}

/// `sandbox.zone` (kernel/sandbox.md §4.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxZone {
    /// `root`.
    pub launcher: String,
    /// `root-egress-only`.
    pub network: String,
}

/// `boot.json.env.sandbox` and `status.sandbox` (kernel/sandbox.md §4.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxStatus {
    /// Paths relative to `INIT_HOME`, sorted.
    pub hidden: Vec<String>,
    /// `seatbelt` | `bwrap`.
    pub kind: String,
    /// Linux: `full` | `partial` | `none`; macOS: null.
    pub landlock: Option<String>,
    pub landlock_abi: Option<u32>,
    /// `kernel-sockets-only`.
    pub network: String,
    pub read_only: Vec<String>,
    pub seccomp: bool,
    pub sweep: Sweep,
    pub write: Vec<String>,
    pub zone: SandboxZone,
}

/// `boot.json.env`: the kernel half of the environment snapshot (awareness.md §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BootEnv {
    pub budget: EnvBudget,
    pub init_home: String,
    pub kernel_version: String,
    pub mem_total_mb: u64,
    pub model: EnvModel,
    pub network: EnvNetwork,
    pub os: EnvOs,
    /// The Root's published status, verbatim (`{level, max_level, next, ceilings, classes,
    /// audited_good}`).
    pub root: Value,
    pub sandbox: SandboxStatus,
}

/// `boot.json.kernel.proto`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtoVersions {
    pub initctl: u32,
    pub root: u32,
    pub ui: u32,
}

/// `boot.json.kernel`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootKernel {
    pub proto: ProtoVersions,
    pub version: String,
}

/// `boot.json.kernel_upgrade`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelUpgrade {
    pub from: String,
    /// `agent` | `maintainer`.
    pub source: String,
    pub to: String,
}

/// Explicit wire/limit descriptor. It grants no routes, credentials or capabilities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDescriptor {
    pub api: String,
    pub context_window: u32,
    pub max_output: u32,
    pub default_max_tokens: u32,
    /// Owner policy for every request, including compaction and length retries.
    pub request_max_tokens: u32,
    /// Request standard Chat stream usage (default true); false supports servers rejecting it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_stream_usage: Option<bool>,
    /// Opt into model-specific reasoning/cache extensions for a custom native adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_extensions: Option<bool>,
}

impl ModelDescriptor {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !matches!(self.api.as_str(), "chat_completions" | "messages" | "responses") {
            return Err("api must be chat_completions, messages or responses");
        }
        if !(1024..=16_777_216).contains(&self.context_window)
            || self.max_output == 0 || self.max_output >= self.context_window
            || self.default_max_tokens == 0 || self.default_max_tokens > self.max_output
            || self.request_max_tokens == 0 || self.request_max_tokens > self.max_output {
            return Err("invalid model window/output/request limits");
        }
        Ok(())
    }
}

/// `boot.json.model`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootModel {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<ModelDescriptor>,
    pub effort: Option<String>,
    pub model: String,
    pub provider: String,
}

/// `boot.json.panic` (from the Root's boot block).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanicInfo {
    pub at: String,
    pub channel: String,
}

/// `boot.json.pending`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub inputs: u64,
    pub notices: u64,
}

/// `boot.json.provisional` (null when on LKG).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionalInfo {
    pub depth: u32,
    /// The G4 job's state, e.g. `queued`.
    pub g4: Option<String>,
    pub probation_until: Option<String>,
    pub since: String,
}

/// `boot.json.zone`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneInfo {
    /// The boot block's `reason`: `cold` `crash` `rollback_system` `demotion` `panic_resume`
    /// `safe_clear` `kernel_upgrade` `install`.
    pub boot_reason: String,
    pub kernel_sha256: String,
}

/// `state/kernel/boot.json`: written before every live spawn and into each probe root.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BootFile {
    pub v: V1,
    /// `b0` or a member's `bid`; equals `INIT_BODY_ID`.
    pub body_id: String,
    /// == `INIT_BOOT_ID` (16 lowercase hex).
    pub boot_id: String,
    pub boot_seq: u64,
    /// The promoted ref on `promote`; else null.
    pub branch: Option<String>,
    pub cause: Option<BootCause>,
    pub crash: Option<CrashInfo>,
    pub env: BootEnv,
    /// Opaque, filled by evolution.md.
    pub evolution: Value,
    pub from_sha: Option<String>,
    pub kernel: BootKernel,
    pub kernel_upgrade: Option<KernelUpgrade>,
    pub lkg_sha: String,
    pub lkg_tree: String,
    pub mode: InitMode,
    pub model: BootModel,
    pub panic: Option<PanicInfo>,
    pub pending: Pending,
    pub provisional: Option<ProvisionalInfo>,
    pub reason: BootReason,
    pub restarts_since_lkg: u64,
    pub safe: Option<Value>,
    /// Opaque, filled by kernel/selfmap.md.
    pub selfmap: Value,
    /// == `INIT_SESSION`.
    pub session: String,
    pub sha: String,
    pub spawned_at: String,
    pub update_failed: Option<UpdateFailedFile>,
    pub zone: ZoneInfo,
}

// ---------------------------------------------------------------------------------------------
// supervisor.json
// ---------------------------------------------------------------------------------------------

/// `{built_from, sha256}` of a binary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryRef {
    pub built_from: String,
    pub sha256: String,
}

/// `binaries.ext`: the kept-component sets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtSets {
    pub current: Option<String>,
    pub lkg: Option<String>,
    pub prev: Option<String>,
    pub staged: Option<String>,
}

/// `supervisor.json.binaries`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binaries {
    pub current: Option<BinaryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext: Option<ExtSets>,
    pub lkg: Option<BinaryRef>,
    pub prev: Option<BinaryRef>,
    pub staged: Option<BinaryRef>,
}

/// `supervisor.json.body`: the primary body's process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisedBody {
    pub boot_id: String,
    pub boot_seq: u64,
    /// The Root's instance id.
    pub instance: String,
    pub pid: i32,
    pub ready_at: Option<String>,
    pub started_at: String,
}

/// `supervisor.json.crash.last_exit`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastExit {
    pub at: String,
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// `supervisor.json.crash`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrashState {
    pub backoff_until: Option<String>,
    pub consecutive: u32,
    pub last_exit: Option<LastExit>,
    /// Timestamps of crashes on LKG within `crash_window_s`.
    pub lkg_window: Vec<String>,
}

/// `supervisor.json.current`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorCurrent {
    pub provisional: bool,
    pub sha: String,
    pub since: String,
}

/// `supervisor.json.handoff` (a single record even at L2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffRecord {
    pub bid: String,
    pub from: String,
    pub new_instance: Option<String>,
    pub old_instance: String,
    pub phase: String,
    pub staged_sha256: String,
    pub started_at: String,
    pub to: String,
    pub token: String,
}

/// One entry of `supervisor.json.instances` (the Root's instance ids and kinds).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceRecord {
    pub id: String,
    pub kind: InstanceKind,
    /// `b0` / `<bid>` for bodies; null otherwise.
    pub lane: Option<String>,
    /// `run/mark/<id>` (macOS); null on Linux.
    pub mark: Option<String>,
    pub pgid: Option<i32>,
    pub pid: i32,
    /// No longer recorded by the kernel (v0.3): always null.
    pub pidns_ino: Option<u64>,
    pub started_at: String,
}

/// `supervisor.json.kernel`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorKernel {
    pub pid: i32,
    pub started_at: String,
    pub version: String,
}

/// `supervisor.json.lkg`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorLkg {
    pub sha: String,
}

/// `supervisor.json.pipeline`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineState {
    pub job: Option<String>,
    /// `idle`, or the running pipeline's stage.
    pub state: String,
}

/// `supervisor.json.root`: from the boot block and the control link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorRoot {
    pub boot_reason: String,
    pub last_lineage_seq: Option<u64>,
    pub level: u8,
    /// `normal` | `safe`.
    pub mode: String,
    pub outbox_unanswered: u64,
}

/// One L2 member in `supervisor.json.fleet` (mirrored in `state/kernel/fleet.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetMemberState {
    pub bid: String,
    pub budget_share_musd_day: u64,
    pub instance: Option<String>,
    pub mode: Mode,
    pub sha: String,
    pub slot: Slot,
    pub started_at: String,
}

/// `state/kernel/supervisor.json`: the kernel's state snapshot, rewritten on every transition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SupervisorFile {
    pub v: V1,
    pub binaries: Binaries,
    pub body: Option<SupervisedBody>,
    pub counters: Counters,
    pub crash: CrashState,
    pub current: SupervisorCurrent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fleet: Option<Vec<FleetMemberState>>,
    pub handoff: Option<HandoffRecord>,
    pub instances: Vec<InstanceRecord>,
    pub kernel: SupervisorKernel,
    pub lkg: SupervisorLkg,
    pub mode: Mode,
    pub pipeline: PipelineState,
    pub root: SupervisorRoot,
    pub slot: Slot,
    pub sweep: Sweep,
}

// ---------------------------------------------------------------------------------------------
// ready, heartbeat (written by the body)
// ---------------------------------------------------------------------------------------------

/// `state/run/ready`. A `ready` whose `boot_id` does not match is ignored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyFile {
    pub v: V1,
    pub boot_id: String,
    /// The body's own pid (inside a Linux pid namespace it is not the host pid).
    pub pid: i32,
    /// `sha256:<64 hex>` (body/log-context.md).
    pub prefix_hash: String,
    pub sha: String,
    pub ts: String,
}

/// `state/run/heartbeat`, rewritten every 5 s.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeartbeatFile {
    pub v: V1,
    pub boot_id: String,
    pub busy: bool,
    pub ts: String,
}

/// Heartbeat interval and staleness (supervise.md §4.4, §4.9).
pub const HEARTBEAT_INTERVAL_S: u64 = 5;
pub const HEARTBEAT_STALE_S: u64 = 30;

// ---------------------------------------------------------------------------------------------
// vault/lineage.jsonl (kernel-local copy, supervise.md §4.7)
// ---------------------------------------------------------------------------------------------

/// One line of `vault/lineage.jsonl`: the kernel-local copy of a record sent to the Root as
/// `lineage.append`. `seq` is the outbox sequence; `dedup_key` = `<kboot>:<outbox seq>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineageLocalRecord {
    pub v: V1,
    pub at: String,
    pub data: Value,
    pub dedup_key: String,
    pub kind: String,
    pub seq: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_mismatch_is_an_error() {
        let ok = br#"{"boot_id":"a3f09c1e5b7d2468","busy":true,"ts":"2026-10-19T12:36:08.302Z","v":1}"#;
        assert!(decode::<HeartbeatFile>(ok).is_ok());
        let bad = br#"{"boot_id":"a3f09c1e5b7d2468","busy":true,"ts":"2026-10-19T12:36:08.302Z","v":2}"#;
        let e = decode::<HeartbeatFile>(bad).unwrap_err().to_string();
        assert!(e.contains(icj::UNSUPPORTED_VERSION), "{e}");
        let missing = br#"{"boot_id":"a3f09c1e5b7d2468","busy":true,"ts":"2026-10-19T12:36:08.302Z"}"#;
        assert!(decode::<HeartbeatFile>(missing).is_err());
    }

    #[test]
    fn unknown_fields_ignored_and_floats_refused() {
        let x = br#"{"boot_id":"b","busy":false,"future":{"x":1},"ts":"t","v":1}"#;
        let h: HeartbeatFile = decode(x).unwrap();
        assert!(!h.busy);
        assert!(decode::<HeartbeatFile>(br#"{"boot_id":"b","busy":false,"f":1.5,"ts":"t","v":1}"#).is_err());
        assert_eq!(encode(&h).unwrap(), b"{\"boot_id\":\"b\",\"busy\":false,\"ts\":\"t\",\"v\":1}\n".to_vec());
    }
}
