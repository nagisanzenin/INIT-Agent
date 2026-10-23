//! The initctl error-code registry (kernel/initctl.md §4.2) and the CLI exit codes (§4.5).
//!
//! Codes are strings on the wire. Kernel refusals are `E_*`; other docs register dotted codes
//! (`trailer.*`, `predicts.*` from evolution.md §4.2; `gate.*` from kernel/gate.md §4.1), which
//! travel as [`ErrorCode::Other`]. A client treats any code it does not know as a failure with
//! exit code 1.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// sysexits-style exit codes of the `initctl` CLI (kernel/initctl.md §4.5).
pub mod exit {
    /// Success, including `wait` returning at `awaiting_boundary` or `awaiting_swap`.
    pub const OK: u8 = 0;
    /// The command ran and failed (`ok:false` with a domain code, `gate` with `pass:false`,
    /// `task submit` with `admitted:false`).
    pub const FAILED: u8 = 1;
    /// `wait`/`--timeout` expired; the job is still running.
    pub const TIMED_OUT: u8 = 2;
    /// EX_USAGE: bad CLI arguments, `E_ARGS`, `E_WOULD_DEADLOCK`.
    pub const USAGE: u8 = 64;
    /// EX_UNAVAILABLE: cannot connect, or the connection dropped before the final response.
    pub const UNAVAILABLE: u8 = 69;
    /// EX_SOFTWARE: `E_INTERNAL`.
    pub const SOFTWARE: u8 = 70;
    /// EX_TEMPFAIL: `E_BUSY`, `E_RATE`, `E_QUOTA`, `E_DISK`, `E_SHUTTING_DOWN`, `E_NO_BODY`, `E_ROOT`.
    pub const TEMPFAIL: u8 = 75;
    /// EX_PROTOCOL: `E_PROTO`, `E_VERSION`, `E_UNKNOWN_CMD`, unparsable response.
    pub const PROTOCOL: u8 = 76;
    /// EX_NOPERM: `E_PEER`, `E_LANE`, `E_HALTED`, `E_SAFE_MODE`, `E_PROVISIONAL`,
    /// `E_PROVISIONAL_DEPTH`, `E_LEVEL`, `E_CEILING`.
    pub const NOPERM: u8 = 77;
}

/// Whether a client may retry (the "Retry?" column of §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retry {
    /// Never: the request itself is wrong.
    No,
    /// Yes, as is (after the hinted wait, e.g. 1 s for `E_RATE`).
    Yes,
    /// Possibly; depends on the cause.
    Maybe,
    /// After a condition changes (a fix, a level-up, a reset, a cleanup, a Root clear).
    After,
}

macro_rules! codes {
    ($( $(#[$m:meta])* $var:ident = $s:literal, $exit:expr, $retry:ident; )*) => {
        /// An initctl error code (kernel/initctl.md §4.2).
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub enum ErrorCode {
            $( $(#[$m])* $var, )*
            /// A dotted code registered by another doc (`trailer.*`, `predicts.*`, `gate.*`), or
            /// any code this build does not know.
            Other(String),
        }

        impl ErrorCode {
            /// Every `E_*` code, in table order.
            pub const ALL: &'static [ErrorCode] = &[ $( ErrorCode::$var, )* ];

            /// The wire string.
            pub fn as_str(&self) -> &str {
                match self {
                    $( ErrorCode::$var => $s, )*
                    ErrorCode::Other(s) => s,
                }
            }

            /// Parse a wire string; unknown codes become [`ErrorCode::Other`].
            pub fn parse(s: &str) -> ErrorCode {
                match s {
                    $( $s => ErrorCode::$var, )*
                    other => ErrorCode::Other(other.to_string()),
                }
            }

            /// The CLI exit code for a final response carrying this code (§4.2, §4.5).
            pub fn exit_code(&self) -> u8 {
                match self {
                    $( ErrorCode::$var => $exit, )*
                    ErrorCode::Other(_) => exit::FAILED,
                }
            }

            /// The retry rule of §4.2.
            pub fn retry(&self) -> Retry {
                match self {
                    $( ErrorCode::$var => Retry::$retry, )*
                    ErrorCode::Other(_) => Retry::After,
                }
            }
        }
    };
}

codes! {
    /// Malformed frame, line too long, not JSON.
    Proto = "E_PROTO", exit::PROTOCOL, No;
    /// Unsupported `v`.
    Version = "E_VERSION", exit::PROTOCOL, No;
    /// Unknown `cmd`.
    UnknownCmd = "E_UNKNOWN_CMD", exit::PROTOCOL, No;
    /// Invalid or missing args.
    Args = "E_ARGS", exit::USAGE, No;
    /// `wait:true` on a job that needs the caller's own step boundary.
    WouldDeadlock = "E_WOULD_DEADLOCK", exit::USAGE, No;
    /// Peer authentication failed.
    Peer = "E_PEER", exit::NOPERM, No;
    /// Command not available on this lane, or a job of another lane.
    Lane = "E_LANE", exit::NOPERM, No;
    /// The kernel is in its own `Halted`.
    Halted = "E_HALTED", exit::NOPERM, After;
    /// Body or system safe mode.
    SafeMode = "E_SAFE_MODE", exit::NOPERM, After;
    /// The level does not grant the command; `data: {need, have}`. Refused locally.
    Level = "E_LEVEL", exit::NOPERM, After;
    /// A Root ceiling would be exceeded; `data: {ceiling, limit, used}`. Refused locally.
    Ceiling = "E_CEILING", exit::NOPERM, After;
    /// Not allowed while provisional.
    Provisional = "E_PROVISIONAL", exit::NOPERM, After;
    /// Provisional chain too deep.
    ProvisionalDepth = "E_PROVISIONAL_DEPTH", exit::NOPERM, After;
    /// Conflicting pipeline / handoff / queue full; `data.job`, `data.reason`.
    Busy = "E_BUSY", exit::TEMPFAIL, Yes;
    /// Rate or connection limit.
    Rate = "E_RATE", exit::TEMPFAIL, Yes;
    /// A count quota is used up; `data.quota`, `data.limit`, `data.resets_at`.
    Quota = "E_QUOTA", exit::TEMPFAIL, After;
    /// `budget set` violates a bound; `data: {key, limit}`.
    Budget = "E_BUDGET", exit::FAILED, After;
    /// Control link down, stale level/ceilings, outbox backlog, or a Root refusal.
    Root = "E_ROOT", exit::TEMPFAIL, Yes;
    /// Free space below threshold.
    Disk = "E_DISK", exit::TEMPFAIL, After;
    /// The kernel is stopping.
    ShuttingDown = "E_SHUTTING_DOWN", exit::TEMPFAIL, Yes;
    /// No live body to deliver to.
    NoBody = "E_NO_BODY", exit::TEMPFAIL, Yes;
    /// Ref, repo, path, job or proposal not found.
    NotFound = "E_NOT_FOUND", exit::FAILED, No;
    /// git fetch failed or timed out.
    Fetch = "E_FETCH", exit::FAILED, Maybe;
    /// Candidate does not descend from lineage `main`.
    NotDescendant = "E_NOT_DESCENDANT", exit::FAILED, After;
    /// A commit in range is not by the agent identity and not upstream.
    ForeignCommit = "E_FOREIGN_COMMIT", exit::FAILED, No;
    /// Submodule, escaping symlink, too many entries.
    Tree = "E_TREE", exit::FAILED, No;
    /// Size limit (range, tree, patch, vendor).
    TooLarge = "E_TOO_LARGE", exit::FAILED, No;
    /// A gate stage failed without a specific `gate.*` code; `data: {stage, reason, report}`.
    GateFailed = "E_GATE_FAILED", exit::FAILED, After;
    /// The new body did not take over; the old body resumed.
    HandoffFailed = "E_HANDOFF_FAILED", exit::FAILED, After;
    /// The body did not reach a step boundary.
    PauseTimeout = "E_PAUSE_TIMEOUT", exit::FAILED, Yes;
    /// Rollback requested while on LKG.
    AlreadyLkg = "E_ALREADY_LKG", exit::FAILED, No;
    /// Non-crates.io source in `Cargo.toml`.
    VendorSource = "E_VENDOR_SOURCE", exit::FAILED, No;
    /// Crate/version not found, egress refused, or network error.
    VendorFetch = "E_VENDOR_FETCH", exit::FAILED, Maybe;
    /// Task admission could not run (infrastructure).
    Admission = "E_ADMISSION", exit::FAILED, Maybe;
    /// A kernel swap failed after the gate-of-gates passed; `data: {stage, reason}`.
    KupgradeFailed = "E_KUPGRADE_FAILED", exit::FAILED, After;
    /// Job past its cancellable stages.
    NotCancellable = "E_NOT_CANCELLABLE", exit::FAILED, No;
    /// Job was cancelled.
    Cancelled = "E_CANCELLED", exit::FAILED, No;
    /// Job deadline passed.
    Timeout = "E_TIMEOUT", exit::FAILED, Maybe;
    /// Kernel bug; details in `kernel.jsonl`.
    Internal = "E_INTERNAL", exit::SOFTWARE, Maybe;
}

impl ErrorCode {
    /// True for a dotted code (evolution.md, gate.md), false for `E_*`.
    pub fn is_dotted(&self) -> bool {
        matches!(self, ErrorCode::Other(s) if s.contains('.'))
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(ErrorCode::parse(&s))
    }
}

/// `data.quota` values of `E_QUOTA` (§4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaName {
    PromoteAttemptsTask,
    PromotesDay,
    QuickGatesDay,
    /// 20 per day.
    VendorDay,
    /// 5 per day.
    ProposalsDay,
    /// `budget.task_submits_day`, default 10.
    TaskSubmitsDay,
    /// 50 per day.
    FleetSpawnsDay,
    /// ADR-011 §4.5 (Root `extend.budget`): open groups, changes per owner day, `group_max`, the
    /// H5a freeze, the kernel lane.
    ExtendBudget,
    /// ADR-011 §6.4 (`extend.bloat`).
    ExtendBloat,
    /// ADR-011 §6.2 (`extend.archive_debt`).
    ExtendArchiveDebt,
    /// ADR-011 §6.3 (`extend.quarantine_debt`).
    ExtendQuarantineDebt,
    /// ADR-011 §3.5 (`extend.readmit`).
    ExtendReadmit,
    /// ADR-011 §7.4 (`extend.dep_grant`): no live owner `dep` grant covers the change's exact crate set.
    ExtendDepGrant,
}

/// `data.reason` values of `E_BUSY` (§4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusyReason {
    Pipeline,
    Handoff,
    QueueFull,
    ThinningBaseline,
    KernelCandidate,
}

/// `data.reason` values of `E_ROOT` that the kernel sets itself (§3.5, §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootReason {
    /// The control link is down.
    LinkDown,
    /// The cached level or ceilings are older than 120 s.
    Stale,
    /// Outbox backlog (supervise.md §3.13.3).
    OutboxBacklog,
    /// The Root answered with an error; `data.root_code` holds it.
    Refused,
}

/// `error.data` of `E_LEVEL`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelData {
    pub need: u8,
    pub have: u8,
}

/// `error.data` of `E_CEILING`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CeilingData {
    pub ceiling: String,
    pub limit: u64,
    pub used: u64,
}

/// `error.data` of `E_QUOTA`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaData {
    pub quota: QuotaName,
    pub limit: u64,
    /// RFC 3339.
    pub resets_at: String,
}

/// `error.data` of `E_BUSY`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusyData {
    /// The blocking job, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<BusyReason>,
}

/// `error.data` of `E_ROOT`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<RootReason>,
    /// A dotted `root.*` code (root/root.md §4.1.2) when the Root refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_code: Option<String>,
}

/// `error.data` of `E_GATE_FAILED` and of a stage's `gate.*` code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateFailedData {
    pub stage: String,
    pub reason: String,
    /// `state/kernel/gate/<job>/report.json`; null when no report exists (e.g. the gate-of-gates).
    pub report: Option<String>,
}

/// `error.data` of `E_KUPGRADE_FAILED`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KupgradeFailedData {
    /// `store`, `prepare`, `boot`, `probation`.
    pub stage: String,
    pub reason: String,
}

/// `error.data` of `E_BUDGET`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetData {
    pub key: String,
    pub limit: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_table() {
        use ErrorCode::*;
        // One assertion per row of kernel/initctl.md §4.5's exit table.
        for c in [Proto, Version, UnknownCmd] {
            assert_eq!(c.exit_code(), 76, "{c}");
        }
        for c in [Args, WouldDeadlock] {
            assert_eq!(c.exit_code(), 64, "{c}");
        }
        for c in [Peer, Lane, Halted, SafeMode, Provisional, ProvisionalDepth, Level, Ceiling] {
            assert_eq!(c.exit_code(), 77, "{c}");
        }
        for c in [Busy, Rate, Quota, Disk, ShuttingDown, NoBody, Root] {
            assert_eq!(c.exit_code(), 75, "{c}");
        }
        assert_eq!(Internal.exit_code(), 70);
        for c in [Budget, NotFound, GateFailed, Cancelled, Timeout, KupgradeFailed] {
            assert_eq!(c.exit_code(), 1, "{c}");
        }
        assert_eq!(ErrorCode::parse("trailer.missing:Predicts").exit_code(), 1);
        assert_eq!(ErrorCode::parse("gate.g1.self_json_drift").exit_code(), 1);
        assert_eq!(ErrorCode::parse("E_SOMETHING_NEW").exit_code(), 1);
    }

    #[test]
    fn round_trip_every_code() {
        assert_eq!(ErrorCode::ALL.len(), 39);
        for c in ErrorCode::ALL {
            let s = serde_json::to_string(c).unwrap();
            let back: ErrorCode = serde_json::from_str(&s).unwrap();
            assert_eq!(&back, c);
            assert!(c.as_str().starts_with("E_"));
            assert!(!c.is_dotted());
        }
        let g = ErrorCode::parse("gate.g4.inconclusive_final");
        assert!(g.is_dotted());
        assert_eq!(serde_json::to_string(&g).unwrap(), "\"gate.g4.inconclusive_final\"");
    }

    #[test]
    fn retry_column() {
        assert_eq!(ErrorCode::Busy.retry(), Retry::Yes);
        assert_eq!(ErrorCode::Proto.retry(), Retry::No);
        assert_eq!(ErrorCode::Fetch.retry(), Retry::Maybe);
        assert_eq!(ErrorCode::Level.retry(), Retry::After);
    }
}
