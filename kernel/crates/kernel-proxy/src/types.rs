//! Small shared enums of the proxy control API (kernel/proxy.md §3.4, §4.4).

use serde::{Deserialize, Serialize};

/// The autonomy level (root/root.md R7), read from the Root.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum Level {
    #[default]
    L0,
    L1,
    L2,
    L3,
    L4,
}

impl Level {
    pub fn from_u8(n: u8) -> Level {
        match n {
            0 => Level::L0,
            1 => Level::L1,
            2 => Level::L2,
            3 => Level::L3,
            _ => Level::L4,
        }
    }
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn as_str(self) -> &'static str {
        ["L0", "L1", "L2", "L3", "L4"][self as usize]
    }
}

/// What a lane socket is for (§3.4).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ScopeKind {
    Live,
    /// An L2 fleet member, by body id.
    Member(String),
    Probe,
    G3,
    Run,
    Quick,
}

impl ScopeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScopeKind::Live => "live",
            ScopeKind::Member(_) => "member",
            ScopeKind::Probe => "probe",
            ScopeKind::G3 => "g3",
            ScopeKind::Run => "run",
            ScopeKind::Quick => "quick",
        }
    }
    /// Gate scopes: attribution is the scope itself, budgets are the gate's.
    pub fn is_gate(&self) -> bool {
        matches!(
            self,
            ScopeKind::Probe | ScopeKind::G3 | ScopeKind::Run | ScopeKind::Quick
        )
    }
    /// `x-egress-scope` (root/egress.md §4.1; `member` per proxy.md Q-EG7).
    pub fn egress_scope(&self) -> &'static str {
        match self {
            ScopeKind::Live => "live",
            ScopeKind::Member(_) => "member",
            _ => "gate",
        }
    }
}

/// Who may read a scope's traces (§3.14).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Visibility {
    #[default]
    Agent,
    Kernel,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Agent => "agent",
            Visibility::Kernel => "kernel",
        }
    }
}

/// Promote gate or quick gate (`budget.gate_usd` / `budget.quick_gate_usd`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GateKind {
    Promote,
    Quick,
}

/// A scope label (§3.4): `{sha, boot_reason, model (default), level}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub sha: String,
    pub boot_reason: String,
    /// The default model (`model.id`).
    pub model: String,
    pub level: Level,
}

impl Label {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"boot_reason": self.boot_reason, "level": self.level.as_str(), "model": self.model, "sha": self.sha})
    }
}
