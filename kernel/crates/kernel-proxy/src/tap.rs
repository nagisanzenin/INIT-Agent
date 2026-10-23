//! Response tap: per-family incremental parsing of the forwarded response bytes
//! (kernel/proxy.md §3.10.1 pre-filter and line splitter, §3.10.2 normalization, §3.12 finish).
//!
//! Streaming bodies (SSE) are split into lines with a carry-over buffer bounded by
//! `max_line_bytes`; a longer line stops parsing for the rest of the response and the result is
//! flagged `usage_missing`. A `data:` line is JSON-decoded only when it can matter (the cheap
//! pre-filter): every line until the first content token, then only lines that can carry usage, a
//! finish or stop reason, an error, or a tool-call start or delta. Non-streaming bodies are
//! accumulated up to `BODY_MAX` and parsed once at end. Nothing here panics on any input.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Instant;

/// `ToolCall::args` is cut at this many bytes, on a char boundary (proxy.md §3.10.2).
pub const TOOL_ARGS_MAX: usize = 4096;
/// A non-streaming body larger than this is not parsed (`usage_missing`); it is still forwarded.
pub const BODY_MAX: usize = 16 << 20;
/// Full argument text kept per `ext` call to find `ext_name`; past it `ext_name` stays unknown.
const EXT_ARGS_MAX: usize = 1 << 20;
/// Tool calls recorded per response; later ones are dropped.
const TOOL_CALLS_MAX: usize = 1024;
/// Bound on recorded provider strings (ids, names, finish reasons, modifier values).
const STR_MAX: usize = 256;
/// The fixed extension tool (ADR 004): arguments `{"name": <extension>, "input": {…}}`.
const EXT_TOOL: &str = "ext";

/// Wire family of a route (proxy.md §3.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Family {
    OpenAiChat,
    Anthropic,
    OpenAiResponses,
    None,
}

impl Family {
    pub fn as_str(self) -> &'static str {
        match self {
            Family::OpenAiChat => "openai-chat",
            Family::Anthropic => "anthropic",
            Family::OpenAiResponses => "openai-responses",
            Family::None => "none",
        }
    }
    pub fn parse(s: &str) -> Option<Family> {
        match s {
            "openai-chat" => Some(Family::OpenAiChat),
            "anthropic" => Some(Family::Anthropic),
            "openai-responses" => Some(Family::OpenAiResponses),
            "none" => Some(Family::None),
            _ => None,
        }
    }
}

/// Normalized usage; all counts are tokens (proxy.md §3.10.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_uncached: u64,
    pub input_cache_read: u64,
    pub input_cache_write: u64,
    pub input_cache_write_1h: u64,
    pub output: u64,
    pub reasoning: u64,
}

impl Usage {
    pub fn input_total(&self) -> u64 {
        self.input_uncached
            + self.input_cache_read
            + self.input_cache_write
            + self.input_cache_write_1h
    }
    /// floor(read × 1000 / total), integer only (ICJ); `None` when there is no input.
    pub fn cache_permille(&self) -> Option<u32> {
        let t = self.input_total();
        (t > 0).then(|| (self.input_cache_read * 1000 / t) as u32)
    }
}

/// One model-issued tool call (proxy.md §3.10.2 "Tool-call records").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Concatenated argument text, cut at 4 KiB on a char boundary.
    pub args: String,
    /// For a call to the `ext` tool: `args.name` when the full args parse as JSON with a string `name`.
    pub ext_name: Option<String>,
}

/// Normalized finish reason (proxy.md §3.12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    ToolUse,
    Stop,
    Length,
    Error,
    Other,
}

impl Finish {
    pub fn as_str(self) -> &'static str {
        match self {
            Finish::ToolUse => "tool_use",
            Finish::Stop => "stop",
            Finish::Length => "length",
            Finish::Error => "error",
            Finish::Other => "other",
        }
    }
}

/// What the tap learned from one response.
#[derive(Clone, Debug, Default)]
pub struct TapResult {
    pub usage: Option<Usage>,
    /// `usage_missing`, `usage_partial` (proxy.md §3.10.1–§3.10.2).
    pub flags: Vec<&'static str>,
    pub finish: Option<Finish>,
    pub finish_raw: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub first_token_at: Option<Instant>,
    pub first_content_at: Option<Instant>,
    pub last_token_at: Option<Instant>,
    /// Response body bytes seen (the no-usage estimate's `content_bytes_seen`).
    pub body_bytes: u64,
    /// A mid-stream error was seen (Anthropic `event: error`, Responses `response.failed`,
    /// DeepSeek `finish_reason: insufficient_system_resource`).
    pub stream_error: bool,
    /// Pricing modifiers reported by the response: `service_tier:<v>`, `speed:<v>`, `inference_geo:<v>`.
    pub modifiers: Vec<String>,
    /// The served model string, exactly as the response names it (the first non-empty `model`,
    /// `message.model` or `response.model`; ≤ 128 bytes). The pool cache keys on it
    /// (m7-design.md §3.2.7; whether DeepSeek's value is stable is UNVERIFIED).
    pub model_served: Option<String>,
}

/// Incremental parser over the forwarded bytes. Never blocks, never fails: anything it cannot
/// parse only costs it information.
pub struct Tap {
    family: Family,
    result: TapResult,
    streaming: bool,
    max_line_bytes: usize,
    /// Streaming: bytes after the last LF (at most `max_line_bytes`).
    carry: Vec<u8>,
    /// Non-streaming: the body so far (at most `BODY_MAX`).
    body: Vec<u8>,
    /// Name of the current SSE event (`event:` line), cleared at a blank line.
    event: Vec<u8>,
    /// Parsing stopped for good: a line or the body went over its bound.
    stopped: bool,
    ended: bool,
    /// Per-call accumulator state, parallel to `result.tool_calls`.
    calls: Vec<CallAcc>,
    /// openai-chat `tool_calls[].index` → position in `result.tool_calls`.
    chat_index: BTreeMap<u64, usize>,
    /// anthropic content block index → position.
    anth_block: BTreeMap<u64, usize>,
    /// openai-responses item id → position.
    resp_item: BTreeMap<String, usize>,
    anth: AnthUsage,
}

/// What one parsed event showed about generation.
#[derive(Clone, Copy, Default)]
struct Tok {
    /// Any generated token, reasoning included.
    token: bool,
    /// Non-reasoning content or tool-call token (implies `token`).
    content: bool,
}

#[derive(Default)]
struct CallAcc {
    /// `args` reached `TOOL_ARGS_MAX`; later fragments are dropped.
    cut: bool,
    /// Full argument text of an `ext` call; `None` for other tools or past `EXT_ARGS_MAX`.
    ext_full: Option<String>,
    /// Args were replaced by a provider `.done`/final record; later fragments are ignored.
    done: bool,
    /// Anthropic `content_block_start.input` when non-empty; used only if no `input_json_delta` follows.
    start_input: Option<String>,
}

/// Anthropic usage as the stream reports it: every field overwritten when present, never summed.
#[derive(Default)]
struct AnthUsage {
    seen: bool,
    input: Option<u64>,
    cache_read: Option<u64>,
    cache_creation: Option<u64>,
    eph_5m: Option<u64>,
    eph_1h: Option<u64>,
    output: Option<u64>,
    thinking: Option<u64>,
    speed: Option<String>,
    geo: Option<String>,
    tier: Option<String>,
}

impl Tap {
    /// `streaming`: the response is SSE (`text/event-stream`); otherwise one JSON body parsed at end.
    pub fn new(family: Family, streaming: bool, max_line_bytes: usize) -> Tap {
        Tap {
            family,
            result: TapResult::default(),
            streaming,
            max_line_bytes,
            carry: Vec::new(),
            body: Vec::new(),
            event: Vec::new(),
            stopped: false,
            ended: false,
            calls: Vec::new(),
            chat_index: BTreeMap::new(),
            anth_block: BTreeMap::new(),
            resp_item: BTreeMap::new(),
            anth: AnthUsage::default(),
        }
    }
    pub fn family(&self) -> Family {
        self.family
    }
    /// Feed one forwarded frame.
    pub fn on_bytes(&mut self, data: &[u8], now: Instant) {
        self.result.body_bytes = self.result.body_bytes.saturating_add(data.len() as u64);
        if self.ended || self.stopped || self.family == Family::None {
            return;
        }
        if self.streaming {
            self.split_lines(data, now);
        } else {
            self.collect_body(data, now);
        }
    }
    /// End of the body (normal end). Flushes a final unterminated line and parses a non-streaming body.
    pub fn on_end(&mut self, now: Instant) {
        if self.ended {
            return;
        }
        if self.family != Family::None && !self.stopped {
            if self.streaming {
                if !self.carry.is_empty() {
                    let line = std::mem::take(&mut self.carry);
                    self.on_line(strip_cr(&line), now);
                }
            } else {
                let body = std::mem::take(&mut self.body);
                self.parse_body(&body);
            }
        }
        self.ended = true;
        if !self.streaming && self.result.first_token_at.is_some() {
            self.result.last_token_at = Some(now);
        }
        self.finish_calls();
        if self.family == Family::None {
            return;
        }
        if self.stopped {
            self.result.usage = None;
        }
        if self.result.usage.is_none() {
            self.flag("usage_missing");
        } else if self.family == Family::Anthropic && self.anth.input.is_none() {
            self.flag("usage_partial");
        }
    }
    pub fn result(&self) -> &TapResult {
        &self.result
    }

    // ------------------------------------------------------------------ framing

    fn split_lines(&mut self, data: &[u8], now: Instant) {
        let mut rest = data;
        while let Some(i) = rest.iter().position(|&b| b == b'\n') {
            let (seg, tail) = rest.split_at(i);
            rest = tail.get(1..).unwrap_or_default();
            // A line's length excludes its LF and includes a CR, so the bound does not depend on
            // where the frames were split.
            if self.carry.len().saturating_add(seg.len()) > self.max_line_bytes {
                self.stop();
                return;
            }
            if self.carry.is_empty() {
                self.on_line(strip_cr(seg), now);
            } else {
                let mut line = std::mem::take(&mut self.carry);
                line.extend_from_slice(seg);
                self.on_line(strip_cr(&line), now);
                line.clear();
                self.carry = line;
            }
        }
        if self.carry.len().saturating_add(rest.len()) > self.max_line_bytes {
            self.stop();
            return;
        }
        self.carry.extend_from_slice(rest);
    }

    fn collect_body(&mut self, data: &[u8], now: Instant) {
        // Non-streaming: the first non-whitespace byte stands for the first token and content
        // (DeepSeek sends blank keep-alive lines before a non-streaming body).
        if self.result.first_token_at.is_none() && data.iter().any(|b| !b.is_ascii_whitespace()) {
            self.result.first_token_at = Some(now);
            self.result.first_content_at = Some(now);
        }
        if self.body.len().saturating_add(data.len()) > BODY_MAX {
            self.stop();
            return;
        }
        self.body.extend_from_slice(data);
    }

    /// Stop parsing for the rest of the response: whatever usage was seen is incomplete.
    fn stop(&mut self) {
        self.stopped = true;
        self.carry = Vec::new();
        self.body = Vec::new();
        self.result.usage = None;
    }

    fn on_line(&mut self, line: &[u8], now: Instant) {
        if line.is_empty() {
            self.event.clear();
            return;
        }
        let (field, value) = match line.iter().position(|&b| b == b':') {
            Some(0) => return, // SSE comment, e.g. `: keep-alive`
            Some(i) => {
                let (f, rest) = line.split_at(i);
                let v = rest.get(1..).unwrap_or_default();
                (f, v.strip_prefix(b" ").unwrap_or(v))
            }
            None => (line, &b""[..]),
        };
        match field {
            b"data" => self.on_data(value, now),
            b"event" => {
                self.event.clear();
                let name = value.trim_ascii();
                self.event.extend_from_slice(name.get(..64).unwrap_or(name));
            }
            _ => {}
        }
    }

    fn on_data(&mut self, data: &[u8], now: Instant) {
        let d = data.trim_ascii();
        if d.is_empty() || d == b"[DONE]" {
            return;
        }
        let event_is_error = self.event.as_slice() == b"error";
        let parse = event_is_error
            || self.result.first_token_at.is_none()
            || wanted(self.family, d)
            || (self.result.first_content_at.is_none() && content_marker(self.family, d));
        if !parse {
            // After the first token, an unparsed line with a delta is taken to be a token.
            if contains(d, b"delta") {
                self.result.last_token_at = Some(now);
            }
            return;
        }
        let t = match serde_json::from_slice::<Value>(d) {
            Ok(v) => {
                self.served_model(&v);
                match self.family {
                    Family::OpenAiChat => self.chat(&v),
                    Family::Anthropic => self.anthropic(&v),
                    Family::OpenAiResponses => self.responses(&v),
                    Family::None => Tok::default(),
                }
            }
            Err(_) => Tok::default(),
        };
        if event_is_error && !self.result.stream_error {
            self.set_finish("error", Finish::Error);
        }
        if t.token || t.content {
            if self.result.first_token_at.is_none() {
                self.result.first_token_at = Some(now);
            }
            self.result.last_token_at = Some(now);
        }
        if t.content && self.result.first_content_at.is_none() {
            self.result.first_content_at = Some(now);
        }
    }

    fn parse_body(&mut self, body: &[u8]) {
        let Ok(v) = serde_json::from_slice::<Value>(body) else {
            return;
        };
        self.served_model(&v);
        match self.family {
            Family::OpenAiChat => {
                self.chat(&v);
            }
            Family::Anthropic => {
                self.anthropic(&v);
            }
            Family::OpenAiResponses => match str_at(&v, &["status"]) {
                Some(status) => self.resp_final(&v, status),
                None => {
                    if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
                        let raw = err_code(e).unwrap_or("error");
                        self.set_finish(raw, Finish::Error);
                    }
                }
            },
            Family::None => {}
        }
    }

    /// Keep the first served model string the response names (GATE-V2b, m7-design.md §3.2.7).
    fn served_model(&mut self, v: &Value) {
        if self.result.model_served.is_some() {
            return;
        }
        for path in [&["model"][..], &["message", "model"], &["response", "model"]] {
            if let Some(m) = str_at(v, path).filter(|m| !m.is_empty()) {
                self.result.model_served = Some(capped(m, 128));
                return;
            }
        }
    }

    // ------------------------------------------------------------------ openai-chat

    fn chat(&mut self, v: &Value) -> Tok {
        let mut t = Tok::default();
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            let raw = err_code(e).unwrap_or("error");
            self.set_finish(raw, Finish::Error);
        }
        // `"usage": null` (DeepSeek with include_usage) is ignored; the last object wins.
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            self.result.usage = Some(chat_usage(u));
        }
        let choice = v.get("choices").and_then(Value::as_array).and_then(|cs| {
            cs.iter()
                .find(|c| c.get("index").and_then(Value::as_u64).unwrap_or(0) == 0)
        });
        let Some(c) = choice else {
            return t;
        };
        if let Some(d) = c.get("delta") {
            if nonempty_str(d.get("content")) {
                t.content = true;
            }
            if nonempty_str(d.get("reasoning_content")) {
                t.token = true;
            }
            if let Some(tcs) = d.get("tool_calls").and_then(Value::as_array)
                && !tcs.is_empty()
            {
                t.content = true;
                for (i, tc) in tcs.iter().enumerate() {
                    self.chat_tool(tc, i);
                }
            }
        }
        if let Some(tcs) = c
            .get("message")
            .and_then(|m| m.get("tool_calls"))
            .and_then(Value::as_array)
        {
            for (i, tc) in tcs.iter().enumerate() {
                self.chat_tool(tc, i);
            }
        }
        if let Some(fr) = c.get("finish_reason").and_then(Value::as_str) {
            let f = match fr {
                "stop" => Finish::Stop,
                "length" => Finish::Length,
                "tool_calls" | "function_call" => Finish::ToolUse,
                "insufficient_system_resource" => Finish::Error,
                _ => Finish::Other,
            };
            self.set_finish(fr, f);
        }
        t
    }

    fn chat_tool(&mut self, tc: &Value, i: usize) {
        let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(i as u64);
        let id = str_at(tc, &["id"]).unwrap_or("");
        let name = str_at(tc, &["function", "name"]).unwrap_or("");
        let pos = match self.chat_index.get(&idx) {
            Some(&p) => {
                self.fill(p, id, name);
                p
            }
            None => {
                let Some(p) = self.new_call(id, name) else {
                    return;
                };
                self.chat_index.insert(idx, p);
                p
            }
        };
        match get(tc, &["function", "arguments"]) {
            Some(Value::String(s)) => self.append_args(pos, s),
            Some(a @ Value::Object(_)) => self.append_args(pos, &a.to_string()),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ anthropic

    fn anthropic(&mut self, v: &Value) -> Tok {
        let mut t = Tok::default();
        match str_at(v, &["type"]).unwrap_or("") {
            "message_start" => {
                if let Some(u) = get(v, &["message", "usage"]) {
                    self.anth_usage(u);
                }
            }
            "message_delta" => {
                if let Some(u) = v.get("usage") {
                    self.anth_usage(u);
                }
                if let Some(sr) = str_at(v, &["delta", "stop_reason"]) {
                    self.set_finish(sr, anth_finish(sr));
                }
            }
            "content_block_start" => {
                if str_at(v, &["content_block", "type"]) == Some("tool_use") {
                    let id = str_at(v, &["content_block", "id"]).unwrap_or("");
                    let name = str_at(v, &["content_block", "name"]).unwrap_or("");
                    if let Some(p) = self.new_call(id, name) {
                        if let Some(idx) = v.get("index").and_then(Value::as_u64) {
                            self.anth_block.insert(idx, p);
                        }
                        let input = get(v, &["content_block", "input"])
                            .filter(|i| i.as_object().is_some_and(|o| !o.is_empty()));
                        if let (Some(input), Some(acc)) = (input, self.calls.get_mut(p)) {
                            acc.start_input = Some(input.to_string());
                        }
                    }
                }
            }
            "content_block_delta" => {
                t.token = true;
                match str_at(v, &["delta", "type"]) {
                    Some("text_delta") => t.content = true,
                    Some("input_json_delta") => {
                        t.content = true;
                        let idx = v.get("index").and_then(Value::as_u64);
                        let pj = str_at(v, &["delta", "partial_json"]);
                        if let (Some(idx), Some(pj)) = (idx, pj)
                            && let Some(&p) = self.anth_block.get(&idx)
                        {
                            self.append_args(p, pj);
                        }
                    }
                    _ => {}
                }
            }
            "error" => {
                let raw = str_at(v, &["error", "type"]).unwrap_or("error");
                self.set_finish(raw, Finish::Error);
            }
            "message" => self.anth_message(v), // non-streaming body
            _ => {}                            // ping, content_block_stop, message_stop, unknown
        }
        t
    }

    fn anth_message(&mut self, v: &Value) {
        if let Some(blocks) = v.get("content").and_then(Value::as_array) {
            for b in blocks {
                if str_at(b, &["type"]) != Some("tool_use") {
                    continue;
                }
                let id = str_at(b, &["id"]).unwrap_or("");
                let name = str_at(b, &["name"]).unwrap_or("");
                if let Some(p) = self.new_call(id, name) {
                    let args = b.get("input").map(Value::to_string).unwrap_or_default();
                    self.set_args(p, &args);
                }
            }
        }
        if let Some(u) = v.get("usage") {
            self.anth_usage(u);
        }
        if let Some(sr) = str_at(v, &["stop_reason"]) {
            self.set_finish(sr, anth_finish(sr));
        }
    }

    fn anth_usage(&mut self, u: &Value) {
        self.anth.merge(u);
        if self.stopped {
            return;
        }
        self.result.usage = self.anth.usage();
        self.result.modifiers = self.anth.modifiers();
    }

    // ------------------------------------------------------------------ openai-responses

    fn responses(&mut self, v: &Value) -> Tok {
        let mut t = Tok::default();
        let ty = str_at(v, &["type"]).unwrap_or("");
        match ty {
            "response.output_text.delta" => t.content = true,
            "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                t.token = true
            }
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
                t.content = true;
                if let (Some(item), Some(d)) = (str_at(v, &["item_id"]), str_at(v, &["delta"]))
                    && let Some(&p) = self.resp_item.get(item)
                {
                    self.append_args(p, d);
                }
            }
            "response.function_call_arguments.done" | "response.custom_tool_call_input.done" => {
                let key = if ty == "response.function_call_arguments.done" {
                    "arguments"
                } else {
                    "input"
                };
                if let (Some(item), Some(full)) = (str_at(v, &["item_id"]), str_at(v, &[key]))
                    && let Some(&p) = self.resp_item.get(item)
                {
                    self.set_args(p, full);
                }
            }
            "response.output_item.added" | "response.output_item.done" => {
                if let Some(item) = v.get("item") {
                    self.resp_tool_item(item, ty == "response.output_item.done");
                }
            }
            "response.completed" | "response.incomplete" | "response.failed" => {
                let kind = ty.trim_start_matches("response.");
                match v.get("response") {
                    Some(r) => self.resp_final(r, kind),
                    None => self.resp_final(&Value::Null, kind),
                }
            }
            "error" => {
                let raw = err_code(v)
                    .or_else(|| v.get("error").and_then(err_code))
                    .unwrap_or("error");
                self.set_finish(raw, Finish::Error);
            }
            _ => {}
        }
        t
    }

    /// A `function_call` / `custom_tool_call` output item. `done`: its arguments are final.
    fn resp_tool_item(&mut self, item: &Value, done: bool) {
        let args_key = match str_at(item, &["type"]) {
            Some("function_call") => "arguments",
            Some("custom_tool_call") => "input",
            _ => return,
        };
        let call_id = str_at(item, &["call_id"]).unwrap_or("");
        let name = str_at(item, &["name"]).unwrap_or("");
        let key = str_at(item, &["id"])
            .filter(|s| !s.is_empty())
            .unwrap_or(call_id);
        let pos = match self.resp_item.get(key) {
            Some(&p) => {
                self.fill(p, call_id, name);
                p
            }
            None => {
                let Some(p) = self.new_call(call_id, name) else {
                    return;
                };
                self.resp_item.insert(capped(key, STR_MAX), p);
                p
            }
        };
        if let Some(a) = str_at(item, &[args_key]) {
            if done {
                self.set_args(pos, a);
            } else if !a.is_empty() {
                self.append_args(pos, a);
            }
        }
    }

    /// The full response object of a terminal event (or a non-streaming body); `kind` is
    /// `completed`, `incomplete` or `failed` (anything else: no finish).
    fn resp_final(&mut self, r: &Value, kind: &str) {
        let mut has_tool = false;
        if let Some(items) = r.get("output").and_then(Value::as_array) {
            for it in items {
                if matches!(
                    str_at(it, &["type"]),
                    Some("function_call" | "custom_tool_call")
                ) {
                    has_tool = true;
                    self.resp_tool_item(it, true);
                }
            }
        }
        if !self.stopped {
            self.result.usage = r.get("usage").filter(|u| u.is_object()).map(resp_usage);
        }
        if let Some(tier) = str_at(r, &["service_tier"]) {
            self.result
                .modifiers
                .retain(|m| !m.starts_with("service_tier:"));
            self.result
                .modifiers
                .push(format!("service_tier:{}", capped(tier, STR_MAX)));
        }
        match kind {
            "failed" => self.set_finish("failed", Finish::Error),
            "incomplete" => match str_at(r, &["incomplete_details", "reason"]) {
                Some(reason @ "max_output_tokens") => self.set_finish(reason, Finish::Length),
                Some(reason) => self.set_finish(reason, Finish::Other),
                None => self.set_finish("incomplete", Finish::Other),
            },
            "completed" => {
                let f = if has_tool || !self.result.tool_calls.is_empty() {
                    Finish::ToolUse
                } else {
                    Finish::Stop
                };
                self.set_finish("completed", f);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ tool calls and finish

    fn new_call(&mut self, id: &str, name: &str) -> Option<usize> {
        if self.result.tool_calls.len() >= TOOL_CALLS_MAX {
            return None;
        }
        self.result.tool_calls.push(ToolCall {
            id: capped(id, STR_MAX),
            name: capped(name, STR_MAX),
            args: String::new(),
            ext_name: None,
        });
        self.calls.push(CallAcc {
            ext_full: (name == EXT_TOOL).then(String::new),
            ..CallAcc::default()
        });
        Some(self.result.tool_calls.len() - 1)
    }

    /// Later chunks may restate id or name; the first non-empty value is kept.
    fn fill(&mut self, pos: usize, id: &str, name: &str) {
        let (Some(tc), Some(acc)) = (self.result.tool_calls.get_mut(pos), self.calls.get_mut(pos))
        else {
            return;
        };
        if tc.id.is_empty() && !id.is_empty() {
            tc.id = capped(id, STR_MAX);
        }
        if tc.name.is_empty() && !name.is_empty() {
            tc.name = capped(name, STR_MAX);
            if name == EXT_TOOL && !acc.cut {
                acc.ext_full = Some(tc.args.clone());
            }
        }
    }

    fn append_args(&mut self, pos: usize, frag: &str) {
        let (Some(tc), Some(acc)) = (self.result.tool_calls.get_mut(pos), self.calls.get_mut(pos))
        else {
            return;
        };
        if acc.done {
            return;
        }
        acc.start_input = None;
        if !acc.cut {
            acc.cut = !push_capped(&mut tc.args, frag, TOOL_ARGS_MAX);
        }
        if let Some(full) = acc.ext_full.as_mut()
            && !push_capped(full, frag, EXT_ARGS_MAX)
        {
            acc.ext_full = None;
        }
    }

    /// Full arguments from a provider's final record; preferred over the fragments.
    fn set_args(&mut self, pos: usize, full: &str) {
        let (Some(tc), Some(acc)) = (self.result.tool_calls.get_mut(pos), self.calls.get_mut(pos))
        else {
            return;
        };
        tc.args.clear();
        acc.cut = !push_capped(&mut tc.args, full, TOOL_ARGS_MAX);
        if tc.name == EXT_TOOL {
            acc.ext_full = (full.len() <= EXT_ARGS_MAX).then(|| full.to_owned());
        }
        acc.done = true;
        acc.start_input = None;
    }

    /// Settle pending Anthropic start inputs and compute `ext_name`s (at finish and at end).
    fn finish_calls(&mut self) {
        for pos in 0..self.calls.len() {
            if let Some(s) = self.calls.get_mut(pos).and_then(|a| a.start_input.take()) {
                self.set_args(pos, &s);
            }
        }
        for (tc, acc) in self.result.tool_calls.iter_mut().zip(&self.calls) {
            if tc.name == EXT_TOOL {
                tc.ext_name = acc.ext_full.as_deref().and_then(ext_name_of);
            }
        }
    }

    /// An error finish is sticky: a later finish reason does not replace it.
    fn set_finish(&mut self, raw: &str, f: Finish) {
        if self.result.stream_error && f != Finish::Error {
            return;
        }
        self.result.finish = Some(f);
        self.result.finish_raw = Some(capped(raw, STR_MAX));
        if f == Finish::Error {
            self.result.stream_error = true;
        }
        self.finish_calls();
    }

    fn flag(&mut self, f: &'static str) {
        if !self.result.flags.contains(&f) {
            self.result.flags.push(f);
        }
    }
}

impl AnthUsage {
    fn merge(&mut self, u: &Value) {
        if !u.is_object() {
            return;
        }
        self.seen = true;
        set_u(&mut self.input, u.get("input_tokens"));
        set_u(&mut self.cache_read, u.get("cache_read_input_tokens"));
        set_u(
            &mut self.cache_creation,
            u.get("cache_creation_input_tokens"),
        );
        set_u(
            &mut self.eph_5m,
            get(u, &["cache_creation", "ephemeral_5m_input_tokens"]),
        );
        set_u(
            &mut self.eph_1h,
            get(u, &["cache_creation", "ephemeral_1h_input_tokens"]),
        );
        set_u(&mut self.output, u.get("output_tokens"));
        set_u(
            &mut self.thinking,
            get(u, &["output_tokens_details", "thinking_tokens"]),
        );
        set_s(&mut self.speed, u.get("speed"));
        set_s(&mut self.geo, u.get("inference_geo"));
        set_s(&mut self.tier, u.get("service_tier"));
    }
    fn usage(&self) -> Option<Usage> {
        if !self.seen {
            return None;
        }
        // The per-TTL split wins when reported; otherwise every cache write counts as 5m.
        let (w5, w1) = if self.eph_5m.is_some() || self.eph_1h.is_some() {
            (self.eph_5m.unwrap_or(0), self.eph_1h.unwrap_or(0))
        } else {
            (self.cache_creation.unwrap_or(0), 0)
        };
        Some(Usage {
            input_uncached: self.input.unwrap_or(0),
            input_cache_read: self.cache_read.unwrap_or(0),
            input_cache_write: w5,
            input_cache_write_1h: w1,
            output: self.output.unwrap_or(0),
            reasoning: self.thinking.unwrap_or(0),
        })
    }
    fn modifiers(&self) -> Vec<String> {
        [
            ("speed", &self.speed),
            ("inference_geo", &self.geo),
            ("service_tier", &self.tier),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}:{v}")))
        .collect()
    }
}

// ---------------------------------------------------------------------- normalization

/// openai-chat usage: DeepSeek when `prompt_cache_hit_tokens` is present, else OpenAI.
fn chat_usage(u: &Value) -> Usage {
    let output = u64_at(u, &["completion_tokens"]).unwrap_or(0);
    let reasoning = u64_at(u, &["completion_tokens_details", "reasoning_tokens"]).unwrap_or(0);
    match u64_at(u, &["prompt_cache_hit_tokens"]) {
        Some(hit) => {
            let miss = u64_at(u, &["prompt_cache_miss_tokens"])
                .or_else(|| u64_at(u, &["prompt_tokens"]).map(|p| p.saturating_sub(hit)))
                .unwrap_or(0);
            Usage {
                input_uncached: miss,
                input_cache_read: hit,
                output,
                reasoning,
                ..Usage::default()
            }
        }
        None => {
            let prompt = u64_at(u, &["prompt_tokens"]).unwrap_or(0);
            let cached = u64_at(u, &["prompt_tokens_details", "cached_tokens"]).unwrap_or(0);
            Usage {
                input_uncached: prompt.saturating_sub(cached),
                input_cache_read: cached,
                output,
                reasoning,
                ..Usage::default()
            }
        }
    }
}

/// openai-responses usage: cached and cache-write tokens are both subsets of `input_tokens`.
fn resp_usage(u: &Value) -> Usage {
    let input = u64_at(u, &["input_tokens"]).unwrap_or(0);
    let cached = u64_at(u, &["input_tokens_details", "cached_tokens"]).unwrap_or(0);
    let write = u64_at(u, &["input_tokens_details", "cache_write_tokens"]).unwrap_or(0);
    Usage {
        input_uncached: input.saturating_sub(cached).saturating_sub(write),
        input_cache_read: cached,
        input_cache_write: write,
        input_cache_write_1h: 0,
        output: u64_at(u, &["output_tokens"]).unwrap_or(0),
        reasoning: u64_at(u, &["output_tokens_details", "reasoning_tokens"]).unwrap_or(0),
    }
}

fn anth_finish(stop_reason: &str) -> Finish {
    match stop_reason {
        "tool_use" => Finish::ToolUse,
        "end_turn" | "stop_sequence" => Finish::Stop,
        "max_tokens" => Finish::Length,
        _ => Finish::Other,
    }
}

/// `args.name` of an `ext` call when the full args are a JSON object with a string `name`.
fn ext_name_of(args: &str) -> Option<String> {
    let v: Value = serde_json::from_str(args).ok()?;
    v.as_object()?
        .get("name")?
        .as_str()
        .map(|s| capped(s, STR_MAX))
}

// ---------------------------------------------------------------------- pre-filter

/// After the first content token, a data line is decoded only if this matches: a usage object,
/// a non-null finish/stop reason, an error, or a tool-call start or delta (proxy.md §3.10.1).
fn wanted(family: Family, d: &[u8]) -> bool {
    key_then(d, b"\"usage\"", b"{")
        || key_then(d, b"\"type\"", b"\"error\"")
        || match family {
            Family::OpenAiChat => {
                key_then(d, b"\"finish_reason\"", b"\"")
                    || contains(d, b"\"tool_calls\"")
                    || key_then(d, b"\"error\"", b"")
            }
            Family::Anthropic => {
                key_then(d, b"\"stop_reason\"", b"\"")
                    || contains(d, b"\"tool_use\"")
                    || contains(d, b"input_json_delta")
            }
            Family::OpenAiResponses => {
                contains(d, b"function_call")
                    || contains(d, b"custom_tool_call")
                    || contains(d, b"response.completed")
                    || contains(d, b"response.incomplete")
                    || contains(d, b"response.failed")
            }
            Family::None => false,
        }
}

/// Between the first token (e.g. reasoning) and the first content token, lines that may carry
/// content are decoded too, so that `first_content_at` is exact.
fn content_marker(family: Family, d: &[u8]) -> bool {
    match family {
        Family::OpenAiChat => key_then(d, b"\"content\"", b"\""),
        Family::Anthropic => contains(d, b"text_delta") || contains(d, b"input_json_delta"),
        Family::OpenAiResponses => contains(d, b"output_text.delta"),
        Family::None => false,
    }
}

/// `key` (quotes included), optional whitespace, `:`, optional whitespace, then `next`.
/// In JSON a quoted key followed by `:` cannot occur inside a string value, so this only
/// matches real keys (at any depth).
fn key_then(h: &[u8], key: &[u8], next: &[u8]) -> bool {
    let mut from = 0;
    while let Some(i) = find_from(h, key, from) {
        let mut j = i + key.len();
        while h.get(j).is_some_and(|b| b.is_ascii_whitespace()) {
            j += 1;
        }
        if h.get(j) == Some(&b':') {
            j += 1;
            while h.get(j).is_some_and(|b| b.is_ascii_whitespace()) {
                j += 1;
            }
            if h.get(j..).is_some_and(|r| r.starts_with(next)) {
                return true;
            }
        }
        from = i + 1;
    }
    false
}

fn contains(h: &[u8], n: &[u8]) -> bool {
    find_from(h, n, 0).is_some()
}

fn find_from(h: &[u8], n: &[u8], from: usize) -> Option<usize> {
    let first = *n.first()?;
    let mut i = from;
    loop {
        i += h.get(i..)?.iter().position(|&b| b == first)?;
        if h.get(i..i.checked_add(n.len())?)? == n {
            return Some(i);
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------- small helpers

fn strip_cr(s: &[u8]) -> &[u8] {
    s.strip_suffix(b"\r").unwrap_or(s)
}

fn get<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |cur, k| cur.get(*k))
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    get(v, path)?.as_str()
}

fn u64_at(v: &Value, path: &[&str]) -> Option<u64> {
    get(v, path)?.as_u64()
}

fn nonempty_str(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str).is_some_and(|s| !s.is_empty())
}

/// `type` or `code` of an error object.
fn err_code(e: &Value) -> Option<&str> {
    str_at(e, &["type"])
        .filter(|t| *t != "error")
        .or_else(|| str_at(e, &["code"]))
}

/// Overwrite with a present, non-null count.
fn set_u(slot: &mut Option<u64>, v: Option<&Value>) {
    if let Some(n) = v.and_then(Value::as_u64) {
        *slot = Some(n);
    }
}

fn set_s(slot: &mut Option<String>, v: Option<&Value>) {
    if let Some(s) = v.and_then(Value::as_str) {
        *slot = Some(capped(s, STR_MAX));
    }
}

/// Append as much of `src` as fits in `cap` bytes, cutting on a char boundary. Returns whether
/// all of it fit.
fn push_capped(dst: &mut String, src: &str, cap: usize) -> bool {
    let room = cap.saturating_sub(dst.len());
    if src.len() <= room {
        dst.push_str(src);
        return true;
    }
    let mut i = room;
    while i > 0 && !src.is_char_boundary(i) {
        i -= 1;
    }
    dst.push_str(src.get(..i).unwrap_or_default());
    false
}

fn capped(s: &str, cap: usize) -> String {
    let mut out = String::new();
    push_capped(&mut out, s, cap);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use Family::{Anthropic, OpenAiChat, OpenAiResponses};
    use serde_json::json;
    use std::time::Duration;

    const MAX_LINE: usize = 1 << 20;

    macro_rules! fixture {
        ($p:literal) => {
            include_str!(concat!("../../init-testkit/fixtures/", $p))
        };
    }

    const DS_DEFAULT_REASONING: &str = fixture!("deepseek/default-reasoning.sse");
    const DS_FAULT_CUT: &str = fixture!("deepseek/fault-cut.sse");
    const DS_FAULT_STREAM_ERROR: &str = fixture!("deepseek/fault-stream-error.sse");
    const DS_KEEPALIVE_CRLF: &str = fixture!("deepseek/keepalive-crlf.sse");
    const DS_MAX_TOKENS: &str = fixture!("deepseek/max-tokens.sse");
    const DS_REFUSAL: &str = fixture!("deepseek/refusal.sse");
    const DS_SPLIT_ARGS: &str = fixture!("deepseek/split-args.sse");
    const DS_TEXT: &str = fixture!("deepseek/text.sse");
    const DS_TOOL_CALLS: &str = fixture!("deepseek/tool-calls.sse");
    const DS_USAGE_EXPLICIT: &str = fixture!("deepseek/usage-explicit.sse");
    const DS_USAGE_SEPARATE: &str = fixture!("deepseek/usage-separate-chunk.sse");
    const AN_TEXT: &str = fixture!("anthropic/text.sse");
    const AN_TEXT_TOOL: &str = fixture!("anthropic/text-tool.sse");
    const OA_MAX_TOKENS: &str = fixture!("openai/max-tokens.sse");
    const OA_TEXT_TOOL: &str = fixture!("openai/text-tool.sse");

    fn all_fixtures() -> Vec<(Family, &'static str)> {
        vec![
            (OpenAiChat, DS_DEFAULT_REASONING),
            (OpenAiChat, DS_FAULT_CUT),
            (OpenAiChat, DS_FAULT_STREAM_ERROR),
            (OpenAiChat, DS_KEEPALIVE_CRLF),
            (OpenAiChat, DS_MAX_TOKENS),
            (OpenAiChat, DS_REFUSAL),
            (OpenAiChat, DS_SPLIT_ARGS),
            (OpenAiChat, DS_TEXT),
            (OpenAiChat, DS_TOOL_CALLS),
            (OpenAiChat, DS_USAGE_EXPLICIT),
            (OpenAiChat, DS_USAGE_SEPARATE),
            (Anthropic, AN_TEXT),
            (Anthropic, AN_TEXT_TOOL),
            (OpenAiResponses, OA_MAX_TOKENS),
            (OpenAiResponses, OA_TEXT_TOOL),
        ]
    }

    /// P-04: in the style of DeepSeek's docs stream example (spaces after colons, an empty first
    /// `content` with `role`, `"usage": null` on every chunk but the last).
    const P04: &str = r#"data: {"id": "930c60df", "object": "chat.completion.chunk", "created": 1760918400, "model": "deepseek-flash", "system_fingerprint": "fp_ds", "choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}, "logprobs": null, "finish_reason": null}], "usage": null}

data: {"id": "930c60df", "object": "chat.completion.chunk", "created": 1760918400, "model": "deepseek-flash", "system_fingerprint": "fp_ds", "choices": [{"index": 0, "delta": {"content": "Hello"}, "logprobs": null, "finish_reason": null}], "usage": null}

data: {"id": "930c60df", "object": "chat.completion.chunk", "created": 1760918400, "model": "deepseek-flash", "system_fingerprint": "fp_ds", "choices": [{"index": 0, "delta": {"content": "!"}, "logprobs": null, "finish_reason": null}], "usage": null}

data: {"id": "930c60df", "object": "chat.completion.chunk", "created": 1760918400, "model": "deepseek-flash", "system_fingerprint": "fp_ds", "choices": [{"index": 0, "delta": {"content": " How can I assist you today?"}, "logprobs": null, "finish_reason": null}], "usage": null}

data: {"id": "930c60df", "object": "chat.completion.chunk", "created": 1760918400, "model": "deepseek-flash", "system_fingerprint": "fp_ds", "choices": [{"index": 0, "delta": {"content": ""}, "logprobs": null, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1040, "completion_tokens": 9, "total_tokens": 1049, "prompt_tokens_details": {"cached_tokens": 1024}, "prompt_cache_hit_tokens": 1024, "prompt_cache_miss_tokens": 16}}

data: [DONE]

"#;

    fn u(
        uncached: u64,
        read: u64,
        write: u64,
        write_1h: u64,
        output: u64,
        reasoning: u64,
    ) -> Usage {
        Usage {
            input_uncached: uncached,
            input_cache_read: read,
            input_cache_write: write,
            input_cache_write_1h: write_1h,
            output,
            reasoning,
        }
    }

    fn call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            args: args.into(),
            ext_name: None,
        }
    }

    fn feed(family: Family, streaming: bool, chunks: &[&[u8]], max_line: usize) -> TapResult {
        let now = Instant::now();
        let mut tap = Tap::new(family, streaming, max_line);
        for c in chunks {
            tap.on_bytes(c, now);
        }
        tap.on_end(now);
        tap.result().clone()
    }

    fn run(family: Family, s: &str) -> TapResult {
        feed(family, true, &[s.as_bytes()], MAX_LINE)
    }

    fn run_body(family: Family, s: &str) -> TapResult {
        feed(family, false, &[s.as_bytes()], MAX_LINE)
    }

    /// One line per frame, line i at `base + i ms`. Returns the result and the line numbers of
    /// the first token, first content and last token.
    fn run_lines(family: Family, s: &str) -> (TapResult, Option<u64>, Option<u64>, Option<u64>) {
        let base = Instant::now();
        let mut tap = Tap::new(family, true, MAX_LINE);
        for (i, line) in s.split_inclusive('\n').enumerate() {
            tap.on_bytes(line.as_bytes(), base + Duration::from_millis(i as u64));
        }
        tap.on_end(base + Duration::from_secs(3600));
        let r = tap.result().clone();
        let ms = |t: Option<Instant>| t.map(|t| t.duration_since(base).as_millis() as u64);
        let (a, b, c) = (
            ms(r.first_token_at),
            ms(r.first_content_at),
            ms(r.last_token_at),
        );
        (r, a, b, c)
    }

    fn line_of(s: &str, needle: &str) -> u64 {
        s.split_inclusive('\n')
            .position(|l| l.contains(needle))
            .expect("needle present") as u64
    }

    /// Everything but the instants (which depend on framing), which are reduced to presence.
    fn summary(r: &TapResult) -> String {
        format!(
            "{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{}|{}|{}|{:?}",
            r.usage,
            r.flags,
            r.finish,
            r.finish_raw,
            r.tool_calls,
            r.first_token_at.is_some(),
            r.first_content_at.is_some(),
            r.last_token_at.is_some(),
            r.body_bytes,
            r.stream_error,
            r.modifiers
        )
    }

    fn assert_split_invariant(family: Family, streaming: bool, bytes: &[u8], max_line: usize) {
        let whole = summary(&feed(family, streaming, &[bytes], max_line));
        for k in 0..=bytes.len() {
            let (a, b) = bytes.split_at(k);
            assert_eq!(
                summary(&feed(family, streaming, &[a, b], max_line)),
                whole,
                "split at {k}"
            );
        }
        let singles: Vec<&[u8]> = bytes.chunks(1).collect();
        assert_eq!(
            summary(&feed(family, streaming, &singles, max_line)),
            whole,
            "byte by byte"
        );
        let mut frames = Vec::new();
        let mut rest = bytes;
        let mut seed = 7u64;
        while !rest.is_empty() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let n = ((seed >> 33) % 97 + 1) as usize;
            let (a, b) = rest.split_at(n.min(rest.len()));
            frames.push(a);
            rest = b;
        }
        assert_eq!(
            summary(&feed(family, streaming, &frames, max_line)),
            whole,
            "irregular frames"
        );
    }

    fn sse(v: serde_json::Value) -> String {
        format!("data: {v}\n\n")
    }

    fn ev(name: &str, v: serde_json::Value) -> String {
        format!("event: {name}\ndata: {v}\n\n")
    }

    fn chat_tool_chunk(index: u64, id: Option<&str>, name: Option<&str>, args: &str) -> String {
        let mut f = json!({ "arguments": args });
        if let Some(n) = name {
            f["name"] = json!(n);
        }
        let mut tc = json!({ "index": index, "function": f });
        if let Some(id) = id {
            tc["id"] = json!(id);
            tc["type"] = json!("function");
        }
        sse(
            json!({"choices": [{"index": 0, "delta": {"tool_calls": [tc]}, "finish_reason": null}]}),
        )
    }

    fn chat_finish(reason: &str, usage: serde_json::Value) -> String {
        sse(
            json!({"choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": reason}], "usage": usage}),
        )
    }

    #[derive(Default)]
    struct Want<'a> {
        usage: Option<Usage>,
        finish: Option<Finish>,
        raw: Option<&'a str>,
        calls: Vec<ToolCall>,
        flags: Vec<&'a str>,
        stream_error: bool,
        first_token: Option<&'a str>,
        first_content: Option<&'a str>,
    }

    fn check(family: Family, text: &str, w: Want) {
        let (r, ft, fc, lt) = run_lines(family, text);
        assert_eq!(r.usage, w.usage, "usage");
        assert_eq!(r.finish, w.finish, "finish");
        assert_eq!(r.finish_raw.as_deref(), w.raw, "finish_raw");
        assert_eq!(r.tool_calls, w.calls, "tool calls");
        assert_eq!(r.flags, w.flags, "flags");
        assert_eq!(r.stream_error, w.stream_error, "stream_error");
        assert_eq!(
            ft,
            w.first_token.map(|n| line_of(text, n)),
            "first token line"
        );
        assert_eq!(
            fc,
            w.first_content.map(|n| line_of(text, n)),
            "first content line"
        );
        assert!(
            lt >= ft && lt.is_some() == ft.is_some(),
            "last token after first"
        );
        assert_eq!(r.body_bytes, text.len() as u64);
        assert_split_invariant(family, true, text.as_bytes(), MAX_LINE);
    }

    // ------------------------------------------------------------------ proxy.md P-04..P-06

    #[test]
    fn p04_deepseek_docs_stream() {
        let (r, ft, fc, lt) = run_lines(OpenAiChat, P04);
        assert_eq!(r.usage, Some(u(16, 1024, 0, 0, 9, 0)));
        assert_eq!(r.finish, Some(Finish::Stop));
        assert_eq!(r.finish_raw.as_deref(), Some("stop"));
        assert!(r.flags.is_empty() && !r.stream_error && r.tool_calls.is_empty());
        let hello = line_of(P04, "\"Hello\"");
        assert_eq!(ft, Some(hello));
        assert_eq!(fc, Some(hello));
        assert_eq!(lt, Some(line_of(P04, "How can I assist")));
        assert_eq!(r.body_bytes, P04.len() as u64);
        assert_eq!(r.model_served.as_deref(), Some("deepseek-flash"));
        assert_split_invariant(OpenAiChat, true, P04.as_bytes(), MAX_LINE);
    }

    fn p05_stream() -> String {
        [
            ev(
                "message_start",
                json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant",
                    "content": [], "model": "claude-x", "stop_reason": null,
                    "usage": {"input_tokens": 120, "cache_creation_input_tokens": 2000, "cache_read_input_tokens": 30000,
                              "cache_creation": {"ephemeral_5m_input_tokens": 1500, "ephemeral_1h_input_tokens": 500},
                              "output_tokens": 1, "service_tier": "standard", "speed": "fast", "inference_geo": "us"}}}),
            ),
            ev("ping", json!({"type": "ping"})),
            ev("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})),
            ev("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hi"}})),
            ev("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            ev(
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": null, "stop_sequence": null},
                    "usage": {"input_tokens": 120, "cache_creation_input_tokens": 2000, "cache_read_input_tokens": 30000, "output_tokens": 40}}),
            ),
            ev(
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                    "usage": {"input_tokens": 120, "cache_creation_input_tokens": 2000, "cache_read_input_tokens": 30000,
                              "output_tokens": 75, "output_tokens_details": {"thinking_tokens": 20}}}),
            ),
            ev("message_stop", json!({"type": "message_stop"})),
        ]
        .concat()
    }

    #[test]
    fn p05_anthropic_message_deltas_overwrite_never_sum() {
        let s = p05_stream();
        let r = run(Anthropic, &s);
        assert_eq!(r.usage, Some(u(120, 30000, 1500, 500, 75, 20)));
        assert_eq!(r.finish, Some(Finish::Stop));
        assert_eq!(r.finish_raw.as_deref(), Some("end_turn"));
        assert_eq!(
            r.modifiers,
            ["speed:fast", "inference_geo:us", "service_tier:standard"]
        );
        assert!(r.flags.is_empty());
        assert_split_invariant(Anthropic, true, s.as_bytes(), MAX_LINE);
    }

    fn p06_stream() -> String {
        [
            ev("response.created", json!({"type": "response.created", "sequence_number": 0, "response": {"id": "resp_1", "status": "in_progress", "output": [], "usage": null}})),
            ev(
                "response.output_item.added",
                json!({"type": "response.output_item.added", "sequence_number": 1, "output_index": 0,
                    "item": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}}),
            ),
            ev("response.output_text.delta", json!({"type": "response.output_text.delta", "sequence_number": 2, "item_id": "msg_1", "delta": "Done."})),
            ev(
                "response.completed",
                json!({"type": "response.completed", "sequence_number": 3, "response": {"id": "resp_1", "status": "completed",
                    "service_tier": "priority",
                    "output": [{"id": "msg_1", "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Done."}]}],
                    "usage": {"input_tokens": 10000, "input_tokens_details": {"cached_tokens": 6000, "cache_write_tokens": 3000},
                              "output_tokens": 50, "output_tokens_details": {"reasoning_tokens": 12}, "total_tokens": 10050}}}),
            ),
        ]
        .concat()
    }

    #[test]
    fn p06_responses_uncached_is_input_minus_cached_minus_write() {
        let s = p06_stream();
        let r = run(OpenAiResponses, &s);
        assert_eq!(r.usage, Some(u(1000, 6000, 3000, 0, 50, 12)));
        assert_eq!(r.finish, Some(Finish::Stop));
        assert_eq!(r.finish_raw.as_deref(), Some("completed"));
        assert_eq!(r.modifiers, ["service_tier:priority"]);
        assert!(r.flags.is_empty());
        assert_split_invariant(OpenAiResponses, true, s.as_bytes(), MAX_LINE);
    }

    // ------------------------------------------------------------------ WP2 golden fixtures

    #[test]
    fn fixture_deepseek_default_reasoning() {
        check(
            OpenAiChat,
            DS_DEFAULT_REASONING,
            Want {
                usage: Some(u(101, 0, 0, 0, 5, 4)),
                finish: Some(Finish::Stop),
                raw: Some("stop"),
                first_token: Some("Mock rea"),
                first_content: Some("\"content\":\"ok\""),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_deepseek_fault_cut() {
        check(
            OpenAiChat,
            DS_FAULT_CUT,
            Want {
                flags: vec!["usage_missing"],
                first_token: Some("Mock rea"),
                first_content: Some("This is "),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_deepseek_fault_stream_error() {
        check(
            OpenAiChat,
            DS_FAULT_STREAM_ERROR,
            Want {
                finish: Some(Finish::Error),
                raw: Some("insufficient_system_resource"),
                flags: vec!["usage_missing"],
                stream_error: true,
                first_token: Some("Mock rea"),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_deepseek_keepalive_crlf() {
        check(
            OpenAiChat,
            DS_KEEPALIVE_CRLF,
            Want {
                usage: Some(u(20, 0, 0, 0, 6, 3)),
                finish: Some(Finish::Stop),
                raw: Some("stop"),
                first_token: Some("Short."),
                first_content: Some("\"content\":\"ok\""),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_deepseek_max_tokens() {
        check(
            OpenAiChat,
            DS_MAX_TOKENS,
            Want {
                usage: Some(u(101, 0, 0, 0, 10, 0)),
                finish: Some(Finish::Length),
                raw: Some("length"),
                calls: vec![call("call_00_mock0", "bash", r#"{"command":"echo cut"}"#)],
                first_token: Some("Partial "),
                first_content: Some("Partial "),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_deepseek_refusal() {
        check(
            OpenAiChat,
            DS_REFUSAL,
            Want {
                usage: Some(u(101, 0, 0, 0, 10, 4)),
                finish: Some(Finish::Other),
                raw: Some("content_filter"),
                first_token: Some("Mock rea"),
                first_content: Some("I can't"),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_deepseek_split_args() {
        check(
            OpenAiChat,
            DS_SPLIT_ARGS,
            Want {
                usage: Some(u(20, 0, 0, 0, 30, 3)),
                finish: Some(Finish::ToolUse),
                raw: Some("tool_calls"),
                calls: vec![
                    call("call_00_hAnDmAdE0", "read", r#"{"path":"a.rs"}"#),
                    call("call_01_hAnDmAdE1", "read", r#"{"path":"b.rs"}"#),
                ],
                first_token: Some("Read both files."),
                first_content: Some("call_00_hAnDmAdE0"),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_deepseek_text() {
        check(
            OpenAiChat,
            DS_TEXT,
            Want {
                usage: Some(u(101, 0, 0, 0, 14, 7)),
                finish: Some(Finish::Stop),
                raw: Some("stop"),
                first_token: Some("The user"),
                first_content: Some("Here are"),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_deepseek_tool_calls() {
        check(
            OpenAiChat,
            DS_TOOL_CALLS,
            Want {
                usage: Some(u(101, 0, 0, 0, 21, 5)),
                finish: Some(Finish::ToolUse),
                raw: Some("tool_calls"),
                calls: vec![
                    call(
                        "call_00_mock0",
                        "bash",
                        r#"{"command":"ls -la","timeout":30}"#,
                    ),
                    call("call_01_mock0", "read", r#"{"path":"src/main.rs"}"#),
                ],
                first_token: Some("Use ls"),
                first_content: Some("call_00_mock0"),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_deepseek_usage_explicit() {
        check(
            OpenAiChat,
            DS_USAGE_EXPLICIT,
            Want {
                usage: Some(u(258, 11776, 0, 0, 31, 12)),
                finish: Some(Finish::Stop),
                raw: Some("stop"),
                first_token: Some("Mock rea"),
                first_content: Some("\"content\":\"cached\""),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_deepseek_usage_separate_chunk() {
        check(
            OpenAiChat,
            DS_USAGE_SEPARATE,
            Want {
                usage: Some(u(20, 0, 0, 0, 6, 3)),
                finish: Some(Finish::Stop),
                raw: Some("stop"),
                first_token: Some("Say hi."),
                first_content: Some("Hi there."),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_anthropic_text() {
        check(
            Anthropic,
            AN_TEXT,
            Want {
                usage: Some(u(58, 0, 0, 0, 5, 0)),
                finish: Some(Finish::Stop),
                raw: Some("end_turn"),
                first_token: Some("Hello fr"),
                first_content: Some("Hello fr"),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_anthropic_text_tool() {
        check(
            Anthropic,
            AN_TEXT_TOOL,
            Want {
                usage: Some(u(58, 0, 0, 0, 10, 0)),
                finish: Some(Finish::ToolUse),
                raw: Some("tool_use"),
                calls: vec![call("toolu_mock0_0", "bash", r#"{"command":"ls"}"#)],
                first_token: Some("signature_delta"),
                first_content: Some("Listing "),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_openai_max_tokens() {
        check(
            OpenAiResponses,
            OA_MAX_TOKENS,
            Want {
                usage: Some(u(61, 0, 0, 0, 2, 0)),
                finish: Some(Finish::Length),
                raw: Some("max_output_tokens"),
                first_token: Some("\"delta\":\"Partial\""),
                first_content: Some("\"delta\":\"Partial\""),
                ..Want::default()
            },
        );
    }

    #[test]
    fn fixture_openai_text_tool() {
        check(
            OpenAiResponses,
            OA_TEXT_TOOL,
            Want {
                usage: Some(u(61, 0, 0, 0, 9, 1)),
                finish: Some(Finish::ToolUse),
                raw: Some("completed"),
                calls: vec![call("call_mock0_0", "bash", r#"{"command":"ls"}"#)],
                first_token: Some("\"delta\":\"Listing \""),
                first_content: Some("\"delta\":\"Listing \""),
                ..Want::default()
            },
        );
    }

    // ------------------------------------------------------------------ tool calls, ext, 4 KiB

    #[test]
    fn ext_name_from_full_args() {
        let big_args = json!({"input": {"data": "x".repeat(5000)}, "name": "big-ext"}).to_string();
        let (b1, b2) = big_args.split_at(3000);
        let s = [
            chat_tool_chunk(0, Some("c0"), Some("ext"), r#"{"name":"sem"#),
            chat_tool_chunk(0, None, None, r#"ver-sort","input":{"v":["1.0.0"]}}"#),
            chat_tool_chunk(1, Some("c1"), Some("bash"), r#"{"name":"x"}"#),
            chat_tool_chunk(2, Some("c2"), Some("ext"), r#"{"name":42}"#),
            chat_tool_chunk(3, Some("c3"), Some("ext"), r#"{"name":"trunc"#),
            chat_tool_chunk(4, Some("c4"), Some("ext"), b1),
            chat_tool_chunk(4, None, None, b2),
            chat_finish(
                "tool_calls",
                json!({"prompt_tokens": 10, "completion_tokens": 5}),
            ),
            "data: [DONE]\n\n".to_string(),
        ]
        .concat();
        let r = run(OpenAiChat, &s);
        let names: Vec<_> = r.tool_calls.iter().map(|c| c.ext_name.as_deref()).collect();
        assert_eq!(
            names,
            [Some("semver-sort"), None, None, None, Some("big-ext")]
        );
        assert_eq!(
            r.tool_calls[0].args,
            r#"{"name":"semver-sort","input":{"v":["1.0.0"]}}"#
        );
        assert_eq!(r.tool_calls[4].args.len(), TOOL_ARGS_MAX);
        assert!(big_args.starts_with(&r.tool_calls[4].args));
        assert_eq!(r.finish, Some(Finish::ToolUse));
        assert_eq!(r.usage, Some(u(10, 0, 0, 0, 5, 0)));
        assert_split_invariant(OpenAiChat, true, s.as_bytes(), MAX_LINE);
    }

    #[test]
    fn args_cut_at_4k_on_char_boundary() {
        // `{"input":"a` is 11 bytes, then 3-byte chars: 4096 falls inside a char, the cut is at 4094.
        let full = format!("{{\"input\":\"a{}\"}}", "€".repeat(2000));
        let (f1, f2) = full.split_at(11);
        let s = [
            chat_tool_chunk(0, Some("c0"), Some("write"), f1),
            chat_tool_chunk(0, None, None, f2),
            chat_tool_chunk(0, None, None, "tail"),
            chat_finish(
                "tool_calls",
                json!({"prompt_tokens": 1, "completion_tokens": 1}),
            ),
        ]
        .concat();
        let r = run(OpenAiChat, &s);
        assert_eq!(r.tool_calls[0].args.len(), 4094);
        assert_eq!(r.tool_calls[0].args, full[..4094]);
        assert_split_invariant(OpenAiChat, true, s.as_bytes(), MAX_LINE);

        // The same cut through a provider's final record (Responses `.done`).
        let s = [
            ev("response.output_item.added", json!({"type": "response.output_item.added",
                "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "write", "arguments": ""}})),
            ev("response.function_call_arguments.delta", json!({"type": "response.function_call_arguments.delta", "item_id": "fc_1", "delta": "{"})),
            ev("response.function_call_arguments.done", json!({"type": "response.function_call_arguments.done", "item_id": "fc_1", "arguments": full})),
        ]
        .concat();
        let r = run(OpenAiResponses, &s);
        assert_eq!(r.tool_calls, [call("call_1", "write", &full[..4094])]);

        let mut out = String::from("ab");
        assert!(!push_capped(&mut out, "€€", 6));
        assert_eq!(out, "ab€");
        assert!(push_capped(&mut out, "c", 6));
        assert_eq!(capped("€", 2), "");
    }

    #[test]
    fn anthropic_tool_input_from_start_or_deltas() {
        let s = [
            ev("message_start", json!({"type": "message_start", "message": {"usage": {"output_tokens": 1}}})),
            ev("content_block_start", json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "tool_use", "id": "t1", "name": "bash", "input": {"command": "pwd"}}})),
            ev("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            ev("content_block_start", json!({"type": "content_block_start", "index": 1,
                "content_block": {"type": "tool_use", "id": "t2", "name": "ext", "input": {}}})),
            ev("content_block_delta", json!({"type": "content_block_delta", "index": 1,
                "delta": {"type": "input_json_delta", "partial_json": "{\"name\": \"semver"}})),
            ev("content_block_delta", json!({"type": "content_block_delta", "index": 1,
                "delta": {"type": "input_json_delta", "partial_json": "-sort\", \"input\": {}}"}})),
            ev("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 9}})),
            ev("message_stop", json!({"type": "message_stop"})),
        ]
        .concat();
        let r = run(Anthropic, &s);
        let mut ext = call("t2", "ext", r#"{"name": "semver-sort", "input": {}}"#);
        ext.ext_name = Some("semver-sort".into());
        assert_eq!(
            r.tool_calls,
            [call("t1", "bash", r#"{"command":"pwd"}"#), ext]
        );
        assert_eq!(r.finish, Some(Finish::ToolUse));
        // DeepSeek's /anthropic usage subset: no input_tokens.
        assert_eq!(r.usage, Some(u(0, 0, 0, 0, 9, 0)));
        assert_eq!(r.flags, ["usage_partial"]);
        assert_split_invariant(Anthropic, true, s.as_bytes(), MAX_LINE);
    }

    #[test]
    fn anthropic_cache_creation_without_split_is_5m() {
        let s = [
            ev("message_start", json!({"type": "message_start", "message": {"usage":
                {"input_tokens": 3, "cache_creation_input_tokens": 700, "cache_read_input_tokens": 0, "output_tokens": 1}}})),
            ev("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "max_tokens"}, "usage": {"output_tokens": 4}})),
        ]
        .concat();
        let r = run(Anthropic, &s);
        assert_eq!(r.usage, Some(u(3, 0, 700, 0, 4, 0)));
        assert_eq!(r.finish, Some(Finish::Length));
    }

    #[test]
    fn responses_custom_tool_and_reasoning_timing() {
        let s = [
            ev("response.created", json!({"type": "response.created", "response": {"id": "r", "status": "in_progress"}})),
            ev("response.reasoning_summary_text.delta", json!({"type": "response.reasoning_summary_text.delta", "item_id": "rs_1", "delta": "Think"})),
            ev("response.reasoning_summary_text.delta", json!({"type": "response.reasoning_summary_text.delta", "item_id": "rs_1", "delta": "ing"})),
            ev("response.output_item.added", json!({"type": "response.output_item.added",
                "item": {"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_9", "name": "apply_patch", "input": ""}})),
            ev("response.custom_tool_call_input.delta", json!({"type": "response.custom_tool_call_input.delta", "item_id": "ctc_1", "delta": "*** Begin"})),
            ev("response.custom_tool_call_input.delta", json!({"type": "response.custom_tool_call_input.delta", "item_id": "ctc_1", "delta": " Patch"})),
            ev("response.custom_tool_call_input.done", json!({"type": "response.custom_tool_call_input.done", "item_id": "ctc_1",
                "input": "*** Begin Patch\n*** End Patch"})),
            ev("response.completed", json!({"type": "response.completed", "response": {"status": "completed",
                "output": [{"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_9", "name": "apply_patch",
                            "input": "*** Begin Patch\n*** End Patch"}],
                "usage": {"input_tokens": 10, "output_tokens": 4}}})),
        ]
        .concat();
        let (r, ft, fc, lt) = run_lines(OpenAiResponses, &s);
        assert_eq!(
            r.tool_calls,
            [call(
                "call_9",
                "apply_patch",
                "*** Begin Patch\n*** End Patch"
            )]
        );
        assert_eq!(r.finish, Some(Finish::ToolUse));
        assert_eq!(r.usage, Some(u(10, 0, 0, 0, 4, 0)));
        assert_eq!(ft, Some(line_of(&s, "\"Think\"")));
        assert_eq!(fc, Some(line_of(&s, "\"delta\":\"*** Begin\"")));
        assert_eq!(lt, Some(line_of(&s, "\"delta\":\" Patch\"")));
        assert_split_invariant(OpenAiResponses, true, s.as_bytes(), MAX_LINE);
    }

    // ------------------------------------------------------------------ errors and finish

    #[test]
    fn anthropic_error_event_mid_stream() {
        let s = [
            ev("message_start", json!({"type": "message_start", "message": {"usage": {"input_tokens": 5, "output_tokens": 1}}})),
            ev("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hi"}})),
            ev("error", json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})),
        ]
        .concat();
        let r = run(Anthropic, &s);
        assert!(r.stream_error);
        assert_eq!(r.finish, Some(Finish::Error));
        assert_eq!(r.finish_raw.as_deref(), Some("overloaded_error"));
        assert_eq!(r.usage, Some(u(5, 0, 0, 0, 1, 0)));
        assert_split_invariant(Anthropic, true, s.as_bytes(), MAX_LINE);

        // `event: error` whose data cannot be parsed still marks the error.
        let r = run(Anthropic, "event: error\ndata: not json\n\n");
        assert!(r.stream_error);
        assert_eq!(r.finish, Some(Finish::Error));
        assert_eq!(r.flags, ["usage_missing"]);
    }

    #[test]
    fn responses_failed_and_error_events() {
        let s = [
            ev(
                "response.output_text.delta",
                json!({"type": "response.output_text.delta", "item_id": "m", "delta": "Hi"}),
            ),
            ev(
                "response.failed",
                json!({"type": "response.failed", "response": {"status": "failed",
                "error": {"code": "server_error", "message": "x"}, "usage": null}}),
            ),
        ]
        .concat();
        let r = run(OpenAiResponses, &s);
        assert!(r.stream_error);
        assert_eq!(r.finish, Some(Finish::Error));
        assert_eq!(r.finish_raw.as_deref(), Some("failed"));
        assert_eq!(r.usage, None);
        assert_eq!(r.flags, ["usage_missing"]);

        let r = run(
            OpenAiResponses,
            &ev(
                "error",
                json!({"type": "error", "code": "rate_limit_exceeded", "message": "slow down"}),
            ),
        );
        assert!(r.stream_error);
        assert_eq!(r.finish_raw.as_deref(), Some("rate_limit_exceeded"));

        let r = run(
            OpenAiResponses,
            &ev(
                "response.incomplete",
                json!({"type": "response.incomplete",
            "response": {"status": "incomplete", "incomplete_details": {"reason": "content_filter"}, "usage": null}}),
            ),
        );
        assert_eq!(r.finish, Some(Finish::Other));
        assert_eq!(r.finish_raw.as_deref(), Some("content_filter"));
    }

    #[test]
    fn openai_chat_normalization_and_error_chunk() {
        let s = [
            sse(json!({"choices": [{"index": 0, "delta": {"role": "assistant", "content": "Hi"}, "finish_reason": null}], "usage": null})),
            sse(json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": null})),
            sse(json!({"choices": [], "usage": {"prompt_tokens": 100, "completion_tokens": 20,
                "prompt_tokens_details": {"cached_tokens": 30}, "completion_tokens_details": {"reasoning_tokens": 4}}})),
            "data: [DONE]\n\n".to_string(),
        ]
        .concat();
        let r = run(OpenAiChat, &s);
        assert_eq!(r.usage, Some(u(70, 30, 0, 0, 20, 4)));
        assert_eq!(r.finish, Some(Finish::Stop));
        assert_split_invariant(OpenAiChat, true, s.as_bytes(), MAX_LINE);

        let s = [
            sse(json!({"choices": [{"index": 0, "delta": {"content": "Hi"}, "finish_reason": null}]})),
            sse(json!({"error": {"message": "boom", "type": "server_error"}})),
        ]
        .concat();
        let r = run(OpenAiChat, &s);
        assert!(r.stream_error);
        assert_eq!(r.finish, Some(Finish::Error));
        assert_eq!(r.finish_raw.as_deref(), Some("server_error"));
        assert_eq!(r.flags, ["usage_missing"]);
    }

    // ------------------------------------------------------------------ framing and bounds

    #[test]
    fn line_over_max_stops_parsing() {
        let longest = DS_TEXT.split('\n').max_by_key(|l| l.len()).expect("lines");
        assert!(
            longest.contains("\"finish_reason\":\"stop\""),
            "the usage chunk is the longest line"
        );
        let ok = feed(OpenAiChat, true, &[DS_TEXT.as_bytes()], longest.len());
        assert!(ok.usage.is_some() && ok.flags.is_empty());
        let r = feed(OpenAiChat, true, &[DS_TEXT.as_bytes()], longest.len() - 1);
        assert_eq!(r.usage, None);
        assert_eq!(r.flags, ["usage_missing"]);
        assert_eq!(r.finish, None);
        assert!(r.first_content_at.is_some());
        assert_eq!(r.body_bytes, DS_TEXT.len() as u64);
        assert_split_invariant(OpenAiChat, true, DS_TEXT.as_bytes(), longest.len() - 1);
        assert_split_invariant(OpenAiChat, true, DS_TEXT.as_bytes(), longest.len());
        // CRLF: the CR counts toward the line.
        let crlf_longest = DS_KEEPALIVE_CRLF
            .split('\n')
            .map(str::len)
            .max()
            .expect("lines");
        assert!(
            feed(
                OpenAiChat,
                true,
                &[DS_KEEPALIVE_CRLF.as_bytes()],
                crlf_longest
            )
            .usage
            .is_some()
        );
        assert!(
            feed(
                OpenAiChat,
                true,
                &[DS_KEEPALIVE_CRLF.as_bytes()],
                crlf_longest - 1
            )
            .usage
            .is_none()
        );
        // Usage already seen is dropped too: parsing stopped, so it may be incomplete.
        let mut s = AN_TEXT.to_string();
        s.push_str(&format!("data: {}\n", "x".repeat(100)));
        let r = feed(Anthropic, true, &[s.as_bytes()], 99);
        assert_eq!((r.usage, r.flags), (None, vec!["usage_missing"]));
    }

    #[test]
    fn keepalive_comments_and_unterminated_last_line() {
        let s = format!(": keep-alive\n\n: keep-alive\r\n\r\n{}", P04);
        let r = run(OpenAiChat, &s);
        assert_eq!(r.usage, Some(u(16, 1024, 0, 0, 9, 0)));
        // The usage chunk without a trailing newline is flushed at end.
        let cut = P04.find("\n\ndata: [DONE]").expect("done");
        let r = run(OpenAiChat, &P04[..cut]);
        assert_eq!(r.usage, Some(u(16, 1024, 0, 0, 9, 0)));
        assert_eq!(r.finish, Some(Finish::Stop));
    }

    #[test]
    fn on_end_is_idempotent_and_late_bytes_only_count() {
        let now = Instant::now();
        let mut tap = Tap::new(OpenAiChat, true, MAX_LINE);
        tap.on_bytes(DS_FAULT_CUT.as_bytes(), now);
        tap.on_end(now);
        let before = tap.result().clone();
        tap.on_bytes(b"data: {\"usage\":{\"prompt_tokens\":1}}\n\n", now);
        tap.on_end(now);
        let after = tap.result();
        assert_eq!(after.usage, None);
        assert_eq!(after.flags, ["usage_missing"]);
        assert_eq!(after.body_bytes, before.body_bytes + 37);
    }

    #[test]
    fn family_none_parses_nothing() {
        let r = feed(Family::None, true, &[P04.as_bytes()], MAX_LINE);
        assert!(
            r.usage.is_none()
                && r.flags.is_empty()
                && r.finish.is_none()
                && r.first_token_at.is_none()
        );
        assert_eq!(r.body_bytes, P04.len() as u64);
        let r = feed(Family::None, false, &[b"{\"usage\":{}}"], MAX_LINE);
        assert!(r.flags.is_empty() && r.first_token_at.is_none());
    }

    // ------------------------------------------------------------------ non-streaming

    #[test]
    fn non_streaming_bodies() {
        let chat = format!(
            "\n\n{}",
            json!({"id": "x", "object": "chat.completion", "choices": [{"index": 0, "message": {"role": "assistant", "content": null,
                "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "ext",
                    "arguments": "{\"name\":\"semver-sort\",\"input\":{}}"}}]}, "finish_reason": "tool_calls"}],
                "usage": {"prompt_tokens": 100, "completion_tokens": 20, "prompt_tokens_details": {"cached_tokens": 64},
                          "completion_tokens_details": {"reasoning_tokens": 5}}})
        );
        let r = run_body(OpenAiChat, &chat);
        assert_eq!(r.usage, Some(u(36, 64, 0, 0, 20, 5)));
        assert_eq!(r.finish, Some(Finish::ToolUse));
        let mut ext = call("call_1", "ext", r#"{"name":"semver-sort","input":{}}"#);
        ext.ext_name = Some("semver-sort".into());
        assert_eq!(r.tool_calls, [ext]);
        assert!(
            r.first_token_at.is_some() && r.first_content_at.is_some() && r.last_token_at.is_some()
        );
        assert_split_invariant(OpenAiChat, false, chat.as_bytes(), MAX_LINE);

        // Timing: first token and content at the first non-whitespace byte, last token at end.
        let base = Instant::now();
        let mut tap = Tap::new(OpenAiChat, false, MAX_LINE);
        tap.on_bytes(b"\n\n", base);
        tap.on_bytes(&chat.as_bytes()[2..], base + Duration::from_millis(5));
        tap.on_end(base + Duration::from_millis(9));
        let r = tap.result();
        assert_eq!(r.first_token_at, Some(base + Duration::from_millis(5)));
        assert_eq!(r.first_content_at, Some(base + Duration::from_millis(5)));
        assert_eq!(r.last_token_at, Some(base + Duration::from_millis(9)));

        let anth = json!({"id": "msg", "type": "message", "role": "assistant",
            "content": [{"type": "text", "text": "Hi"}, {"type": "tool_use", "id": "toolu_1", "name": "bash", "input": {"command": "ls"}}],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "cache_creation_input_tokens": 200, "cache_read_input_tokens": 300,
                      "cache_creation": {"ephemeral_5m_input_tokens": 150, "ephemeral_1h_input_tokens": 50},
                      "output_tokens": 7, "service_tier": "standard", "inference_geo": "us"}})
        .to_string();
        let r = run_body(Anthropic, &anth);
        assert_eq!(r.usage, Some(u(10, 300, 150, 50, 7, 0)));
        assert_eq!(r.finish, Some(Finish::ToolUse));
        assert_eq!(
            r.tool_calls,
            [call("toolu_1", "bash", r#"{"command":"ls"}"#)]
        );
        assert_eq!(r.modifiers, ["inference_geo:us", "service_tier:standard"]);
        assert!(r.flags.is_empty());
        assert_split_invariant(Anthropic, false, anth.as_bytes(), MAX_LINE);

        let resp = json!({"id": "resp_1", "object": "response", "status": "completed", "service_tier": "flex",
            "output": [{"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "bash",
                        "arguments": "{\"command\":\"ls\"}", "status": "completed"}],
            "usage": {"input_tokens": 50, "input_tokens_details": {"cached_tokens": 20}, "output_tokens": 5,
                      "output_tokens_details": {"reasoning_tokens": 2}}})
        .to_string();
        let r = run_body(OpenAiResponses, &resp);
        assert_eq!(r.usage, Some(u(30, 20, 0, 0, 5, 2)));
        assert_eq!(r.finish, Some(Finish::ToolUse));
        assert_eq!(
            r.tool_calls,
            [call("call_1", "bash", r#"{"command":"ls"}"#)]
        );
        assert_eq!(r.modifiers, ["service_tier:flex"]);
        assert_split_invariant(OpenAiResponses, false, resp.as_bytes(), MAX_LINE);

        let r = run_body(
            OpenAiResponses,
            r#"{"error": {"message": "x", "type": "invalid_request_error"}}"#,
        );
        assert!(r.stream_error && r.flags == ["usage_missing"]);
        let r = run_body(OpenAiChat, "not json");
        assert_eq!((r.usage, r.flags), (None, vec!["usage_missing"]));
    }

    #[test]
    fn non_streaming_body_over_limit_is_not_parsed() {
        let mut tap = Tap::new(OpenAiChat, false, MAX_LINE);
        let now = Instant::now();
        tap.on_bytes(
            br#"{"usage":{"prompt_tokens":1,"completion_tokens":1},"pad":""#,
            now,
        );
        let pad = vec![b'x'; 1 << 20];
        for _ in 0..17 {
            tap.on_bytes(&pad, now);
        }
        tap.on_bytes(b"\"}", now);
        tap.on_end(now);
        assert_eq!(tap.result().usage, None);
        assert_eq!(tap.result().flags, ["usage_missing"]);
    }

    // ------------------------------------------------------------------ pre-filter and robustness

    #[test]
    fn prefilter_matches_only_what_matters() {
        assert!(wanted(
            OpenAiChat,
            br#"{"choices":[],"usage": {"prompt_tokens":1}}"#
        ));
        assert!(wanted(OpenAiChat, b"{\"usage\" :\t{\"prompt_tokens\":1}}"));
        assert!(!wanted(
            OpenAiChat,
            br#"{"choices":[{"delta":{"content":"x"},"finish_reason":null}],"usage": null}"#
        ));
        assert!(wanted(
            OpenAiChat,
            br#"{"choices":[{"delta":{},"finish_reason": "stop"}]}"#
        ));
        assert!(wanted(
            OpenAiChat,
            br#"{"choices":[{"delta":{"tool_calls":[{"index":0}]}}]}"#
        ));
        assert!(wanted(OpenAiChat, br#"{"error": {"message":"x"}}"#));
        // Quoted keys inside string values are escaped, so they never match.
        assert!(!wanted(
            OpenAiChat,
            br#"{"choices":[{"delta":{"content":"say \"usage\": {x} \"error\": 1"}}]}"#
        ));
        assert!(!wanted(Anthropic, br#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#));
        assert!(!wanted(Anthropic, br#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hm"}}"#));
        assert!(wanted(Anthropic, br#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{"}}"#));
        assert!(wanted(Anthropic, br#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t"}}"#));
        assert!(wanted(Anthropic, br#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#));
        assert!(wanted(
            Anthropic,
            br#"{"type": "error", "error": {"type": "overloaded_error"}}"#
        ));
        assert!(!wanted(
            OpenAiResponses,
            br#"{"type":"response.output_text.delta","delta":"hi"}"#
        ));
        assert!(!wanted(
            OpenAiResponses,
            br#"{"type":"response.output_text.done","text":"hi"}"#
        ));
        assert!(wanted(
            OpenAiResponses,
            br#"{"type":"response.function_call_arguments.delta","delta":"{"}"#
        ));
        assert!(wanted(
            OpenAiResponses,
            br#"{"type":"response.custom_tool_call_input.delta","delta":"x"}"#
        ));
        assert!(wanted(
            OpenAiResponses,
            br#"{"type":"response.failed","response":{"usage":null}}"#
        ));
        assert!(content_marker(OpenAiChat, br#"{"delta":{"content": "x"}}"#));
        assert!(!content_marker(
            OpenAiChat,
            br#"{"delta":{"content":null,"reasoning_content":"x"}}"#
        ));
        assert!(content_marker(
            Anthropic,
            br#"{"delta":{"type":"text_delta"}}"#
        ));
        assert!(!key_then(b"\"usage\"", b"\"usage\"", b"{"));
        assert_eq!(find_from(b"abcabc", b"bc", 2), Some(4));
        assert_eq!(find_from(b"abc", b"", 0), None);
        assert_eq!(find_from(b"ab", b"abc", 0), None);
    }

    #[test]
    fn never_panics_on_prefixes_and_garbage() {
        for (family, text) in all_fixtures() {
            let b = text.as_bytes();
            for k in 0..=b.len() {
                let r = feed(family, true, &[&b[..k]], MAX_LINE);
                assert_eq!(r.usage.is_none(), r.flags.contains(&"usage_missing"));
                let _ = feed(family, false, &[&b[..k]], MAX_LINE);
            }
        }
        let pieces: &[&[u8]] = &[
            b"data: ",
            b"data:",
            b"event: error\n",
            b"event: message_delta\n",
            b"\n",
            b"\r\n",
            b"\r",
            b": c\n",
            b"{",
            b"}",
            b"[",
            b"]",
            b",",
            b":",
            b"\"",
            b"\\",
            b"null",
            b"-1",
            b"1e400",
            b"18446744073709551616",
            b"\xff\xfe",
            b"\"usage\": {",
            b"\"tool_calls\":[",
            b"\"index\":",
            b"\"function\":{\"name\":\"ext\",\"arguments\":\"",
            b"\"finish_reason\":\"stop\"",
            b"\"type\":\"error\"",
            b"\"type\":\"content_block_start\"",
            b"\"content_block\":{\"type\":\"tool_use\"",
            b"\"type\":\"response.completed\"",
            b"\"response\":",
            b"\"output\":[{\"type\":\"function_call\"",
            b"\"choices\":[{\"delta\":",
            b"\"partial_json\":\"\xe2\x82",
            b"[DONE]",
            b"\"item_id\":\"x\"",
            b"\"delta\":\"\xe2\x82\xac\"",
        ];
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for round in 0..300 {
            let mut s = Vec::new();
            for _ in 0..400 {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                s.extend_from_slice(pieces[(seed >> 33) as usize % pieces.len()]);
            }
            for family in [OpenAiChat, Anthropic, OpenAiResponses, Family::None] {
                for streaming in [true, false] {
                    let max = if round % 2 == 0 { MAX_LINE } else { 64 };
                    let r = feed(family, streaming, &[&s], max);
                    assert_eq!(r.body_bytes, s.len() as u64);
                    assert!(r.tool_calls.iter().all(|c| c.args.len() <= TOOL_ARGS_MAX));
                    if family != Family::None {
                        assert_eq!(r.usage.is_none(), r.flags.contains(&"usage_missing"));
                    }
                }
            }
        }
        // Deep nesting hits serde_json's recursion limit instead of the stack.
        let deep = format!("data: {}\n\n", "[".repeat(100_000));
        let _ = feed(OpenAiChat, true, &[deep.as_bytes()], MAX_LINE);
        let _ = feed(Anthropic, false, &[&deep.as_bytes()[6..]], MAX_LINE);
    }

    #[test]
    fn family_round_trip() {
        for f in [OpenAiChat, Anthropic, OpenAiResponses, Family::None] {
            assert_eq!(Family::parse(f.as_str()), Some(f));
        }
        assert_eq!(Family::parse("x"), None);
        assert_eq!(Finish::ToolUse.as_str(), "tool_use");
    }
}
