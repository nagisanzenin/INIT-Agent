//! Per-run caps of G4 and screen runs (m7-design.md §3.1.3, §3.2.8; kernel/gate.md §3.11): the
//! task's task.json v3 `limits`, else the pool defaults. The proxy enforces them on actuals on the
//! run's lane (kernel-proxy `ProxyHandle::set_run_caps`); a run that hits one ends `cost_capped`, is
//! scored on the workspace it left, and its RunRecord names the cap (`caps_hit`).

use serde::{Deserialize, Serialize};

use crate::g4::Limits;

/// `max_turns` for a pool task without v3 limits.
pub const DEFAULT_MAX_TURNS: u64 = 12;
/// `max_output_tokens` (reasoning included) for a pool task without v3 limits.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 24_000;
/// `max_input_tokens` (cached included) for a pool task without v3 limits.
pub const DEFAULT_MAX_INPUT_TOKENS: u64 = 400_000;

/// The caps one run's lane gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCaps {
    pub max_turns: u64,
    pub max_output_tokens: u64,
    pub max_input_tokens: u64,
}

impl Default for RunCaps {
    fn default() -> Self {
        RunCaps {
            max_turns: DEFAULT_MAX_TURNS,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            max_input_tokens: DEFAULT_MAX_INPUT_TOKENS,
        }
    }
}

impl RunCaps {
    /// The task's v3 limits, each absent one replaced by its pool default.
    pub fn for_task(l: &Limits) -> RunCaps {
        RunCaps {
            max_turns: l.max_turns.unwrap_or(DEFAULT_MAX_TURNS),
            max_output_tokens: l.max_output_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS),
            max_input_tokens: l.max_input_tokens.unwrap_or(DEFAULT_MAX_INPUT_TOKENS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M7-G07 / M7-G14: a v2 task (no caps) gets the pool defaults; v3 caps are taken as given.
    #[test]
    fn caps_from_limits() {
        let mut l = Limits { wall_s: 600, max_cost_musd: 100_000, max_turns: None, max_output_tokens: None, max_input_tokens: None };
        assert_eq!(RunCaps::for_task(&l), RunCaps { max_turns: 12, max_output_tokens: 24_000, max_input_tokens: 400_000 });
        l.max_turns = Some(8);
        l.max_input_tokens = Some(50_000);
        assert_eq!(RunCaps::for_task(&l), RunCaps { max_turns: 8, max_output_tokens: 24_000, max_input_tokens: 50_000 });
    }
}
