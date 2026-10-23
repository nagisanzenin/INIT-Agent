//! DeepSeek Chat Completions adapter (body/provider.md §3.8.1): request bytes, stream decoder,
//! finish-reason and usage mapping, error classification, and the foreign-turn flatten fallback
//! (§3.10). Also serves `mock` (`mock-frontier-1` speaks the DeepSeek wire format).

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::http::HttpRequest;
use super::sse::SseEvent;
use super::{
    Decoder, Prompt, Provider, base_kind, effort_wire, handoff, jstr, ju64, mentions_overflow,
};
use crate::log::icj;
use crate::model::{
    Api, AssistantMessage, ContentBlock, ErrorKind, Message, ModelInfo, Origin, ProviderError,
    StopReason, StreamEvent, Thinking, ToolCall, ToolResult, Usage,
};

/// The proxy path: `/<provider id>/chat/completions` (`mock` is routed by the proxy in test mode).
pub fn path(info: &ModelInfo) -> String {
    format!("/{}/chat/completions", info.provider)
}

pub struct DeepSeek {
    info: &'static ModelInfo,
}

impl DeepSeek {
    pub fn new(info: &'static ModelInfo) -> Self {
        DeepSeek { info }
    }
}

/// The fallback causes recorded for the loop's `error` event, in the order they are tried.
/// First: add `"reasoning_content": ""` to every assistant message without it and keep the tool
/// calls (live V-DS-3, 2026-10-20: DeepSeek accepts a foreign tool turn with an empty
/// `reasoning_content`, and the model then uses the foreign tool result as given).
pub const EMPTY_REASONING_CAUSE: &str = "deepseek_empty_reasoning_foreign_turns";
/// Second, only if the first retry is refused too: the §3.10 flatten. Live, a flattened history is
/// accepted but the model re-runs the flattened tool call (V-DS-3b), so it is the last resort.
pub const FLATTEN_CAUSE: &str = "deepseek_flatten_foreign_turns";

impl Provider for DeepSeek {
    fn info(&self) -> &ModelInfo {
        self.info
    }

    fn build(&self, p: &Prompt) -> Result<HttpRequest, ProviderError> {
        Ok(self.wire(build_body(p, self.info)?))
    }

    fn wire(&self, body: Vec<u8>) -> HttpRequest {
        HttpRequest {
            path: path(self.info),
            headers: Vec::new(),
            body,
        }
    }

    fn decoder(&self) -> Box<dyn Decoder> {
        Box::new(DsDecoder::new(self.info.provider, self.info.id))
    }

    fn classify(
        &self,
        status: u16,
        _headers: &[(String, String)],
        _body: Option<&Value>,
        message: &str,
    ) -> ErrorKind {
        match status {
            400 | 422 if mentions_overflow(message) => ErrorKind::ContextOverflow,
            s => base_kind(s),
        }
    }

    /// §3.10 step 2: a 400 on a request that carries foreign tool turns (assistant tool calls with
    /// no `reasoning_content`) is retried once with `"reasoning_content": ""` added and the tool
    /// calls kept; if that is refused as well, once more with those turns flattened to text.
    fn fallback(
        &self,
        req: &HttpRequest,
        err: &ProviderError,
        used: &[&'static str],
    ) -> Option<(HttpRequest, &'static str)> {
        // Generic OpenAI-compatible/local servers must never receive DeepSeek recovery fields.
        if !matches!(self.info.provider, "deepseek" | "mock") {
            return None;
        }
        let is_400 = matches!(
            err,
            ProviderError::Http {
                status: 400,
                kind: ErrorKind::InvalidRequest,
                ..
            }
        );
        if !is_400 {
            return None;
        }
        if !used.contains(&EMPTY_REASONING_CAUSE)
            && let Some(body) = add_empty_reasoning(&req.body)
        {
            return Some((self.wire(body), EMPTY_REASONING_CAUSE));
        }
        if used.contains(&FLATTEN_CAUSE) {
            return None;
        }
        let body = flatten_foreign(&req.body)?;
        Some((self.wire(body), FLATTEN_CAUSE))
    }
}

/// The first fallback: `"reasoning_content": ""` on every assistant message that has none, every
/// other byte kept (the tool calls stay structured). None when every assistant message has one.
pub fn add_empty_reasoning(body: &[u8]) -> Option<Vec<u8>> {
    let mut v: Value = serde_json::from_slice(body).ok()?;
    let mut changed = false;
    for m in v.get_mut("messages")?.as_array_mut()? {
        if m.get("role").and_then(Value::as_str) == Some("assistant")
            && m.get("reasoning_content").is_none()
            && let Some(o) = m.as_object_mut()
        {
            o.insert("reasoning_content".into(), Value::String(String::new()));
            changed = true;
        }
    }
    if !changed {
        return None;
    }
    icj::to_vec(&v).ok()
}

// ---------------------------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------------------------

fn image_part(media_type: &str, data_b64: &str) -> Value {
    json!({"image_url": {"url": format!("data:{media_type};base64,{data_b64}")}, "type": "image_url"})
}

fn omitted(info: &ModelInfo) -> String {
    format!("[image omitted: {} has no image input]", info.id)
}

/// A string when only text (joined with `sep`), else a content-part array.
fn content(blocks: &[ContentBlock], sep: &str, info: &ModelInfo) -> Value {
    let has_image = blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }));
    if !has_image || !info.images {
        let parts: Vec<String> = blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                ContentBlock::Image { .. } => Some(omitted(info)),
                _ => None,
            })
            .collect();
        return Value::String(parts.join(sep));
    }
    Value::Array(
        blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(json!({"text": text, "type": "text"})),
                ContentBlock::Image {
                    media_type,
                    data_b64,
                } => Some(image_part(media_type, data_b64)),
                _ => None,
            })
            .collect(),
    )
}

/// One `Message` → its Chat Completions wire messages (provider.md §3.8.1 field rules).
pub fn wire_messages(m: &Message, info: &ModelInfo) -> Vec<Value> {
    match m {
        Message::User { content: c } => vec![json!({"content": content(c, "\n\n", info), "role": "user"})],
        Message::Note { text } => vec![json!({"content": text, "role": "user"})],
        Message::Assistant(a) => {
            let mut text = String::new();
            let mut reasoning: Option<String> = None;
            let mut calls = Vec::new();
            for b in &a.content {
                match b {
                    ContentBlock::Text { text: t } => text.push_str(t),
                    ContentBlock::Thinking(t) if t.provider == info.provider && t.model == info.id => {
                        reasoning.get_or_insert_with(String::new).push_str(&t.text);
                    }
                    ContentBlock::ToolCall(c) => {
                        let args = match (&c.raw_arguments, info.cache.replay_raw_arguments) {
                            (Some(r), true) => r.clone(),
                            _ => icj::to_string(&Value::Object(c.input.clone())).unwrap_or_default(),
                        };
                        calls.push(json!({"function": {"arguments": args, "name": c.name}, "id": c.id, "type": "function"}));
                    }
                    _ => {} // foreign thinking, images, opaque blocks: not on this wire
                }
            }
            let mut o = Map::new();
            o.insert("content".into(), text.into());
            o.insert("role".into(), "assistant".into());
            if let Some(r) = reasoning {
                o.insert("reasoning_content".into(), r.into());
            }
            if !calls.is_empty() {
                o.insert("tool_calls".into(), Value::Array(calls));
            }
            vec![Value::Object(o)]
        }
        Message::ToolResults(rs) => rs
            .iter()
            .map(|r: &ToolResult| {
                json!({"content": content(&r.content, "\n", info), "role": "tool", "tool_call_id": r.call_id})
            })
            .collect(),
    }
}

pub fn chat_tool(name: &str, description: &str, parameters: &Value) -> Value {
    json!({"function": {"description": description, "name": name, "parameters": parameters}, "type": "function"})
}

/// The full request body (ICJ bytes). The handoff filter is applied here too (it is idempotent),
/// so a caller cannot replay foreign thinking by mistake.
pub fn build_body(p: &Prompt, info: &ModelInfo) -> Result<Vec<u8>, ProviderError> {
    let msgs = handoff::filter(p.messages, info);
    let mut messages =
        vec![json!({"content": format!("{}\n{}", p.head.core, p.head.self_), "role": "system"})];
    for m in &msgs {
        messages.extend(wire_messages(m, info));
    }
    let tools: Vec<Value> = p
        .tools
        .iter()
        .map(|t| chat_tool(&t.name, &t.description, &t.input_schema))
        .collect();
    let mut body = json!({
        "max_tokens": super::models::request_tokens(info.provider, info.id, p.max_tokens),
        "messages": messages,
        "model": info.id,
        "reasoning_effort": effort_wire(Api::ChatCompletions, p.effort),
        "stream": true,
        "thinking": {"type": "enabled"},
        "tools": tools,
    });
    if !matches!(info.provider, "deepseek" | "mock") {
        let fields = body.as_object_mut().expect("request is an object");
        fields.remove("reasoning_effort");
        fields.remove("thinking");
        // Standard Chat streaming reports usage in a final empty-choices chunk only
        // when requested. Keep missing usage unknown in Root if a server omits it.
        if super::models::chat_stream_usage(info.provider, info.id) {
            fields.insert("stream_options".into(), json!({"include_usage": true}));
        }
        for message in fields.get_mut("messages").and_then(Value::as_array_mut).expect("messages is an array") {
            if let Some(message) = message.as_object_mut() {
                message.remove("reasoning_content");
            }
        }
    }
    icj::to_vec(&body).map_err(|e| ProviderError::Protocol(format!("request: {e}")))
}

/// §3.10 flatten: every assistant turn with `tool_calls` and no (or an empty) `reasoning_content`
/// becomes one text message (`[called <name> <arguments>]` per call), and its tool results become user
/// messages `[result of <name> (<id>)]\n<content>`. Every assistant message that lacks
/// `reasoning_content` gets `"reasoning_content": ""` (deviation: DeepSeek requires the field on
/// every assistant message in tool mode, so a bare flattened turn would 400 again). None when there
/// is nothing to change.
pub fn flatten_foreign(body: &[u8]) -> Option<Vec<u8>> {
    let mut v: Value = serde_json::from_slice(body).ok()?;
    let msgs = v.get_mut("messages")?.as_array_mut()?;
    let mut out = Vec::with_capacity(msgs.len());
    let mut names: BTreeMap<String, String> = BTreeMap::new(); // foreign call id → name
    let mut changed = false;
    for m in msgs.drain(..) {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("");
        if role == "assistant"
            && m.get("tool_calls").is_some()
            && m.get("reasoning_content")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            let mut lines: Vec<String> = Vec::new();
            if let Some(t) = m
                .get("content")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                lines.push(t.to_string());
            }
            for c in m["tool_calls"].as_array().into_iter().flatten() {
                let id = jstr(c, "/id").unwrap_or("").to_string();
                let name = jstr(c, "/function/name").unwrap_or("").to_string();
                let args = jstr(c, "/function/arguments").unwrap_or("");
                lines.push(format!("[called {name} {args}]"));
                names.insert(id, name);
            }
            out.push(
                json!({"content": lines.join("\n"), "reasoning_content": "", "role": "assistant"}),
            );
            changed = true;
        } else if role == "tool" && names.contains_key(jstr(&m, "/tool_call_id").unwrap_or("")) {
            let id = jstr(&m, "/tool_call_id").unwrap_or("").to_string();
            let tag = format!("[result of {} ({id})]", names[&id]);
            let c = match &m["content"] {
                Value::String(s) => Value::String(format!("{tag}\n{s}")),
                Value::Array(parts) => {
                    let mut p = vec![json!({"text": tag, "type": "text"})];
                    p.extend(parts.iter().cloned());
                    Value::Array(p)
                }
                other => Value::String(format!("{tag}\n{other}")),
            };
            out.push(json!({"content": c, "role": "user"}));
        } else if role == "assistant" && m.get("reasoning_content").is_none() {
            let mut m = m;
            m["reasoning_content"] = Value::String(String::new());
            out.push(m);
            changed = true;
        } else {
            out.push(m);
        }
    }
    if !changed {
        return None;
    }
    *msgs = out;
    icj::to_vec(&v).ok()
}

// ---------------------------------------------------------------------------------------------
// Stream decoder
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    args: String,
}

pub struct DsDecoder {
    provider: String,
    model: String,
    id: Option<String>,
    reasoning: String,
    text: String,
    calls: BTreeMap<u32, PartialCall>,
    finish: Option<String>,
    usage: Option<Usage>,
    done: bool,
    ready_sent: bool,
}

impl DsDecoder {
    pub fn new(provider: &str, model: &str) -> Self {
        DsDecoder {
            provider: provider.into(),
            model: model.into(),
            id: None,
            reasoning: String::new(),
            text: String::new(),
            calls: BTreeMap::new(),
            finish: None,
            usage: None,
            done: false,
            ready_sent: false,
        }
    }

    fn origin(&self) -> Origin {
        Origin {
            provider: self.provider.clone(),
            api: Api::ChatCompletions,
            model: self.model.clone(),
            response_id: self.id.clone(),
        }
    }

    fn head_blocks(&self) -> Vec<ContentBlock> {
        let mut c = Vec::new();
        if !self.reasoning.is_empty() {
            c.push(ContentBlock::Thinking(Thinking {
                provider: self.provider.clone(),
                model: self.model.clone(),
                text: self.reasoning.clone(),
                native: None,
            }));
        }
        if !self.text.is_empty() {
            c.push(ContentBlock::Text {
                text: self.text.clone(),
            });
        }
        c
    }

    fn call(p: &PartialCall) -> ToolCall {
        ToolCall {
            id: p.id.clone(),
            name: p.name.clone(),
            input: ToolCall::parse_input(&p.args),
            raw_arguments: Some(p.args.clone()),
        }
    }
}

/// DeepSeek `usage` → `Usage` (provider.md §3.8.1 usage table).
pub fn map_usage(u: &Value) -> Usage {
    let cache_read = match u.get("prompt_cache_hit_tokens").and_then(Value::as_u64) {
        Some(n) => n,
        None => ju64(u, "/prompt_tokens_details/cached_tokens"),
    };
    Usage {
        input: ju64(u, "/prompt_tokens"),
        cache_read,
        cache_write: 0,
        output: ju64(u, "/completion_tokens"),
        reasoning: ju64(u, "/completion_tokens_details/reasoning_tokens"),
    }
}

fn proto(m: impl Into<String>) -> ProviderError {
    ProviderError::Protocol(m.into())
}

/// Sets a first-value field: Protocol when a later chunk carries a different non-empty value.
fn first_value(slot: &mut String, v: Option<&str>, what: &str) -> Result<(), ProviderError> {
    match v {
        Some(x) if !x.is_empty() => {
            if slot.is_empty() {
                *slot = x.to_string();
            } else if slot != x {
                return Err(proto(format!("tool call {what} changed mid-stream")));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

impl Decoder for DsDecoder {
    fn feed(&mut self, ev: &SseEvent, out: &mut Vec<StreamEvent>) -> Result<(), ProviderError> {
        if self.done {
            return Err(proto("event after [DONE]"));
        }
        if ev.data.trim() == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let v: Value =
            serde_json::from_str(&ev.data).map_err(|e| proto(format!("chunk JSON: {e}")))?;
        if let Some(err) = v.get("error").filter(|e| e.is_object()) {
            let message = jstr(err, "/message").unwrap_or("stream error").to_string();
            return Err(ProviderError::Stream {
                kind: ErrorKind::Other,
                message,
            });
        }
        if self.id.is_none() {
            self.id = v.get("id").and_then(Value::as_str).map(str::to_string);
        }
        let choices = match v.get("choices") {
            None | Some(Value::Null) => &[][..],
            Some(Value::Array(a)) => a.as_slice(),
            Some(_) => return Err(proto("choices is not an array")),
        };
        if choices.len() > 1 {
            return Err(proto("more than one choice"));
        }
        if let Some(choice) = choices.first() {
            let delta = choice.get("delta").filter(|d| d.is_object());
            if let Some(d) = delta {
                if let Some(r) = d
                    .get("reasoning_content")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    self.reasoning.push_str(r);
                    out.push(StreamEvent::ThinkingDelta(r.to_string()));
                }
                if let Some(c) = d
                    .get("content")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    self.text.push_str(c);
                    out.push(StreamEvent::TextDelta(c.to_string()));
                }
                match d.get("tool_calls") {
                    None | Some(Value::Null) => {}
                    Some(Value::Array(tcs)) => {
                        for tc in tcs {
                            let index = tc
                                .get("index")
                                .and_then(Value::as_u64)
                                .and_then(|i| u32::try_from(i).ok())
                                .ok_or_else(|| proto("tool call delta without index"))?;
                            let p = self.calls.entry(index).or_default();
                            first_value(&mut p.id, jstr(tc, "/id"), "id")?;
                            first_value(&mut p.name, jstr(tc, "/function/name"), "name")?;
                            if let Some(a) = jstr(tc, "/function/arguments") {
                                p.args.push_str(a);
                            }
                        }
                    }
                    Some(_) => return Err(proto("tool_calls is not an array")),
                }
            }
            match choice.get("finish_reason") {
                None | Some(Value::Null) => {}
                Some(Value::String(f)) => {
                    match f.as_str() {
                        "insufficient_system_resource" => {
                            return Err(ProviderError::Stream {
                                kind: ErrorKind::Overloaded,
                                message: "finish_reason insufficient_system_resource".into(),
                            });
                        }
                        "aborted" => {
                            return Err(ProviderError::Stream {
                                kind: ErrorKind::Server,
                                message: "finish_reason aborted (server side)".into(),
                            });
                        }
                        _ => {}
                    }
                    self.finish = Some(f.clone());
                    if !self.ready_sent {
                        self.ready_sent = true;
                        for p in self
                            .calls
                            .values()
                            .filter(|p| !p.id.is_empty() && !p.name.is_empty())
                        {
                            out.push(StreamEvent::ToolCallReady(Self::call(p)));
                        }
                    }
                }
                Some(_) => return Err(proto("finish_reason is not a string")),
            }
        }
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            let u = map_usage(u);
            self.usage = Some(u);
            out.push(StreamEvent::Usage(u));
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
        let finish = self
            .finish
            .clone()
            .ok_or_else(|| proto("[DONE] without a finish_reason"))?;
        let mut content = self.head_blocks();
        for p in self.calls.values() {
            if p.id.is_empty() || p.name.is_empty() {
                return Err(proto("tool call without id or name"));
            }
            content.push(ContentBlock::ToolCall(Self::call(p)));
        }
        let has_calls = !self.calls.is_empty();
        let stop = match finish.as_str() {
            "tool_calls" if has_calls => StopReason::ToolUse,
            "tool_calls" => return Err(proto("finish_reason tool_calls without a tool call")),
            "stop" if has_calls => StopReason::ToolUse,
            "stop" => StopReason::EndTurn,
            "length" => StopReason::MaxTokens,
            "content_filter" => StopReason::Refusal {
                category: Some("content_filter".into()),
            },
            other => return Err(proto(format!("unknown finish_reason {other:?}"))),
        };
        Ok(AssistantMessage {
            content,
            stop,
            usage: self.usage.unwrap_or_default(),
            origin: self.origin(),
        })
    }

    fn partial(&mut self) -> AssistantMessage {
        let mut content = self.head_blocks();
        if self.finish.is_some() {
            // Calls are complete only once the finish_reason arrived.
            for p in self
                .calls
                .values()
                .filter(|p| !p.id.is_empty() && !p.name.is_empty())
            {
                content.push(ContentBlock::ToolCall(Self::call(p)));
            }
        }
        AssistantMessage {
            content,
            stop: StopReason::Aborted,
            usage: self.usage.unwrap_or_default(),
            origin: self.origin(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{Effort, ToolMeta, ToolSpec};
    use crate::provider::models;
    use crate::provider::testutil::{decode, fixture};

    fn dec() -> Box<dyn Decoder> {
        Box::new(DsDecoder::new("deepseek", "deepseek-flash"))
    }

    fn run(name: &str) -> (Vec<StreamEvent>, Result<AssistantMessage, ProviderError>) {
        decode(dec(), &fixture(&format!("deepseek/{name}")))
    }

    fn calls(m: &AssistantMessage) -> Vec<(String, String, String)> {
        m.tool_calls()
            .map(|c| {
                (
                    c.id.clone(),
                    c.name.clone(),
                    c.raw_arguments.clone().unwrap(),
                )
            })
            .collect()
    }

    fn thinking(m: &AssistantMessage) -> Option<String> {
        m.content.iter().find_map(|b| match b {
            ContentBlock::Thinking(t) => Some(t.text.clone()),
            _ => None,
        })
    }

    /// (a) text only (plus reasoning, which thinking mode always sends).
    #[test]
    fn a_text_only() {
        let (evs, m) = run("text.sse");
        let m = m.unwrap();
        assert_eq!(m.stop, StopReason::EndTurn);
        assert!(m.tool_calls().next().is_none());
        assert!(!m.text().is_empty());
        let deltas: String = evs
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TextDelta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, m.text());
        assert_eq!(m.origin.response_id.as_deref(), Some("mock-turn-0"));
        assert_eq!(m.origin.model, "deepseek-flash");
    }

    /// (b) reasoning, then 2 tool calls whose argument fragments interleave by `index`;
    /// (c) `id`/`name` only in the first fragment.
    #[test]
    fn b_c_interleaved_calls() {
        let (evs, m) = run("split-args.sse");
        let m = m.unwrap();
        assert_eq!(m.stop, StopReason::ToolUse);
        assert_eq!(thinking(&m).as_deref(), Some("Read both files."));
        let c = calls(&m);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].0, "call_00_hAnDmAdE0");
        assert_eq!(c[1].0, "call_01_hAnDmAdE1");
        for (_, name, raw) in &c {
            assert_eq!(name, "read");
            assert!(
                serde_json::from_str::<Value>(raw).unwrap().is_object(),
                "{raw}"
            );
        }
        assert!(
            m.tool_calls()
                .all(|c| c.args_error().is_none() && c.input.contains_key("path"))
        );
        let ready = evs
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolCallReady(_)))
            .count();
        assert_eq!(ready, 2);
        // The block order is thinking, (text), calls.
        assert!(matches!(m.content[0], ContentBlock::Thinking(_)));
        let (_, m2) = run("tool-calls.sse");
        let m2 = m2.unwrap();
        assert_eq!(m2.stop, StopReason::ToolUse);
        assert!(m2.tool_calls().count() >= 1);
        assert!(m2.tool_calls().all(|c| c.args_error().is_none()));
    }

    /// (d) `finish_reason:"length"` with a partial call: MaxTokens, the call is kept.
    #[test]
    fn d_length_keeps_partial_call() {
        let (_, m) = run("max-tokens.sse");
        let m = m.unwrap();
        assert_eq!(m.stop, StopReason::MaxTokens);
        assert_eq!(m.tool_calls().count(), 1);
        assert!(m.stop.is_length());
    }

    /// (e) `insufficient_system_resource`: retryable `Stream{Overloaded}`.
    #[test]
    fn e_insufficient_system_resource() {
        let (_, r) = run("fault-stream-error.sse");
        let e = r.unwrap_err();
        assert!(
            matches!(
                e,
                ProviderError::Stream {
                    kind: ErrorKind::Overloaded,
                    ..
                }
            ),
            "{e:?}"
        );
        assert!(e.retryable());
    }

    /// (f) usage only on the last chunk (a separate, choice-less chunk), with cache hit counts.
    #[test]
    fn f_usage_last_chunk() {
        let (evs, m) = run("usage-separate-chunk.sse");
        let m = m.unwrap();
        assert_eq!(
            m.usage,
            Usage {
                input: 20,
                cache_read: 0,
                cache_write: 0,
                output: 6,
                reasoning: 3
            }
        );
        assert_eq!(
            evs.iter()
                .filter(|e| matches!(e, StreamEvent::Usage(_)))
                .count(),
            1
        );
        let (_, m) = run("usage-explicit.sse");
        let u = m.unwrap().usage;
        assert_eq!(
            (u.input, u.cache_read, u.output, u.reasoning),
            (12034, 11776, 31, 12)
        );
        assert_eq!(u.cache_miss(), 258); // = prompt_cache_miss_tokens
        assert_eq!(u.hit_permille(), 978);
    }

    /// (g) keep-alive comments (CRLF) before the first chunk.
    #[test]
    fn g_keepalive_comments() {
        let (_, m) = run("keepalive-crlf.sse");
        let m = m.unwrap();
        assert_eq!(m.text(), "ok");
        assert_eq!(thinking(&m).as_deref(), Some("Short."));
    }

    /// (h) an event after `[DONE]` is Protocol.
    #[test]
    fn h_event_after_done() {
        let mut bytes = fixture("deepseek/text.sse");
        bytes.extend_from_slice(b"data: {\"choices\":[]}\n\n");
        let (_, r) = decode(dec(), &bytes);
        assert!(matches!(r, Err(ProviderError::Protocol(m)) if m.contains("after [DONE]")));
    }

    /// (i) the documented example stream from the API reference (reconstructed; see WP10.md).
    #[test]
    fn i_api_reference_example() {
        let (_, m) = run("api-reference-example.sse");
        let m = m.unwrap();
        assert_eq!(m.text(), "Hello! How can I assist you today?");
        assert_eq!(m.stop, StopReason::EndTurn);
        assert_eq!((m.usage.input, m.usage.output), (16, 10));
    }

    #[test]
    fn refusal_and_truncation_and_bad_chunks() {
        let (_, m) = run("refusal.sse");
        assert_eq!(
            m.unwrap().stop,
            StopReason::Refusal {
                category: Some("content_filter".into())
            }
        );
        let (_, r) = run("fault-cut.sse");
        assert!(matches!(r, Err(ProviderError::Truncated)));
        for bad in [
            "data: {not json}\n\n",
            "data: {\"choices\":[{},{}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"x\"}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"a\"}]}}]}\n\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"b\"}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"weird\"}]}\n\ndata: [DONE]\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n",
            "data: [DONE]\n\n",
        ] {
            let (_, r) = decode(dec(), bad.as_bytes());
            assert!(matches!(r, Err(ProviderError::Protocol(_))), "{bad}: {r:?}");
        }
        // "aborted" server-side is retryable Stream{Server}.
        let (_, r) = decode(
            dec(),
            b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"aborted\"}]}\n\n",
        );
        assert!(matches!(
            r,
            Err(ProviderError::Stream {
                kind: ErrorKind::Server,
                ..
            })
        ));
    }

    #[test]
    fn partial_drops_incomplete_calls() {
        let bytes = fixture("deepseek/tool-calls.sse");
        let cut = bytes
            .windows(15)
            .position(|w| w == b"\"finish_reason\"")
            .unwrap();
        // Feed everything up to the chunk that carries a non-null finish_reason.
        let text = String::from_utf8(bytes.clone()).unwrap();
        let last_event = text.rfind("finish_reason\":\"").unwrap();
        let head = &text[..text[..last_event].rfind("data:").unwrap()];
        let mut d = DsDecoder::new("deepseek", "deepseek-flash");
        let mut r = crate::provider::sse::SseReader::new(head.as_bytes());
        let mut out = Vec::new();
        while let Some(ev) = r.next_event().unwrap() {
            d.feed(&ev, &mut out).unwrap();
        }
        let p = d.partial();
        assert_eq!(p.stop, StopReason::Aborted);
        assert_eq!(p.tool_calls().count(), 0);
        assert!(thinking(&p).is_some());
        let _ = cut;
    }

    // -----------------------------------------------------------------------------------------
    // build()
    // -----------------------------------------------------------------------------------------

    pub(crate) fn tools() -> Vec<ToolSpec> {
        let v: Value = serde_json::from_slice(
            &std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/context/testdata/tools.json"),
            )
            .unwrap(),
        )
        .unwrap();
        v.as_array()
            .unwrap()
            .iter()
            .map(|t| ToolSpec {
                name: t["name"].as_str().unwrap().into(),
                description: t["description"].as_str().unwrap().into(),
                input_schema: t["input_schema"].clone(),
            })
            .collect()
    }

    pub(crate) fn ds_assistant(think: &str, text: &str, calls: &[(&str, &str, &str)]) -> Message {
        let mut content = Vec::new();
        if !think.is_empty() {
            content.push(ContentBlock::Thinking(Thinking {
                provider: "deepseek".into(),
                model: "deepseek-flash".into(),
                text: think.into(),
                native: None,
            }));
        }
        if !text.is_empty() {
            content.push(ContentBlock::Text { text: text.into() });
        }
        for (id, name, raw) in calls {
            content.push(ContentBlock::ToolCall(ToolCall {
                id: (*id).into(),
                name: (*name).into(),
                input: ToolCall::parse_input(raw),
                raw_arguments: Some((*raw).into()),
            }));
        }
        Message::Assistant(AssistantMessage {
            content,
            stop: StopReason::ToolUse,
            usage: Usage::default(),
            origin: Origin {
                provider: "deepseek".into(),
                api: Api::ChatCompletions,
                model: "deepseek-flash".into(),
                response_id: None,
            },
        })
    }

    pub(crate) fn result(id: &str, text: &str, is_error: bool) -> ToolResult {
        ToolResult {
            call_id: id.into(),
            content: vec![ContentBlock::Text { text: text.into() }],
            is_error,
            meta: ToolMeta::default(),
        }
    }

    pub(crate) fn user(text: &str) -> Message {
        Message::User {
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub(crate) fn six() -> Vec<Message> {
        vec![
            user("Fix the failing test in work/demo."),
            ds_assistant(
                "The user wants ... I should run the tests first.",
                "",
                &[(
                    "call_00_kw66qNnNto11bSfJVIdlV5Oo",
                    "bash",
                    "{\"command\": \"cd work/demo && cargo test 2>&1 | tail -20\"}",
                )],
            ),
            Message::ToolResults(vec![result(
                "call_00_kw66qNnNto11bSfJVIdlV5Oo",
                "test add ... FAILED\n[exit_code=101 wall_time=3.21s]",
                false,
            )]),
            Message::Note {
                text: "[init:signal] ctx 12%".into(),
            },
            ds_assistant("Fix the sign.", "Fixed: `a - b` → `a + b`.", &[]),
            user("Thanks. Commit it."),
        ]
    }

    fn prompt<'a>(msgs: &'a [Message], tools: &'a [ToolSpec]) -> Prompt<'a> {
        Prompt {
            head: crate::provider::PromptHead {
                core: "CORE",
                self_: "SELF",
            },
            tools,
            messages: msgs,
            b3: None,
            max_tokens: 65_536,
            effort: Effort::High,
            session: "20261019-203015-9f3a2c",
        }
    }

    #[test]
    fn generic_chat_does_not_send_deepseek_extensions() {
        let mut info = models::lookup("deepseek", "deepseek-flash").unwrap().clone();
        info.provider = "local";
        info.id = "custom-model";
        let ts = tools();
        let msgs = six();
        let p = prompt(&msgs, &ts);
        let body: Value = serde_json::from_slice(&build_body(&p, &info).unwrap()).unwrap();
        assert_eq!(body["model"], "custom-model");
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
        assert!(body["messages"].as_array().unwrap().iter().all(|m| m.get("reasoning_content").is_none()));
        assert!(body["messages"].as_array().unwrap().iter().any(|m| m.get("tool_calls").is_some()));
        assert_eq!(path(&info), "/local/chat/completions");
        let provider = DeepSeek::new(Box::leak(Box::new(info)));
        let req = provider.wire(icj::to_vec(&body).unwrap());
        let err = ProviderError::Http {
            status: 400, kind: ErrorKind::InvalidRequest, message: "unsupported parameter".into(),
            retry_after: None, request_id: None,
        };
        assert!(provider.fallback(&req, &err, &[]).is_none());
    }

    #[test]
    fn generic_chat_usage_only_tail_preserves_tool_call_and_totals() {
        let mut decoder = DsDecoder::new("local", "custom-model");
        let mut events = Vec::new();
        for data in [
            json!({"id":"chat-local","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"a\"}"}}]},"finish_reason":null}],"usage":null}).to_string(),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":null}).to_string(),
            json!({"choices":[],"usage":{"prompt_tokens":123,"completion_tokens":17,"prompt_tokens_details":{"cached_tokens":100}}}).to_string(),
            "[DONE]".into(),
        ] {
            decoder.feed(&SseEvent { event:String::new(), data, id:None }, &mut events).unwrap();
        }
        let message = decoder.finish().unwrap();
        assert_eq!(message.stop, StopReason::ToolUse);
        assert_eq!(message.tool_calls().count(), 1);
        assert_eq!((message.usage.input, message.usage.output, message.usage.cache_read), (123,17,100));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Usage(u) if u.input == 123 && u.output == 17)));
    }

    /// provider.md §8 item 5: golden bytes for a 6-message conversation, sorted keys, reasoning on
    /// every DeepSeek assistant message, system first, deterministic.
    #[test]
    fn build_golden_six_messages() {
        let info = models::lookup("deepseek", "deepseek-flash").unwrap();
        let ts = tools();
        let msgs = six();
        let p = prompt(&msgs, &ts);
        let a = build_body(&p, info).unwrap();
        let b = build_body(&p, info).unwrap();
        assert_eq!(a, b);
        let v: Value = serde_json::from_slice(&a).unwrap();
        assert!(icj::is_canonical(&a));
        let wm = v["messages"].as_array().unwrap();
        assert_eq!(wm[0], json!({"content": "CORE\nSELF", "role": "system"}));
        let roles: Vec<&str> = wm.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(
            roles,
            [
                "system",
                "user",
                "assistant",
                "tool",
                "user",
                "assistant",
                "user"
            ]
        );
        for m in wm.iter().filter(|m| m["role"] == "assistant") {
            assert!(m.get("reasoning_content").is_some(), "{m}");
        }
        assert_eq!(wm[2]["content"], "");
        assert_eq!(
            wm[2]["tool_calls"][0]["function"]["arguments"],
            "{\"command\": \"cd work/demo && cargo test 2>&1 | tail -20\"}"
        );
        assert_eq!(v["reasoning_effort"], "high");
        assert_eq!(v["thinking"], json!({"type": "enabled"}));
        for k in [
            "temperature",
            "tool_choice",
            "stream_options",
            "user_id",
            "top_p",
        ] {
            assert!(v.get(k).is_none(), "{k}");
        }
        let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/provider/testdata/golden/deepseek-six.json");
        if std::env::var("INIT_WP10_BLESS").as_deref() == Ok("1") {
            std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
            std::fs::write(&golden, &a).unwrap();
        }
        assert_eq!(
            a,
            std::fs::read(&golden).unwrap(),
            "re-bless with INIT_WP10_BLESS=1"
        );
    }

    /// The adapter's bytes equal WP12's context renderer's bytes for the same conversation, and the
    /// head-only request equals WP12's golden `head-request-deepseek-flash.json`.
    #[test]
    fn build_matches_context_renderer() {
        let golden = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/context/testdata/golden/head-request-deepseek-flash.json"),
        )
        .unwrap();
        let g: Value = serde_json::from_slice(&golden).unwrap();
        let sys = g["messages"][0]["content"].as_str().unwrap();
        let (core, self_) = sys.split_once('\n').unwrap();
        let ts = tools();
        let info = models::lookup("deepseek", "deepseek-flash").unwrap();
        let p = Prompt {
            head: crate::provider::PromptHead { core, self_ },
            tools: &ts,
            messages: &[],
            b3: None,
            max_tokens: g["max_tokens"].as_u64().unwrap() as u32,
            effort: Effort::High,
            session: "s",
        };
        assert_eq!(build_body(&p, info).unwrap(), golden);

        // A conversation rendered by both: context::render (from log blocks) and this adapter.
        use crate::context::project::{Msg, ToolRes};
        use crate::log::event::Block;
        let target = models::target(info, None);
        let r = crate::context::render::Renderer {
            target: &target,
            home: std::path::Path::new("/nonexistent"),
        };
        let ds_think = Block::Thinking {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            text: "plan".into(),
            native: None,
        };
        let msgs = vec![
            Msg::User {
                blocks: vec![Block::text("hi"), Block::text("there")],
                tokens: 1,
            },
            Msg::Assistant {
                event: 1,
                blocks: vec![
                    ds_think.clone(),
                    Block::text("ok"),
                    Block::ToolUse {
                        call_id: "call_00_x".into(),
                        name: "bash".into(),
                        input: ToolCall::parse_input("{\"command\":\"ls\"}"),
                        raw_arguments: Some("{\"command\": \"ls\"}".into()),
                    },
                ],
                tokens: 1,
            },
            Msg::ToolResults(vec![ToolRes {
                event: 2,
                call_id: "call_00_x".into(),
                name: "bash".into(),
                content: vec![Block::text("a"), Block::text("b")],
                is_error: false,
                tokens: 1,
            }]),
            Msg::Note {
                text: "[init:note] n".into(),
                tokens: 1,
            },
            Msg::Assistant {
                event: 3,
                blocks: vec![ds_think, Block::text("done")],
                tokens: 1,
            },
        ];
        let mut pieces: Vec<Vec<u8>> = vec![
            icj::to_vec(&crate::context::render::system_message(
                &crate::context::Head {
                    core: core.into(),
                    self_: self_.into(),
                    tools: ts.iter().map(|t| t.to_value()).collect(),
                    warnings: Vec::new(),
                },
            ))
            .unwrap(),
        ];
        for m in &msgs {
            pieces.push(r.msg_bytes(m).unwrap());
        }
        let refs: Vec<&[u8]> = pieces.iter().map(|p| p.as_slice()).collect();
        let fields = crate::context::render::head_fields(
            &crate::context::Head {
                core: core.into(),
                self_: self_.into(),
                tools: ts.iter().map(|t| t.to_value()).collect(),
                warnings: Vec::new(),
            },
            &target,
        )
        .unwrap();
        let ctx_bytes = crate::context::render::assemble(&fields, 65_536, &refs).unwrap();
        let origin = Origin {
            provider: "deepseek".into(),
            api: Api::ChatCompletions,
            model: "deepseek-flash".into(),
            response_id: None,
        };
        let converted = crate::provider::handoff::from_msgs(&msgs, None, &origin);
        let p = Prompt {
            messages: &converted,
            ..p
        };
        assert_eq!(
            String::from_utf8(build_body(&p, info).unwrap()).unwrap(),
            String::from_utf8(ctx_bytes).unwrap()
        );
    }

    /// The strict-mode round trip at the adapter level: a decoded DeepSeek message replayed through
    /// `build` carries its `reasoning_content` verbatim, and tool arguments byte for byte.
    #[test]
    fn reasoning_content_replayed_verbatim() {
        let (_, m) = run("split-args.sse");
        let m = m.unwrap();
        let info = models::lookup("deepseek", "deepseek-flash").unwrap();
        let ts = tools();
        let raws: Vec<String> = m
            .tool_calls()
            .map(|c| c.raw_arguments.clone().unwrap())
            .collect();
        let ids: Vec<String> = m.tool_calls().map(|c| c.id.clone()).collect();
        let msgs = vec![
            user("read a and b"),
            Message::Assistant(m),
            Message::ToolResults(ids.iter().map(|i| result(i, "x", false)).collect()),
        ];
        let body = build_body(&prompt(&msgs, &ts), info).unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        let a = &v["messages"][2];
        assert_eq!(a["reasoning_content"], "Read both files.");
        for (i, raw) in raws.iter().enumerate() {
            assert_eq!(a["tool_calls"][i]["function"]["arguments"], raw.as_str());
        }
    }

    #[test]
    fn flatten_foreign_turns() {
        let foreign = Message::Assistant(AssistantMessage {
            content: vec![
                ContentBlock::Text {
                    text: "Listing.".into(),
                },
                ContentBlock::ToolCall(ToolCall {
                    id: "toolu_01".into(),
                    name: "bash".into(),
                    input: ToolCall::parse_input("{\"command\":\"ls\"}"),
                    raw_arguments: None,
                }),
            ],
            stop: StopReason::ToolUse,
            usage: Usage::default(),
            origin: Origin {
                provider: "anthropic".into(),
                api: Api::Messages,
                model: "claude-opus-5-5".into(),
                response_id: None,
            },
        });
        let msgs = vec![
            user("list"),
            foreign,
            Message::ToolResults(vec![result("toolu_01", "a\nb", false)]),
            user("next"),
        ];
        let info = models::lookup("deepseek", "deepseek-flash").unwrap();
        let ts = tools();
        let body = build_body(&prompt(&msgs, &ts), info).unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert!(v["messages"][2].get("reasoning_content").is_none());
        let flat = flatten_foreign(&body).unwrap();
        let f: Value = serde_json::from_slice(&flat).unwrap();
        assert_eq!(
            f["messages"][2],
            json!({"content": "Listing.\n[called bash {\"command\":\"ls\"}]", "reasoning_content": "", "role": "assistant"})
        );
        assert_eq!(
            f["messages"][3],
            json!({"content": "[result of bash (toolu_01)]\na\nb", "role": "user"})
        );
        assert_eq!(flatten_foreign(&flat), None);
        assert_eq!(flatten_foreign(&flat), flatten_foreign(&flat)); // deterministic
        // The provider applies it once, on a 400 only.
        let p = DeepSeek::new(info);
        let req = p.wire(body);
        let e400 = ProviderError::Http {
            status: 400,
            kind: ErrorKind::InvalidRequest,
            message: String::new(),
            retry_after: None,
            request_id: None,
        };
        // The first fallback keeps the tool calls (`reasoning_content: ""`); the flatten comes second.
        let (r1, c1) = p.fallback(&req, &e400, &[]).unwrap();
        assert_eq!(c1, EMPTY_REASONING_CAUSE);
        let (r2, cause) = p.fallback(&req, &e400, &[c1]).unwrap();
        assert_eq!(cause, FLATTEN_CAUSE);
        assert_eq!(r2.body, flat);
        assert_eq!(p.fallback(&r1, &e400, &[c1]).unwrap().0.body, flat);
        assert!(p.fallback(&req, &e400, &[c1, cause]).is_none());
        let e500 = ProviderError::Http {
            status: 500,
            kind: ErrorKind::Server,
            message: String::new(),
            retry_after: None,
            request_id: None,
        };
        assert!(p.fallback(&req, &e500, &[]).is_none());
    }

    /// S-REAL V-DS-3 (live, 2026-10-20): DeepSeek refuses a foreign tool turn without
    /// `reasoning_content` (400 "The `reasoning_content` in the thinking mode must be passed back"),
    /// accepts the same turn with `"reasoning_content": ""`, and the model then uses the foreign
    /// result as given; with the flattened history it re-ran the tool. So the first fallback adds the
    /// empty field and keeps every other byte; the flatten is the second.
    #[test]
    fn fallback_adds_empty_reasoning_before_flattening() {
        let foreign = Message::Assistant(AssistantMessage {
            content: vec![ContentBlock::ToolCall(ToolCall {
                id: "toolu_01VdsForeignCall0000000001".into(),
                name: "bash".into(),
                input: ToolCall::parse_input("{\"command\": \"echo vds-ok\"}"),
                raw_arguments: None,
            })],
            stop: StopReason::ToolUse,
            usage: Usage::default(),
            origin: Origin {
                provider: "anthropic".into(),
                api: Api::Messages,
                model: "claude-opus-5-5".into(),
                response_id: None,
            },
        });
        let msgs = vec![
            user("run it"),
            foreign,
            Message::ToolResults(vec![result(
                "toolu_01VdsForeignCall0000000001",
                "vds-ok",
                false,
            )]),
        ];
        let info = models::lookup("deepseek", "deepseek-flash").unwrap();
        let ts = tools();
        let body = build_body(&prompt(&msgs, &ts), info).unwrap();
        let p = DeepSeek::new(info);
        let e400 = ProviderError::Http {
            status: 400,
            kind: p.classify(
                400,
                &[],
                None,
                "The `reasoning_content` in the thinking mode must be passed back to the API. (request_id: 468b8a3f-b382-4e41-a617-316f3a3582e4)",
            ),
            message: String::new(),
            retry_after: None,
            request_id: None,
        };
        let (r1, c1) = p.fallback(&p.wire(body.clone()), &e400, &[]).unwrap();
        assert_eq!(c1, EMPTY_REASONING_CAUSE);
        let mut v: Value = serde_json::from_slice(&r1.body).unwrap();
        let a = &v["messages"][2];
        assert_eq!(a["reasoning_content"], "");
        assert_eq!(a["tool_calls"][0]["id"], "toolu_01VdsForeignCall0000000001");
        assert_eq!(v["messages"][3]["role"], "tool");
        // Every other byte is kept: without the added field the request is the original.
        v["messages"][2]
            .as_object_mut()
            .unwrap()
            .remove("reasoning_content");
        assert_eq!(icj::to_vec(&v).unwrap(), body);
        assert_eq!(add_empty_reasoning(&r1.body), None);
        // A second 400 flattens; nothing after that.
        let (r2, c2) = p.fallback(&r1, &e400, &[c1]).unwrap();
        assert_eq!(
            (c2, r2.body.clone()),
            (FLATTEN_CAUSE, flatten_foreign(&body).unwrap())
        );
        assert!(p.fallback(&r2, &e400, &[c1, c2]).is_none());
    }

    /// Live DeepSeek streams recorded by S-REAL (V-DS-1/2/4/5, 2026-10-20, `deepseek-flash`,
    /// thinking on): tests/real/real.sh real run vds, raw bytes from the kernel proxy.
    #[test]
    fn real_streams_decode() {
        // A tool call whose reasoning came back EMPTY in thinking mode: no Thinking block.
        let (_, m) = run("real-tool-call-empty-reasoning.sse");
        let m = m.unwrap();
        assert_eq!(m.stop, StopReason::ToolUse);
        assert!(thinking(&m).is_none());
        assert_eq!(
            calls(&m),
            vec![(
                "call_00_ET_cs5UP1KydOzcmgVtqyIn0301".to_string(),
                "bash".to_string(),
                "{\"command\": \"echo vds-ok\"}".to_string()
            )]
        );
        assert!(m.tool_calls().all(|c| c.args_error().is_none()));
        let u = |input, cache_read, output, reasoning| Usage {
            input,
            cache_read,
            cache_write: 0,
            output,
            reasoning,
        };
        assert_eq!(m.usage, u(3931, 1280, 41, 0));
        // With reasoning.
        let (_, m) = run("real-reasoning-tool-call.sse");
        let m = m.unwrap();
        assert_eq!(m.stop, StopReason::ToolUse);
        assert_eq!(thinking(&m).map(|t| t.chars().count()), Some(64));
        assert_eq!(m.usage, u(3954, 3712, 71, 32));
        // The second request of a tool round trip: text, a 96% cache hit.
        let (evs, m) = run("real-text-cache-hit.sse");
        let m = m.unwrap();
        assert_eq!(m.text(), "vds-ok");
        assert_eq!(m.stop, StopReason::EndTurn);
        assert_eq!(m.usage, u(4002, 3840, 5, 0));
        assert_eq!(m.usage.hit_permille(), 959);
        assert!(evs.iter().any(|e| matches!(e, StreamEvent::Usage(_))));
    }

    #[test]
    fn classify_errors() {
        let p = DeepSeek::new(models::lookup("deepseek", "deepseek-flash").unwrap());
        let k = |s, m: &str| p.classify(s, &[], None, m);
        assert_eq!(k(400, "Invalid format"), ErrorKind::InvalidRequest);
        // V-DS-6 (live, 2026-10-20): the parameter-error text is not an overflow.
        assert_eq!(
            k(
                400,
                "Invalid max_tokens value, the valid range of max_tokens is [1, 393216] (request_id: 5f0f8aa8-d842-4339-b65e-a6c76f08d92d)"
            ),
            ErrorKind::InvalidRequest
        );
        assert_eq!(
            k(400, "This model's maximum context length is 1000000 tokens"),
            ErrorKind::ContextOverflow
        );
        assert_eq!(k(422, "bad param"), ErrorKind::InvalidRequest);
        assert_eq!(k(401, ""), ErrorKind::Auth);
        assert_eq!(k(402, "Insufficient Balance"), ErrorKind::Billing);
        assert_eq!(k(429, ""), ErrorKind::RateLimit);
        assert_eq!(k(500, ""), ErrorKind::Server);
        assert_eq!(k(503, ""), ErrorKind::Overloaded);
    }
}
