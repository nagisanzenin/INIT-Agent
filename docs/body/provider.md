# Provider: model I/O for the body
Status: v0.3 + build sync 2026-10-20 · Area: body
Depends on: CONVENTIONS.md, kernel/proxy.md, body/log-context.md, body/loop.md, testing.md   Defines: normalized message and stream-event model (`Message`, `ContentBlock`, `ToolCall`, `ToolSpec`, `StreamEvent`, `Usage`, `StopReason`, `ProviderError`), the `Provider` trait, the body's HTTP/1.1 + SSE client to `run/proxy.sock`, the three adapters (DeepSeek Chat Completions, Anthropic Messages, OpenAI Responses), retry/timeout policy, cross-provider handoff rules, the seed model table, and the scripted mock-provider fixture format

---

## 1. Purpose

The provider is the only model-specific seam in the body (spec §2 Article 2, §4). It turns a normalized request (system text, tool specs, messages) into one HTTP request to the kernel proxy, and turns the streamed reply into a normalized stream of events that ends in one complete assistant message. Everything else in the body (loop, tools, log, context) sees only the normalized types defined here.

The first real provider is DeepSeek (CONVENTIONS §5). This doc specifies its adapter completely, then the Anthropic Messages and OpenAI Responses adapters.

## 2. Responsibilities / non-goals

**Responsibilities**
- Normalized types and their JSON shapes (the log embeds them; see §4.1).
- Request building per API, with deterministic bytes (sorted keys, no volatile fields).
- A std-only HTTP/1.1 client over `std::os::unix::net::UnixStream`, with chunked decoding and an SSE parser.
- Stream decoding per API, including incremental tool-call argument assembly and reasoning/thinking capture.
- Retries, backoff, timeouts, cancellation hooks for abort.
- Cross-provider handoff rules (what is replayed when the model or provider changes).
- The seed model table (`ModelInfo`), which is what "the model id is checked at boot" checks against.
- A deterministic scripted mock-provider fixture format, so A4–A6 and loop tests run at $0.

**Non-goals**
- Authentication, TLS, spend caps, traces, persistent upstream connections: all in `kernel/proxy.md`. The body sends no auth header.
- Choosing what goes in the prompt, cache layout, compaction, prefix hash: `body/log-context.md`. The provider only serializes what it is given and places provider-specific cache markers (§3.7, §3.8).
- Executing tools, steering, abort policy: `body/loop.md`. The provider only exposes the hooks (`Abort`).
- Server-side tools (web search, code execution), server-side compaction, prompt-cache pre-warming, server-side model fallbacks. None are used at seed; each is a possible growth step through the gate.

## 3. Design

### 3.1 Module layout (inside `seed/src/provider/`)

| File | Contents | Pure? |
|---|---|---|
| `mod.rs` | the `Provider` and `Decoder` traits, `Prompt`, the adapter lookup | yes |
| `models.rs` | the `ModelInfo` table with `CacheProfile` values (§3.3). It is not a per-model *profile*: grown model-specific behavior goes in `src/profiles/<family>.rs`, the location kernel/selfmap.md inventories as `profile/<family>` (selfmap Q-P1, R1). The seed ships no profiles | yes |
| `handoff.rs` | the §3.10 filter, and the conversion from log blocks to normalized messages | yes |
| `http.rs` | `send()`, response head parser, `ChunkedReader`, `Body` | I/O |
| `sse.rs` | `SseReader` (WHATWG event-stream parser) | over `BufRead` |
| `transport.rs` | `EventStream` iterator: connect, retry, backoff, timeouts, abort | I/O |
| `deepseek.rs` | Chat Completions `build()` + `Decoder` | yes |
| `anthropic.rs` | Messages `build()` + `Decoder` | yes |
| `openai.rs` | Responses `build()` + `Decoder` | yes |
| `client.rs` | `Client`: one adapter plus the transport; it implements the loop's `ModelClient` seam (§3.4) | I/O |

The normalized types (§3.2), `StreamEvent`, `ModelInfo`, `Effort`, `Purpose`, the byte-based `Request` (§3.3), the `ModelClient` trait and `Abort` (§3.11) live in `seed/src/model.rs`, shared with the loop, tools and context. `Api` is re-exported from `context::model`.

Adapters are pure: `build()` maps a `Prompt` to bytes, and a `Decoder` maps SSE events to `StreamEvent`s. That makes every adapter testable from fixture files with no socket (§8). One shared `transport.rs` does all I/O for all adapters.

### 3.2 Normalized types

These types are defined in `seed/src/model.rs` (§3.1).

```rust
use serde_json::Value;          // serde_json 1.0.154 (crates.io, 2026-10-11); Map is BTreeMap (no preserve_order)

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Api { ChatCompletions, Messages, Responses }   // JSON: "chat_completions" | "messages" | "responses"

/// The projected, grouped context that body/log-context.md §3.9 and §3.13 hand to the adapter.
#[derive(Clone, Debug)]
pub enum Message {
    /// Owner text and steers (log-context renders `[init:…]` harness text the same way).
    User { content: Vec<ContentBlock> },              // Text and Image only
    /// One model response, in the order the model emitted the blocks.
    Assistant(AssistantMessage),
    /// Results for the tool calls of the immediately preceding Assistant message, in call order.
    ToolResults(Vec<ToolResult>),
    /// One concatenated harness note (signals, reload, notices): log-context §3.13. Rendered per
    /// `CacheProfile::note_placement`: Anthropic mid-conversation system message, DeepSeek user
    /// message, OpenAI developer message. Never directly after an Assistant message.
    Note { text: String },
}

#[derive(Clone, Debug)]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,   // Text, Thinking, ToolCall, Opaque
    pub stop: StopReason,
    pub usage: Usage,
    pub origin: Origin,
}

/// Who produced an assistant message. Drives the handoff filter (§3.10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub provider: String,             // proxy provider id: "deepseek" | "anthropic" | "openai" | "mock"
    pub api: Api,
    pub model: String,                // model id as sent in the request
    pub response_id: Option<String>,  // upstream id: chatcmpl id / msg_… / resp_…
}

#[derive(Clone, Debug)]
pub enum ContentBlock {
    Text { text: String },
    /// In memory only as base64. The log stores images as blobs (log-context §4.2 `block_image`),
    /// and the projection loads and encodes them for the adapter.
    Image { media_type: String, data_b64: String },          // image/png | image/jpeg | image/gif | image/webp
    Thinking(Thinking),
    ToolCall(ToolCall),
    /// A provider block the body does not interpret (e.g. an Anthropic block type added later).
    /// Replayed verbatim to the same `api` only; dropped for any other api.
    Opaque { api: Api, raw: Value },
}

/// Mirrors log-context §4.2 `block_thinking` one to one.
#[derive(Clone, Debug)]
pub struct Thinking {
    pub provider: String,             // who produced it; the handoff filter compares provider + model
    pub model: String,
    pub text: String,                 // visible reasoning or summary; DeepSeek: reasoning_content verbatim; "" when omitted
    /// The exact provider object for replay, owned by this doc:
    ///   anthropic: {"type":"thinking","thinking":…,"signature":…} or {"type":"redacted_thinking","data":…}
    ///   openai:    the whole reasoning item {"type":"reasoning","id":…,"summary":[…],"encrypted_content":…}
    ///   deepseek:  None (the text is the replay)
    pub native: Option<Value>,
}

/// Mirrors log-context §4.2 `block_tool_use` one to one.
#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,                   // provider call id, verbatim ("call_00_…", "toolu_…", "call_…"); log: `call_id`
    pub name: String,
    pub input: serde_json::Map<String, Value>,   // parsed arguments; {} when they did not parse
    /// The argument JSON exactly as the model produced it. Always set for DeepSeek and OpenAI, whose
    /// wire carries a string; replayed byte for byte so the cached prefix matches the model's own
    /// output. For Anthropic it is set only when the assembled `partial_json` failed to parse.
    pub raw_arguments: Option<String>,
}
impl ToolCall {
    /// Some(serde_json message) when `raw_arguments` does not parse as a JSON object ("" counts as {}).
    pub fn args_error(&self) -> Option<String>;
}

#[derive(Clone, Debug)]
pub struct ToolResult {
    pub call_id: String,
    pub content: Vec<ContentBlock>,   // Text and Image only
    pub is_error: bool,
    pub meta: ToolMeta,               // not sent to the model; logged (log-context X8)
}
#[derive(Clone, Debug, Default)]
pub struct ToolMeta { pub spill_path: Option<String>, pub exit_code: Option<i32>, pub signal: Option<i32>, pub wall_ms: u64 }

#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,                 // ^[a-z][a-z0-9_]{0,63}$
    pub description: String,
    pub input_schema: Value,          // JSON Schema object; body/tools.md owns the five seed schemas
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,        // ALL prompt tokens: uncached + cache_read + cache_write
    pub cache_read: u64,   // served from the provider cache ("hit")
    pub cache_write: u64,  // written to the cache this request (billed as write where the provider bills it)
    pub output: u64,       // all generated tokens, reasoning included
    pub reasoning: u64,    // subset of `output`; 0 when the provider does not report it
}
impl Usage {
    pub fn cache_miss(&self) -> u64 { self.input - self.cache_read }  // uncached + written
    pub fn hit_permille(&self) -> u64 { if self.input == 0 { 0 } else { self.cache_read * 1000 / self.input } }  // ICJ: integers only (CONVENTIONS §5)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,                               // natural end, no tool calls
    ToolUse,                               // ≥1 complete tool call; the loop runs them
    MaxTokens,                             // hit max_tokens; tool calls (if any) are NOT run (loop.md §3.6)
    StopSequence,
    ContextWindow,                         // the reply filled the context window; treat as MaxTokens
    Refusal { category: Option<String> },  // provider safety stop / content filter
    PauseTurn,                             // Anthropic server-tool pause; re-send to continue
    Aborted,                               // owner abort; content is the partial reply (loop.md §3.5)
}

#[derive(Debug)]
pub enum ProviderError {
    Connect(std::io::Error),               // proxy socket missing/refused
    Io(std::io::Error),                    // read/write failure mid-request
    Timeout(TimeoutPhase),
    Http { status: u16, kind: ErrorKind, message: String, retry_after: Option<std::time::Duration>, request_id: Option<String> },
    Budget { message: String },            // the kernel proxy's own cap (never retried)
    Stream { kind: ErrorKind, message: String },  // error reported inside a 200 stream
    Protocol(String),                      // malformed HTTP/SSE/JSON, or limits exceeded
    Truncated,                             // EOF before the API's terminal event
    Aborted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeoutPhase { Write, Headers, Idle }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind { RateLimit, Overloaded, Server, InvalidRequest, ContextOverflow, Auth, Billing, Permission, NotFound, TooLarge, Other }
impl ProviderError { pub fn retryable(&self) -> bool; }   // table in §3.9
```

Notes are always appended and never rewritten. All three APIs accept them in the placements above, and append-only notes keep every cache and Anthropic's thinking-block binding valid (§3.8.2).

**`CacheProfile`.** It is defined by log-context.md §3.10.0; this doc owns the values. It is part of `ModelInfo`:

| Field | deepseek (and mock) | anthropic | openai |
|---|---|---|---|
| `strip_thinking_before_epoch` | false: DeepSeek returns 400 without `reasoning_content` in tool mode | true: stripping every thinking block before an epoch is valid under preserved thinking | true (conservative; UNVERIFIED whether older encrypted items stay valid) |
| `note_placement` | `UserMessage` | `MidSystemMessage` (both seed models support it) | `DeveloperMessage` |
| `breakpoints` | `None` (automatic disk cache) | `Anthropic { max: 4 }` | `OpenAiExplicit { max: 4 }` (GPT-5.6 and later, which covers both seed models) |
| `replay_raw_arguments` | true | false (input is an object) | true |
| `min_cacheable_tokens` | 0 (none stated) | 512 | 1,024 |
| `max_output` | `ModelInfo.default_max_tokens` | same | same |

**Usage and stop reason in the log.** log-context §4.2 stores `usage` with `input_tokens` meaning the *uncached* part. The writer maps:
- `input_tokens = input − cache_read − cache_write`;
- `cache_read_tokens`, `cache_write_tokens`, `output_tokens` and `reasoning_tokens` map one to one;
- `native` is the provider's raw usage object. At seed `AssistantMessage` has no field for it, so the loop logs `usage.native` as null.

`stop_reason` mapping:
- `EndTurn`, `StopSequence` and `PauseTurn` → `end_turn` (the latter two are unreachable at seed: no stop sequences, no server tools);
- `ToolUse` → `tool_use`;
- `MaxTokens` and `ContextWindow` → `max_tokens`;
- `Refusal` → `refusal`, with the category written as `message.stop_details` (log-context §4.2);
- `Aborted` → `aborted`.

### 3.3 Request and `ModelInfo`

```rust
/// What an adapter's `build()` takes. (The byte-based `Request` below is what the loop hands to the model.)
pub struct Prompt<'a> {
    pub head: PromptHead<'a>,        // log-context §3.8.2: the cached head, in two parts
    pub tools: &'a [ToolSpec],       // sorted byte-wise by name: bash, edit, ext, read, write (tools.md §4.1, log-context §3.8.2)
    pub messages: &'a [Message],     // projection + grouping (log-context §3.9, §3.13), after the handoff filter (§3.10)
    pub b3: Option<usize>,           // Anthropic only: index of the message whose last block gets breakpoint B3 (log-context §3.10.2)
    pub max_tokens: u32,
    pub effort: Effort,
    pub session: &'a str,            // INIT_SESSION (OpenAI prompt_cache_key)
}
pub struct PromptHead<'a> { pub core: &'a str, pub self_: &'a str }   // core = genesis + constitution; self_ = SELF.md + skills index

/// src/model.rs. log-context already renders the wire body (`Context::next_request`), so `Client` sends `body`
/// unchanged with the API's path and headers (loop.md §3.2, §4.2).
pub struct Request<'a> {
    pub body: &'a [u8],
    pub head_hash: &'a str,
    pub messages: &'a [Msg],         // the projection the body was rendered from (adapters, scripted test models)
    pub max_tokens: u32,
    pub effort: Effort,
    pub purpose: Purpose,            // header only, never in the body
    pub session: &'a str,            // INIT_SESSION → x-init-session
    pub task: Option<&'a str>,       // → x-init-task (kernel/proxy.md §3.4): the input_id of the owner input that started the run
                                     //   (evolution.md §3.1), on every request of the run, by loop.md §3.3's rule; None when the
                                     //   path has no owner message yet, which the proxy books as `_none`
}
#[derive(Clone, Copy)] pub enum Effort { Low, Medium, High, XHigh, Max }
#[derive(Clone, Copy)] pub enum Purpose { Turn, Probe, Compaction }   // header x-init-purpose: turn | probe | compaction

#[derive(Clone, Debug)]
pub struct ModelInfo {
    pub provider: &'static str, pub api: Api, pub id: &'static str,
    pub context_window: u64, pub max_output: u32, pub default_max_tokens: u32,
    pub default_effort: Effort, pub images: bool,
    pub reasoning: ReasoningRule,
    pub cache: CacheProfile,          // log-context §3.10.0; values in §3.2
}
pub enum ReasoningRule {
    PassBackPlain,     // DeepSeek: reasoning_content must be replayed whenever `tools` is present
    SignedModelBound,  // Anthropic: replay thinking blocks unchanged, same model + unedited prefix only
    EncryptedItems,    // OpenAI: replay reasoning items (encrypted_content) to the same model
}
```

**Seed model table** (the boot check accepts exactly these; anything else exits 78, boot.md §3.4):

| provider | api | model id | context_window | max_output | default_max_tokens | default_effort | images | reasoning | source |
|---|---|---|---|---|---|---|---|---|---|
| deepseek | chat_completions | `deepseek-flash` | 1,000,000 | 393,216 | 65,536 | High | yes | PassBackPlain | DeepSeek docs (served by DeepSeek-V4.1-Flash-1017) |
| deepseek | chat_completions | `deepseek-v4-pro` | 1,000,000 | 393,216 | 65,536 | High | no | PassBackPlain | DeepSeek docs (DeepSeek-V4-Pro-0813) |
| anthropic | messages | `claude-opus-5-5` | 1,000,000 | 128,000 | 64,000 | High | yes | SignedModelBound | claude-api skill model table, cached 2026-10-06 |
| anthropic | messages | `claude-sonnet-5-5` | 1,000,000 | 128,000 | 64,000 | High | yes | SignedModelBound | same |
| openai | responses | `gpt-6-astra` | 922,000 (max input) | 128,000 | 64,000 | Medium | yes | EncryptedItems | OpenAI model page |
| openai | responses | `gpt-6.1-sol` | 922,000 (max input) | 128,000 | 64,000 | Medium | yes | EncryptedItems | OpenAI model page |
| mock | chat_completions | `mock-frontier-1` | 131,072 | 32,768 | 8,192 | High | no | PassBackPlain | testing.md scenarios (DeepSeek wire format). The proxy routes it only when `proxy.test_mode = true`, which also refuses every key file (kernel/proxy.md §4.1), so accepting it in production costs nothing |

- DeepSeek lists the context as "1M". This doc uses 1,000,000, the lower reading; whether it means 1,048,576 is UNVERIFIED and does not matter for a compaction threshold of window − 16k.
- For OpenAI, `context_window` is the documented *maximum input* (922,000), because that is the number compaction must respect. The documented total window is 1,050,000.
- `default_max_tokens` is 64k for every real model: DeepSeek's own thinking-mode default (65,536), and the claude-api skill's streaming default (~64,000). It is sent explicitly, never left to the server default, so that bytes and behavior do not move when a provider changes its default.
- Effort, model and provider come from `state/kernel/boot.json` field `model: {provider, model, effort}` (kernel/supervise.md §4.3; `model` holds config `model.id`). The table gives only defaults: `effort: null` means `default_effort`.

### 3.4 The `Provider` trait

```rust
pub trait Provider: Send + Sync {
    fn info(&self) -> &ModelInfo;
    /// Pure. Serializes a Prompt. Same Prompt ⇒ same bytes (sorted keys, no clocks, no randomness).
    fn build(&self, p: &Prompt) -> Result<HttpRequest, ProviderError>;
    /// Pure. Wraps request body bytes (rendered by log-context, or by `build`) for this API: path + headers.
    fn wire(&self, body: Vec<u8>) -> HttpRequest;
    /// A fresh decoder for one attempt.
    fn decoder(&self) -> Box<dyn Decoder>;
    /// Pure. The ErrorKind of a non-200 upstream response (the transport handles the proxy's own cap errors first).
    fn classify(&self, status: u16, headers: &[(String, String)], body: Option<&Value>, message: &str) -> ErrorKind;
    /// Pure. A one-shot recovery rebuild after a non-retryable error: Anthropic binding-off and strip-thinking (§3.8.2),
    /// DeepSeek flatten (§3.10). `used` lists the causes already applied in this stream. Returns the new request and the cause.
    fn fallback(&self, req: &HttpRequest, err: &ProviderError, used: &[&'static str]) -> Option<(HttpRequest, &'static str)>;   // default: None
}

pub trait Decoder: Send {
    /// Called once per SSE event (comments never reach it). Pushes zero or more normalized events.
    fn feed(&mut self, ev: &SseEvent, out: &mut Vec<StreamEvent>) -> Result<(), ProviderError>;
    /// True once the API's terminal event was seen ([DONE] / message_stop / response.completed|incomplete).
    fn finished(&self) -> bool;
    /// Assembles the final message. Err(Truncated) if !finished().
    fn finish(&mut self) -> Result<AssistantMessage, ProviderError>;
    /// The partial message for an abort: complete blocks + partial text/thinking; incomplete tool calls dropped.
    fn partial(&mut self) -> AssistantMessage;
}

pub struct HttpRequest {
    pub path: String,                          // "/deepseek/chat/completions"
    pub headers: Vec<(&'static str, String)>,  // provider headers only; transport adds the rest
    pub body: Vec<u8>,
}
```

The spec's single function `stream(&Context) -> impl Iterator<Item = Event>` (§4) is kept as one transport entry point that works for every adapter:

```rust
pub fn stream<'a>(p: &'a dyn Provider, t: &'a Transport, req: HttpRequest, cx: Cx, abort: &'a Abort) -> EventStream<'a>;
impl Iterator for EventStream<'_> { type Item = StreamEvent; }
```

`Cx` carries `x-init-session`, `x-init-purpose` and `x-init-task`. `Client` (`client.rs`) holds one adapter and a `Transport` and implements the loop's `ModelClient`: it calls `wire(request.body)` and `stream`, so the bytes log-context rendered go out unchanged. `build()` renders from a `Prompt` instead, for tests and for APIs whose bodies log-context does not render yet.

The fallbacks rewrite the body JSON of the request that was already built, so they also work on bytes that log-context rendered. The transport applies each cause at most once per stream, emits `Warning{code: <cause>}` and then `Retry`, and resends. The loop logs the warning as an `error` event (loop.md §3.6).

A trait object (`&dyn Provider`) is used instead of `impl Iterator` in the trait (return-position `impl Trait` in traits is stable since Rust 1.75, but then the trait is not dyn-compatible). The provider is chosen at boot from `boot.json`, so dynamic dispatch is needed. The cost is one virtual call per SSE event, which is negligible next to network time (spec §9 rank 7).

### 3.5 Normalized stream events

```rust
#[derive(Debug)]
pub enum StreamEvent {
    Started { attempt: u32, request_id: Option<String>,     // response headers received (status 200)
              proxy_req_id: Option<String>,                 // x-init-req-id
              head_hash: Option<String> },                  // x-init-head-hash: the proxy's own head hash (log-context §3.10.4)
    Warning { code: &'static str, detail: String },         // e.g. "thinking_dropped" (Anthropic input_transformations)
    TextDelta(String),                                      // for display only; the final message is authoritative
    ThinkingDelta(String),                                  // for display only
    ToolCallReady(ToolCall),                                // a tool call whose args are complete (display/log hint)
    Usage(Usage),                                           // cumulative; may repeat; last one wins
    Retry { attempt: u32, delay: std::time::Duration, error: ProviderError },  // the attempt failed; partial output is void
    Done(AssistantMessage),                                 // terminal: success, or StopReason::Aborted with the partial
    Failed(ProviderError),                                  // terminal: not retryable, or retries exhausted
}
```

Contract:
- Exactly one terminal event (`Done` or `Failed`) per `stream()` call. The iterator returns `None` after it.
- After `Retry`, every delta seen so far is void. The loop tells the owner's UI to clear the partial display (loop.md §3.6, `display{kind:"info"}`).
- `ToolCallReady` never triggers execution at seed. Tools run only after `Done` (spec §4 excludes speculative execution). A failed attempt therefore never leaves side effects behind.
- An abort ends the stream with `Done(msg)` where `msg.stop == StopReason::Aborted`, so the partial output is kept (spec §4 Loop row).

### 3.6 HTTP/1.1 client over the proxy socket

One request per connection (`Connection: close`). Connecting to a local Unix socket costs microseconds, and the expensive persistent connections live between the proxy and the upstream (spec §7.2). One connection per request also gives each request its own socket for abort (§3.12) and makes framing errors impossible to carry over to the next request.

**Request bytes** (header names lowercase, fixed order, CRLF line endings):

```
POST /deepseek/chat/completions HTTP/1.1\r\n
host: init-proxy\r\n
content-type: application/json\r\n
accept: text/event-stream\r\n
content-length: 18342\r\n
connection: close\r\n
x-init-session: 2026-10-19-a1b2\r\n
x-init-purpose: turn\r\n
x-init-attempt: 1\r\n
\r\n
{...body...}
```

- **Provider headers** are inserted after `connection`. Anthropic adds `anthropic-version: 2023-06-01` and `anthropic-beta: thinking-binding-controls-2026-08-01` (§3.8.2).
- **Credentials.** No `authorization`, `x-api-key` or cookie header is ever sent; the proxy injects credentials (CONVENTIONS §4).
- **`x-init-*` headers.** kernel/proxy.md §3.4 and §3.6 define `x-init-session`, `x-init-purpose`, `x-init-attempt` and `x-init-task` (sent only when `Request.task` is set). The proxy records them and drops every `x-init-*` header before forwarding upstream. Its forwarding allowlist is `content-type`, `accept`, `anthropic-version`, `anthropic-beta`, `openai-beta` and `idempotency-key`.

**Response headers the transport reads:**

| Header | Meaning |
|---|---|
| `retry-after-ms`, then `retry-after` | backoff hint (§3.9) |
| `x-init-error`, `x-init-cap` | proxy-generated error / cap refusal (§3.9) |
| `x-init-retries` | upstream retries the proxy already made (§3.9) |
| `x-init-req-id`, `request-id`, `x-request-id` | trace ids for `error` events |
| `x-init-head-hash` | the proxy's head hash. The loop compares it with the body's own value and log-context appends `error{code:"head_hash_mismatch"}` on a difference |

```rust
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;   // connect, try_clone, shutdown, set_read_timeout: stable since 1.10.0
use std::path::Path;
use std::time::Duration;

pub struct Response { pub status: u16, pub headers: Vec<(String, String)>, pub body: Body }
pub enum Body {
    Chunked(ChunkedReader<BufReader<UnixStream>>),
    Length(io::Take<BufReader<UnixStream>>),
    UntilClose(BufReader<UnixStream>),
}
impl Read for Body { /* dispatch */ }

pub fn send(sock: &Path, req: &HttpRequest, attempt: u32, cx: &Cx, abort: &Abort, t: &Timeouts) -> Result<Response, ProviderError> {
    let mut s = UnixStream::connect(sock).map_err(ProviderError::Connect)?;
    abort.register_stream(s.try_clone().map_err(ProviderError::Io)?)?;   // Err(Aborted) if already aborted
    let _ = s.set_write_timeout(Some(t.write));   // best effort, see §3.9
    let mut head = Vec::with_capacity(512);
    write!(head, "POST {} HTTP/1.1\r\nhost: init-proxy\r\ncontent-type: application/json\r\n\
                  accept: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n", req.path, req.body.len()).unwrap();
    for (k, v) in &req.headers { write!(head, "{k}: {v}\r\n").unwrap(); }
    write!(head, "x-init-session: {}\r\nx-init-purpose: {}\r\nx-init-attempt: {attempt}\r\n", cx.session, cx.purpose).unwrap();
    if let Some(t) = cx.task { write!(head, "x-init-task: {t}\r\n").unwrap(); }
    head.extend_from_slice(b"\r\n");
    s.write_all(&head).and_then(|_| s.write_all(&req.body)).map_err(|e| io_or_timeout(e, TimeoutPhase::Write, abort))?;
    let _ = s.set_read_timeout(Some(t.headers));
    let mut r = BufReader::with_capacity(64 * 1024, s);
    let (status, headers) = read_head(&mut r).map_err(|e| io_or_timeout(e, TimeoutPhase::Headers, abort))?;
    let _ = r.get_ref().set_read_timeout(Some(t.idle));
    let body = frame_body(r, &headers)?;   // chunked > content-length > until-close
    Ok(Response { status, headers, body })
}
```

**Response head parsing** (`read_head`):
1. Read lines with `read_until(b'\n')`. Each line must end in `\r\n`; a bare `\n` is accepted. The whole head is capped at 64 KiB, or it is `Protocol("header block too large")`.
2. Status line: `HTTP/1.1 <3 digits> <reason>`. `HTTP/1.0` is accepted. Anything else is `Protocol`.
3. Header lines are `name: value`. Names are lowercased, values trimmed of spaces and tabs. Obsolete line folding (a line that starts with whitespace) is `Protocol` (RFC 9112 §5.2).
4. `1xx` interim responses (for example `100 Continue`) are skipped, and parsing restarts at step 2.

**Body framing** (RFC 9112 §6.3):
- `transfer-encoding` whose last coding is `chunked`: use `ChunkedReader`. Any other transfer coding is `Protocol`.
- else `content-length: N`: use `Take(N)`. Several differing values are `Protocol`.
- else read until the connection closes.

**`ChunkedReader`** (RFC 9112 §7.1):

```rust
pub struct ChunkedReader<R> { inner: R, remaining: u64, done: bool }
impl<R: BufRead> Read for ChunkedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() { return Ok(0); }
        if self.remaining == 0 {
            let size = read_chunk_size(&mut self.inner)?;   // see below
            if size == 0 { read_trailers(&mut self.inner)?; self.done = true; return Ok(0); }
            self.remaining = size;
        }
        let want = buf.len().min(self.remaining.min(usize::MAX as u64) as usize);
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 { return Err(io::ErrorKind::UnexpectedEof.into()); }
        self.remaining -= n as u64;
        if self.remaining == 0 { expect_crlf(&mut self.inner)?; }
        Ok(n)
    }
}
```

- `read_chunk_size`: read one line (cap 4,096 bytes). Take the bytes before the first `;` (chunk extensions are ignored) and trim spaces and tabs. Parse 1–16 hex digits as `u64`; anything else is `InvalidData`. Sizes above 1 GiB are `InvalidData`, because no SSE stream sends such chunks.
- `read_trailers`: read lines until an empty line, capped at 64 KiB in total, and discard them.
- `expect_crlf`: the two bytes after the chunk data must be `\r\n` (a bare `\n` is accepted).
- EOF inside a chunk is `UnexpectedEof`, which the transport maps to `Truncated` (retryable).

**Error bodies.** For `status != 200`, read at most 64 KiB of the body, try to parse it as JSON and classify it (§3.11). The body is never streamed to the loop.

### 3.7 SSE parser

This follows the WHATWG "interpreting an event stream" rules, checked 2026-10-19. It works on bytes and decodes UTF-8 only per event.

```rust
pub struct SseEvent { pub event: String, pub data: String, pub id: Option<String> }  // event "" ⇒ "message"

pub struct SseReader<R> {
    inner: R,
    line: Vec<u8>, data: Vec<u8>, event: Vec<u8>, id: Option<String>,
    skip_lf: bool,        // last byte consumed was CR: a following LF belongs to the same terminator
    at_start: bool,       // strip one leading UTF-8 BOM
    pub comments: u64,    // count of ':' lines (keep-alives); reading any bytes already reset the idle timer
}
impl<R: BufRead> SseReader<R> {
    /// Ok(None) at clean EOF. A pending event without its terminating blank line is discarded (spec).
    pub fn next_event(&mut self) -> Result<Option<SseEvent>, ProviderError>;
}
```

Algorithm for `next_event`:
1. Loop on `fill_buf()`. An empty buffer means EOF: return `Ok(None)` and discard any partial event.
2. Scan the buffer. If `skip_lf` is set and the first byte is `\n`, consume it and clear the flag. Otherwise copy bytes into `line` up to the next `\r` or `\n`. On `\r`, set `skip_lf`. A line ends at CRLF, a lone LF, or a lone CR (WHATWG). `consume()` exactly what was scanned.
3. Cap: if `line` exceeds 32 MiB, or `data` exceeds 64 MiB, return `Protocol("sse line too long")`. OpenAI's `response.completed` event carries the whole response object, so the cap is generous. It bounds memory against a broken upstream (spec §9 rank 5).
4. Process each complete line:
   - At the very first line only, strip a leading `EF BB BF`.
   - Empty line: if `data` is empty, reset `event` and continue. Otherwise remove one trailing `\n` from `data`, build the `SseEvent` (`String::from_utf8`; invalid UTF-8 is `Protocol`), reset `data` and `event`, and return it.
   - The line starts with `:`: it is a comment. `comments += 1`. DeepSeek sends `: keep-alive` while a request is queued.
   - Otherwise split at the first `:` into field and value. With no colon, the whole line is the field and the value is empty. Remove one leading U+0020 from the value. Field names are compared case-sensitively:
     - `event`: replace `event`;
     - `data`: append the value, then `\n`;
     - `id`: set `id` unless the value contains NUL;
     - `retry` and anything else: ignored. The body never reconnects an SSE stream; a retry repeats the whole request.

### 3.8 Adapters

All request bodies are built as `serde_json::Value` (whose `Map` is a `BTreeMap`) and written with `serde_json::to_vec`. Keys are therefore sorted at every level, and the same `Prompt` always yields the same bytes. The examples below show sorted keys because that is exactly what goes on the wire.

#### 3.8.1 DeepSeek Chat Completions (first real provider)

Endpoint: `POST /deepseek/chat/completions`. The proxy maps this to `https://api.deepseek.com/chat/completions`. No extra headers.

**Request body (exact shape):**

```json
{
  "max_tokens": 65536,
  "messages": [
    {"content": "<head.core>\n<head.self_>", "role": "system"},
    {"content": "Fix the failing test in work/demo.", "role": "user"},
    {"content": "", "reasoning_content": "The user wants ... I should run the tests first.", "role": "assistant",
     "tool_calls": [{"function": {"arguments": "{\"command\": \"cd work/demo && cargo test 2>&1 | tail -20\"}", "name": "bash"},
                     "id": "call_00_kw66qNnNto11bSfJVIdlV5Oo", "type": "function"}]},
    {"content": "test add ... FAILED\n[exit_code=101 wall_time=3.21s]", "role": "tool", "tool_call_id": "call_00_kw66qNnNto11bSfJVIdlV5Oo"}
  ],
  "model": "deepseek-flash",
  "reasoning_effort": "high",
  "stream": true,
  "thinking": {"type": "enabled"},
  "tools": [
    {"function": {"description": "<tools.md text>", "name": "read", "parameters": {"additionalProperties": false, "properties": {"...": {}}, "required": ["path"], "type": "object"}}, "type": "function"}
  ]
}
```

Field rules:
- `messages[0]` is `{"role":"system","content": core + "\n" + self_}` (log-context §3.10.1). The system text is always one string, never content parts.
- `Message::Note` → `{"role":"user","content":<text>}` after the group's tool messages (`note_placement = UserMessage`).
- `Message::User`: `content` is a string when it holds only text blocks (joined with `\n\n`). With any image it is a content-part array: `{"type":"text","text":…}` and `{"type":"image_url","image_url":{"url":"data:<media_type>;base64,<data>"}}`. Images are sent only when `ModelInfo.images`; otherwise the handoff filter has already replaced them (§3.10).
- `Message::Assistant` maps to one message with:
  - `content`: all `Text` blocks concatenated in order, or `""` when there are none. DeepSeek returns `content=''` on tool-call turns, so `""` replays what the model produced.
  - `reasoning_content`: the `text` of every `Thinking` block whose `provider` is `deepseek` (or `mock`), concatenated. Required (see below), so it is on every assistant message: `""` when the message has no such block. That covers a turn paused before its first token (loop.md §3.5.1, replayed as `[no visible output]`), a tool call whose reasoning came back empty, and a foreign turn (§3.10). Up to v0.3.2 the field was left out there, so each such request was refused once (400) before the fallback succeeded (REL-032 observation). Generic Chat servers never get the field.
  - `tool_calls`: `[{"id","type":"function","function":{"name","arguments": ToolCall.raw_arguments verbatim}}]` in emitted order. Omitted when there are none.
- `Message::ToolResults` maps to one `{"role":"tool","tool_call_id":…,"content":…}` message per result, in call order. `content` is a string, or a content-part array when the result has images (the tool message accepts `image_url` parts; DeepSeek API reference, checked 2026-10-19).
  - `is_error` has no wire field. Error results already start with `error:` (tools.md §4.6), so the model can still tell them apart.
- Steer text that follows tool results is a separate `{"role":"user"}` message after the tool messages. The API accepts that order.
- `thinking: {"type":"enabled"}` is sent explicitly, even though it is the default, so a server default change cannot silently change behavior.
- `reasoning_effort` mapping: Low→`"low"`, Medium→`"high"`, High→`"high"`, XHigh→`"high"`, Max→`"max"`. The server maps `medium` and `xhigh` to `high` itself; the body sends the mapped value so the bytes say what actually runs.
- Not sent:
  - `temperature`, `top_p`, `presence_penalty`, `frequency_penalty`: no effect in thinking mode, or deprecated.
  - `tool_choice`: the default is `auto` when tools are present. `required` and named choices return 400 in thinking mode.
  - `stream_options`: usage arrives on the last chunk anyway. `include_usage: true` would add `"usage":null` to every chunk. log-context §3.10.1's illustrative skeleton shows it. The head hash excludes it, so the difference changes no cache behavior; this doc, as the adapter owner, leaves it out.
  - `user_id`: it changes KV-cache isolation, and the body has one user.
  - `response_format`, `stop`, `logprobs`.
  - `strict` on tools: beta, needs the `/beta` base URL, and requires every property to be required. The seed schemas have optional fields.

**The reasoning pass-back rule (verified).** DeepSeek's thinking-mode guide says that when a request carries `tools`, "the `reasoning_content` must be fully passed back to the API in all subsequent requests — even for turns where the model did not perform a tool call. If your code does not correctly pass back `reasoning_content`, the API will return a 400 error." INIT always sends tools, so:
- every DeepSeek-origin assistant message keeps its `reasoning_content` in the log and in every later request, until compaction removes the message itself;
- neither the handoff filter (§3.10) nor log-context's epoch stripping (`strip_thinking_before_epoch = false`) ever drops DeepSeek thinking replayed to the same DeepSeek model;
- this is the opposite of the no-tools case, where DeepSeek ignores reasoning from earlier turns.

**Stream decoding.** Each SSE `data` is either `[DONE]` or a `chat.completion.chunk` object. The decoder state is:

```rust
struct DsDecoder {
    id: Option<String>, model: String,
    reasoning: String, text: String,
    calls: std::collections::BTreeMap<u32, PartialCall>,   // keyed by delta.tool_calls[].index
    finish: Option<String>, usage: Option<Usage>, done: bool, ready_sent: bool,
}
struct PartialCall { id: String, name: String, args: String }
```

For each chunk:
1. Parse the JSON. A parse error is `Protocol`. A chunk that carries an `error` object is `Stream{Other}` (not retried). Record `id` from the first chunk. `Origin.model` and `Thinking.model` are the request's model id, not the chunk's `model`.
2. Read `choices[0]`. A chunk with no choices is legal; only its `usage` is used. `choices.len() > 1` is `Protocol`, because `n` is never sent.
3. `delta.reasoning_content` (string, non-empty): append to `reasoning` and emit `ThinkingDelta`.
4. `delta.content` (string, non-empty): append to `text` and emit `TextDelta`. Null and `""` are ignored.
5. `delta.tool_calls[]`: for each element, `index` is required (else `Protocol`). Look up or create `calls[index]`.
   - `id`: when non-empty, set it if unset. If it is already set to a different value, that is `Protocol`.
   - `function.name`: the same first-value rule.
   - `function.arguments`: append, possibly an empty string.
   - The API documents that the first chunk of each call carries `id`, `type` and `name`, and later chunks carry only argument fragments. Calls can interleave by `index`; the map handles that.
6. `finish_reason` non-null: record it, and emit `ToolCallReady` for every call that has an `id` and a `name`, once.
7. `usage` non-null: map it as below and emit `Usage`.
8. `data: [DONE]`: set `done`. Events after `[DONE]` are `Protocol`.

`finish()` requires `done` and a recorded `finish_reason`. It then assembles the content as `[Thinking{provider:"deepseek", model, text: reasoning, native: None}]` (if non-empty), then `[Text{text}]` (if non-empty), then the calls in ascending `index`. For each call, `id` and `name` must be non-empty, otherwise it is `Protocol`. (`ToolCallReady` was already emitted at step 6.) Invalid JSON in `args` is *not* a decode error: the loop returns an `invalid_json` error result for that call (loop.md §3.6).

**finish_reason mapping:**

| finish_reason | Result |
|---|---|
| `tool_calls` | `ToolUse`; there must be ≥1 call, else `Protocol` |
| `stop` | `ToolUse` if any calls were assembled, else `EndTurn` |
| `length` | `MaxTokens`; calls are kept in the message but not run |
| `content_filter` | `Refusal { category: Some("content_filter") }` |
| `insufficient_system_resource` | `Stream { kind: Overloaded }`: retryable; the attempt's output is void |
| `aborted` (server-side) | `Stream { kind: Server }`: retryable |
| anything else | `Protocol` |

**Usage mapping:**

| DeepSeek field | `Usage` field |
|---|---|
| `prompt_tokens` (= hit + miss, per API reference) | `input` |
| `prompt_cache_hit_tokens` (same as `prompt_tokens_details.cached_tokens`) | `cache_read` |
| — | `cache_write = 0`; DeepSeek does not bill writes |
| `completion_tokens` | `output` |
| `completion_tokens_details.reasoning_tokens` (absent ⇒ 0) | `reasoning` |

**Errors.** HTTP statuses:
- 400 Invalid Format, 422 Invalid Parameters: `InvalidRequest` (not retried). If the message mentions context length it is `ContextOverflow` (heuristic below).
- 401: `Auth`. 402 Insufficient Balance: `Billing`.
- 429: `RateLimit` (concurrency limit), retryable.
- 500: `Server`. 503 Server Overloaded: `Overloaded`. Both retryable.

The context-overflow heuristic matches case-insensitively on `context length`, `maximum context`, `too long` or `prompt is too long`. DeepSeek's exact message is UNVERIFIED (Phase 0 check V-DS-6).

**Keep-alives and the 10-minute queue.** DeepSeek says that under load, streaming requests "continuously return SSE keep-alive comments", and a request that has not started inference after 10 minutes is closed by the server. Comments reset the idle timer (§3.11), so a queued request does not time out early. A server close after 10 minutes arrives as `Truncated`, which is retried.

**DeepSeek facts that drive design elsewhere:**
- Context caching is automatic, on disk, best-effort, and matches whole persisted "cache prefix units". Units are persisted at the end of the user input and at the end of the model output of each request, plus common-prefix and fixed-interval units. A request hits only if it *fully* matches a unit. So replaying the model's own output byte-for-byte (`ToolCall.raw_arguments` verbatim, `content` and `reasoning_content` verbatim) is what lets request N+1 hit the unit that ends at request N's output.
- Price (checked 2026-10-19), deepseek-flash, per 1M tokens: cache hit $0.003 off-peak / $0.006 peak; miss $0.15 / $0.30; output $0.60 / $1.20. A miss costs 50× a hit, so the cache is the dominant cost lever (spec §9 rank 1).
- "The Chat Completion API does not support inserting tool calls mid-conversation" (DeepSeek tool-calls guide). Tool calls must come from the model. This matters only for handoff (§3.10).

#### 3.8.2 Anthropic Messages

Endpoint: `POST /anthropic/v1/messages`. Extra headers: `anthropic-version: 2023-06-01` and `anthropic-beta: thinking-binding-controls-2026-08-01` (see the binding rules below).

**Request body:**

```json
{
  "cache_control": {"type": "ephemeral"},
  "max_tokens": 64000,
  "messages": [
    {"content": [{"text": "Fix the failing test in work/demo.", "type": "text"}], "role": "user"},
    {"content": [
       {"signature": "EqQBCgIYAh...", "thinking": "", "type": "thinking"},
       {"text": "Running the tests first.", "type": "text"},
       {"id": "toolu_01A09q90qw90lq917835lq9", "input": {"command": "cd work/demo && cargo test"}, "name": "bash", "type": "tool_use"}],
     "role": "assistant"},
    {"content": [
       {"content": [{"text": "test add ... FAILED\n[exit_code=101 wall_time=3.21s]", "type": "text"}], "is_error": true, "tool_use_id": "toolu_01A09q90qw90lq917835lq9", "type": "tool_result"}],
     "role": "user"}
  ],
  "model": "claude-opus-5-5",
  "output_config": {"effort": "high"},
  "stream": true,
  "system": [{"cache_control": {"type": "ephemeral"}, "text": "<head.core>", "type": "text"},
             {"cache_control": {"type": "ephemeral"}, "text": "<head.self_>", "type": "text"}],
  "thinking": {"block_binding": {"prefix_mismatch_behavior": "drop_block"}, "type": "adaptive"},
  "tools": [{"description": "<tools.md text>", "input_schema": {"...": {}}, "name": "read"}]
}
```

Rules:
- **Cache markers: at most 4, placed exactly as log-context.md §3.10.2 specifies** (that doc owns the layout; the adapter renders it):
  - B1: explicit `cache_control` on `system[0]` (core). Tools render before the system prompt, so this caches tools + core together.
  - B2: explicit `cache_control` on `system[1]` (self).
  - B3: explicit `cache_control` on the last block of message `Prompt.b3`, only when log-context sets it (more than 15 positions appended since the previous request, to stay inside the 20-position lookback).
  - A: the top-level automatic `cache_control`, which the API places on the last cacheable block and moves forward as the conversation grows.
  - Default 5-minute TTL everywhere. Busy agent turns start well under 5 minutes apart, so the 1-hour TTL's 2× write price does not pay.
- `Message::Note` → `{"role":"system","content":<text>}` after the group's user message (mid-conversation system message, supported by both seed models; it must follow a user message and be followed by the next assistant turn, which log-context's grouping guarantees).
- `thinking: {"type":"adaptive", "block_binding":{…}}` (binding rule 3 below), no `display`. The default display is `omitted`: thinking blocks stream with empty text plus a signature. This keeps the log small, and billing is the same either way.
- `output_config.effort`: Low→`low`, Medium→`medium`, High→`high`, XHigh→`xhigh`, Max→`max`. It is always sent: Claude Opus 5.5 defaults to `medium`, so leaving it out would silently change depth.
- Not sent:
  - `temperature`, `top_p`, `top_k`: 400 on current models.
  - `tool_choice`: the default is `auto`. Forced `any`/`tool` returns 400 on Opus 5.5.
  - `budget_tokens`, `thinking.type: "disabled"`: 400 on Opus 5.5.
  - `eager_input_streaming`: the loop waits for complete messages, so buffered tool input costs nothing.
  - `fallbacks`: see Q-P5.
- **Message mapping:**
  - `User`: content blocks `{"type":"text"}` and `{"type":"image","source":{"type":"base64","media_type","data"}}`.
  - `Assistant`: blocks in emitted order.
    - `Thinking` with `provider == "anthropic"` → its `native` object verbatim (`{"type":"thinking","thinking","signature"}` or `{"type":"redacted_thinking","data"}`).
    - `Text` → `{"type":"text","text"}`.
    - `ToolCall` → `{"type":"tool_use","id","name","input"}`. If the arguments failed to parse, `input` is `{}`; the loop already returned an `invalid_json` error result for that call.
    - `Opaque{Messages}` → `raw` verbatim.
  - `ToolResults` → one `user` message of `tool_result` blocks in call order. Each is `{"type":"tool_result","tool_use_id","content":[text/image blocks],"is_error":true}`, with `is_error` present only when true.
  - A `User` message that directly follows `ToolResults` (steer text) is **merged** into the same `user` message, after the `tool_result` blocks. All results go in a single user message (parallel tool use guidance), and text after the results is allowed.
  - Empty `text` blocks are skipped; the API rejects them.
- **Thinking-block binding (preserved thinking).** On Claude Opus 5.5 and Sonnet 5.5, a thinking block's signature binds it to the model *and* to the exact conversation prefix. Editing, removing or reordering an earlier turn, or rebuilding `system` or `tools`, invalidates every later thinking block. Accounts created on or after 2026-08-31 get a 400 `invalid_request_error`. Four rules handle it:
  1. **Model.** The handoff filter (§3.10) replays Anthropic thinking only to the model that produced it.
  2. **Epochs** (masking, compaction, a head reload) are log-context's. Its projection omits every thinking block before the latest epoch (`strip_thinking_before_epoch = true`), and removing them all is valid.
  3. **Detection.** Every request sends `anthropic-beta: thinking-binding-controls-2026-08-01` and `thinking.block_binding.prefix_mismatch_behavior`:
     - `"drop_block"` in production;
     - `"error"` when `INIT_TEST=1` and `INIT_TEST_THINKING_BINDING=error` (tests, CI).

     With the header, `message_start.message.input_transformations` lists any dropped block. Each non-empty entry becomes `StreamEvent::Warning{code:"thinking_dropped", detail:<entry JSON>}`, which the loop logs as `error{code:"thinking_dropped"}`. This is how an accidental history edit is caught (log-context §3.10.2).
  4. **Fallback.** If the API rejects the beta or the `block_binding` field (a 400 that names `block_binding` or `anthropic-beta`), resend once without both, and keep them off for the rest of the process. If a 400 says `bound to a different conversation`, strip **all** thinking blocks from the request and retry once (the documented "strip and retry" recovery). Both cases emit `Retry` and record the cause for the loop's `error` event.
- **Prompt-cache minimum.** Opus 5.5's minimum cacheable prefix is model-dependent; the claude-api skill lists 512 tokens on the newest models. INIT's system prefix (~2–4k tokens) is above it.

**Stream decoding** (event name = `data.type`; per the streaming docs, checked 2026-10-19):

| SSE event | Action |
|---|---|
| `message_start` | record `message.id` and `message.model`; usage = `message.usage` → emit `Usage` |
| `content_block_start` | open block `index`: `text` → Text(""); `thinking` → Thinking{provider:"anthropic", model, text:"", native:{type:"thinking", thinking:"", signature:""}}; `redacted_thinking` → Thinking{text:"", native: the block verbatim}; `tool_use` → PartialCall{id, name, json: ""}; any other type → Opaque(raw block) |
| `content_block_delta` `text_delta` | append `text` → `TextDelta` |
| `content_block_delta` `thinking_delta` | append `thinking` to `text` and to `native.thinking` → `ThinkingDelta` (empty under `display: omitted`) |
| `content_block_delta` `signature_delta` | set `native.signature` (arrives just before `content_block_stop`) |
| `content_block_delta` `input_json_delta` | append `partial_json` to the call's `json` |
| `content_block_delta` `citations_delta` / unknown | ignore |
| `content_block_stop` | close block. tool_use: parse `json` (`""` ⇒ `{}`) into `input`; on a parse failure set `input = {}` and `raw_arguments = Some(json)`; → `ToolCallReady`. A thinking block that closes without a signature is dropped. In an aborted partial message its text is kept with `native: None`, so it reaches the log but is never replayed |
| `message_delta` | `delta.stop_reason`, `delta.stop_sequence`, `stop_details`; `usage` is **cumulative**, merge field-wise → emit `Usage` |
| `message_stop` | finished |
| `ping` | ignore |
| `error` | `{"error":{"type","message"}}` → `Stream{kind}`: `overloaded_error`→Overloaded, `api_error`→Server, `rate_limit_error`→RateLimit (retryable); others not retryable |
| unknown event type | ignore (versioning policy: new event types may be added) |

**stop_reason mapping:** `end_turn`→EndTurn; `tool_use`→ToolUse; `max_tokens`→MaxTokens; `stop_sequence`→StopSequence; `pause_turn`→PauseTurn; `refusal`→Refusal{category: stop_details.category}; `model_context_window_exceeded`→ContextWindow. Any other value is `Protocol`.

**Usage:** `input = input_tokens + cache_creation_input_tokens + cache_read_input_tokens` (Anthropic's `input_tokens` counts only the uncached part); `cache_read = cache_read_input_tokens`; `cache_write = cache_creation_input_tokens`; `output = output_tokens`; `reasoning = 0`, because it is not reported separately.

**HTTP errors** (error shape `{"type":"error","error":{"type","message"},"request_id"}`):
- 400 `invalid_request_error`: `InvalidRequest`, or `ContextOverflow` when the message contains `prompt is too long`.
- 401: Auth. 402: Billing. 403: Permission. 404: NotFound. 413: TooLarge (all not retried).
- 429: RateLimit, retryable, *unless* it is a spend-limit 429: `error.details.error_code == "enforced_spend_limit_reached"` (kernel/proxy.md §3.9), or a tier spend-cap 429, which has no `retry-after` header. Those are `Billing`, not retried.
- 500 `api_error`: Server. 504 `timeout_error`: Server. 529 `overloaded_error`: Overloaded (all retryable).

#### 3.8.3 OpenAI Responses

Endpoint: `POST /openai/v1/responses`. No extra headers.

**Request body:**

```json
{
  "include": ["reasoning.encrypted_content"],
  "input": [
    {"content": [{"text": "<head.core>", "type": "input_text"}, {"prompt_cache_breakpoint": {"mode": "explicit"}, "text": "<head.self_>", "type": "input_text"}], "role": "developer", "type": "message"},
    {"content": [{"text": "Fix the failing test in work/demo.", "type": "input_text"}], "role": "user", "type": "message"},
    {"encrypted_content": "gAAAAB...", "id": "rs_6806bfca0b2481918a57", "summary": [], "type": "reasoning"},
    {"arguments": "{\"command\":\"cd work/demo && cargo test\"}", "call_id": "call_12345xyz", "name": "bash", "type": "function_call"},
    {"call_id": "call_12345xyz", "output": "test add ... FAILED\n[exit_code=101 wall_time=3.21s]", "type": "function_call_output"}
  ],
  "max_output_tokens": 64000,
  "model": "gpt-6-astra",
  "parallel_tool_calls": true,
  "prompt_cache_key": "<INIT_SESSION>",
  "reasoning": {"effort": "high"},
  "store": false,
  "stream": true,
  "tools": [{"description": "<tools.md text>", "name": "read", "parameters": {"...": {}}, "strict": false, "type": "function"}]
}
```

Rules:
- The head goes in a leading `developer` message with two `input_text` parts, not in `instructions`, with an explicit `prompt_cache_breakpoint` on the `self_` part (log-context §3.10.3). `instructions` cannot carry a breakpoint. On GPT-5.6 and later models, implicit caching (the default mode) also places a breakpoint at "the last developer message in the initial consecutive group of developer messages". So the stable head gets its own cache entry, reusable across sessions.
- `Message::Note` → a `developer` message item after the group's `function_call_output` items (`note_placement = DeveloperMessage`; mid-input developer semantics UNVERIFIED, log-context T-PROV-6).
- `include: ["reasoning.encrypted_content"]` is legacy. The reasoning guide says stateless requests return `encrypted_content` by default and still accept the field. It is sent anyway, so replay never depends on that default.
- `previous_response_id` is never sent: every request carries the full context. A change of the proxy's WebSocket connection (`x-init-ws-conn`, kernel/proxy.md §3.15) therefore needs no handling, and `previous_response_not_found` cannot occur. If an `error` with that code ever arrives, it is classified `InvalidRequest` and logged as a body bug.
- `store: false`: the request is stateless (no server-side conversation, no `previous_response_id`). Reasoning items then carry `encrypted_content` by default (reasoning guide, checked 2026-10-19). The full history is replayed each turn, exactly as for the other two APIs, so the log stays the single source of truth.
- `prompt_cache_key`: the session id. It is optional for cache accounting on GPT-5.6+, and recommended for routing on earlier models.
- `strict: false` is sent explicitly. Omitting `strict` lets Responses normalize the schema into strict mode, which would rewrite the tool contract server-side.
- `reasoning.effort` passes the normalized level through (`low` … `max`). No `summary` is requested (smaller log; same idea as Anthropic `omitted`).
- **Item mapping:**
  - `User` → `{"type":"message","role":"user","content":[input_text | input_image{"image_url":"data:…"}]}`.
  - `Assistant` blocks in order:
    - `Thinking` with `provider == "openai"` → its `native` reasoning item verbatim (`{"type":"reasoning","id","encrypted_content","summary":[…]}`), exactly as received in `response.output_item.done`.
    - `Text` → `{"type":"message","role":"assistant","content":[{"type":"output_text","text"}]}`. Consecutive texts are grouped into one message.
    - `ToolCall` → `{"type":"function_call","call_id","name","arguments": raw_arguments verbatim}`.
    - `Opaque{Responses}` → raw.
  - `ToolResults` → one `function_call_output` per result. `output` is a string, or an array of `input_text` / `input_image` objects when the result has images (function-calling guide, checked 2026-10-19).

**Stream decoding** (events from the Responses streaming reference, checked 2026-10-19):

| Event | Action |
|---|---|
| `response.created` / `response.in_progress` / `response.queued` | record `response.id`, `response.model` |
| `response.output_item.added` | by `item.type`: `message` → open text slot at `output_index`; `reasoning` → open Thinking{provider:"openai", model, text:"", native: None}; `function_call` → PartialCall{id: call_id, name, json: item.arguments}; other → Opaque slot |
| `response.output_text.delta` | append `delta` to the slot `output_index` → `TextDelta` |
| `response.refusal.delta` | append to the slot's text; mark the response refused |
| `response.reasoning_summary_text.delta` / `response.reasoning_text.delta` | append → `ThinkingDelta` |
| `response.function_call_arguments.delta` | append `delta` to the PartialCall's `json` at `output_index` |
| `response.function_call_arguments.done` | set `json` = `arguments` (authoritative) |
| `response.output_item.done` | replace the slot with the final `item`; for reasoning set `native` = the done `item` verbatim and `text` = its summary texts joined with `\n` (the docs say the `added` copy's `encrypted_content` may be incomplete); for function_call set `raw_arguments` = `arguments`, parse `input`, take `call_id`, `name` → `ToolCallReady` |
| `response.completed` | usage; stop = ToolUse if any function_call, else EndTurn (`Refusal{category: None}` if a refusal part was seen); finished |
| `response.incomplete` | `incomplete_details.reason`: `max_output_tokens`→MaxTokens; `content_filter`→Refusal{"content_filter"}; other → `Stream{Other}`; finished |
| `response.failed` | `response.error{code,message}` → `Stream{kind}` (`server_error`→Server, retryable; `rate_limit_exceeded`→RateLimit; else Other) |
| `error` | `{code,message,param}` → `Stream`; retryable when code is `server_error`, `server_is_overloaded`, `slow_down` or `rate_limit_exceeded` |
| any other `response.*` | ignore |

Output order is ascending `output_index`. Within a message item, content parts concatenate in order.

**Usage** (`response.usage`): `input = input_tokens`; `cache_read = input_tokens_details.cached_tokens`; `cache_write = input_tokens_details.cache_write_tokens` (absent ⇒ 0); `output = output_tokens`; `reasoning = output_tokens_details.reasoning_tokens`.

**HTTP errors:**
- 400: InvalidRequest, or ContextOverflow when `error.code == "context_length_exceeded"` (UNVERIFIED code name).
- 401: Auth.
- 429: retryable only for the "rate limit reached" and `slow_down` cases. The codes `credit_balance_exhausted`, `organization_spend_limit_exceeded`, `project_spend_limit_exceeded` and `organization_usage_limit_exceeded` are `Billing`, not retried (OpenAI error-codes guide: "Retrying billing, spend, or quota errors won't restore API access").
- 500: Server. 503 `server_is_overloaded`: Overloaded. Honor `Retry-After`.

### 3.9 Transport: retries, backoff, timeouts

**Defaults** (constants in `transport.rs`):

| Name | Value | Why |
|---|---|---|
| `WRITE_TIMEOUT` | 30 s | request bodies are ≤ a few MB over a local socket |
| `HEADERS_TIMEOUT` | 120 s | proxy → upstream connect + TLS + queueing before the first byte; DeepSeek streams keep-alives while queued |
| `IDLE_TIMEOUT` | 300 s | max silence between any two bytes (events *or* comments). Long thinking with omitted display sends only pings (Anthropic) |
| total request time | none | Fable-class turns legitimately run many minutes; the kernel budget governs cost |
| `MAX_ATTEMPTS` | 12 | per request |
| `RETRY_WINDOW` | 30 min | stop retrying once the first failure is older than this |
| backoff | `delay = min(60 s, 1 s · 2^(attempt−1))`, then full jitter: uniform in `[delay/2, delay]` | |
| `retry-after-ms` / `retry-after` | the first present wins; `retry-after-ms` in integer ms, `retry-after` in integer seconds; capped at 600 s; wait `max(hint, backoff)` | the proxy forwards both (kernel/proxy.md §4.1); HTTP-date values are ignored (backoff is used) |
| `x-init-retries: n` | the proxy already retried upstream `n` times | the body's attempt counter for the *delay* jumps by `n`, so body and proxy backoff do not restart from 1 s together (proxy.md §3.9) |
| `MAX_ERROR_BODY` | 64 KiB | |

- Jitter randomness: `std::collections::hash_map::RandomState::new().hash_one(attempt)` gives OS-seeded random bits with no dependency. It never reaches a prompt, so CONVENTIONS' "no `HashMap` in prompts" rule is not involved.
- A read timeout surfaces as `io::ErrorKind::WouldBlock` or `TimedOut`. The std docs say the kind is platform-dependent: Unix "typically" returns `WouldBlock`. Both map to `Timeout(phase)`.
- The socket timeouts are set best-effort. macOS returns EINVAL from `setsockopt` once the peer has closed; the read or write that follows then reports the real condition, so a failed set is not an error. The three timeouts travel in `Transport.timeouts`, so tests can shrink them.

**Retry classification** (`ProviderError::retryable`):

| Error | Retried? |
|---|---|
| `Connect` (ENOENT/ECONNREFUSED: the proxy is restarting) | yes |
| `Io`, `Truncated`, `Timeout(Headers \| Idle)` | yes |
| `Timeout(Write)` | yes |
| `Http` 408, 409, 425, 429 (RateLimit), 500, 502, 503, 504, 529 | yes |
| `Stream{RateLimit \| Overloaded \| Server}` | yes |
| `Http` 400/401/402/403/404/413/422, `Billing`, `Auth`, `Permission`, `InvalidRequest` | no |
| `ContextOverflow` | no (the loop compacts and calls again; loop.md §3.6) |
| `Budget` (kernel proxy cap) | **never** (it is the owner's spend limit, invariant 3) |
| `Protocol` | no (a bug or a broken upstream; fail loudly) |
| `Aborted` | no |
| Anthropic thinking-binding 400 | once, with thinking stripped (§3.8.2) |

**Proxy-generated errors** (kernel/proxy.md §4.2). Every response the proxy makes itself carries `x-init-error: <code>`:
- cap refusals also carry `x-init-cap: <cap name>` and an error object of type `init_cap_exceeded` (OpenAI-style `{"error":{"code":"budget.day","type":"init_cap_exceeded",…},"init":{…}}` or Anthropic-style `{"type":"error","error":{"type":"init_cap_exceeded",…}}`, by route family). Either `x-init-cap` or that type makes the response `Budget`, which is never retried. It is the owner's spend limit (invariant 3), not an upstream rate limit;
- any other `x-init-error` response is classified by HTTP status as usual, but **not retried** by the body. The proxy has already applied its own retry policy, or the error is local (`request.too_large`, `upstream.auth`, panic close).
- An upstream response passed through after the proxy's retries (`x-init-retries`) is classified normally and may still be retried by the body, with the delay jump above.

**Attempt loop** (inside `EventStream::next`):

```rust
loop {
    if abort.is_set() { return terminal(Done(decoder.partial_or_empty(Aborted))) }
    match self.state {
        State::Connect => match http::send(sock, &built, attempt, &cx, abort) {
            Ok(resp) if resp.status == 200 => { self.sse = SseReader::new(resp.body); push(Started{attempt, request_id}); State::Read }
            Ok(resp) => fail_or_retry(classify_http(resp)),
            Err(e)   => fail_or_retry(e),
        },
        State::Read => match self.sse.next_event() {
            Ok(Some(ev)) => { decoder.feed(&ev, &mut buf)?; drain buf; if decoder.finished() { return terminal(Done(decoder.finish()?)) } }
            Ok(None)     => fail_or_retry(if abort.is_set() { Aborted } else { Truncated }),   // shutdown() reads as EOF
            Err(e)       => fail_or_retry(e),
        },
    }
}
// fail_or_retry: if retryable && attempt < MAX_ATTEMPTS && first_failure.elapsed() < RETRY_WINDOW:
//     emit Retry{attempt, delay, error}; if abort.sleep(delay) { aborted } ; attempt += 1; decoder = p.decoder(req); State::Connect
//   else emit Failed(error)
```

- `abort.sleep(d)` waits on a `Condvar` with `wait_timeout`. It returns early (true) when abort is triggered, so a backoff never delays an abort.
- `build()` runs once per `stream()` call; retries resend the same bytes. The single exception is the Anthropic strip-and-retry, which rebuilds.
- `x-init-attempt` increments on each retry, so the proxy can tell a retry from a new request in its traces.

### 3.10 Cross-provider and cross-model handoff

When the owner changes provider or model (`boot.json`, then a `handoff` boot), the conversation continues in the same log. Every request is built through one pure filter, `handoff_filter(messages, target: &ModelInfo) -> Vec<Message>`, applied after log-context's projection. Epoch-based stripping is log-context's job, per `CacheProfile.strip_thinking_before_epoch`. The rules key on each block's own `provider`/`model` and the assistant message's `origin`:

| Content | Same provider + model | Different model, same api | Different api |
|---|---|---|---|
| User text, ToolResults text, Notes | keep | keep | keep |
| Images (User, ToolResults) | keep if `target.images`, else `Text{"[image omitted: <model> has no image input]"}` | same | same |
| Assistant `Text` | keep | keep | keep |
| Assistant `ToolCall` | keep | keep | keep, id sanitized (below) |
| `Thinking` (any provider) | keep | **drop** | **drop** |
| `Opaque{api}` | keep | keep | drop |

- Text and tool calls carry over everywhere, as in pi-ai's current rule ("assistant messages from a different model have their thinking blocks dropped; text and tool calls carry over").
- An assistant message left with no blocks gets one text block, `"[no visible output]"`, so every API accepts it (log-context §3.10.2 uses the same text).
- **Call id sanitizing (different api only).** Replace every byte outside `[A-Za-z0-9_-]` with `_` and truncate to 64 bytes. If two ids collide after sanitizing, append `_2`, `_3` … in order. The mapping is a pure function of the message list, so it yields the same bytes on every request. Anthropic's exact id pattern is UNVERIFIED; this subset is accepted by all three APIs as far as the docs show.
- **Into DeepSeek from another provider.** The foreign assistant messages carry tool calls and no DeepSeek thinking. DeepSeek documents that Chat Completions does not support inserting tool calls the model did not generate, and that missing `reasoning_content` gives 400. Live V-DS-3 (2026-10-20) found that a foreign tool turn without the field is refused, and the same turn with `"reasoning_content": ""` is accepted, the model using the foreign result as given. The plan:
  1. Send it with `"reasoning_content": ""` and the tool calls kept (§3.8.1).
  2. The fallback is a safety net. On a 400, a request with an assistant message that lacks the field (the builders no longer produce one) is retried once with `""` added (`deepseek_empty_reasoning_foreign_turns`). If DeepSeek still returns 400, the adapter's fallback flattens every foreign assistant turn into a single `assistant` text message. That text is the turn's text plus one line per call, `[called <name> <raw_arguments>]`. Each foreign tool result becomes a `user` message `"[result of <name> (<call id>)]\n<content>"`. The adapter retries once.
     Each flattened turn carries `"reasoning_content": ""`, and so does any other assistant message that lacks the field. DeepSeek documents the field as required on every assistant message in tool mode, `--strict` enforces that, and a bare flattened turn would get another 400. V-DS-3 checks this live.
  3. Flattening is deterministic, so later requests hit the cache.
- A provider or model switch is a full cache miss whatever the filter does (caches are per model). That cost is accepted: switches are rare owner actions.

### 3.11 Cancellation hooks (`Abort`)

The loop owns the abort policy (loop.md §3.5). The provider needs only this:

```rust
pub struct Abort {
    flag: std::sync::atomic::AtomicBool,
    stream: std::sync::Mutex<Option<UnixStream>>,   // clone of the in-flight connection
    wake: (std::sync::Mutex<()>, std::sync::Condvar),
    // ...live bash process groups, and the `kill_group: fn(i32)` hook given to `Abort::new` (boot passes tools::kill_group): see loop.md §3.1, §3.5
}
impl Abort {
    pub fn register_stream(&self, s: UnixStream) -> Result<(), ProviderError>;  // Err(Aborted) if flag already set
    pub fn is_set(&self) -> bool;
    pub fn sleep(&self, d: std::time::Duration) -> bool;                         // true = aborted
}
```

`trigger()`, called from the input thread, sets the flag, takes the registered stream and calls `shutdown(Shutdown::Both)` on it. A blocked `read` on the other handle then returns `Ok(0)`. This was verified on macOS 14 (Darwin 23.6, rustc 1.97.1, 2026-10-19): a reader blocked in `read` returned `Ok(0)` 200 ms after another thread called `shutdown` on a `try_clone` of the stream. On Linux, `shutdown(SHUT_RDWR)` on a connected AF_UNIX socket wakes blocked readers with EOF as well (standard behavior; not re-tested here, UNVERIFIED on the target distro). The transport sees EOF with `is_set()` and ends with `Done(partial, Aborted)`.

### 3.12 Scripted mock provider (deterministic, $0)

The mock exists so the loop, boot, handoff and the A4–A6 scenarios can run with no spend. It is a **mock upstream that speaks the real wire format of each API**. Two consequences follow:
- the body never knows it is talking to a mock (no body code path differs);
- the same fixtures exercise the real decoders.

It can be reached two ways:
1. **Body-only tests:** set `INIT_PROXY_SOCK` to the mock's Unix socket. The mock accepts `POST /<provider_id>/<path>` and ignores auth.
2. **Kernel tests (A4–A6):** the proxy forwards a provider to the mock's upstream address, so caps, traces and stats are also exercised. kernel/proxy.md §4.1 allows a loopback or `unix:` upstream only when `proxy.test_mode = true`, which refuses every key file.

The mock binary is `init-mockllm` in `kernel/crates/init-testkit`. It is installed read-only as `$INIT_HOME/kernel/bin/init-mockllm` and exposed to tests as `INIT_MOCKLLM_BIN`. testing.md §3.4 owns the binary and its harness additions (strict DeepSeek protocol checks, `--simulate-cache`, extra recording fields). This doc fixes the **script format** and the **rendering rules**.

**Accepted harness behaviour** (testing.md §3.4.4, §3.4.6; R2). These are additions around the script format, which they leave unchanged:
- **Built-in responders.** The **probe responder** (stateless) answers every `x-init-purpose: probe` request: one `bash {"command":"true"}` call, then `ready` once the last message is a tool result, which is exactly boot.md §3.9's round trip. Being stateless, it serves the two G2 arms that run in parallel on the same session id, where a shared cursor would interleave them. The **G3 responder** answers sessions whose first user message *contains* `[init:g3 v1 nonce=` (a G3 session opens with a probe round trip, so the marker need not be at the start). **Per-task scripts** (`tasks/<id>.mock.jsonl`, one cursor per session) answer G4 task sessions: the first user message has a line that starts with `[task <id>]` (a boot block may precede it), and `<id>` matches `[A-Za-z0-9._-]{1,128}`. A give-up responder answers when a script is missing. `--no-probe-responder` and `--no-g3-responder` hand those requests back to the script cursor, which is how a body-only test scripts the probe turns shown below.
- **Upstream mode** (the mock behind the real proxy, which strips every `x-init-*` header): the session is `_none`, a request is a probe when its latest user message contains `[init:probe]`, and a task session is keyed by a hash of its `[task <id>]` message.
- `--strict deepseek` protocol checks, `--simulate-cache`, `--record-bodies`, and the extra recording fields (`protocol_error`, `responder`, `cache_read`, `t_recv_ms`, `t_done_ms`), which never reach the response bytes.

**Script file** (`*.mock.jsonl`, one JSON object per line, `"v":1`):

```jsonl
{"v":1,"kind":"meta","provider":"deepseek","model":"deepseek-flash","name":"a4-probe-then-fix","chunk":16}
{"v":1,"kind":"turn","purpose":"probe","reply":{"calls":[{"input":{"command":"true"},"name":"bash"}],"stop":"tool_use","usage":{"cache_read":11776,"input":12034,"output":31}}}
{"v":1,"kind":"turn","purpose":"probe","expect":{"last_role":"tool"},"reply":{"stop":"end_turn","text":"ready"}}
{"v":1,"kind":"turn","expect":{"last_role":"user","last_text_contains":"Fix"},"reply":{"think":"Run the tests.","text":"","calls":[{"input":{"command":"cargo test"},"name":"bash"}],"stop":"tool_use"}}
{"v":1,"kind":"turn","fault":{"kind":"http","status":429,"headers":{"retry-after":"1"},"body":{"error":{"message":"rate limited","type":"rate_limit_error"}}}}
{"v":1,"kind":"turn","expect":{"last_role":"tool","prefix":"same"},"reply":{"text":"Done.","stop":"end_turn"}}
{"v":1,"kind":"turn","raw":{"status":200,"headers":{"content-type":"text/event-stream"},"body_file":"fixtures/deepseek/split-args.sse"}}
```

Line kinds and fields:
- `meta` (first line): `provider` (`deepseek` | `anthropic` | `openai`), `model`, `name`, `chunk` (default 16).
- `turn`: one response, consumed in order from a cursor keyed by (`x-init-session`, `x-init-purpose`). `purpose` defaults to `turn`.
- `expect` (optional): assertions on the request. On failure the mock answers 500 with `{"mock_error":"expect failed: …"}`, which the transport treats as `Server` and retries, so the test sees the failure in the mock's report and through exhausted retries. The assertions are:
  - `last_role` (`user` | `tool` | `assistant`);
  - `last_text_contains`;
  - `tools` (an exact list of names);
  - `prefix`: `"same"` (equal to the previous request in this cursor) or a hex SHA-256. The prefix bytes are the system text plus the tools array as sent, per api: DeepSeek `messages[0]` + `tools`; Anthropic `system` + `tools`; OpenAI the leading developer message + `tools`. This is how A2 checks the cache at $0.
- `reply`: rendered into the provider's real wire format:
  - `think` (string): DeepSeek `reasoning_content` deltas, Anthropic thinking block (empty text, signature `mock-sig-<n>`), OpenAI reasoning item (`encrypted_content: "mock-enc-<n>"`). A DeepSeek turn with no `think`, on a request that does not disable thinking, renders `reasoning_content: "Mock reasoning."`; `"think": ""` opts out. Without the default, `--strict deepseek` would reject the next request of this doc's own scripts, because the body replays no `reasoning_content` for a reply that had none, while real DeepSeek always reasons in thinking mode.
  - `text` (string).
  - `calls` (array of `{name, input}`): input is serialized with sorted keys, and arguments are cut into `chunk`-byte fragments.
  - `stop`: `end_turn` | `tool_use` | `max_tokens` | `refusal`.
  - `usage`: `{input, cache_read, cache_write, output, reasoning}`. Omitted fields are computed: input = request body bytes / 4, cache_read = 0.
- `fault` (instead of `reply`):
  - `{"kind":"http","status","headers","body"}`;
  - `{"kind":"cut","after_events":N}`: close the connection after N SSE events (gives `Truncated`);
  - `{"kind":"stall","after_events":N,"ms":M}`: go silent (gives `Timeout(Idle)` when M > the idle timeout; tests shrink the timeout through an `INIT_TEST_*` env var, testing.md);
  - `{"kind":"stream_error","after_events":N,"error":"overloaded_error"}`: Anthropic `error` event, OpenAI `response.failed`, DeepSeek `finish_reason:"insufficient_system_resource"`;
  - `{"kind":"budget"}`: 429 with `x-init-cap: day`, `x-init-error: budget.day` and the `init_cap_exceeded` body of kernel/proxy.md §4.2. proxy.md owns the cap format, so an earlier draft's `daily` is not used.
- `raw`: a byte-exact recorded response for adapter edge cases (`body_file` relative to the script).

**Rendering rules (deterministic):**
- `<n>` is the number of assistant messages already in the request, so ids are stateless and unique within a conversation. The response id is `mock-<label>-<n>`, where `<label>` is the purpose for a scripted turn and the responder's name otherwise. Call ids: DeepSeek `call_<ii>_mock<n>` (`ii` = the call's index in the reply, DeepSeek's own pattern), Anthropic `toolu_mock<n>_<i>`, OpenAI `call_mock<n>_<i>` and `fc_mock<n>_<i>`; an OpenAI reasoning item is `rs_mock<n>`. The response `model` echoes the request's (the meta `model` is the fallback);
- `created` timestamps are `0`;
- text and reasoning are cut into `chunk`-character deltas;
- DeepSeek ends with a final chunk carrying `finish_reason` + `usage`, then `data: [DONE]`;
- Anthropic emits `message_start`, then `ping` once, then the blocks, then `message_delta`, then `message_stop`;
- OpenAI emits `response.created`, then the per-item `added`/delta/`done` events, then `response.completed`.
- No clocks, no randomness: the same script always produces the same bytes.

**Recording:** the mock writes each request body to `<record_dir>/<seq>-<purpose>.json` and a summary line to `<record_dir>/mock.log.jsonl`: `{seq, purpose, expect_ok, prefix_sha256, fault}`. When the script runs out, the mock answers 500 `{"mock_error":"script exhausted"}`.

The same `.sse` fixtures (DeepSeek, Anthropic, OpenAI recordings plus hand-made edge cases) are fed straight into `SseReader` + `Decoder` in unit tests, with no socket (§8).

## 4. Interfaces

### 4.1 How the normalized types appear in the event log

body/log-context.md §4.2 owns the event schema. The normalized types map one to one onto its blocks:

| Rust (this doc) | Log (log-context §4.2) |
|---|---|
| `ContentBlock::Text{text}` | `{"type":"text","text"}` |
| `ContentBlock::Image{media_type, data_b64}` | `{"type":"image","media_type","blob":"sha256:…","path"?,"tokens_est"}`: bytes stored as a blob; the projection re-encodes them |
| `Thinking{provider, model, text, native}` | `{"type":"thinking","provider","model","text","native"?}` |
| `ToolCall{id, name, input, raw_arguments}` | `{"type":"tool_use","call_id","name","input","raw_arguments"?}` |
| `ContentBlock::Opaque{api, raw}` | `{"type":"opaque","api","raw"}` (log-context §4.2 `block_opaque`): preserved, replayed only to the same `api`, ignored by other readers |
| `AssistantMessage{stop, usage, origin}` | `message{role:"assistant", provider, model, stop_reason, usage, request{…}}`; mapping in §3.2 |
| `ToolResult{call_id, content, is_error, meta}` | `tool_result{call_id, output, is_error, spill_path, exit_code, wall_ms, …}` (log-context X8) |

Writers go through `serde_json::Value`, whose `Map` is a `BTreeMap`, so keys are sorted. A struct derive alone emits *declaration* order, so it is never written directly.

**Image token estimate** (log-context X4 asks this doc for it; stored as `tokens_est`):
- Anthropic: `ceil(w·h / 750)` after scaling the long edge down to 1,568 px (UNVERIFIED against the current vision docs).
- DeepSeek and OpenAI: no formula is published; use log-context's default of 1,600.
- `w` and `h` come from the image header: PNG IHDR bytes 16–23; GIF bytes 6–9; WebP VP8/VP8L/VP8X headers; JPEG the first SOF0/SOF2 marker. All are parsed in std, with no decoder.

### 4.2 Wire endpoints used (through `run/proxy.sock`)

| Provider id | Path | Extra headers | Upstream (proxy-side) |
|---|---|---|---|
| `deepseek` | `/deepseek/chat/completions` | — | `https://api.deepseek.com/chat/completions` |
| `anthropic` | `/anthropic/v1/messages` | `anthropic-version: 2023-06-01`, `anthropic-beta: thinking-binding-controls-2026-08-01` | `https://api.anthropic.com/v1/messages` |
| `openai` | `/openai/v1/responses` | — | `https://api.openai.com/v1/responses` |

### 4.3 Env vars read

`INIT_PROXY_SOCK` (the socket path). It must be ≤ 103 bytes on macOS: `sun_path` holds 104 bytes including the NUL. Locally, `UnixListener::bind` with a 249-byte path failed with `InvalidInput: "path must be shorter than SUN_LEN"`. Linux's limit is 107. boot.md §3.2 checks this. Test-only overrides (testing.md §3.4.6) are honored **only when `INIT_TEST=1`** is in the environment; the kernel sets it only with `test_mode = true`:
- `INIT_TEST_IDLE_TIMEOUT_MS`, `INIT_TEST_CONNECT_TIMEOUT_MS` (accepted but not applied: `connect` on an AF_UNIX socket does not block), `INIT_TEST_RETRY_BASE_MS`;
- this doc adds `INIT_TEST_THINKING_BINDING` (`error` | `drop_block`) and `INIT_TEST_PROBE_WATCHDOG_MS` (boot.md §3.9).

## 5. Failure modes and recovery

| Failure | Detection | Recovery |
|---|---|---|
| Proxy down or restarting | `Connect` ENOENT/ECONNREFUSED | retry with backoff up to 30 min; the loop shows `retry` to the owner |
| Upstream 429/5xx/529/overloaded | HTTP status / stream error event | backoff, honor `retry-after` |
| Owner budget reached | proxy 429 + `x-init-cap` | `Failed(Budget)`; the loop stops the run and reports; no retry (invariant 3) |
| Mid-stream disconnect | EOF before terminal event | `Truncated` → full retry; partial output void (no tool side effects, since tools run after `Done`) |
| Stalled stream | no byte for 300 s | `Timeout(Idle)` → retry |
| Malformed SSE/JSON | parse failure | `Protocol` → `Failed`; the raw request id goes into the loop's `error` event; the proxy trace has the bytes |
| Tool args not valid JSON | `ToolCall::args_error()` is Some | not a provider error: the loop returns an `invalid_json` error result and the model retries |
| Context too long | 400 + overflow message | `Failed(ContextOverflow)` → the loop compacts and calls again once |
| Anthropic thinking binding | 400 "bound to a different conversation" | strip thinking, retry once; report so log-context can find the edit |
| DeepSeek missing `reasoning_content` | 400 | indicates a body bug (DeepSeek thinking must never be dropped); `Failed`; the Phase 0 check V-DS-2 pins the behavior |
| Abort during stream | `Abort` flag + socket shutdown | `Done(partial, Aborted)` |
| Abort during backoff | `Condvar` wake | `Done(empty, Aborted)` |

## 6. Performance notes (spec §9)

- **Rank 1, cache stability.**
  - Sorted keys everywhere.
  - No timestamps, request ids or random values in any body.
  - Model output is replayed byte-for-byte (`raw_arguments`, `content`, `reasoning_content`, Anthropic/OpenAI `native` objects).
  - No `stream_options` (DeepSeek would otherwise add `"usage":null` per chunk; this is about output bytes, not the cache, but costs nothing to avoid).
  - Anthropic: one explicit breakpoint after `system` plus automatic caching of the moving tail.
  - OpenAI: the prefix sits in a leading developer message, so it gets its own implicit breakpoint, plus `prompt_cache_key`.
  - Thinking is replayed only where it stays valid, so no request is rejected and rebuilt, which would cost a full miss.
- **Rank 2, round-trips.**
  - The proxy keeps upstream connections alive; the body's per-request local connect costs microseconds.
  - Retries reuse the already-built bytes.
  - No pre-flight or probe request in normal boots (boot.md §3.10 and §10).
- **Rank 5, footprint.**
  - Bounded buffers: 64 KiB read buffer, 64 KiB header cap, 32 MiB SSE line cap.
  - The streamed message is held once, as the decoder accumulates it; deltas are moved out to the loop.
- **Rank 7, per-turn CPU.**
  - SSE parsing is O(bytes received).
  - Each chunk is parsed once.
  - Building the request is O(context): serializing ~200 KB takes ~1 ms, against seconds of model time.
  - `log-context.md` may cache the serialized prefix; the adapter accepts a pre-serialized system string.

## 7. Invariants and security (spec §2.1)

- **Invariant 3 (blast radius).**
  - The body never sends credentials, and no code path in the provider opens a TCP socket. The only transport is `UnixStream` to `INIT_PROXY_SOCK`, and the sandbox allows only the two kernel sockets (kernel/sandbox.md).
  - A `Budget` 429 is never retried.
- **Invariant 2 (honest measuring stick).** The proxy, not the body, records traces and usage. The body's `Usage` is advisory (self-signals); the proxy's numbers are authoritative for caps and the gate.
- **Invariant 4 (lineage).** Every completed model response, aborted partials included, becomes exactly one `message` event. Failed attempts are not logged as messages; the proxy's traces keep them.
- **Untrusted input.**
  - All upstream bytes are untrusted. Every length has a cap; JSON is parsed with serde_json (recursion limit 128).
  - No `unwrap` on upstream data: decode errors become `Protocol`, never a panic.
  - A fuzz-style test feeds random byte streams to `SseReader` and the decoders (§8).
- **G2 cross-check.** The tools array sent here is exactly `init-body --manifest`'s registry rendered per api (boot.md §4.3), so the proxy-observed tool list matches `self.json`.

## 8. Test plan

**Unit, pure, no socket (G0 `cargo test`):**
1. `SseReader`:
   - CRLF, lone LF and lone CR line ends;
   - a CR at the end of one `fill_buf` chunk with the LF in the next;
   - a leading BOM;
   - comments (`: keep-alive`);
   - multi-line `data` joined with `\n`;
   - a field with no colon;
   - `data:x` without a space;
   - an event without a terminating blank line at EOF (discarded);
   - a 33 MiB line (Protocol);
   - invalid UTF-8 (Protocol).
2. `ChunkedReader`:
   - sizes in upper and lower case hex;
   - `;ext=1` chunk extensions;
   - trailers;
   - a 0-length chunk with no trailers;
   - EOF mid-chunk (`UnexpectedEof`);
   - a size line over 4 KiB;
   - every split of a recorded stream into 1-byte reads (property test).
3. `read_head`: `HTTP/1.0`, `100 Continue` then 200, folded header (Protocol), duplicate differing `content-length` (Protocol).
4. DeepSeek decoder fixtures:
   - (a) text only;
   - (b) reasoning, then content, then 2 tool calls whose argument fragments interleave by `index`;
   - (c) `id`/`name` only in the first fragment;
   - (d) `finish_reason:"length"` with a partial call (MaxTokens; call kept, not run);
   - (e) `insufficient_system_resource` (retryable `Stream`);
   - (f) usage only on the last chunk;
   - (g) keep-alive comments before the first chunk;
   - (h) an event after `[DONE]` (Protocol);
   - (i) the documented example stream from the API reference, verbatim.
5. DeepSeek `build()`:
   - golden bytes for a 6-message conversation;
   - sorted keys;
   - `reasoning_content` present on every assistant message (`""` without DeepSeek thinking);
   - the system message first;
   - the same input built twice gives identical bytes.
6. Anthropic decoder:
   - the streaming-doc examples (text, tool_use with `input_json_delta`, thinking + `signature_delta`);
   - `error` overloaded mid-stream;
   - an unknown event type ignored;
   - cumulative usage merge;
   - a `refusal` with `stop_details`.
7. Anthropic `build()`: steer merged after `tool_result` blocks; empty text skipped; thinking replayed only for the same model (epoch stripping is tested in log-context); notes rendered as `role:"system"` messages; B1/B2/B3/A markers placed as log-context §3.10.2 says.
8. OpenAI decoder:
   - the streaming-reference examples (`output_item.added` function_call, args deltas, `output_item.done`);
   - `response.incomplete` with `max_output_tokens`;
   - `response.failed`;
   - an `error` event.
9. Handoff filter: the full table in §3.10, including id sanitizing and collisions.
10. Retry policy: the table in §3.9; `retry-after` parsing; budget 429 not retried; the jitter range.
11. Fuzz: 10k random and mutated byte streams through `SseReader` + each decoder, with no panic. Runs in under 2 s; seeded with a fixed seed.

**Integration with the mock (no spend):**
12. A full turn per provider through the mock socket: the deltas arrive, `Done` matches the script, `prefix:"same"` holds across 3 turns.
13. Faults: `http 429` + `retry-after` gives one `Retry` then success; `cut` gives `Truncated` then retry; `stall` with a shrunk idle timeout gives `Timeout(Idle)` then retry; `budget` gives `Failed(Budget)` with no retry.
14. Abort mid-stream: a `stall` fault, trigger abort, `Done(partial, Aborted)` arrives in < 50 ms.

**Phase 0 live checks against DeepSeek** (owner key; total < $0.05 at off-peak prices; the owner authorizes the cap first):
- V-DS-1: one streaming tool-call round trip with the five real tools. The decoded message is well-formed, every `raw_arguments` parses, `usage` is present, and `finish_reason` is `tool_calls`.
- V-DS-2: replay a tool turn **without** `reasoning_content`; expect 400 (pins the pass-back rule).
- V-DS-3: a history whose assistant tool call has no `reasoning_content` (simulated foreign turn); record whether it is accepted. This decides the flatten fallback in §3.10.
- V-DS-4: two identical-prefix requests; the second has `prompt_cache_hit_tokens > 0`. Record the hit ratio.
- V-DS-5: record a raw stream with `: keep-alive` comments if any appear; keep it as a fixture.
- V-DS-6: record the exact 400 message for the overflow heuristic. First send `max_tokens: 393217`, one above the documented maximum, to capture the parameter-error format at no cost. Then, only if the owner approves, send one request just over the context window (~1.01M tokens). That is ≤ $0.16 at the off-peak miss price if billed; whether a rejected over-length request is billed is UNVERIFIED. Until this runs, the overflow message text is UNVERIFIED.

Scenario IDs from testing.md (roadmap.md Q2):
- L1 suite "seed `provider`" (testing.md §3.2): items 1–11, including the split fuzz at every byte offset.
- E2E-1, E2E-2 and E2E-3 (§3.10): the live DeepSeek checks V-DS-1 to V-DS-4 run inside them. E2E-2 is the reasoning pass-back check, E2E-3 the cache hit after a restart.
- F-hang (c): items 13–14 (mock `stall`).
- M-4 (§3.11): prefix bytes identical across a no-op T2 handoff (`--simulate-cache`).
- A1–A6 use the mock (`init-mockllm`, testing.md §3.4) except A1-real, A2-real and A3-real, which use DeepSeek.

## 9. Open questions

Resolved in R1/R2 and removed: Q-P1 (proxy routing, headers, cap format, idle timeout 330 s vs the body's 300 s; head-hash extraction for `mock` via `[providers.mock] preset = "deepseek"`, kernel/proxy.md §3.12.1, §4.1), Q-P2 (`boot.json.model`, supervise §4.3), Q-P3 (`stop_details`, `opaque`, epochs; log-context §3.3, §4.2), Q-P4 (testing.md harness additions and responders, §3.12) and Q-P7 (task id = the starting owner input's id, evolution.md §3.1, set by loop.md §3.3).

**Cross-doc:** none.

**Owner decisions** (not in CONVENTIONS §6; the stated default ships unless the owner objects; reconciliation/body-core.md R2 asks the main session to list them)
- **Q-P5.** *Settled 2026-10-19, keep as proposed (CONVENTIONS §6).* Anthropic server-side refusal fallbacks (`fallbacks: "default"` with beta `server-side-fallback-2026-07-01`) are **off** at seed. A silent switch to another model would mix models inside one gate measurement and forfeit the model-scoped cache. Turn on?
- **Q-P6.** *Settled 2026-10-19, keep as proposed (CONVENTIONS §6).* The DeepSeek effort default is `high` (DeepSeek's "daily agent tasks" level). `max` costs more and is meant for complex tasks. Keep `high`?

## 10. Spec deltas

1. **Interface.** Spec §4 says `stream(&Context) -> impl Iterator<Item = Event>`. Here: `stream(&dyn Provider, &Transport, HttpRequest, Cx, &Abort) -> EventStream`. Why: the provider is chosen at runtime (boot.json), so the trait must be dyn-compatible; request building and decoding are split out as pure functions so they can be tested from fixtures.
2. **Events.** The spec lists `{text, thinking, tool_call, usage, stop_reason}`. Added `Started`, `Retry`, `Failed`, and the terminal `Done(AssistantMessage)`, which carries the stop reason. Why: retries and abort must be visible to the loop and the owner's UI.
3. **Spec §4 says the provider "drops thinking blocks from other providers when switching providers".** Refined: thinking is dropped per *model* (not just per provider). Anthropic and OpenAI thinking from before a context epoch is also dropped, by log-context's projection. DeepSeek reasoning from the *same* model is never dropped, because DeepSeek returns 400 without it when tools are present. Why: verified current API rules (DeepSeek thinking-mode guide; Anthropic preserved-thinking binding).
4. **The adapter set.** The spec names Anthropic Messages and OpenAI Responses. DeepSeek Chat Completions is added as the first adapter (CONVENTIONS §5).

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.16):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP10 D1 | the normalized types, `ModelInfo`, `Effort`, `Purpose`, `Abort` and the byte-based `Request` live in `src/model.rs`; `Abort::new` takes a `kill_group: fn(i32)` hook | §3.1, §3.2, §3.11 |
| build: WP10 D2 | the adapter input is `provider::Prompt`; the loop hands the model a byte-based `Request` that `Client` sends unchanged; `Provider` gains `wire`, `classify` and `fallback`, and `decoder()` takes no request; `stream()` takes a built `HttpRequest` and a `Cx` | §3.3, §3.4, §10 |
| build: WP10 D3 | the fallbacks rewrite the body JSON, so they also work on bytes that log-context rendered; each cause is also emitted as a `Warning` | §3.4 |
| build: WP10 D4 | the DeepSeek flatten adds `"reasoning_content": ""` to flattened turns and to any assistant message that lacks the field | §3.10 |
| build: WP10 D5 | the DeepSeek decoder emits `ToolCallReady` at the `finish_reason` chunk; a chunk with an `error` object is `Stream{Other}`; `Origin.model` and `Thinking.model` are the request's model id | §3.8.1 |
| build: WP10 D6 | unsigned Anthropic thinking in an aborted partial keeps its text with `native: None`; an OpenAI refusal is `Refusal{category: None}` | §3.8.2, §3.8.3 |
| build: WP10 D7 | the socket timeouts are set best-effort (macOS EINVAL after the peer closed) | §3.6, §3.9 |
| build: WP10 D8 | `INIT_TEST_CONNECT_TIMEOUT_MS` is accepted but not applied | §4.3 |
| build: WP10 D10 | no native usage object at seed: `usage.native` is logged as null | §3.2 |
| build: WP2 D1 | a DeepSeek mock turn with no `think` renders `"Mock reasoning."`; `"think": ""` opts out | §3.12 |
| build: WP2 D2 | the budget fault sends `x-init-cap: day`, `x-init-error: budget.day` and proxy.md's body | §3.12 |
| build: WP2 D3 | the id scheme: `<n>` = assistant messages already in the request; response id `mock-<label>-<n>`; call ids per API; the response `model` echoes the request's | §3.12 |
| build: WP2 D4 | upstream-mode fallbacks: session `_none`, probe by `[init:probe]` in the latest user message, task sessions keyed by a hash of the `[task <id>]` message | §3.12 |
| build: WP2 D5, D6 | G3 detection by "contains"; task detection by a line starting `[task <id>]` with an id of `[A-Za-z0-9._-]{1,128}` | §3.12 |

## 11. References (all checked 2026-10-19)

- DeepSeek Chat Completions API reference (request/response/chunk schemas, finish_reason enum, usage fields, max_tokens bounds): https://api-docs.deepseek.com/api/create-chat-completion
- DeepSeek Thinking Mode guide (toggle, effort mapping, reasoning_content pass-back rule, 400 on missing): https://api-docs.deepseek.com/guides/thinking_mode
- DeepSeek Tool Calls guide (tools format, strict beta, "does not support inserting tool calls mid-conversation"): https://api-docs.deepseek.com/guides/tool_calls
- DeepSeek Models & Pricing (deepseek-flash = DeepSeek-V4.1-Flash-1017, deepseek-v4-pro = DeepSeek-V4-Pro-0813, 1M context, 384K max output, prices): https://api-docs.deepseek.com/quick_start/pricing/
- DeepSeek Rate Limit (keep-alive comments, 10-minute close, concurrency 429): https://api-docs.deepseek.com/quick_start/rate_limit
- DeepSeek Error Codes: https://api-docs.deepseek.com/quick_start/error_codes
- DeepSeek Context Caching (cache prefix units, prompt_cache_hit/miss_tokens): https://api-docs.deepseek.com/guides/kv_cache
- DeepSeek Change Log (V4.1-Flash-1017 update 2026-10-17; V4-Pro GA 2026-08-13): https://api-docs.deepseek.com/updates
- Anthropic streaming messages (event flow, delta types, ping, error events, cumulative usage): https://platform.claude.com/docs/en/build-with-claude/streaming
- Anthropic stop reasons: https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons
- Anthropic API errors (status codes, error shape, 529, spend-cap 429): https://platform.claude.com/docs/en/api/errors
- Claude API skill bundle 2.1.295 (model table cached 2026-10-06, adaptive thinking, effort, preserved-thinking binding, caching rules), local file `claude-api/SKILL.md` and `shared/model-migration.md`, `shared/prompt-caching.md`
- OpenAI Responses streaming events reference: https://developers.openai.com/api/reference/resources/responses/streaming-events
- OpenAI function calling guide (function definition, function_call / function_call_output, strict, streaming): https://developers.openai.com/api/docs/guides/function-calling
- OpenAI reasoning guide (stateless mode, encrypted_content default): https://developers.openai.com/api/docs/guides/reasoning
- OpenAI prompt caching guide (GPT-5.6+ implicit/explicit breakpoints, developer-message breakpoint, prompt_cache_key): https://developers.openai.com/api/docs/guides/prompt-caching
- OpenAI error codes (429 variants, 503 server_is_overloaded, Retry-After): https://developers.openai.com/api/docs/guides/error-codes
- OpenAI GPT-6 Astra / GPT-6.1 Sol model pages: https://developers.openai.com/api/docs/models/gpt-6-astra , https://developers.openai.com/api/docs/models/gpt-6.1-sol
- WHATWG HTML, Server-sent events, "Interpreting an event stream": https://html.spec.whatwg.org/multipage/server-sent-events.html
- RFC 9112, HTTP/1.1 (§6.3 message body length, §7.1 chunked coding): https://www.rfc-editor.org/rfc/rfc9112
- Rust std `UnixStream` (connect/try_clone/shutdown/set_read_timeout since 1.10.0; read-timeout error kind note): https://doc.rust-lang.org/stable/std/os/unix/net/struct.UnixStream.html
- Rust std `Condvar::wait_timeout`: https://doc.rust-lang.org/stable/std/sync/struct.Condvar.html
- serde_json 1.0.154 (crates.io, updated 2026-10-11): https://crates.io/crates/serde_json
- pi-ai README (cross-provider handoff rule): https://github.com/earendil-works/pi/blob/main/packages/ai/README.md (via research_notes/minimal_harnesses.md)
- Local verification on macOS 14.6 (Darwin 23.6.0), rustc 1.97.1, 2026-10-19: socket shutdown unblocks read (Ok(0) in 200 ms); SUN_LEN path limit error.
