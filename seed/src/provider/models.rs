//! The seed model table (body/provider.md §3.3) with each model's `CacheProfile` (values: §3.2).
//! The boot check accepts exactly these models; anything else exits 78 (boot.md §3.4).
//! Grown model-specific behavior goes in `src/profiles/<family>.rs`, not here.

use crate::context::model::{Breakpoints, CacheProfile, NotePlacement, Target};
use crate::model::{Api, Effort, ModelInfo, ReasoningRule};

const fn deepseek_cache(provider: &'static str, window: u32, max_tokens: u32) -> CacheProfile {
    CacheProfile {
        provider,
        model_window: window,
        max_output: max_tokens,
        strip_thinking_before_epoch: false, // DeepSeek returns 400 without reasoning_content in tool mode
        note_placement: NotePlacement::UserMessage,
        breakpoints: Breakpoints::None, // automatic disk cache
        replay_raw_arguments: true,
        min_cacheable_tokens: 0,
    }
}

const fn anthropic_cache(window: u32, max_tokens: u32) -> CacheProfile {
    CacheProfile {
        provider: "anthropic",
        model_window: window,
        max_output: max_tokens,
        strip_thinking_before_epoch: true,
        note_placement: NotePlacement::MidSystemMessage,
        breakpoints: Breakpoints::Anthropic { max: 4 },
        replay_raw_arguments: false,
        min_cacheable_tokens: 512,
    }
}

const fn openai_cache(window: u32, max_tokens: u32) -> CacheProfile {
    CacheProfile {
        provider: "openai",
        model_window: window,
        max_output: max_tokens,
        strip_thinking_before_epoch: true, // conservative; UNVERIFIED (provider.md §3.2)
        note_placement: NotePlacement::DeveloperMessage,
        breakpoints: Breakpoints::OpenAiExplicit { max: 4 },
        replay_raw_arguments: true,
        min_cacheable_tokens: 1_024,
    }
}

const fn deepseek(id: &'static str, images: bool) -> ModelInfo {
    ModelInfo {
        provider: "deepseek",
        api: Api::ChatCompletions,
        id,
        context_window: 1_000_000,
        max_output: 393_216,
        default_max_tokens: 65_536,
        default_effort: Effort::High,
        images,
        reasoning: ReasoningRule::PassBackPlain,
        cache: deepseek_cache("deepseek", 1_000_000, 65_536),
    }
}

const fn anthropic(id: &'static str) -> ModelInfo {
    ModelInfo {
        provider: "anthropic",
        api: Api::Messages,
        id,
        context_window: 1_000_000,
        max_output: 128_000,
        default_max_tokens: 64_000,
        default_effort: Effort::High,
        images: true,
        reasoning: ReasoningRule::SignedModelBound,
        cache: anthropic_cache(1_000_000, 64_000),
    }
}

const fn openai(id: &'static str) -> ModelInfo {
    ModelInfo {
        provider: "openai",
        api: Api::Responses,
        id,
        context_window: 922_000, // the documented maximum input (total window 1,050,000)
        max_output: 128_000,
        default_max_tokens: 64_000,
        default_effort: Effort::Medium,
        images: true,
        reasoning: ReasoningRule::EncryptedItems,
        cache: openai_cache(922_000, 64_000),
    }
}

/// provider.md §3.3 seed model table.
pub static SEED_MODELS: [ModelInfo; 7] = [
    deepseek("deepseek-flash", true),
    deepseek("deepseek-v4-pro", false),
    anthropic("claude-opus-5-5"),
    anthropic("claude-sonnet-5-5"),
    openai("gpt-6-astra"),
    openai("gpt-6.1-sol"),
    // DeepSeek wire format; the proxy routes `mock` only in test mode (kernel/proxy.md §4.1).
    ModelInfo {
        provider: "mock",
        api: Api::ChatCompletions,
        id: "mock-frontier-1",
        context_window: 131_072,
        max_output: 32_768,
        default_max_tokens: 8_192,
        default_effort: Effort::High,
        images: false,
        reasoning: ReasoningRule::PassBackPlain,
        cache: deepseek_cache("mock", 131_072, 8_192),
    },
];

/// The table row for `(provider, model id)` from boot.json, or None (boot exits 78).
pub fn lookup(provider: &str, id: &str) -> Option<&'static ModelInfo> {
    SELECTED_INFO.get().filter(|m| m.provider == provider && m.id == id)
        .or_else(|| SEED_MODELS.iter().find(|m| m.provider == provider && m.id == id))
}

/// log-context's `Target` for a model; `effort: None` means `default_effort`.
pub fn target(info: &ModelInfo, effort: Option<Effort>) -> Target {
    Target {
        provider: info.provider.to_string(),
        api: info.api,
        model: info.id.to_string(),
        effort: super::effort_wire(info.api, effort.unwrap_or(info.default_effort)).to_string(),
        context_window: info.context_window,
        max_tokens: info.default_max_tokens,
        images: info.images,
        profile: info.cache.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_rows_and_lookup() {
        assert_eq!(SEED_MODELS.len(), 7);
        let f = lookup("deepseek", "deepseek-flash").unwrap();
        assert_eq!(
            (f.context_window, f.max_output, f.default_max_tokens),
            (1_000_000, 393_216, 65_536)
        );
        assert!(f.images);
        assert!(!lookup("deepseek", "deepseek-v4-pro").unwrap().images);
        assert_eq!(
            lookup("openai", "gpt-6-astra").unwrap().default_effort,
            Effort::Medium
        );
        assert_eq!(
            lookup("anthropic", "claude-opus-5-5")
                .unwrap()
                .cache
                .min_cacheable_tokens,
            512
        );
        assert!(lookup("deepseek", "deepseek-chat").is_none());
        assert!(lookup("mock", "deepseek-flash").is_none());
        for m in &SEED_MODELS {
            assert_eq!(m.cache.max_output, m.default_max_tokens, "{}", m.id);
            assert_eq!(m.cache.model_window as u64, m.context_window, "{}", m.id);
            assert!(m.default_max_tokens <= m.max_output);
        }
    }

    /// The table agrees with WP12's hand-written targets.
    #[test]
    fn targets_match_context() {
        assert_eq!(
            target(lookup("deepseek", "deepseek-flash").unwrap(), None),
            Target::deepseek_flash()
        );
        assert_eq!(
            target(lookup("mock", "mock-frontier-1").unwrap(), None),
            Target::mock()
        );
        let t = target(
            lookup("deepseek", "deepseek-flash").unwrap(),
            Some(Effort::Max),
        );
        assert_eq!(t.effort, "max");
        let t = target(
            lookup("deepseek", "deepseek-flash").unwrap(),
            Some(Effort::Medium),
        );
        assert_eq!(t.effort, "high");
        let t = target(
            lookup("anthropic", "claude-opus-5-5").unwrap(),
            Some(Effort::XHigh),
        );
        assert_eq!(t.effort, "xhigh");
    }
}

/// One explicit model selected for this process. No endpoint or auth authority lives here.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub api: String,
    pub context_window: u32,
    pub max_output: u32,
    pub default_max_tokens: u32,
    pub request_max_tokens: u32,
    /// Request standard Chat stream usage (default true); false supports servers rejecting it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_stream_usage: Option<bool>,
    /// Opt into model-specific reasoning/cache extensions for a custom native adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_extensions: Option<bool>,
}

impl Descriptor {
    fn validate(&self, provider: &str, id: &str) -> Result<Api, String> {
        if provider.is_empty() || provider.len() > 32 || provider == "_init"
            || !provider.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            || !provider.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
            || id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
            return Err("invalid bounded provider/model identifier".into());
        }
        let api = match self.api.as_str() {
            "chat_completions" => Api::ChatCompletions,
            "messages" => Api::Messages,
            "responses" => Api::Responses,
            _ => return Err("unknown API wire format".into()),
        };
        if !(1024..=16_777_216).contains(&self.context_window)
            || self.max_output == 0 || self.max_output >= self.context_window
            || self.default_max_tokens == 0 || self.default_max_tokens > self.max_output
            || self.request_max_tokens == 0 || self.request_max_tokens > self.max_output {
            return Err("invalid model window/output/request limits".into());
        }
        if let Some(known) = SEED_MODELS.iter().find(|m| m.provider == provider && m.id == id) {
            if api != known.api {
                return Err("descriptor cannot change a known model's wire format".into());
            }
        } else if matches!(provider, "deepseek" | "mock") {
            return Err("unknown DeepSeek/mock model requires an explicit qualified reasoning profile".into());
        }
        Ok(api)
    }
}

#[derive(PartialEq, Eq)]
struct Selected {
    provider: String,
    id: String,
    descriptor: Descriptor,
}
static SELECTED: std::sync::OnceLock<Selected> = std::sync::OnceLock::new();
static SELECTED_INFO: std::sync::OnceLock<ModelInfo> = std::sync::OnceLock::new();

/// Bounded to exactly one descriptor per body process, retaining owned strings without leaking.
/// Repeated identical registration is idempotent; changing model requires restarting the body.
pub fn register(provider: &str, id: &str, descriptor: Descriptor) -> Result<&'static ModelInfo, String> {
    let api = descriptor.validate(provider, id)?;
    let candidate = Selected { provider: provider.into(), id: id.into(), descriptor: descriptor.clone() };
    let selected = SELECTED.get_or_init(|| candidate);
    // Compare the caller's complete configuration even when another boot won initialization.
    // (Boot runs once in production; this also fails closed for accidental reconfiguration.)
    if selected.provider != provider || selected.id != id || selected.descriptor != descriptor {
        return Err("a different model is already registered in this process".into());
    }
    let info = SELECTED_INFO.get_or_init(|| {
        let d = &selected.descriptor;
        let known = SEED_MODELS.iter().find(|m| m.provider == provider && m.id == id);
        let mut info = known.cloned().unwrap_or_else(|| ModelInfo {
            provider: &selected.provider,
            id: &selected.id,
            api,
            context_window: u64::from(d.context_window),
            max_output: d.max_output,
            default_max_tokens: d.default_max_tokens,
            default_effort: Effort::Medium,
            images: false,
            reasoning: match api { Api::Messages => ReasoningRule::SignedModelBound, Api::Responses => ReasoningRule::EncryptedItems, Api::ChatCompletions => ReasoningRule::PassBackPlain },
            cache: CacheProfile {
                provider: &selected.provider,
                model_window: d.context_window,
                max_output: d.default_max_tokens,
                strip_thinking_before_epoch: true,
                note_placement: NotePlacement::UserMessage,
                breakpoints: Breakpoints::None,
                replay_raw_arguments: true,
                min_cacheable_tokens: 0,
            },
        });
        info.context_window = u64::from(d.context_window);
        info.max_output = d.max_output;
        info.default_max_tokens = d.default_max_tokens.min(d.request_max_tokens);
        info.cache.model_window = d.context_window;
        info.cache.max_output = info.default_max_tokens;
        info
    });
    Ok(info)
}

/// Optional request features are metadata, never route or credential authority.
pub fn chat_stream_usage(provider: &str, id: &str) -> bool {
    SELECTED.get().filter(|s| s.provider == provider && s.id == id)
        .and_then(|s| s.descriptor.chat_stream_usage).unwrap_or(true)
}
pub fn native_extensions(info: &ModelInfo) -> bool {
    SELECTED.get().filter(|s| s.provider == info.provider && s.id == info.id)
        .and_then(|s| s.descriptor.native_extensions)
        .unwrap_or_else(|| SEED_MODELS.iter().any(|m| m.provider == info.provider && m.id == info.id))
}

/// Explicit owner request policy is separate from the model's advertised output maximum.
pub fn request_tokens(provider: &str, id: &str, requested: u32) -> u32 {
    let cap = SELECTED.get().filter(|s| s.provider == provider && s.id == id)
        .map(|s| s.descriptor.request_max_tokens);
    let maximum = lookup(provider, id).map(|m| m.max_output);
    requested.min(cap.unwrap_or(u32::MAX)).min(maximum.unwrap_or(u32::MAX))
}

#[cfg(test)]
mod descriptor_tests {
    use super::*;
    fn descriptor() -> Descriptor {
        Descriptor { api: "chat_completions".into(), context_window: 131072,
            max_output: 32768, default_max_tokens: 8192, request_max_tokens: 4096, chat_stream_usage: None, native_extensions: None }
    }
    #[test]
    fn bounded_descriptor_validation_is_conservative() {
        let d = descriptor();
        assert_eq!(d.validate("local", "org/custom-model").unwrap(), Api::ChatCompletions);
        for provider in ["", "../escape", "https://host", "local/name"] {
            assert!(d.validate(provider, "m").is_err());
        }
        assert!(d.validate("local", &"m".repeat(257)).is_err());
        assert!(d.validate("local", "bad\nmodel").is_err());
        let mut bad = d.clone(); bad.request_max_tokens = 0;
        assert!(bad.validate("local", "m").is_err());
        bad = d.clone(); bad.max_output = bad.context_window;
        assert!(bad.validate("local", "m").is_err());
        bad = d.clone(); bad.api = "responses".into();
        assert_eq!(bad.validate("local", "m").unwrap(), Api::Responses);
        bad.api = "messages".into();
        assert_eq!(bad.validate("custom", "m").unwrap(), Api::Messages);
        assert!(bad.validate("deepseek", "deepseek-flash").is_err());
        assert!(serde_json::from_value::<Descriptor>(serde_json::json!({
            "api":"chat_completions", "context_window":131072, "max_output":8192,
            "default_max_tokens":4096, "request_max_tokens":4096, "images":true
        })).is_err());
    }
    #[test]
    fn descriptor_registration_and_request_cap() {
        // A unique custom id avoids mutating static model behavior in other parallel tests.
        let mut d = descriptor();
        d.chat_stream_usage = Some(false);
        d.native_extensions = Some(true);
        let info = register("descriptor-test", "custom-test-model", d.clone()).unwrap();
        assert_eq!(info.max_output, 32768);
        assert_eq!(info.default_max_tokens, 4096);
        assert!(!info.images);
        assert!(!chat_stream_usage(info.provider, info.id));
        assert!(native_extensions(info));
        assert!(std::ptr::eq(info, register("descriptor-test", "custom-test-model", d).unwrap()));
        assert_eq!(request_tokens(info.provider, info.id, 16000), 4096);
        assert_eq!(request_tokens(info.provider, info.id, 2048), 2048);
        assert_eq!(lookup(info.provider, info.id).unwrap().api, Api::ChatCompletions);
        let adapter = crate::provider::for_model(info);
        for requested in [8192, 16000, 32768] {
            let request = adapter.build(&crate::provider::Prompt {
                head: crate::provider::PromptHead { core: "core", self_: "self" },
                tools: &[], messages: &[], b3: None, max_tokens: requested,
                effort: Effort::Medium, session: "descriptor-test",
            }).unwrap();
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["max_tokens"], 4096);
            assert!(body.get("stream_options").is_none());
            assert!(body.get("thinking").is_none());
            assert!(body.get("reasoning_effort").is_none());
        }

        let mut changed = descriptor(); changed.request_max_tokens = 2048;
        assert!(register(info.provider, info.id, changed).is_err());
        assert!(register("other", "m", descriptor()).is_err());
    }
}
