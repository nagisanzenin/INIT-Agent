//! Normalized model I/O types shared by the loop, tools, context and provider
//! (body/provider.md §3.2 types, §3.5 stream events, §3.11 / loop.md §3.1 `Abort`).
//!
//! Owner: whichever body lane created this file first (WP10, lane B2). The other lane adapts and
//! records changes in its status file. Keep it free of I/O except `Abort`'s socket shutdown.

use std::collections::BTreeSet;
use std::fmt;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

/// Wire format of a request. One definition for the whole body (log-context's `Target.api` uses it).
/// JSON: `"chat_completions" | "messages" | "responses"` (`Api::as_str`).
pub use crate::context::model::Api;

/// The projected, grouped context that body/log-context.md §3.9 and §3.13 hand to the adapter.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// Owner text and steers (log-context renders `[init:…]` harness text the same way).
    User { content: Vec<ContentBlock> }, // Text and Image only
    /// One model response, in the order the model emitted the blocks.
    Assistant(AssistantMessage),
    /// Results for the tool calls of the immediately preceding Assistant message, in call order.
    ToolResults(Vec<ToolResult>),
    /// One concatenated harness note (signals, reload, notices): log-context §3.13. Rendered per
    /// `CacheProfile::note_placement`. Never directly after an Assistant message.
    Note { text: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>, // Text, Thinking, ToolCall, Opaque
    pub stop: StopReason,
    pub usage: Usage,
    pub origin: Origin,
}

impl AssistantMessage {
    /// The complete tool calls of this message, in emitted order.
    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        self.content.iter().filter_map(|b| match b {
            ContentBlock::ToolCall(c) => Some(c),
            _ => None,
        })
    }

    /// All `Text` blocks concatenated.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// Who produced an assistant message. Drives the handoff filter (provider.md §3.10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub provider: String,            // proxy provider id: "deepseek" | "anthropic" | "openai" | "mock"
    pub api: Api,
    pub model: String,               // model id as sent in the request
    pub response_id: Option<String>, // upstream id: chatcmpl id / msg_… / resp_…
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContentBlock {
    Text { text: String },
    /// In memory only as base64; the log stores images as blobs (log-context §4.2 `block_image`).
    Image { media_type: String, data_b64: String }, // image/png | image/jpeg | image/gif | image/webp
    Thinking(Thinking),
    ToolCall(ToolCall),
    /// A provider block the body does not interpret. Replayed verbatim to the same `api` only.
    Opaque { api: Api, raw: Value },
}

/// Mirrors log-context §4.2 `block_thinking` one to one.
#[derive(Clone, Debug, PartialEq)]
pub struct Thinking {
    pub provider: String,
    pub model: String,
    /// Visible reasoning or summary; DeepSeek: `reasoning_content` verbatim; "" when omitted.
    pub text: String,
    /// The exact provider object for replay (anthropic thinking / redacted_thinking block, openai
    /// reasoning item); `None` for DeepSeek (the text is the replay).
    pub native: Option<Value>,
}

/// Mirrors log-context §4.2 `block_tool_use` one to one.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String, // provider call id, verbatim; log: `call_id`
    pub name: String,
    pub input: Map<String, Value>, // parsed arguments; {} when they did not parse
    /// The argument JSON exactly as the model produced it (always set for DeepSeek and OpenAI;
    /// for Anthropic only when the assembled `partial_json` failed to parse).
    pub raw_arguments: Option<String>,
}

impl ToolCall {
    /// Some(serde_json message) when `raw_arguments` does not parse as a JSON object ("" counts as {}).
    pub fn args_error(&self) -> Option<String> {
        let raw = self.raw_arguments.as_deref()?;
        if raw.trim().is_empty() {
            return None;
        }
        match serde_json::from_str::<Value>(raw) {
            Ok(Value::Object(_)) => None,
            Ok(other) => Some(format!(
                "expected a JSON object, found {}",
                json_type_name(&other)
            )),
            Err(e) => Some(e.to_string()),
        }
    }

    /// Parses `raw` into the `input` map: `{}` when it is empty or not a JSON object.
    pub fn parse_input(raw: &str) -> Map<String, Value> {
        match serde_json::from_str::<Value>(raw) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        }
    }
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub call_id: String,
    pub content: Vec<ContentBlock>, // Text and Image only
    pub is_error: bool,
    pub meta: ToolMeta, // not sent to the model; logged (log-context X8)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolMeta {
    pub spill_path: Option<String>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub wall_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String, // ^[a-z][a-z0-9_]{0,63}$
    pub description: String,
    pub input_schema: Value, // JSON Schema object; body/tools.md owns the seed schemas
}

impl ToolSpec {
    /// The registry object `{"description","input_schema","name"}` (tools.md §4.1, context head).
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("description".into(), self.description.clone().into());
        m.insert("input_schema".into(), self.input_schema.clone());
        m.insert("name".into(), self.name.clone().into());
        Value::Object(m)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,       // ALL prompt tokens: uncached + cache_read + cache_write
    pub cache_read: u64,  // served from the provider cache ("hit")
    pub cache_write: u64, // written to the cache this request
    pub output: u64,      // all generated tokens, reasoning included
    pub reasoning: u64,   // subset of `output`; 0 when the provider does not report it
}

impl Usage {
    pub fn cache_miss(&self) -> u64 {
        self.input.saturating_sub(self.cache_read)
    }
    pub fn hit_permille(&self) -> u64 {
        (self.cache_read * 1000).checked_div(self.input).unwrap_or(0)
    }
    /// The log's `usage` (provider.md §3.2): `input_tokens` is the uncached part.
    pub fn to_log(self, native: Option<Value>) -> crate::log::event::UsageEv {
        crate::log::event::UsageEv {
            input_tokens: self
                .input
                .saturating_sub(self.cache_read)
                .saturating_sub(self.cache_write),
            cache_read_tokens: self.cache_read,
            cache_write_tokens: self.cache_write,
            output_tokens: self.output,
            reasoning_tokens: Some(self.reasoning),
            native,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,                              // natural end, no tool calls
    ToolUse,                              // ≥1 complete tool call; the loop runs them
    MaxTokens,                            // hit max_tokens; tool calls (if any) are NOT run
    StopSequence,
    ContextWindow,                        // the reply filled the context window; treat as MaxTokens
    Refusal { category: Option<String> }, // provider safety stop / content filter
    PauseTurn,                            // Anthropic server-tool pause; re-send to continue
    Aborted,                              // owner abort; content is the partial reply
}

impl StopReason {
    /// log-context §4.2 `stop_reason` (provider.md §3.2 mapping).
    pub fn log_str(&self) -> &'static str {
        match self {
            StopReason::EndTurn | StopReason::StopSequence | StopReason::PauseTurn => "end_turn",
            StopReason::ToolUse => "tool_use",
            StopReason::MaxTokens | StopReason::ContextWindow => "max_tokens",
            StopReason::Refusal { .. } => "refusal",
            StopReason::Aborted => "aborted",
        }
    }
    /// A length stop: tool calls are not run (loop.md §3.6).
    pub fn is_length(&self) -> bool {
        matches!(self, StopReason::MaxTokens | StopReason::ContextWindow)
    }
}

#[derive(Debug)]
pub enum ProviderError {
    Connect(std::io::Error), // proxy socket missing/refused
    Io(std::io::Error),      // read/write failure mid-request
    Timeout(TimeoutPhase),
    Http {
        status: u16,
        kind: ErrorKind,
        message: String,
        retry_after: Option<Duration>,
        request_id: Option<String>,
    },
    Budget { message: String },                  // the kernel proxy's own cap (never retried)
    Stream { kind: ErrorKind, message: String }, // error reported inside a 200 stream
    Protocol(String),                            // malformed HTTP/SSE/JSON, or limits exceeded
    Truncated,                                   // EOF before the API's terminal event
    Aborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeoutPhase {
    Write,
    Headers,
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    RateLimit,
    Overloaded,
    Server,
    InvalidRequest,
    ContextOverflow,
    Auth,
    Billing,
    Permission,
    NotFound,
    TooLarge,
    Other,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::RateLimit => "rate_limit",
            ErrorKind::Overloaded => "overloaded",
            ErrorKind::Server => "server",
            ErrorKind::InvalidRequest => "invalid_request",
            ErrorKind::ContextOverflow => "context_overflow",
            ErrorKind::Auth => "auth",
            ErrorKind::Billing => "billing",
            ErrorKind::Permission => "permission",
            ErrorKind::NotFound => "not_found",
            ErrorKind::TooLarge => "too_large",
            ErrorKind::Other => "other",
        }
    }
}

impl ProviderError {
    /// provider.md §3.9 retry classification. The transport adds two rules this method cannot
    /// see: proxy-generated (`x-init-error`) responses are never retried by the body, and the
    /// Anthropic thinking-binding 400 is retried once with thinking stripped.
    pub fn retryable(&self) -> bool {
        match self {
            ProviderError::Connect(_)
            | ProviderError::Io(_)
            | ProviderError::Truncated
            | ProviderError::Timeout(_) => true,
            ProviderError::Http { status, kind, .. } => {
                !matches!(
                    kind,
                    ErrorKind::Billing
                        | ErrorKind::Auth
                        | ErrorKind::Permission
                        | ErrorKind::InvalidRequest
                        | ErrorKind::ContextOverflow
                ) && matches!(status, 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 529)
            }
            ProviderError::Stream { kind, .. } => matches!(
                kind,
                ErrorKind::RateLimit | ErrorKind::Overloaded | ErrorKind::Server
            ),
            ProviderError::Budget { .. }
            | ProviderError::Protocol(_)
            | ProviderError::Aborted => false,
        }
    }

    /// The error's kind for the loop's `error{code:"provider", kind}` event.
    pub fn kind_str(&self) -> &'static str {
        match self {
            ProviderError::Connect(_) => "connect",
            ProviderError::Io(_) => "io",
            ProviderError::Timeout(_) => "timeout",
            ProviderError::Http { kind, .. } | ProviderError::Stream { kind, .. } => kind.as_str(),
            ProviderError::Budget { .. } => "budget",
            ProviderError::Protocol(_) => "protocol",
            ProviderError::Truncated => "truncated",
            ProviderError::Aborted => "aborted",
        }
    }

    pub fn is_context_overflow(&self) -> bool {
        matches!(
            self,
            ProviderError::Http { kind: ErrorKind::ContextOverflow, .. }
                | ProviderError::Stream { kind: ErrorKind::ContextOverflow, .. }
        )
    }

    pub fn request_id(&self) -> Option<&str> {
        match self {
            ProviderError::Http { request_id, .. } => request_id.as_deref(),
            _ => None,
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::Connect(e) => write!(f, "cannot connect to the proxy: {e}"),
            ProviderError::Io(e) => write!(f, "i/o error: {e}"),
            ProviderError::Timeout(p) => write!(f, "timeout ({p:?})"),
            ProviderError::Http {
                status,
                kind,
                message,
                ..
            } => write!(f, "HTTP {status} {}: {message}", kind.as_str()),
            ProviderError::Budget { message } => write!(f, "budget: {message}"),
            ProviderError::Stream { kind, message } => {
                write!(f, "stream error {}: {message}", kind.as_str())
            }
            ProviderError::Protocol(m) => write!(f, "protocol error: {m}"),
            ProviderError::Truncated => write!(f, "stream ended before its terminal event"),
            ProviderError::Aborted => write!(f, "aborted"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// Normalized stream events (provider.md §3.5). Exactly one terminal event (`Done` or `Failed`).
#[derive(Debug)]
pub enum StreamEvent {
    Started {
        attempt: u32,
        request_id: Option<String>,
        proxy_req_id: Option<String>, // x-init-req-id
        head_hash: Option<String>,    // x-init-head-hash
    },
    Warning { code: &'static str, detail: String },
    TextDelta(String),     // display only; the final message is authoritative
    ThinkingDelta(String), // display only
    ToolCallReady(ToolCall),
    Usage(Usage), // cumulative; last one wins
    Retry { attempt: u32, delay: Duration, error: ProviderError },
    Done(AssistantMessage),
    Failed(ProviderError),
}

/// The abort hooks (provider.md §3.11; loop.md §3.1 owns the policy). `trigger()` is idempotent and
/// safe from any thread: it sets the flag, shuts the in-flight proxy connection down (a blocked read
/// returns EOF), kills every registered bash process group through the `kill_group` hook (tools.md
/// §3.5.4, supplied by the loop), and wakes `sleep`.
pub struct Abort {
    flag: AtomicBool,
    /// Interrupt-and-steer (loop.md §3.5): the owner wants the run to continue with a note.
    cont: AtomicBool,
    stream: Mutex<Option<UnixStream>>,
    groups: Mutex<BTreeSet<i32>>,
    wake: (Mutex<()>, Condvar),
    kill_group: fn(i32),
    /// Sub-task aborts (`delegate`): `trigger()` reaches them, and `groups()` reports their
    /// process groups, so an owner abort stops every sub-task's stream and bash groups.
    children: Mutex<Vec<Weak<Abort>>>,
}

impl fmt::Debug for Abort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Abort")
            .field("flag", &self.is_set())
            .finish_non_exhaustive()
    }
}

impl Abort {
    /// `kill_group(pgid)` SIGKILLs one process group the loop registered (tools::kill_group).
    pub fn new(kill_group: fn(i32)) -> Self {
        Abort {
            flag: AtomicBool::new(false),
            cont: AtomicBool::new(false),
            stream: Mutex::new(None),
            groups: Mutex::new(BTreeSet::new()),
            wake: (Mutex::new(()), Condvar::new()),
            kill_group,
            children: Mutex::new(Vec::new()),
        }
    }

    /// A child abort for one sub-task (`delegate`): its own in-flight stream and process groups,
    /// so concurrent sub-tasks never overwrite each other's handles. Triggering this abort
    /// triggers the child; triggering the child (a sub-task's wall-time cap) leaves this one alone.
    /// A child made after this abort fired starts set.
    pub fn child(&self) -> Arc<Abort> {
        let c = Arc::new(Abort::new(self.kill_group));
        let mut g = lock(&self.children);
        g.retain(|w| w.strong_count() > 0);
        if self.is_set() {
            c.flag.store(true, Ordering::SeqCst);
        }
        g.push(Arc::downgrade(&c));
        c
    }

    /// A plain abort. It also cancels an earlier continue wish (a second Ctrl-C stops).
    pub fn trigger(&self) {
        self.cont.store(false, Ordering::SeqCst);
        self.fire();
    }

    /// Interrupt-and-steer: abort what runs now, and record that the run should continue at the
    /// next boundary if there is an owner note to answer (loop.md §3.5).
    pub fn trigger_continue(&self) {
        self.cont.store(true, Ordering::SeqCst);
        self.fire();
    }

    /// The last trigger was `trigger_continue` (cleared by `trigger` and `reset`).
    pub fn wants_continue(&self) -> bool {
        self.cont.load(Ordering::SeqCst)
    }

    fn fire(&self) {
        self.flag.store(true, Ordering::SeqCst);
        if let Some(s) = lock(&self.stream).take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        for &pg in lock(&self.groups).iter() {
            (self.kill_group)(pg);
        }
        let kids: Vec<Arc<Abort>> = lock(&self.children).iter().filter_map(Weak::upgrade).collect();
        for c in kids {
            c.trigger();
        }
        let _g = lock(&self.wake.0);
        self.wake.1.notify_all();
    }

    /// Start of each run: clears the flag and stale handles.
    pub fn reset(&self) {
        *lock(&self.stream) = None;
        lock(&self.groups).clear();
        self.cont.store(false, Ordering::SeqCst);
        self.flag.store(false, Ordering::SeqCst);
    }

    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub fn flag(&self) -> &AtomicBool {
        &self.flag
    }

    /// Keeps a clone of the in-flight connection. Err(Aborted) when the flag is already set.
    pub fn register_stream(&self, s: UnixStream) -> Result<(), ProviderError> {
        let mut g = lock(&self.stream);
        if self.is_set() {
            let _ = s.shutdown(std::net::Shutdown::Both);
            return Err(ProviderError::Aborted);
        }
        *g = Some(s);
        Ok(())
    }

    /// Drops the registered connection (the request is over).
    pub fn clear_stream(&self) {
        *lock(&self.stream) = None;
    }

    /// Waits up to `d`; returns true (early) when aborted.
    pub fn sleep(&self, d: Duration) -> bool {
        let deadline = Instant::now() + d;
        let mut g = lock(&self.wake.0);
        loop {
            if self.is_set() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            g = match self.wake.1.wait_timeout(g, deadline - now) {
                Ok((g, _)) => g,
                Err(p) => p.into_inner().0,
            };
        }
    }

    /// false ⇒ already aborted: the caller kills the group itself.
    pub fn register_group(&self, pg: i32) -> bool {
        let mut g = lock(&self.groups);
        if self.is_set() {
            return false;
        }
        g.insert(pg);
        true
    }

    pub fn unregister_group(&self, pg: i32) {
        lock(&self.groups).remove(&pg);
    }
}

/// Lock ignoring poison: every critical section here is panic-free.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(raw: Option<&str>) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: "bash".into(),
            input: Map::new(),
            raw_arguments: raw.map(str::to_string),
        }
    }

    #[test]
    fn args_error_rules() {
        assert_eq!(call(None).args_error(), None);
        assert_eq!(call(Some("")).args_error(), None);
        assert_eq!(call(Some("{\"a\":1}")).args_error(), None);
        assert!(call(Some("{\"a\":")).args_error().is_some());
        assert!(call(Some("[1]")).args_error().unwrap().contains("array"));
    }

    #[test]
    fn usage_math_and_log_mapping() {
        let u = Usage {
            input: 1000,
            cache_read: 900,
            cache_write: 50,
            output: 10,
            reasoning: 4,
        };
        assert_eq!(u.cache_miss(), 100);
        assert_eq!(u.hit_permille(), 900);
        assert_eq!(Usage::default().hit_permille(), 0);
        let l = u.to_log(None);
        assert_eq!(
            (l.input_tokens, l.cache_read_tokens, l.cache_write_tokens),
            (50, 900, 50)
        );
    }

    #[test]
    fn retry_table() {
        let http = |status, kind| ProviderError::Http {
            status,
            kind,
            message: String::new(),
            retry_after: None,
            request_id: None,
        };
        for s in [408, 409, 425, 429, 500, 502, 503, 504, 529] {
            assert!(http(s, ErrorKind::Server).retryable(), "{s}");
        }
        for s in [400, 401, 402, 403, 404, 413, 422] {
            assert!(!http(s, ErrorKind::Other).retryable(), "{s}");
        }
        assert!(!http(429, ErrorKind::Billing).retryable());
        assert!(!http(400, ErrorKind::ContextOverflow).retryable());
        assert!(
            ProviderError::Connect(std::io::ErrorKind::NotFound.into()).retryable()
        );
        assert!(ProviderError::Truncated.retryable());
        for p in [TimeoutPhase::Write, TimeoutPhase::Headers, TimeoutPhase::Idle] {
            assert!(ProviderError::Timeout(p).retryable());
        }
        assert!(!ProviderError::Budget { message: "x".into() }.retryable());
        assert!(!ProviderError::Protocol("x".into()).retryable());
        assert!(!ProviderError::Aborted.retryable());
        for k in [ErrorKind::RateLimit, ErrorKind::Overloaded, ErrorKind::Server] {
            assert!(ProviderError::Stream { kind: k, message: String::new() }.retryable());
        }
        assert!(
            !ProviderError::Stream { kind: ErrorKind::Other, message: String::new() }.retryable()
        );
    }

    #[test]
    fn abort_sleep_wakes_and_refuses_late_registration() {
        let a = std::sync::Arc::new(Abort::new(|_| {}));
        assert!(!a.sleep(Duration::from_millis(5)));
        let b = a.clone();
        let t = std::thread::spawn(move || {
            let t0 = Instant::now();
            let aborted = b.sleep(Duration::from_secs(30));
            (aborted, t0.elapsed())
        });
        std::thread::sleep(Duration::from_millis(30));
        a.trigger();
        let (aborted, took) = t.join().unwrap();
        assert!(aborted);
        assert!(took < Duration::from_secs(5));
        assert!(!a.register_group(7));
        let (x, _y) = UnixStream::pair().unwrap();
        assert!(matches!(a.register_stream(x), Err(ProviderError::Aborted)));
        a.reset();
        assert!(!a.is_set());
        assert!(a.register_group(7));
    }

    #[test]
    fn stop_reason_log_mapping() {
        assert_eq!(StopReason::StopSequence.log_str(), "end_turn");
        assert_eq!(StopReason::ContextWindow.log_str(), "max_tokens");
        assert_eq!(StopReason::Refusal { category: None }.log_str(), "refusal");
        assert!(StopReason::ContextWindow.is_length());
    }
}

// ---------------------------------------------------------------------------------------------
// WP11 additions (lane B1): the loop's model seam (body/loop.md §3.2) and the request/model
// descriptors of body/provider.md §3.3. Agreed with WP10: the provider implements `ModelClient`.
// ---------------------------------------------------------------------------------------------

/// provider.md §3.3. JSON / wire: `low | medium | high | xhigh | max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
        }
    }
    pub fn parse(s: &str) -> Option<Effort> {
        Some(match s {
            "low" => Effort::Low,
            "medium" => Effort::Medium,
            "high" => Effort::High,
            "xhigh" => Effort::XHigh,
            "max" => Effort::Max,
            _ => return None,
        })
    }
}

/// Header `x-init-purpose`: `turn | probe | compaction` (never in the body).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Turn,
    Probe,
    Compaction,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::Turn => "turn",
            Purpose::Probe => "probe",
            Purpose::Compaction => "compaction",
        }
    }
}

/// How a model's reasoning must be replayed (provider.md §3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasoningRule {
    PassBackPlain,
    SignedModelBound,
    EncryptedItems,
}

/// One row of the seed model table (provider.md §3.3; the table itself is WP10's).
#[derive(Clone, Debug)]
pub struct ModelInfo {
    pub provider: &'static str,
    pub api: Api,
    pub id: &'static str,
    pub context_window: u64,
    pub max_output: u32,
    pub default_max_tokens: u32,
    pub default_effort: Effort,
    pub images: bool,
    pub reasoning: ReasoningRule,
    pub cache: crate::context::model::CacheProfile,
}

/// One model request as the loop hands it to the model (loop.md §3.2).
///
/// Deviation from provider.md §3.3 (agreed with WP10): log-context (`Context::next_request`)
/// already renders the wire body, so `body` is sent unchanged with the API's path and headers;
/// `messages` is the projection it was rendered from (for adapters and scripted test models).
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub body: &'a [u8],
    pub head_hash: &'a str,
    pub messages: &'a [crate::context::project::Msg],
    pub max_tokens: u32,
    pub effort: Effort,
    pub purpose: Purpose,
    /// `INIT_SESSION` → `x-init-session`.
    pub session: &'a str,
    /// → `x-init-task`; `None` when the path has no owner message (the proxy books `_none`).
    pub task: Option<&'a str>,
    /// → `x-init-subtask` (kernel/proxy.md §3.5a): `delegate-<call id>` on a sub-task's requests,
    /// so traces and per-turn signals can tell sub-task traffic from the main loop's. Recorded,
    /// never enforced.
    pub subtask: Option<&'a str>,
}

/// The loop's only view of the model (loop.md §3.2). The real implementation is provider.md's
/// `stream()`; tests supply a scripted one. Exactly one terminal event per call.
pub trait ModelClient: Send + Sync {
    fn info(&self) -> &ModelInfo;
    fn stream<'a>(
        &'a self,
        req: &'a Request<'a>,
        abort: &'a Abort,
    ) -> Box<dyn Iterator<Item = StreamEvent> + 'a>;
}

impl Abort {
    /// The live bash process groups (heartbeat `tool_pgids`, boot.md §4.2). WP11 addition.
    pub fn groups(&self) -> Vec<i32> {
        let mut v: Vec<i32> = lock(&self.groups).iter().copied().collect();
        let kids: Vec<Arc<Abort>> = lock(&self.children).iter().filter_map(Weak::upgrade).collect();
        for c in kids {
            v.extend(c.groups());
        }
        v
    }
}
