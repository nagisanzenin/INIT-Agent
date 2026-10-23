//! What the context needs to know about the target model: wire format, `CacheProfile`
//! (log-context.md §3.10.0; values owned by body/provider.md §3.2), compaction thresholds (§3.12.1)
//! and token calibration (§3.14).

use crate::log::{K0_PPM, tokens_for_bytes};

/// Wire format of a request (body/provider.md `Api`; kernel/proxy.md route family).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    ChatCompletions,
    Messages,
    Responses,
}

impl Api {
    pub fn as_str(self) -> &'static str {
        match self {
            Api::ChatCompletions => "chat_completions",
            Api::Messages => "messages",
            Api::Responses => "responses",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotePlacement {
    MidSystemMessage,
    TextAfterToolResults,
    UserMessage,
    DeveloperMessage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Breakpoints {
    None,
    Anthropic { max: u8 },
    OpenAiExplicit { max: u8 },
}

/// log-context.md §3.10.0 (contract with body/provider.md).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheProfile {
    pub provider: &'static str,
    pub model_window: u32,
    /// `max_tokens` this body requests per call.
    pub max_output: u32,
    pub strip_thinking_before_epoch: bool,
    pub note_placement: NotePlacement,
    pub breakpoints: Breakpoints,
    pub replay_raw_arguments: bool,
    pub min_cacheable_tokens: u32,
}

impl CacheProfile {
    /// Seed DeepSeek values (body/provider.md §3.2); `mock` uses the same.
    pub fn deepseek(provider: &'static str, model_window: u32, max_output: u32) -> Self {
        CacheProfile {
            provider,
            model_window,
            max_output,
            strip_thinking_before_epoch: false,
            note_placement: NotePlacement::UserMessage,
            breakpoints: Breakpoints::None,
            replay_raw_arguments: true,
            min_cacheable_tokens: 0,
        }
    }
}

/// The model a request is rendered for. Built by the provider layer from its `ModelInfo` (WP10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// Proxy provider id: `deepseek` | `anthropic` | `openai` | `mock`.
    pub provider: String,
    pub api: Api,
    /// Model id as sent.
    pub model: String,
    /// The wire effort value (DeepSeek `reasoning_effort`: `low` | `high` | `max`).
    pub effort: String,
    pub context_window: u64,
    /// Default `max_tokens` per request.
    pub max_tokens: u32,
    pub images: bool,
    pub profile: CacheProfile,
}

impl Target {
    /// `deepseek-flash` (body/provider.md §3.3 seed table).
    pub fn deepseek_flash() -> Self {
        Target {
            provider: "deepseek".into(),
            api: Api::ChatCompletions,
            model: "deepseek-flash".into(),
            effort: "high".into(),
            context_window: 1_000_000,
            max_tokens: 65_536,
            images: true,
            profile: CacheProfile::deepseek("deepseek", 1_000_000, 65_536),
        }
    }

    /// `mock-frontier-1` (DeepSeek wire format).
    pub fn mock() -> Self {
        Target {
            provider: "mock".into(),
            api: Api::ChatCompletions,
            model: "mock-frontier-1".into(),
            effort: "high".into(),
            context_window: 131_072,
            max_tokens: 8_192,
            images: false,
            profile: CacheProfile::deepseek("mock", 131_072, 8_192),
        }
    }
}

pub const EFFECTIVE_WINDOW_CAP: u64 = 200_000;
pub const RESERVE_TOKENS: u64 = 16_384;
pub const KEEP_RECENT_CAP: u64 = 20_000;
pub const MASK_MIN_TOKENS: u64 = 256;
pub const MASK_ERROR_MIN_TOKENS: u64 = 2_000;
pub const MASK_INPUT_MIN_TOKENS: u64 = 1_000;
/// A string inside a masked tool input is replaced when its estimate exceeds this (about twice the
/// placeholder). log-context.md §3.11 says "the long string values" without a number (deviation note).
pub const MASK_INPUT_STRING_MIN_TOKENS: u64 = 64;
pub const SUMMARY_MAX_OUTPUT: u32 = 16_000;
pub const SUMMARY_SOFT_CAP: u64 = 8_000;
pub const OWNER_MSG_CHARS: usize = 2_000;
pub const OWNER_MSGS_KEPT: usize = 20;
pub const ARTIFACT_ROWS: usize = 200;
/// Manual `/compact` does nothing under this many tokens (§3.12.6).
pub const MANUAL_COMPACT_MIN: u64 = 10_000;
/// Thrash guard: a rebuild that would be the 3rd within 6 consecutive requests is suspended.
pub const THRASH_WINDOW: usize = 6;
pub const THRASH_MAX: usize = 3;
pub const IMAGE_DEFAULT_TOKENS: u64 = 1_600;

/// §3.12.1
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thresholds {
    pub w_eff: u64,
    pub t_trigger: u64,
    pub t_target: u64,
    pub keep: u64,
    pub max_tail: u64,
    /// The request no longer fits: `model_window − max_output`.
    pub hard_limit: u64,
}

impl Thresholds {
    pub fn new(model_window: u64, max_output: u64) -> Self {
        let w_eff = model_window.min(EFFECTIVE_WINDOW_CAP);
        let t_trigger = (w_eff.saturating_sub(RESERVE_TOKENS))
            .min(model_window.saturating_sub(max_output + 1_024));
        Thresholds {
            w_eff,
            t_trigger,
            t_target: w_eff / 2,
            keep: KEEP_RECENT_CAP.min(w_eff * 15 / 100),
            max_tail: w_eff * 40 / 100,
            hard_limit: model_window.saturating_sub(max_output),
        }
    }

    pub fn for_target(t: &Target) -> Self {
        Self::new(t.context_window, t.profile.max_output as u64)
    }
}

/// Tokens-per-byte calibration (§3.14), in tokens per million bytes (integers only: ICJ rule 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calib {
    pub k_ppm: u64,
}

impl Default for Calib {
    fn default() -> Self {
        Calib { k_ppm: K0_PPM }
    }
}

impl Calib {
    pub const MIN_PPM: u64 = 120_000;
    pub const MAX_PPM: u64 = 600_000;

    /// `k ← clamp(0.8·k + 0.2·k_obs, 0.12, 0.60)` with `k_obs = (P − I_req) / B_req`.
    pub fn observe(&mut self, prompt_tokens: u64, image_tokens: u64, request_bytes: u64) {
        if request_bytes == 0 {
            return;
        }
        let k_obs = (prompt_tokens.saturating_sub(image_tokens) as u128 * 1_000_000
            / request_bytes as u128) as u64;
        let k = (4 * self.k_ppm + k_obs) / 5;
        self.k_ppm = k.clamp(Self::MIN_PPM, Self::MAX_PPM);
    }

    pub fn tokens(&self, bytes: u64) -> u64 {
        tokens_for_bytes(self.k_ppm, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §3.12.1 table.
    #[test]
    fn thresholds_table() {
        let t = |w: u64| Thresholds::new(w, 8_192);
        let row = |x: Thresholds| (x.w_eff, x.t_trigger, x.t_target, x.keep, x.max_tail);
        assert_eq!(row(t(64_000)), (64_000, 47_616, 32_000, 9_600, 25_600));
        assert_eq!(row(t(128_000)), (128_000, 111_616, 64_000, 19_200, 51_200));
        assert_eq!(row(t(200_000)), (200_000, 183_616, 100_000, 20_000, 80_000));
        assert_eq!(
            row(t(1_000_000)),
            (200_000, 183_616, 100_000, 20_000, 80_000)
        );
        // The second term binds for 128k with 32k output.
        assert_eq!(Thresholds::new(128_000, 32_000).t_trigger, 94_976);
        assert_eq!(
            Thresholds::for_target(&Target::deepseek_flash()).t_trigger,
            183_616
        );
    }

    #[test]
    fn calibration_converges_and_clamps() {
        let mut c = Calib::default();
        for _ in 0..40 {
            c.observe(300_000, 0, 1_000_000); // k_obs = 0.3
        }
        assert!((299_000..=300_000).contains(&c.k_ppm), "{}", c.k_ppm);
        for _ in 0..40 {
            c.observe(5, 0, 1_000_000);
        }
        assert_eq!(c.k_ppm, Calib::MIN_PPM);
        assert_eq!(Calib { k_ppm: 250_000 }.tokens(10), 3); // ceil(2.5)
    }
}
