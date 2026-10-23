//! The provider (body/provider.md): the only model-specific seam in the body. Pure adapters turn a
//! normalized prompt into request bytes and SSE events into normalized `StreamEvent`s; one shared
//! transport does all I/O to the kernel proxy's Unix socket.
//!
//! The normalized types live in `crate::model` (shared with loop, tools and context). `ModelInfo`,
//! `Effort`, `Purpose`, the byte-based `Request` and `ModelClient` are lane B1's (loop.md §3.2),
//! also in `crate::model`. In the running body the context renders the request bytes
//! (log-context §3.10) and `Client` sends them unchanged; `Provider::build` is the adapter's own
//! renderer from normalized messages (provider.md §3.8), used for tests, fallbacks and APIs the
//! context does not render yet.

pub mod anthropic;
pub mod caps;
pub mod client;
pub mod deepseek;
pub mod handoff;
pub mod http;
pub mod image;
pub mod models;
pub mod openai;
pub mod sse;
pub mod summarizer;
pub mod transport;

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod mock_tests;

use serde_json::Value;

use crate::model::{
    Api, AssistantMessage, Effort, ErrorKind, Message, ModelInfo, ProviderError, StreamEvent,
    ToolSpec,
};

#[allow(unused_imports)]
pub use client::Client;
#[allow(unused_imports)]
pub use http::HttpRequest;
pub use sse::SseEvent;
#[allow(unused_imports)]
pub use transport::{EventStream, Transport};

/// The adapter input: provider.md §3.3's `Request` (named `Prompt` here because `model::Request`
/// is the byte-based request the loop hands over).
#[derive(Clone, Copy, Debug)]
pub struct Prompt<'a> {
    /// log-context §3.8.2: the cached head, in two parts.
    pub head: PromptHead<'a>,
    /// Sorted byte-wise by name (tools.md §4.1).
    pub tools: &'a [ToolSpec],
    /// Projection + grouping, after the handoff filter (§3.10).
    pub messages: &'a [Message],
    /// Anthropic only: index of the message whose last block gets breakpoint B3.
    pub b3: Option<usize>,
    pub max_tokens: u32,
    pub effort: Effort,
    /// `INIT_SESSION` (OpenAI `prompt_cache_key`).
    pub session: &'a str,
}

/// `core` = genesis + constitution; `self_` = SELF.md + skills index.
#[derive(Clone, Copy, Debug)]
pub struct PromptHead<'a> {
    pub core: &'a str,
    pub self_: &'a str,
}

/// One adapter (provider.md §3.4). `build`, `wire`, `decoder`, `classify` and `fallback` are pure.
pub trait Provider: Send + Sync {
    fn info(&self) -> &ModelInfo;
    /// Serializes a prompt. Same prompt ⇒ same bytes (sorted keys, no clocks, no randomness).
    fn build(&self, p: &Prompt) -> Result<HttpRequest, ProviderError>;
    /// Wraps request body bytes (rendered by the context or by `build`) for this API: path + headers.
    fn wire(&self, body: Vec<u8>) -> HttpRequest;
    /// A fresh decoder for one attempt.
    fn decoder(&self) -> Box<dyn Decoder>;
    /// The `ErrorKind` of a non-200 upstream response (the transport handles proxy caps first).
    fn classify(
        &self,
        status: u16,
        headers: &[(String, String)],
        body: Option<&Value>,
        message: &str,
    ) -> ErrorKind;
    /// A one-shot recovery rebuild after a non-retryable error (Anthropic thinking binding,
    /// DeepSeek foreign-turn flatten). `used` lists the causes already applied in this stream.
    /// Returns the new request and the cause.
    fn fallback(
        &self,
        _req: &HttpRequest,
        _err: &ProviderError,
        _used: &[&'static str],
    ) -> Option<(HttpRequest, &'static str)> {
        None
    }
}

pub trait Decoder: Send {
    /// Called once per SSE event (comments never reach it). Pushes zero or more normalized events.
    fn feed(&mut self, ev: &SseEvent, out: &mut Vec<StreamEvent>) -> Result<(), ProviderError>;
    /// True once the API's terminal event was seen.
    fn finished(&self) -> bool;
    /// Assembles the final message. Err(Truncated) if !finished().
    fn finish(&mut self) -> Result<AssistantMessage, ProviderError>;
    /// The partial message for an abort: complete blocks + partial text/thinking; incomplete tool
    /// calls dropped; `stop = Aborted`.
    fn partial(&mut self) -> AssistantMessage;
}

/// The adapter for a model (chosen at boot from boot.json).
pub fn for_model(info: &'static ModelInfo) -> Box<dyn Provider> {
    match info.api {
        Api::ChatCompletions => Box::new(deepseek::DeepSeek::new(info)),
        Api::Messages => Box::new(anthropic::Anthropic::from_env(info)),
        Api::Responses => Box::new(openai::OpenAi::new(info)),
    }
}

/// The wire effort value per API (provider.md §3.8.1–3).
pub fn effort_wire(api: Api, e: Effort) -> &'static str {
    match (api, e) {
        (Api::ChatCompletions, Effort::Low) => "low",
        (Api::ChatCompletions, Effort::Max) => "max",
        (Api::ChatCompletions, _) => "high",
        (_, Effort::Low) => "low",
        (_, Effort::Medium) => "medium",
        (_, Effort::High) => "high",
        (_, Effort::XHigh) => "xhigh",
        (_, Effort::Max) => "max",
    }
}

/// The DeepSeek/Anthropic context-overflow heuristic (provider.md §3.8.1).
pub fn mentions_overflow(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    [
        "context length",
        "maximum context",
        "too long",
        "prompt is too long",
    ]
    .iter()
    .any(|k| m.contains(k))
}

/// Status → kind rules shared by the adapters; each adapter refines them.
pub fn base_kind(status: u16) -> ErrorKind {
    match status {
        400 | 422 => ErrorKind::InvalidRequest,
        401 => ErrorKind::Auth,
        402 => ErrorKind::Billing,
        403 => ErrorKind::Permission,
        404 => ErrorKind::NotFound,
        413 => ErrorKind::TooLarge,
        429 => ErrorKind::RateLimit,
        503 | 529 => ErrorKind::Overloaded,
        408 | 409 | 425 | 500 | 502 | 504 => ErrorKind::Server,
        s if s >= 500 => ErrorKind::Server,
        _ => ErrorKind::Other,
    }
}

/// `error.message` (OpenAI/DeepSeek/Anthropic shapes) or the body text, truncated.
pub fn error_message(body: Option<&Value>, raw: &[u8]) -> String {
    let from_json = body.and_then(|v| {
        v.pointer("/error/message")
            .or_else(|| v.get("message"))
            .or_else(|| v.get("mock_error"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let mut m = from_json.unwrap_or_else(|| String::from_utf8_lossy(raw).trim().to_string());
    if m.len() > 2000 {
        let mut cut = 2000;
        while !m.is_char_boundary(cut) {
            cut -= 1;
        }
        m.truncate(cut);
        m.push('…');
    }
    m
}

/// Reads a string at a JSON pointer.
pub(crate) fn jstr<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}

/// Reads a u64 at a JSON pointer (0 when absent).
pub(crate) fn ju64(v: &Value, ptr: &str) -> u64 {
    v.pointer(ptr).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::sse::SseReader;
    use super::*;

    /// Runs an SSE byte stream through a decoder; returns the events and the final result.
    pub fn decode(
        mut d: Box<dyn Decoder>,
        bytes: &[u8],
    ) -> (Vec<StreamEvent>, Result<AssistantMessage, ProviderError>) {
        let mut r = SseReader::new(bytes);
        let mut out = Vec::new();
        loop {
            match r.next_event() {
                Ok(Some(ev)) => {
                    if let Err(e) = d.feed(&ev, &mut out) {
                        return (out, Err(e));
                    }
                    if d.finished() {
                        // Keep reading: events after the terminal one are a protocol error.
                    }
                }
                Ok(None) => break,
                Err(e) => return (out, Err(e)),
            }
        }
        let r = if d.finished() {
            d.finish()
        } else {
            Err(ProviderError::Truncated)
        };
        (out, r)
    }

    pub fn fixture(path: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/provider/testdata")
            .join(path);
        std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }
}
