//! Anthropic Messages adapter (body/provider.md §3.8.2): request bytes with the B1/B2/B3/A cache
//! markers, thinking-binding controls and their fallbacks, stream decoder, stop and usage mapping,
//! error classification.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value, json};

use super::http::{HttpRequest, header};
use super::sse::SseEvent;
use super::{Decoder, Prompt, Provider, base_kind, effort_wire, handoff, jstr};
use crate::log::icj;
use crate::model::{
    Api, AssistantMessage, ContentBlock, ErrorKind, Message, ModelInfo, Origin, ProviderError,
    StopReason, StreamEvent, Thinking, ToolCall, Usage,
};

pub const PATH: &str = "/anthropic/v1/messages";
pub const VERSION: &str = "2023-06-01";
pub const BETA: &str = "thinking-binding-controls-2026-08-01";
pub const CAUSE_BINDING_OFF: &str = "anthropic_binding_controls_rejected";
pub const CAUSE_STRIP_THINKING: &str = "anthropic_strip_thinking";

pub struct Anthropic {
    info: &'static ModelInfo,
    /// `prefix_mismatch_behavior: "error"` (tests, CI) instead of `"drop_block"`.
    binding_error: bool,
    /// Set once the API rejected the beta or `block_binding`: both stay off for the process.
    binding_off: AtomicBool,
}

impl Anthropic {
    pub fn new(info: &'static ModelInfo, binding_error: bool) -> Self {
        Anthropic {
            info,
            binding_error,
            binding_off: AtomicBool::new(false),
        }
    }

    /// `INIT_TEST=1` and `INIT_TEST_THINKING_BINDING=error` select `"error"`.
    pub fn from_env(info: &'static ModelInfo) -> Self {
        let test = std::env::var("INIT_TEST").as_deref() == Ok("1");
        let err = std::env::var("INIT_TEST_THINKING_BINDING").as_deref() == Ok("error");
        Self::new(info, test && err)
    }

    fn binding_on(&self) -> bool {
        super::models::native_extensions(self.info) && !self.binding_off.load(Ordering::SeqCst)
    }
}

fn ephemeral() -> Value {
    json!({"type": "ephemeral"})
}

fn image_block(media_type: &str, data: &str) -> Value {
    json!({"source": {"data": data, "media_type": media_type, "type": "base64"}, "type": "image"})
}

/// Text (non-empty) and image blocks.
fn user_blocks(content: &[ContentBlock]) -> Vec<Value> {
    content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } if !text.is_empty() => {
                Some(json!({"text": text, "type": "text"}))
            }
            ContentBlock::Image {
                media_type,
                data_b64,
            } => Some(image_block(media_type, data_b64)),
            _ => None,
        })
        .collect()
}

fn assistant_blocks(a: &AssistantMessage) -> Vec<Value> {
    let mut out = Vec::new();
    for b in &a.content {
        match b {
            ContentBlock::Thinking(t) if t.provider == a.origin.provider => {
                if let Some(n) = &t.native {
                    out.push(n.clone());
                }
            }
            ContentBlock::Text { text } if !text.is_empty() => out.push(json!({"text": text, "type": "text"})),
            ContentBlock::ToolCall(c) => {
                out.push(json!({"id": c.id, "input": Value::Object(c.input.clone()), "name": c.name, "type": "tool_use"}))
            }
            ContentBlock::Opaque { api: Api::Messages, raw } => out.push(raw.clone()),
            _ => {}
        }
    }
    if out.is_empty() {
        out.push(json!({"text": handoff::NO_VISIBLE_OUTPUT, "type": "text"}));
    }
    out
}

/// The `messages` array. `b3` is an index into `msgs` (the normalized list).
pub fn wire_messages(msgs: &[Message], b3: Option<usize>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut last_was_results = false;
    for (i, m) in msgs.iter().enumerate() {
        let mut was_results = false;
        match m {
            Message::User { content } => {
                let blocks = user_blocks(content);
                if last_was_results {
                    // A steer after tool results goes into the same user message, after the results.
                    if let Some(Value::Array(c)) = out.last_mut().and_then(|w| w.get_mut("content"))
                    {
                        c.extend(blocks);
                    }
                    was_results = true; // a later steer still merges
                } else if !blocks.is_empty() {
                    out.push(json!({"content": blocks, "role": "user"}));
                }
            }
            Message::Assistant(a) => {
                out.push(json!({"content": assistant_blocks(a), "role": "assistant"}))
            }
            Message::ToolResults(rs) => {
                let blocks: Vec<Value> = rs
                    .iter()
                    .map(|r| {
                        let mut o = Map::new();
                        let c = user_blocks(&r.content);
                        if !c.is_empty() {
                            o.insert("content".into(), Value::Array(c));
                        }
                        if r.is_error {
                            o.insert("is_error".into(), true.into());
                        }
                        o.insert("tool_use_id".into(), r.call_id.clone().into());
                        o.insert("type".into(), "tool_result".into());
                        Value::Object(o)
                    })
                    .collect();
                out.push(json!({"content": blocks, "role": "user"}));
                was_results = true;
            }
            Message::Note { text } => out.push(json!({"content": text, "role": "system"})),
        }
        last_was_results = was_results;
        if b3 == Some(i)
            && let Some(w) = out.last_mut()
        {
            mark_last_block(w);
        }
    }
    out
}

/// Puts `cache_control` on the last content block of a wire message (a string becomes a block).
fn mark_last_block(w: &mut Value) {
    if let Some(s) = w.get("content").and_then(Value::as_str).map(str::to_string) {
        w["content"] = json!([{"cache_control": ephemeral(), "text": s, "type": "text"}]);
        return;
    }
    if let Some(Value::Object(b)) = w
        .get_mut("content")
        .and_then(Value::as_array_mut)
        .and_then(|c| c.last_mut())
    {
        b.insert("cache_control".into(), ephemeral());
    }
}

pub fn build_body(
    p: &Prompt,
    info: &ModelInfo,
    binding: Option<&str>,
) -> Result<Vec<u8>, ProviderError> {
    let msgs = handoff::filter(p.messages, info);
    let mut system =
        vec![json!({"cache_control": ephemeral(), "text": p.head.core, "type": "text"})];
    if !p.head.self_.is_empty() {
        system.push(json!({"cache_control": ephemeral(), "text": p.head.self_, "type": "text"}));
    }
    let tools: Vec<Value> = p
        .tools
        .iter()
        .map(|t| json!({"description": t.description, "input_schema": t.input_schema, "name": t.name}))
        .collect();
    let mut thinking = json!({"type": "adaptive"});
    if let Some(mode) = binding {
        thinking["block_binding"] = json!({"prefix_mismatch_behavior": mode});
    }
    let mut body = json!({
        "cache_control": ephemeral(),
        "max_tokens": super::models::request_tokens(info.provider, info.id, p.max_tokens),
        "messages": wire_messages(&msgs, p.b3),
        "model": info.id,
        "output_config": {"effort": effort_wire(Api::Messages, p.effort)},
        "stream": true,
        "system": system,
        "thinking": thinking,
        "tools": tools,
    });
    if !super::models::native_extensions(info) {
        let fields = body.as_object_mut().expect("request object");
        for key in ["thinking", "output_config", "cache_control"] { fields.remove(key); }
        for key in ["system", "tools"] {
            for item in fields.get_mut(key).and_then(Value::as_array_mut).into_iter().flatten() {
                if let Some(item) = item.as_object_mut() { item.remove("cache_control"); }
            }
        }
        for message in fields.get_mut("messages").and_then(Value::as_array_mut).into_iter().flatten() {
            for block in message.get_mut("content").and_then(Value::as_array_mut).into_iter().flatten() {
                if let Some(block) = block.as_object_mut() { block.remove("cache_control"); }
            }
        }
    }
    icj::to_vec(&body).map_err(|e| ProviderError::Protocol(format!("request: {e}")))
}

/// Removes `thinking.block_binding` (binding controls rejected).
pub fn strip_binding(body: &[u8]) -> Option<Vec<u8>> {
    let mut v: Value = serde_json::from_slice(body).ok()?;
    v.get_mut("thinking")?
        .as_object_mut()?
        .remove("block_binding")?;
    icj::to_vec(&v).ok()
}

/// Removes every `thinking` / `redacted_thinking` block (the documented strip-and-retry recovery).
pub fn strip_thinking(body: &[u8]) -> Option<Vec<u8>> {
    let mut v: Value = serde_json::from_slice(body).ok()?;
    let mut changed = false;
    for m in v.get_mut("messages")?.as_array_mut()? {
        if m.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if let Some(c) = m.get_mut("content").and_then(Value::as_array_mut) {
            let before = c.len();
            c.retain(|b| {
                !matches!(
                    b.get("type").and_then(Value::as_str),
                    Some("thinking" | "redacted_thinking")
                )
            });
            changed |= c.len() != before;
            if c.is_empty() {
                c.push(json!({"text": handoff::NO_VISIBLE_OUTPUT, "type": "text"}));
            }
        }
    }
    if !changed {
        return None;
    }
    icj::to_vec(&v).ok()
}

impl Provider for Anthropic {
    fn info(&self) -> &ModelInfo {
        self.info
    }

    fn build(&self, p: &Prompt) -> Result<HttpRequest, ProviderError> {
        let mode = if self.binding_error {
            "error"
        } else {
            "drop_block"
        };
        let body = build_body(p, self.info, self.binding_on().then_some(mode))?;
        Ok(self.wire(body))
    }

    fn wire(&self, body: Vec<u8>) -> HttpRequest {
        let mut headers = vec![("anthropic-version", VERSION.to_string())];
        let body = if self.binding_on() {
            headers.push(("anthropic-beta", BETA.to_string()));
            body
        } else {
            strip_binding(&body).unwrap_or(body)
        };
        HttpRequest {
            path: if self.info.provider == "anthropic" { PATH.into() } else { format!("/{}/v1/messages", self.info.provider) },
            headers,
            body,
        }
    }

    fn decoder(&self) -> Box<dyn Decoder> {
        let mut decoder = AnDecoder::new(self.info.id);
        decoder.provider = self.info.provider.into();
        Box::new(decoder)
    }

    fn classify(
        &self,
        status: u16,
        headers: &[(String, String)],
        body: Option<&Value>,
        message: &str,
    ) -> ErrorKind {
        match status {
            400 if message.to_ascii_lowercase().contains("prompt is too long") => {
                ErrorKind::ContextOverflow
            }
            429 => {
                let spend = body.and_then(|v| jstr(v, "/error/details/error_code"))
                    == Some("enforced_spend_limit_reached");
                let hint = header(headers, "retry-after").is_some()
                    || header(headers, "retry-after-ms").is_some();
                if spend || !hint {
                    ErrorKind::Billing
                } else {
                    ErrorKind::RateLimit
                }
            }
            504 => ErrorKind::Server,
            s => base_kind(s),
        }
    }

    fn fallback(
        &self,
        req: &HttpRequest,
        err: &ProviderError,
        used: &[&'static str],
    ) -> Option<(HttpRequest, &'static str)> {
        let ProviderError::Http {
            status: 400,
            message,
            ..
        } = err
        else {
            return None;
        };
        let m = message.to_ascii_lowercase();
        if (m.contains("block_binding") || m.contains("anthropic-beta"))
            && !used.contains(&CAUSE_BINDING_OFF)
        {
            self.binding_off.store(true, Ordering::SeqCst);
            return Some((self.wire(req.body.clone()), CAUSE_BINDING_OFF));
        }
        if m.contains("bound to a different conversation") && !used.contains(&CAUSE_STRIP_THINKING)
        {
            let body = strip_thinking(&req.body)?;
            return Some((self.wire(body), CAUSE_STRIP_THINKING));
        }
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Stream decoder
// ---------------------------------------------------------------------------------------------

enum Slot {
    Text(String),
    /// `native` is `{"type":"thinking","thinking","signature"}`.
    Thinking {
        text: String,
        native: Map<String, Value>,
    },
    Redacted(Value),
    Call {
        id: String,
        name: String,
        json: String,
        done: Option<ToolCall>,
    },
    Opaque(Value),
}

#[derive(Default, Clone, Copy)]
struct RawUsage {
    input: u64,
    cache_creation: u64,
    cache_read: u64,
    output: u64,
}

impl RawUsage {
    /// Cumulative, field-wise: fields present in `u` replace the old values.
    fn merge(&mut self, u: &Value) {
        let f = |k: &str| u.get(k).and_then(Value::as_u64);
        if let Some(n) = f("input_tokens") {
            self.input = n;
        }
        if let Some(n) = f("cache_creation_input_tokens") {
            self.cache_creation = n;
        }
        if let Some(n) = f("cache_read_input_tokens") {
            self.cache_read = n;
        }
        if let Some(n) = f("output_tokens") {
            self.output = n;
        }
    }
    fn usage(&self) -> Usage {
        Usage {
            input: self.input + self.cache_creation + self.cache_read,
            cache_read: self.cache_read,
            cache_write: self.cache_creation,
            output: self.output,
            reasoning: 0,
        }
    }
}

pub struct AnDecoder {
    provider: String,
    model: String,
    id: Option<String>,
    slots: BTreeMap<u64, Slot>,
    stop: Option<String>,
    category: Option<String>,
    usage: RawUsage,
    done: bool,
}

impl AnDecoder {
    pub fn new(model: &str) -> Self {
        AnDecoder {
            provider: "anthropic".into(),
            model: model.into(),
            id: None,
            slots: BTreeMap::new(),
            stop: None,
            category: None,
            usage: RawUsage::default(),
            done: false,
        }
    }

    fn origin(&self) -> Origin {
        Origin {
            provider: self.provider.clone(),
            api: Api::Messages,
            model: self.model.clone(),
            response_id: self.id.clone(),
        }
    }

    fn thinking(&self, text: &str, native: Option<Value>) -> ContentBlock {
        ContentBlock::Thinking(Thinking {
            provider: self.provider.clone(),
            model: self.model.clone(),
            text: text.into(),
            native,
        })
    }

    /// `complete`: final assembly (unsigned thinking dropped, open calls an error) vs partial.
    fn blocks(&self, complete: bool) -> Result<Vec<ContentBlock>, ProviderError> {
        let mut out = Vec::new();
        for s in self.slots.values() {
            match s {
                Slot::Text(t) if !t.is_empty() => out.push(ContentBlock::Text { text: t.clone() }),
                Slot::Text(_) => {}
                Slot::Thinking { text, native } => {
                    let signed = native
                        .get("signature")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty());
                    if signed {
                        out.push(self.thinking(text, Some(Value::Object(native.clone()))));
                    } else if !complete && !text.is_empty() {
                        // Partial reasoning is kept for the log but never replayed (no signature).
                        out.push(self.thinking(text, None));
                    }
                }
                Slot::Redacted(v) => out.push(self.thinking("", Some(v.clone()))),
                Slot::Call { done: Some(c), .. } => out.push(ContentBlock::ToolCall(c.clone())),
                Slot::Call { .. } if complete => return Err(proto("tool_use block never closed")),
                Slot::Call { .. } => {}
                Slot::Opaque(v) => out.push(ContentBlock::Opaque {
                    api: Api::Messages,
                    raw: v.clone(),
                }),
            }
        }
        Ok(out)
    }
}

fn proto(m: impl Into<String>) -> ProviderError {
    ProviderError::Protocol(m.into())
}

/// An `error` event / error object type → kind.
pub fn error_kind(t: &str) -> ErrorKind {
    match t {
        "overloaded_error" => ErrorKind::Overloaded,
        "api_error" | "timeout_error" => ErrorKind::Server,
        "rate_limit_error" => ErrorKind::RateLimit,
        "invalid_request_error" => ErrorKind::InvalidRequest,
        "authentication_error" => ErrorKind::Auth,
        "permission_error" => ErrorKind::Permission,
        "not_found_error" => ErrorKind::NotFound,
        "request_too_large" => ErrorKind::TooLarge,
        "billing_error" => ErrorKind::Billing,
        _ => ErrorKind::Other,
    }
}

impl Decoder for AnDecoder {
    fn feed(&mut self, ev: &SseEvent, out: &mut Vec<StreamEvent>) -> Result<(), ProviderError> {
        if self.done {
            return Err(proto("event after message_stop"));
        }
        let v: Value =
            serde_json::from_str(&ev.data).map_err(|e| proto(format!("event JSON: {e}")))?;
        let ty = v
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(ev.event.as_str())
            .to_string();
        let index = || {
            v.get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| proto(format!("{ty} without index")))
        };
        match ty.as_str() {
            "message_start" => {
                let m = v
                    .get("message")
                    .ok_or_else(|| proto("message_start without message"))?;
                self.id = m.get("id").and_then(Value::as_str).map(str::to_string);
                if let Some(u) = m.get("usage") {
                    self.usage.merge(u);
                    out.push(StreamEvent::Usage(self.usage.usage()));
                }
                for t in m
                    .get("input_transformations")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let empty = t.is_null()
                        || t.as_object().is_some_and(Map::is_empty)
                        || t.as_array().is_some_and(Vec::is_empty);
                    if !empty {
                        out.push(StreamEvent::Warning {
                            code: "thinking_dropped",
                            detail: icj::to_string(t).unwrap_or_default(),
                        });
                    }
                }
            }
            "content_block_start" => {
                let i = index()?;
                let b = v
                    .get("content_block")
                    .ok_or_else(|| proto("content_block_start without block"))?;
                let slot = match b.get("type").and_then(Value::as_str) {
                    Some("text") => Slot::Text(jstr(b, "/text").unwrap_or("").into()),
                    Some("thinking") => {
                        let text = jstr(b, "/thinking").unwrap_or("").to_string();
                        let mut native = Map::new();
                        native.insert(
                            "signature".into(),
                            jstr(b, "/signature").unwrap_or("").into(),
                        );
                        native.insert("thinking".into(), text.clone().into());
                        native.insert("type".into(), "thinking".into());
                        Slot::Thinking { text, native }
                    }
                    Some("redacted_thinking") => Slot::Redacted(b.clone()),
                    Some("tool_use") => Slot::Call {
                        id: jstr(b, "/id").unwrap_or("").into(),
                        name: jstr(b, "/name").unwrap_or("").into(),
                        json: String::new(),
                        done: None,
                    },
                    _ => Slot::Opaque(b.clone()),
                };
                self.slots.insert(i, slot);
            }
            "content_block_delta" => {
                let i = index()?;
                let d = v
                    .get("delta")
                    .ok_or_else(|| proto("content_block_delta without delta"))?;
                let slot = self
                    .slots
                    .get_mut(&i)
                    .ok_or_else(|| proto(format!("delta for unknown block {i}")))?;
                match (d.get("type").and_then(Value::as_str), slot) {
                    (Some("text_delta"), Slot::Text(t)) => {
                        let s = jstr(d, "/text").unwrap_or("");
                        t.push_str(s);
                        if !s.is_empty() {
                            out.push(StreamEvent::TextDelta(s.into()));
                        }
                    }
                    (Some("thinking_delta"), Slot::Thinking { text, native }) => {
                        let s = jstr(d, "/thinking").unwrap_or("");
                        text.push_str(s);
                        native.insert("thinking".into(), text.clone().into());
                        if !s.is_empty() {
                            out.push(StreamEvent::ThinkingDelta(s.into()));
                        }
                    }
                    (Some("signature_delta"), Slot::Thinking { native, .. }) => {
                        native.insert(
                            "signature".into(),
                            jstr(d, "/signature").unwrap_or("").into(),
                        );
                    }
                    (Some("input_json_delta"), Slot::Call { json, .. }) => {
                        json.push_str(jstr(d, "/partial_json").unwrap_or(""))
                    }
                    _ => {} // citations_delta, unknown deltas
                }
            }
            "content_block_stop" => {
                let i = index()?;
                if let Some(Slot::Call {
                    id,
                    name,
                    json,
                    done,
                }) = self.slots.get_mut(&i)
                {
                    let (input, raw) = if json.trim().is_empty() {
                        (Map::new(), None)
                    } else {
                        match serde_json::from_str::<Value>(json) {
                            Ok(Value::Object(m)) => (m, None),
                            _ => (Map::new(), Some(json.clone())),
                        }
                    };
                    let c = ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input,
                        raw_arguments: raw,
                    };
                    if c.id.is_empty() || c.name.is_empty() {
                        return Err(proto("tool_use without id or name"));
                    }
                    out.push(StreamEvent::ToolCallReady(c.clone()));
                    *done = Some(c);
                }
            }
            "message_delta" => {
                if let Some(d) = v.get("delta") {
                    if let Some(s) = d.get("stop_reason").and_then(Value::as_str) {
                        self.stop = Some(s.into());
                    }
                    let cat = d
                        .pointer("/stop_details/category")
                        .or_else(|| v.pointer("/stop_details/category"))
                        .and_then(Value::as_str);
                    if let Some(c) = cat {
                        self.category = Some(c.into());
                    }
                }
                if let Some(u) = v.get("usage") {
                    self.usage.merge(u);
                    out.push(StreamEvent::Usage(self.usage.usage()));
                }
            }
            "message_stop" => self.done = true,
            "error" => {
                let t = jstr(&v, "/error/type").unwrap_or("");
                let message = jstr(&v, "/error/message")
                    .unwrap_or("stream error")
                    .to_string();
                return Err(ProviderError::Stream {
                    kind: error_kind(t),
                    message,
                });
            }
            _ => {} // ping and unknown event types (versioning policy)
        }
        Ok(())
    }

    fn finished(&self) -> bool {
        self.done
    }

    fn finish(&mut self) -> Result<AssistantMessage, ProviderError> {
        if !self.done {
            return Err(ProviderError::Truncated);
        }
        let stop = match self.stop.as_deref() {
            Some("end_turn") => StopReason::EndTurn,
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            Some("stop_sequence") => StopReason::StopSequence,
            Some("pause_turn") => StopReason::PauseTurn,
            Some("refusal") => StopReason::Refusal {
                category: self.category.clone(),
            },
            Some("model_context_window_exceeded") => StopReason::ContextWindow,
            other => return Err(proto(format!("unknown stop_reason {other:?}"))),
        };
        Ok(AssistantMessage {
            content: self.blocks(true)?,
            stop,
            usage: self.usage.usage(),
            origin: self.origin(),
        })
    }

    fn partial(&mut self) -> AssistantMessage {
        AssistantMessage {
            content: self.blocks(false).unwrap_or_default(),
            stop: StopReason::Aborted,
            usage: self.usage.usage(),
            origin: self.origin(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Effort, ToolMeta, ToolResult};
    use crate::provider::deepseek::tests::{result, tools, user};
    use crate::provider::testutil::{decode, fixture};
    use crate::provider::{PromptHead, models};

    fn info() -> &'static ModelInfo {
        models::lookup("anthropic", "claude-opus-5-5").unwrap()
    }

    fn dec() -> Box<dyn Decoder> {
        Box::new(AnDecoder::new("claude-opus-5-5"))
    }

    fn sse(events: &[Value]) -> Vec<u8> {
        let mut s = String::new();
        for e in events {
            s.push_str(&format!(
                "event: {}\ndata: {}\n\n",
                e["type"].as_str().unwrap(),
                e
            ));
        }
        s.into_bytes()
    }

    fn start(usage: Value) -> Value {
        json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": [], "model": "claude-opus-5-5", "stop_reason": null, "usage": usage}})
    }

    /// The streaming-doc flow: text, tool_use with input_json_delta, thinking + signature_delta.
    #[test]
    fn custom_route_keeps_provider_identity() {
        let mut info = models::lookup("anthropic", "claude-opus-5-5").unwrap().clone();
        info.provider = "custom_proxy";
        let info = Box::leak(Box::new(info));
        let adapter = Anthropic::new(info, false);
        assert_eq!(adapter.wire(b"{}".to_vec()).path, "/custom_proxy/v1/messages");
        let (_, result) = decode(adapter.decoder(), &fixture("anthropic/text-tool.sse"));
        let result = result.unwrap();
        assert_eq!(result.origin.provider, "custom_proxy");
        let native: Vec<Value> = result.content.iter().filter_map(|block| {
            if let ContentBlock::Thinking(t) = block {
                assert_eq!(t.provider, "custom_proxy");
                t.native.clone()
            } else { None }
        }).collect();
        assert!(!native.is_empty(), "fixture must exercise native reasoning replay");
        let messages = vec![Message::Assistant(result)];
        let prompt = Prompt {
            head: PromptHead { core: "core", self_: "self" }, tools: &[],
            messages: &messages, b3: None, max_tokens: 1024,
            effort: info.default_effort, session: "test-session",
        };
        let request = adapter.build(&prompt).unwrap();
        let payload: Value = serde_json::from_slice(&request.body).unwrap();
        assert!(payload.get("thinking").is_none());
        assert!(payload.get("max_tokens").is_some());
        assert!(payload.get("input").is_none());
        assert!(!request.headers.iter().any(|(k, _)| *k == "anthropic-beta"));
        let rendered = String::from_utf8(request.body).unwrap();
        for item in native {
            assert!(rendered.contains(&icj::to_string(&item).unwrap()), "native reasoning must survive alias replay");
        }
    }

    #[test]
    fn decode_fixture_thinking_text_tool() {
        let (evs, m) = decode(dec(), &fixture("anthropic/text-tool.sse"));
        let m = m.unwrap();
        assert_eq!(m.stop, StopReason::ToolUse);
        match &m.content[0] {
            ContentBlock::Thinking(t) => {
                assert_eq!(t.native.as_ref().unwrap()["signature"], "mock-sig-0");
                assert_eq!(t.native.as_ref().unwrap()["type"], "thinking");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(m.text(), "Listing now.");
        let c = m.tool_calls().next().unwrap();
        assert_eq!((c.id.as_str(), c.name.as_str()), ("toolu_mock0_0", "bash"));
        assert_eq!(c.input["command"], "ls");
        assert_eq!(c.raw_arguments, None);
        assert_eq!(m.usage.input, 58);
        assert_eq!(m.usage.output, 10);
        assert_eq!(
            evs.iter()
                .filter(|e| matches!(e, StreamEvent::ToolCallReady(_)))
                .count(),
            1
        );
        let (_, m) = decode(dec(), &fixture("anthropic/text.sse"));
        assert_eq!(m.unwrap().stop, StopReason::EndTurn);
    }

    #[test]
    fn decode_usage_merge_unknown_event_refusal_and_errors() {
        let evs = vec![
            start(
                json!({"input_tokens": 10, "cache_creation_input_tokens": 200, "cache_read_input_tokens": 3000, "output_tokens": 1}),
            ),
            json!({"type": "ping"}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "I can't"}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "citations_delta", "citation": {}}}),
            json!({"type": "brand_new_event", "x": 1}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "message_delta", "delta": {"stop_reason": "refusal", "stop_sequence": null, "stop_details": {"category": "cyber"}}, "usage": {"output_tokens": 7}}),
            json!({"type": "message_stop"}),
        ];
        let (out, m) = decode(dec(), &sse(&evs));
        let m = m.unwrap();
        assert_eq!(
            m.stop,
            StopReason::Refusal {
                category: Some("cyber".into())
            }
        );
        assert_eq!(
            m.usage,
            Usage {
                input: 3210,
                cache_read: 3000,
                cache_write: 200,
                output: 7,
                reasoning: 0
            }
        );
        assert_eq!(
            out.iter()
                .filter(|e| matches!(e, StreamEvent::Usage(_)))
                .count(),
            2
        );
        // Overloaded mid-stream: retryable Stream error.
        let evs = vec![
            start(json!({"input_tokens": 1})),
            json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
        ];
        let (_, r) = decode(dec(), &sse(&evs));
        let e = r.unwrap_err();
        assert!(
            matches!(
                e,
                ProviderError::Stream {
                    kind: ErrorKind::Overloaded,
                    ..
                }
            ) && e.retryable()
        );
        let evs = vec![
            start(json!({})),
            json!({"type": "error", "error": {"type": "invalid_request_error", "message": "bad"}}),
        ];
        assert!(!decode(dec(), &sse(&evs)).1.unwrap_err().retryable());
        // Unknown stop reason, truncation.
        let evs = vec![
            start(json!({})),
            json!({"type": "message_delta", "delta": {"stop_reason": "weird"}}),
            json!({"type": "message_stop"}),
        ];
        assert!(matches!(
            decode(dec(), &sse(&evs)).1,
            Err(ProviderError::Protocol(_))
        ));
        assert!(matches!(
            decode(dec(), &sse(&[start(json!({}))])).1,
            Err(ProviderError::Truncated)
        ));
    }

    #[test]
    fn decode_unsigned_thinking_dropped_bad_json_kept_raw_and_transformations() {
        let evs = vec![
            json!({"type": "message_start", "message": {"id": "msg_2", "usage": {}, "input_transformations": [{"type": "thinking_block_dropped", "index": 3}, {}]}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "hmm"}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "redacted_thinking", "data": "EmwKAhgBEgy"}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_9", "name": "write", "input": {}}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"path\": \"a\", \"content\": \"cut"}}),
            json!({"type": "content_block_stop", "index": 2}),
            json!({"type": "content_block_start", "index": 3, "content_block": {"type": "server_widget", "z": 1}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "max_tokens"}, "usage": {"output_tokens": 5}}),
            json!({"type": "message_stop"}),
        ];
        let (out, m) = decode(dec(), &sse(&evs));
        let m = m.unwrap();
        let warnings: Vec<&StreamEvent> = out
            .iter()
            .filter(|e| matches!(e, StreamEvent::Warning { .. }))
            .collect();
        assert_eq!(warnings.len(), 1);
        assert_eq!(m.content.len(), 3, "{:?}", m.content); // redacted, call, opaque (unsigned thinking dropped)
        assert!(
            matches!(&m.content[0], ContentBlock::Thinking(t) if t.native.as_ref().unwrap()["type"] == "redacted_thinking")
        );
        let c = m.tool_calls().next().unwrap();
        assert!(c.input.is_empty());
        assert_eq!(
            c.raw_arguments.as_deref(),
            Some("{\"path\": \"a\", \"content\": \"cut")
        );
        assert!(c.args_error().is_some());
        assert!(matches!(
            &m.content[2],
            ContentBlock::Opaque {
                api: Api::Messages,
                ..
            }
        ));
        assert_eq!(m.stop, StopReason::MaxTokens);
    }

    fn prompt<'a>(
        msgs: &'a [Message],
        ts: &'a [crate::model::ToolSpec],
        b3: Option<usize>,
    ) -> Prompt<'a> {
        Prompt {
            head: PromptHead {
                core: "CORE",
                self_: "SELF",
            },
            tools: ts,
            messages: msgs,
            b3,
            max_tokens: 64_000,
            effort: Effort::High,
            session: "s",
        }
    }

    fn an_assistant(model: &str) -> Message {
        Message::Assistant(AssistantMessage {
            content: vec![
                ContentBlock::Thinking(Thinking {
                    provider: "anthropic".into(),
                    model: model.into(),
                    text: String::new(),
                    native: Some(
                        json!({"signature": "EqQBCgIYAh", "thinking": "", "type": "thinking"}),
                    ),
                }),
                ContentBlock::Text {
                    text: "Running the tests first.".into(),
                },
                ContentBlock::Text {
                    text: String::new(),
                },
                ContentBlock::ToolCall(ToolCall {
                    id: "toolu_01A".into(),
                    name: "bash".into(),
                    input: ToolCall::parse_input("{\"command\":\"cd work/demo && cargo test\"}"),
                    raw_arguments: None,
                }),
            ],
            stop: StopReason::ToolUse,
            usage: Usage::default(),
            origin: Origin {
                provider: "anthropic".into(),
                api: Api::Messages,
                model: model.into(),
                response_id: None,
            },
        })
    }

    /// §8 item 7.
    #[test]
    fn build_rules_and_markers() {
        let ts = tools();
        let msgs = vec![
            user("Fix the failing test in work/demo."),
            an_assistant("claude-opus-5-5"),
            Message::ToolResults(vec![ToolResult {
                call_id: "toolu_01A".into(),
                content: vec![ContentBlock::Text {
                    text: "test add ... FAILED\n[exit_code=101 wall_time=3.21s]".into(),
                }],
                is_error: true,
                meta: ToolMeta::default(),
            }]),
            user("also check lib.rs"),
            Message::Note {
                text: "[init:signal] ctx 40%".into(),
            },
            an_assistant("claude-sonnet-5-5"),
            Message::ToolResults(vec![result("toolu_01A", "ok", false)]),
        ];
        let p = Anthropic::new(info(), false);
        let req = p.build(&prompt(&msgs, &ts, Some(3))).unwrap();
        assert_eq!(req.path, PATH);
        assert_eq!(
            req.headers,
            vec![
                ("anthropic-version", VERSION.to_string()),
                ("anthropic-beta", BETA.to_string())
            ]
        );
        assert_eq!(req, p.build(&prompt(&msgs, &ts, Some(3))).unwrap());
        let v: Value = serde_json::from_slice(&req.body).unwrap();
        assert!(icj::is_canonical(&req.body));
        // B1, B2 on system; A at top level.
        assert_eq!(v["system"][0]["cache_control"], ephemeral());
        assert_eq!(v["system"][1]["cache_control"], ephemeral());
        assert_eq!(v["cache_control"], ephemeral());
        assert_eq!(
            v["thinking"],
            json!({"block_binding": {"prefix_mismatch_behavior": "drop_block"}, "type": "adaptive"})
        );
        assert_eq!(v["output_config"], json!({"effort": "high"}));
        for k in ["temperature", "top_p", "top_k", "tool_choice"] {
            assert!(v.get(k).is_none());
        }
        let m = v["messages"].as_array().unwrap();
        let roles: Vec<&str> = m.iter().map(|x| x["role"].as_str().unwrap()).collect();
        assert_eq!(
            roles,
            ["user", "assistant", "user", "system", "assistant", "user"]
        );
        // Same-model thinking replayed verbatim; empty text skipped.
        assert_eq!(
            m[1]["content"][0],
            json!({"signature": "EqQBCgIYAh", "thinking": "", "type": "thinking"})
        );
        assert_eq!(m[1]["content"].as_array().unwrap().len(), 3);
        assert_eq!(
            m[1]["content"][2]["input"],
            json!({"command": "cd work/demo && cargo test"})
        );
        // Steer merged after the tool_result block, with B3 on its last block.
        assert_eq!(m[2]["content"][0]["type"], "tool_result");
        assert_eq!(m[2]["content"][0]["is_error"], true);
        assert_eq!(
            m[2]["content"][1],
            json!({"cache_control": ephemeral(), "text": "also check lib.rs", "type": "text"})
        );
        // Notes are mid-conversation system messages.
        assert_eq!(
            m[3],
            json!({"content": "[init:signal] ctx 40%", "role": "system"})
        );
        // Sonnet thinking is not replayed to Opus.
        assert!(
            m[4]["content"]
                .as_array()
                .unwrap()
                .iter()
                .all(|b| b["type"] != "thinking")
        );
        assert!(m[5]["content"][0].get("is_error").is_none());
        // At most 4 markers.
        let n = String::from_utf8(req.body.clone())
            .unwrap()
            .matches("cache_control")
            .count();
        assert_eq!(n, 4);
        // Test mode binding "error".
        let pe = Anthropic::new(info(), true);
        let v: Value =
            serde_json::from_slice(&pe.build(&prompt(&msgs, &ts, None)).unwrap().body).unwrap();
        assert_eq!(
            v["thinking"]["block_binding"]["prefix_mismatch_behavior"],
            "error"
        );
    }

    #[test]
    fn fallbacks_binding_off_and_strip_thinking() {
        let ts = tools();
        let msgs = vec![
            user("hi"),
            an_assistant("claude-opus-5-5"),
            Message::ToolResults(vec![result("toolu_01A", "ok", false)]),
        ];
        let p = Anthropic::new(info(), false);
        let req = p.build(&prompt(&msgs, &ts, None)).unwrap();
        let http400 = |msg: &str| ProviderError::Http {
            status: 400,
            kind: ErrorKind::InvalidRequest,
            message: msg.into(),
            retry_after: None,
            request_id: None,
        };
        let (r2, cause) = p
            .fallback(
                &req,
                &http400("thinking.block_binding: Extra inputs are not permitted"),
                &[],
            )
            .unwrap();
        assert_eq!(cause, CAUSE_BINDING_OFF);
        assert_eq!(r2.headers, vec![("anthropic-version", VERSION.to_string())]);
        let v: Value = serde_json::from_slice(&r2.body).unwrap();
        assert_eq!(v["thinking"], json!({"type": "adaptive"}));
        // Off for the rest of the process: later builds omit both.
        let later = p.build(&prompt(&msgs, &ts, None)).unwrap();
        assert!(later.headers.iter().all(|(k, _)| *k != "anthropic-beta"));
        assert!(
            !String::from_utf8(later.body.clone())
                .unwrap()
                .contains("block_binding")
        );
        let (r3, cause) = p
            .fallback(
                &later,
                &http400("thinking block is bound to a different conversation"),
                &[CAUSE_BINDING_OFF],
            )
            .unwrap();
        assert_eq!(cause, CAUSE_STRIP_THINKING);
        assert!(
            !String::from_utf8(r3.body)
                .unwrap()
                .contains("\"thinking\",\"type\":\"thinking\"")
        );
        assert!(
            p.fallback(
                &later,
                &http400("bound to a different conversation"),
                &[CAUSE_STRIP_THINKING]
            )
            .is_none()
        );
    }

    #[test]
    fn classify_errors() {
        let p = Anthropic::new(info(), false);
        let h = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        };
        assert_eq!(
            p.classify(
                400,
                &[],
                None,
                "prompt is too long: 1000001 tokens > 1000000 maximum"
            ),
            ErrorKind::ContextOverflow
        );
        assert_eq!(
            p.classify(400, &[], None, "messages: bad"),
            ErrorKind::InvalidRequest
        );
        assert_eq!(
            p.classify(429, &h(&[("retry-after", "5")]), None, ""),
            ErrorKind::RateLimit
        );
        assert_eq!(p.classify(429, &[], None, ""), ErrorKind::Billing);
        let spend = json!({"error": {"type": "rate_limit_error", "details": {"error_code": "enforced_spend_limit_reached"}}});
        assert_eq!(
            p.classify(429, &h(&[("retry-after", "5")]), Some(&spend), ""),
            ErrorKind::Billing
        );
        assert_eq!(p.classify(529, &[], None, ""), ErrorKind::Overloaded);
        assert_eq!(p.classify(504, &[], None, ""), ErrorKind::Server);
        assert_eq!(p.classify(413, &[], None, ""), ErrorKind::TooLarge);
        assert_eq!(p.classify(403, &[], None, ""), ErrorKind::Permission);
    }

    /// v0.5.0 L3, offline: a cold request writes the prefix to the cache (5-minute TTL, the only one INIT asks for)
    /// and the next request reads it back. The decoder's `Usage` is what the session log records (`to_log`) and what
    /// the cache % in the status line comes from. The Root meters the same bytes with its own tap:
    /// root/crates/init-root/src/providers/tests.rs `cache_writes_bill_at_the_charter_write_price`.
    #[test]
    fn cache_write_then_read_usage_reaches_the_log() {
        let (_, m1) = decode(dec(), &fixture("anthropic/cache-write.sse"));
        let m1 = m1.unwrap();
        assert_eq!(m1.stop, StopReason::ToolUse);
        assert_eq!(m1.usage, Usage { input: 21_507, cache_read: 0, cache_write: 21_504, output: 87, reasoning: 0 });
        let (_, m2) = decode(dec(), &fixture("anthropic/cache-read.sse"));
        let m2 = m2.unwrap();
        assert_eq!(m2.stop, StopReason::EndTurn);
        // message_delta carries only output_tokens: the input fields of message_start stand (cumulative, never summed)
        assert_eq!(m2.usage, Usage { input: 22_148, cache_read: 21_504, cache_write: 640, output: 132, reasoning: 0 });
        assert_eq!(m2.usage.hit_permille(), 970);
        let l1 = m1.usage.to_log(None);
        assert_eq!((l1.input_tokens, l1.cache_read_tokens, l1.cache_write_tokens, l1.output_tokens), (3, 0, 21_504, 87));
        let l2 = m2.usage.to_log(None);
        assert_eq!((l2.input_tokens, l2.cache_read_tokens, l2.cache_write_tokens, l2.output_tokens), (4, 21_504, 640, 132));
        assert_eq!(l2.prompt_total(), m2.usage.input);
    }

    /// The cache is read only if request 2 starts with request 1's content: the same system blocks with B1 and B2,
    /// the same tools and options, the earlier messages unchanged. B3 moves to the newest message and the top-level
    /// `cache_control` (A) stays; four markers, the API's limit (report "Bước tiếp theo cho INIT": anthropic.rs:157-181).
    #[test]
    fn consecutive_requests_keep_the_cached_prefix_stable() {
        let ts = tools();
        let first = vec![user("Run the qualify command.")];
        let (_, m1) = decode(dec(), &fixture("anthropic/cache-write.sse"));
        let mut second = first.clone();
        second.push(Message::Assistant(m1.unwrap()));
        second.push(Message::ToolResults(vec![result("toolu_q1", "3f1c0b9e  -", false)]));
        let p = Anthropic::new(info(), false);
        let body = |msgs: &[Message], b3| -> Value { serde_json::from_slice(&p.build(&prompt(msgs, &ts, Some(b3))).unwrap().body).unwrap() };
        let (v1, v2) = (body(&first, 0), body(&second, 2));
        for k in ["system", "tools", "model", "thinking", "output_config", "max_tokens", "cache_control"] {
            assert_eq!(v1[k], v2[k], "{k} differs between consecutive requests");
        }
        assert_eq!(v1["system"][0]["cache_control"], ephemeral()); // B1: the core head
        assert_eq!(v1["system"][1]["cache_control"], ephemeral()); // B2: the self head
        assert_eq!(v1["cache_control"], ephemeral()); // A
        let unmarked = |m: &Value| {
            let mut m = m.clone();
            for b in m["content"].as_array_mut().into_iter().flatten() {
                b.as_object_mut().unwrap().remove("cache_control");
            }
            m
        };
        assert_eq!(unmarked(&v1["messages"][0]), unmarked(&v2["messages"][0]));
        assert_eq!(v1["messages"][0]["content"][0]["cache_control"], ephemeral()); // B3 on the newest message
        assert!(v2["messages"][0]["content"][0].get("cache_control").is_none());
        let newest = v2["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(newest["content"].as_array().unwrap().last().unwrap()["cache_control"], ephemeral());
        for v in [&v1, &v2] {
            assert_eq!(icj::to_string(v).unwrap().matches("cache_control").count(), 4);
        }
    }
}
