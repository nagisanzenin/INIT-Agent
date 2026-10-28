//! OpenAI Responses adapter (body/provider.md §3.8.3): stateless request bytes with the head in a
//! leading developer message (explicit breakpoint on `self_`), encrypted reasoning replay, stream
//! decoder keyed by `output_index`, stop and usage mapping, error classification.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::http::HttpRequest;
use super::sse::SseEvent;
use super::{Decoder, Prompt, Provider, base_kind, effort_wire, handoff, jstr, ju64};
use crate::log::icj;
use crate::model::{
    Api, AssistantMessage, ContentBlock, ErrorKind, Message, ModelInfo, Origin, ProviderError,
    StopReason, StreamEvent, Thinking, ToolCall, Usage,
};

pub const PATH: &str = "/openai/v1/responses";

pub struct OpenAi {
    info: &'static ModelInfo,
}

impl OpenAi {
    pub fn new(info: &'static ModelInfo) -> Self {
        OpenAi { info }
    }
}

fn input_parts(content: &[ContentBlock]) -> Vec<Value> {
    content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(json!({"text": text, "type": "input_text"})),
            ContentBlock::Image { media_type, data_b64 } => {
                Some(json!({"image_url": format!("data:{media_type};base64,{data_b64}"), "type": "input_image"}))
            }
            _ => None,
        })
        .collect()
}

fn message(role: &str, parts: Vec<Value>) -> Value {
    json!({"content": parts, "role": role, "type": "message"})
}

/// One normalized message → Responses input items.
pub fn input_items(m: &Message, info: &ModelInfo) -> Vec<Value> {
    match m {
        Message::User { content } => vec![message("user", input_parts(content))],
        Message::Note { text } => vec![message(
            "developer",
            vec![json!({"text": text, "type": "input_text"})],
        )],
        Message::ToolResults(rs) => rs
            .iter()
            .map(|r| {
                let has_image = r
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Image { .. }));
                let output = if has_image {
                    Value::Array(input_parts(&r.content))
                } else {
                    let t: Vec<&str> = r
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    Value::String(t.join("\n"))
                };
                json!({"call_id": r.call_id, "output": output, "type": "function_call_output"})
            })
            .collect(),
        Message::Assistant(a) => {
            let mut out = Vec::new();
            let mut texts: Vec<Value> = Vec::new();
            let flush = |texts: &mut Vec<Value>, out: &mut Vec<Value>| {
                if !texts.is_empty() {
                    out.push(message("assistant", std::mem::take(texts)));
                }
            };
            for b in &a.content {
                match b {
                    ContentBlock::Text { text } if !text.is_empty() => {
                        texts.push(json!({"text": text, "type": "output_text"}));
                    }
                    ContentBlock::Thinking(t) if t.provider == info.provider => {
                        if let Some(n) = &t.native {
                            flush(&mut texts, &mut out);
                            out.push(n.clone());
                        }
                    }
                    ContentBlock::ToolCall(c) => {
                        flush(&mut texts, &mut out);
                        let args = match (&c.raw_arguments, info.cache.replay_raw_arguments) {
                            (Some(r), true) => r.clone(),
                            _ => {
                                icj::to_string(&Value::Object(c.input.clone())).unwrap_or_default()
                            }
                        };
                        out.push(json!({"arguments": args, "call_id": c.id, "name": c.name, "type": "function_call"}));
                    }
                    ContentBlock::Opaque {
                        api: Api::Responses,
                        raw,
                    } => {
                        flush(&mut texts, &mut out);
                        out.push(raw.clone());
                    }
                    _ => {}
                }
            }
            flush(&mut texts, &mut out);
            if out.is_empty() {
                out.push(message(
                    "assistant",
                    vec![json!({"text": handoff::NO_VISIBLE_OUTPUT, "type": "output_text"})],
                ));
            }
            out
        }
    }
}

pub fn build_body(p: &Prompt, info: &ModelInfo) -> Result<Vec<u8>, ProviderError> {
    let msgs = handoff::filter(p.messages, info);
    let mut input = vec![message(
        "developer",
        vec![
            json!({"text": p.head.core, "type": "input_text"}),
            json!({"prompt_cache_breakpoint": {"mode": "explicit"}, "text": p.head.self_, "type": "input_text"}),
        ],
    )];
    for m in &msgs {
        input.extend(input_items(m, info));
    }
    let tools: Vec<Value> = p
        .tools
        .iter()
        .map(|t| json!({"description": t.description, "name": t.name, "parameters": t.input_schema, "strict": false, "type": "function"}))
        .collect();
    let mut body = json!({
        "include": ["reasoning.encrypted_content"],
        "input": input,
        "max_output_tokens": super::models::request_tokens(info.provider, info.id, p.max_tokens),
        "model": info.id,
        "parallel_tool_calls": true,
        "prompt_cache_key": p.session,
        "reasoning": {"effort": effort_wire(Api::Responses, p.effort)},
        "store": false,
        "stream": true,
        "tools": tools,
    });
    if !super::models::native_extensions(info) {
        let fields = body.as_object_mut().expect("request object");
        for key in ["include", "reasoning", "prompt_cache_key"] { fields.remove(key); }
        for item in fields.get_mut("input").and_then(Value::as_array_mut).into_iter().flatten() {
            for part in item.get_mut("content").and_then(Value::as_array_mut).into_iter().flatten() {
                if let Some(part) = part.as_object_mut() { part.remove("prompt_cache_breakpoint"); }
            }
        }
    }
    icj::to_vec(&body).map_err(|e| ProviderError::Protocol(format!("request: {e}")))
}

const BILLING_CODES: [&str; 4] = [
    "credit_balance_exhausted",
    "organization_spend_limit_exceeded",
    "project_spend_limit_exceeded",
    "organization_usage_limit_exceeded",
];

impl Provider for OpenAi {
    fn info(&self) -> &ModelInfo {
        self.info
    }

    fn build(&self, p: &Prompt) -> Result<HttpRequest, ProviderError> {
        Ok(self.wire(build_body(p, self.info)?))
    }

    fn wire(&self, body: Vec<u8>) -> HttpRequest {
        HttpRequest {
            path: if self.info.provider == "openai" { PATH.into() } else { format!("/{}/v1/responses", self.info.provider) },
            headers: Vec::new(),
            body,
        }
    }

    fn decoder(&self) -> Box<dyn Decoder> {
        let mut decoder = OaDecoder::new(self.info.id);
        decoder.provider = self.info.provider.into();
        Box::new(decoder)
    }

    fn classify(
        &self,
        status: u16,
        _headers: &[(String, String)],
        body: Option<&Value>,
        _message: &str,
    ) -> ErrorKind {
        let code = body.and_then(|v| jstr(v, "/error/code")).unwrap_or("");
        match status {
            400 if code == "context_length_exceeded" => ErrorKind::ContextOverflow,
            429 if BILLING_CODES.contains(&code) => ErrorKind::Billing,
            s => base_kind(s),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Stream decoder
// ---------------------------------------------------------------------------------------------

enum Slot {
    Text(String),
    Thinking {
        text: String,
        native: Option<Value>,
    },
    Call {
        id: String,
        name: String,
        json: String,
        done: bool,
    },
    Opaque(Value),
}

pub struct OaDecoder {
    provider: String,
    model: String,
    id: Option<String>,
    slots: BTreeMap<u64, Slot>,
    refused: bool,
    stop: Option<StopReason>,
    usage: Usage,
    done: bool,
}

/// Responses `usage` → `Usage`.
pub fn map_usage(u: &Value) -> Usage {
    Usage {
        input: ju64(u, "/input_tokens"),
        cache_read: ju64(u, "/input_tokens_details/cached_tokens"),
        cache_write: ju64(u, "/input_tokens_details/cache_write_tokens"),
        output: ju64(u, "/output_tokens"),
        reasoning: ju64(u, "/output_tokens_details/reasoning_tokens"),
    }
}

fn proto(m: impl Into<String>) -> ProviderError {
    ProviderError::Protocol(m.into())
}

/// An `error` / `response.failed` code → kind.
pub fn error_kind(code: &str) -> ErrorKind {
    match code {
        "server_error" => ErrorKind::Server,
        "server_is_overloaded" => ErrorKind::Overloaded,
        "slow_down" | "rate_limit_exceeded" => ErrorKind::RateLimit,
        "context_length_exceeded" => ErrorKind::ContextOverflow,
        "invalid_request_error" | "previous_response_not_found" => ErrorKind::InvalidRequest,
        c if BILLING_CODES.contains(&c) => ErrorKind::Billing,
        _ => ErrorKind::Other,
    }
}

impl OaDecoder {
    pub fn new(model: &str) -> Self {
        OaDecoder {
            provider: "openai".into(),
            model: model.into(),
            id: None,
            slots: BTreeMap::new(),
            refused: false,
            stop: None,
            usage: Usage::default(),
            done: false,
        }
    }

    fn origin(&self) -> Origin {
        Origin {
            provider: self.provider.clone(),
            api: Api::Responses,
            model: self.model.clone(),
            response_id: self.id.clone(),
        }
    }

    fn call(id: &str, name: &str, json: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            input: ToolCall::parse_input(json),
            raw_arguments: Some(json.into()),
        }
    }

    fn blocks(&self, complete: bool) -> Vec<ContentBlock> {
        let mut out = Vec::new();
        for s in self.slots.values() {
            match s {
                Slot::Text(t) if !t.is_empty() => out.push(ContentBlock::Text { text: t.clone() }),
                Slot::Text(_) => {}
                Slot::Thinking { text, native } => {
                    if native.is_some() || !text.is_empty() {
                        out.push(ContentBlock::Thinking(Thinking {
                            provider: self.provider.clone(),
                            model: self.model.clone(),
                            text: text.clone(),
                            native: native.clone(),
                        }))
                    }
                }
                Slot::Call {
                    id,
                    name,
                    json,
                    done,
                } if *done || complete => {
                    out.push(ContentBlock::ToolCall(Self::call(id, name, json)))
                }
                Slot::Call { .. } => {}
                Slot::Opaque(v) => out.push(ContentBlock::Opaque {
                    api: Api::Responses,
                    raw: v.clone(),
                }),
            }
        }
        out
    }

    fn slot_text(&mut self, idx: u64) -> &mut String {
        let s = self
            .slots
            .entry(idx)
            .or_insert_with(|| Slot::Text(String::new()));
        if !matches!(s, Slot::Text(_)) {
            *s = Slot::Text(String::new());
        }
        match s {
            Slot::Text(t) => t,
            _ => unreachable!(),
        }
    }
}

impl Decoder for OaDecoder {
    fn feed(&mut self, ev: &SseEvent, out: &mut Vec<StreamEvent>) -> Result<(), ProviderError> {
        if self.done {
            return Err(proto("event after the terminal response event"));
        }
        let v: Value =
            serde_json::from_str(&ev.data).map_err(|e| proto(format!("event JSON: {e}")))?;
        let ty = v
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(ev.event.as_str())
            .to_string();
        let idx = || {
            v.get("output_index")
                .and_then(Value::as_u64)
                .ok_or_else(|| proto(format!("{ty} without output_index")))
        };
        let delta = || jstr(&v, "/delta").unwrap_or("").to_string();
        match ty.as_str() {
            "response.created" | "response.in_progress" | "response.queued" => {
                if let Some(id) = jstr(&v, "/response/id") {
                    self.id = Some(id.into());
                }
            }
            "response.output_item.added" => {
                let i = idx()?;
                let item = v
                    .get("item")
                    .ok_or_else(|| proto("output_item.added without item"))?;
                let slot = match item.get("type").and_then(Value::as_str) {
                    Some("message") => Slot::Text(String::new()),
                    Some("reasoning") => Slot::Thinking {
                        text: String::new(),
                        native: None,
                    },
                    Some("function_call") => Slot::Call {
                        id: jstr(item, "/call_id").unwrap_or("").into(),
                        name: jstr(item, "/name").unwrap_or("").into(),
                        json: jstr(item, "/arguments").unwrap_or("").into(),
                        done: false,
                    },
                    _ => Slot::Opaque(item.clone()),
                };
                self.slots.insert(i, slot);
            }
            "response.output_text.delta" => {
                let (i, d) = (idx()?, delta());
                self.slot_text(i).push_str(&d);
                if !d.is_empty() {
                    out.push(StreamEvent::TextDelta(d));
                }
            }
            "response.refusal.delta" => {
                let (i, d) = (idx()?, delta());
                self.slot_text(i).push_str(&d);
                self.refused = true;
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let (i, d) = (idx()?, delta());
                if let Some(Slot::Thinking { text, .. }) = self.slots.get_mut(&i) {
                    text.push_str(&d);
                }
                if !d.is_empty() {
                    out.push(StreamEvent::ThinkingDelta(d));
                }
            }
            "response.function_call_arguments.delta" => {
                let (i, d) = (idx()?, delta());
                if let Some(Slot::Call { json, .. }) = self.slots.get_mut(&i) {
                    json.push_str(&d);
                }
            }
            "response.function_call_arguments.done" => {
                let i = idx()?;
                if let (Some(Slot::Call { json, .. }), Some(a)) =
                    (self.slots.get_mut(&i), jstr(&v, "/arguments"))
                {
                    *json = a.into();
                }
            }
            "response.output_item.done" => {
                let i = idx()?;
                let item = v
                    .get("item")
                    .ok_or_else(|| proto("output_item.done without item"))?;
                let slot = match item.get("type").and_then(Value::as_str) {
                    Some("message") => {
                        let mut t = String::new();
                        for part in item
                            .get("content")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            match part.get("type").and_then(Value::as_str) {
                                Some("output_text") => {
                                    t.push_str(jstr(part, "/text").unwrap_or(""))
                                }
                                Some("refusal") => {
                                    t.push_str(jstr(part, "/refusal").unwrap_or(""));
                                    self.refused = true;
                                }
                                _ => {}
                            }
                        }
                        Slot::Text(t)
                    }
                    Some("reasoning") => {
                        let text: Vec<&str> = item
                            .get("summary")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|s| jstr(s, "/text"))
                            .collect();
                        Slot::Thinking {
                            text: text.join("\n"),
                            native: Some(item.clone()),
                        }
                    }
                    Some("function_call") => {
                        let (id, name, json) = (
                            jstr(item, "/call_id").unwrap_or(""),
                            jstr(item, "/name").unwrap_or(""),
                            jstr(item, "/arguments").unwrap_or(""),
                        );
                        if id.is_empty() || name.is_empty() {
                            return Err(proto("function_call without call_id or name"));
                        }
                        out.push(StreamEvent::ToolCallReady(Self::call(id, name, json)));
                        Slot::Call {
                            id: id.into(),
                            name: name.into(),
                            json: json.into(),
                            done: true,
                        }
                    }
                    _ => Slot::Opaque(item.clone()),
                };
                self.slots.insert(i, slot);
            }
            "response.completed" => {
                if let Some(u) = v.pointer("/response/usage").filter(|u| u.is_object()) {
                    self.usage = map_usage(u);
                    out.push(StreamEvent::Usage(self.usage));
                }
                let calls = self.slots.values().any(|s| matches!(s, Slot::Call { .. }));
                self.stop = Some(if calls {
                    StopReason::ToolUse
                } else if self.refused {
                    StopReason::Refusal { category: None }
                } else {
                    StopReason::EndTurn
                });
                self.done = true;
            }
            "response.incomplete" => {
                if let Some(u) = v.pointer("/response/usage").filter(|u| u.is_object()) {
                    self.usage = map_usage(u);
                    out.push(StreamEvent::Usage(self.usage));
                }
                let reason = jstr(&v, "/response/incomplete_details/reason").unwrap_or("");
                self.stop = Some(match reason {
                    "max_output_tokens" => StopReason::MaxTokens,
                    "content_filter" => StopReason::Refusal {
                        category: Some("content_filter".into()),
                    },
                    other => {
                        return Err(ProviderError::Stream {
                            kind: ErrorKind::Other,
                            message: format!("response incomplete: {other}"),
                        });
                    }
                });
                self.done = true;
            }
            "response.failed" => {
                let code = jstr(&v, "/response/error/code").unwrap_or("");
                let message = jstr(&v, "/response/error/message")
                    .unwrap_or("response failed")
                    .to_string();
                let kind = match error_kind(code) {
                    k @ (ErrorKind::Server | ErrorKind::RateLimit) => k,
                    _ => ErrorKind::Other,
                };
                return Err(ProviderError::Stream { kind, message });
            }
            "error" => {
                let e = v.get("error").filter(|e| e.is_object()).unwrap_or(&v);
                let code = jstr(e, "/code").unwrap_or("");
                let message = jstr(e, "/message").unwrap_or("stream error").to_string();
                return Err(ProviderError::Stream {
                    kind: error_kind(code),
                    message,
                });
            }
            _ => {} // other response.* events
        }
        Ok(())
    }

    fn finished(&self) -> bool {
        self.done
    }

    fn finish(&mut self) -> Result<AssistantMessage, ProviderError> {
        let stop = self
            .stop
            .clone()
            .filter(|_| self.done)
            .ok_or(ProviderError::Truncated)?;
        Ok(AssistantMessage {
            content: self.blocks(true),
            stop,
            usage: self.usage,
            origin: self.origin(),
        })
    }

    fn partial(&mut self) -> AssistantMessage {
        AssistantMessage {
            content: self.blocks(false),
            stop: StopReason::Aborted,
            usage: self.usage,
            origin: self.origin(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Effort;
    use crate::provider::deepseek::tests::{ds_assistant, result, tools, user};
    use crate::provider::testutil::{decode, fixture};
    use crate::provider::{PromptHead, models};

    fn dec() -> Box<dyn Decoder> {
        Box::new(OaDecoder::new("gpt-6-astra"))
    }

    fn sse(events: &[Value]) -> Vec<u8> {
        events
            .iter()
            .map(|e| format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e))
            .collect::<String>()
            .into_bytes()
    }

    /// §8 item 8: output_item.added function_call, argument deltas, output_item.done.
    #[test]
    fn custom_route_keeps_provider_identity() {
        let mut info = models::lookup("openai", "gpt-6-astra").unwrap().clone();
        info.provider = "custom_proxy";
        let info = Box::leak(Box::new(info));
        let adapter = OpenAi::new(info);
        assert_eq!(adapter.wire(b"{}".to_vec()).path, "/custom_proxy/v1/responses");
        let (_, result) = decode(adapter.decoder(), &fixture("openai/text-tool.sse"));
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
        assert!(payload.get("reasoning").is_none());
        assert!(payload.get("max_output_tokens").is_some());
        assert!(payload.get("messages").is_none());
        let rendered = String::from_utf8(request.body).unwrap();
        for item in native {
            assert!(rendered.contains(&icj::to_string(&item).unwrap()), "native reasoning must survive alias replay");
        }
    }

    #[test]
    fn decode_fixture_reasoning_text_call() {
        let (evs, m) = decode(dec(), &fixture("openai/text-tool.sse"));
        let m = m.unwrap();
        assert_eq!(m.stop, StopReason::ToolUse);
        match &m.content[0] {
            ContentBlock::Thinking(t) => {
                assert_eq!(t.text, "plan");
                assert_eq!(
                    t.native.as_ref().unwrap()["encrypted_content"],
                    "mock-enc-0"
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(m.text(), "Listing now.");
        let c = m.tool_calls().next().unwrap();
        assert_eq!(
            (c.id.as_str(), c.raw_arguments.as_deref()),
            ("call_mock0_0", Some("{\"command\":\"ls\"}"))
        );
        assert_eq!(
            m.usage,
            Usage {
                input: 61,
                cache_read: 0,
                cache_write: 0,
                output: 9,
                reasoning: 1
            }
        );
        assert_eq!(m.origin.response_id.as_deref(), Some("resp_mock0"));
        assert_eq!(
            evs.iter()
                .filter(|e| matches!(e, StreamEvent::ToolCallReady(_)))
                .count(),
            1
        );
        let (_, m) = decode(dec(), &fixture("openai/max-tokens.sse"));
        assert_eq!(m.unwrap().stop, StopReason::MaxTokens);
    }

    #[test]
    fn decode_failed_error_incomplete_refusal() {
        let failed = sse(&[
            json!({"type": "response.created", "response": {"id": "resp_1"}}),
            json!({"type": "response.failed", "response": {"error": {"code": "server_error", "message": "boom"}}}),
        ]);
        let e = decode(dec(), &failed).1.unwrap_err();
        assert!(
            matches!(
                e,
                ProviderError::Stream {
                    kind: ErrorKind::Server,
                    ..
                }
            ) && e.retryable()
        );
        let err = sse(&[
            json!({"type": "error", "code": "rate_limit_exceeded", "message": "slow", "param": null}),
        ]);
        assert!(decode(dec(), &err).1.unwrap_err().retryable());
        let err = sse(&[
            json!({"type": "error", "code": "invalid_prompt", "message": "no", "param": null}),
        ]);
        assert!(!decode(dec(), &err).1.unwrap_err().retryable());
        let cf = sse(&[
            json!({"type": "response.incomplete", "response": {"incomplete_details": {"reason": "content_filter"}}}),
        ]);
        assert_eq!(
            decode(dec(), &cf).1.unwrap().stop,
            StopReason::Refusal {
                category: Some("content_filter".into())
            }
        );
        let refusal = sse(&[
            json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "content": []}}),
            json!({"type": "response.refusal.delta", "output_index": 0, "delta": "No."}),
            json!({"type": "response.completed", "response": {"usage": {"input_tokens": 5, "input_tokens_details": {"cached_tokens": 2, "cache_write_tokens": 3}, "output_tokens": 1}}}),
        ]);
        let m = decode(dec(), &refusal).1.unwrap();
        assert_eq!(m.stop, StopReason::Refusal { category: None });
        assert_eq!(
            m.usage,
            Usage {
                input: 5,
                cache_read: 2,
                cache_write: 3,
                output: 1,
                reasoning: 0
            }
        );
        assert!(matches!(
            decode(
                dec(),
                &sse(&[json!({"type": "response.created", "response": {"id": "r"}})])
            )
            .1,
            Err(ProviderError::Truncated)
        ));
    }

    #[test]
    fn build_shape() {
        let info = models::lookup("openai", "gpt-6-astra").unwrap();
        let ts = tools();
        let (_, m) = decode(dec(), &fixture("openai/text-tool.sse"));
        let m = m.unwrap();
        let msgs = vec![
            user("Fix the failing test in work/demo."),
            ds_assistant("deepseek thought", "from deepseek", &[]),
            Message::Assistant(m),
            Message::ToolResults(vec![result("call_mock0_0", "a\nb", false)]),
            Message::Note {
                text: "[init:signal] n".into(),
            },
        ];
        let p = Prompt {
            head: PromptHead {
                core: "CORE",
                self_: "SELF",
            },
            tools: &ts,
            messages: &msgs,
            b3: None,
            max_tokens: 64_000,
            effort: Effort::XHigh,
            session: "sid-1",
        };
        let req = OpenAi::new(info).build(&p).unwrap();
        assert_eq!(req.path, PATH);
        assert!(req.headers.is_empty());
        let v: Value = serde_json::from_slice(&req.body).unwrap();
        assert!(icj::is_canonical(&req.body));
        assert_eq!(v["store"], false);
        assert_eq!(v["prompt_cache_key"], "sid-1");
        assert_eq!(v["reasoning"], json!({"effort": "xhigh"}));
        assert_eq!(v["include"], json!(["reasoning.encrypted_content"]));
        assert!(v.get("previous_response_id").is_none() && v.get("instructions").is_none());
        assert_eq!(v["tools"][0]["strict"], false);
        let input = v["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "developer");
        assert_eq!(
            input[0]["content"][1]["prompt_cache_breakpoint"],
            json!({"mode": "explicit"})
        );
        let types: Vec<String> = input
            .iter()
            .map(|i| {
                format!(
                    "{}:{}",
                    i["type"].as_str().unwrap(),
                    i["role"].as_str().unwrap_or("")
                )
            })
            .collect();
        assert_eq!(
            types,
            [
                "message:developer",
                "message:user",
                "message:assistant",
                "reasoning:",
                "message:assistant",
                "function_call:",
                "function_call_output:",
                "message:developer"
            ]
        );
        // DeepSeek reasoning is not replayed to OpenAI; the encrypted item is, verbatim.
        assert!(
            !String::from_utf8(req.body.clone())
                .unwrap()
                .contains("deepseek thought")
        );
        assert_eq!(input[3]["encrypted_content"], "mock-enc-0");
        assert_eq!(input[5]["arguments"], "{\"command\":\"ls\"}");
        assert_eq!(input[6]["output"], "a\nb");
    }

    #[test]
    fn classify_errors() {
        let p = OpenAi::new(models::lookup("openai", "gpt-6.1-sol").unwrap());
        let b = |c: &str| json!({"error": {"code": c, "message": "m"}});
        assert_eq!(
            p.classify(400, &[], Some(&b("context_length_exceeded")), ""),
            ErrorKind::ContextOverflow
        );
        assert_eq!(
            p.classify(400, &[], Some(&b("x")), ""),
            ErrorKind::InvalidRequest
        );
        assert_eq!(
            p.classify(429, &[], Some(&b("rate_limit_exceeded")), ""),
            ErrorKind::RateLimit
        );
        for c in BILLING_CODES {
            assert_eq!(p.classify(429, &[], Some(&b(c)), ""), ErrorKind::Billing);
        }
        assert_eq!(p.classify(503, &[], None, ""), ErrorKind::Overloaded);
        assert_eq!(p.classify(500, &[], None, ""), ErrorKind::Server);
    }

    /// v0.5.0 L3, offline: `response.completed`'s `input_tokens_details.cached_tokens` and `.cache_write_tokens`.
    /// The log's `input_tokens` is the uncached rest (input − read − write). The Root meters the same bytes with its
    /// own tap: root/crates/init-root/src/providers/tests.rs `cache_writes_bill_at_the_charter_write_price`.
    #[test]
    fn cache_write_then_read_usage_reaches_the_log() {
        let (_, m1) = decode(dec(), &fixture("openai/cache-write.sse"));
        let m1 = m1.unwrap();
        assert_eq!(m1.stop, StopReason::ToolUse);
        assert_eq!(m1.usage, Usage { input: 21_510, cache_read: 0, cache_write: 21_504, output: 95, reasoning: 64 });
        let (_, m2) = decode(dec(), &fixture("openai/cache-read.sse"));
        let m2 = m2.unwrap();
        assert_eq!(m2.stop, StopReason::EndTurn);
        assert_eq!(m2.usage, Usage { input: 22_150, cache_read: 21_504, cache_write: 640, output: 140, reasoning: 96 });
        assert_eq!(m2.usage.hit_permille(), 970);
        let l1 = m1.usage.to_log(None);
        assert_eq!((l1.input_tokens, l1.cache_read_tokens, l1.cache_write_tokens, l1.output_tokens), (6, 0, 21_504, 95));
        let l2 = m2.usage.to_log(None);
        assert_eq!((l2.input_tokens, l2.cache_read_tokens, l2.cache_write_tokens, l2.reasoning_tokens), (6, 21_504, 640, Some(96)));
        assert_eq!(l2.prompt_total(), m2.usage.input);
    }

    /// Every request of a session carries `prompt_cache_key` = the session id (report "Bước tiếp theo cho INIT":
    /// openai.rs:152), the head is one developer message with the explicit breakpoint on its `self_` part, and
    /// request 2's input starts with request 1's items unchanged, so the cached prefix can match.
    #[test]
    fn prompt_cache_key_and_a_stable_prefix_across_requests() {
        let info = models::lookup("openai", "gpt-6.1-sol").unwrap();
        let ts = tools();
        let first = vec![user("Run the qualify command.")];
        let (_, m1) = decode(dec(), &fixture("openai/cache-write.sse"));
        let mut second = first.clone();
        second.push(Message::Assistant(m1.unwrap()));
        second.push(Message::ToolResults(vec![result("call_q1", "3f1c0b9e  -", false)]));
        let body = |msgs: &[Message]| -> Value {
            let p = Prompt {
                head: PromptHead { core: "CORE", self_: "SELF" }, tools: &ts, messages: msgs, b3: None,
                max_tokens: 64_000, effort: Effort::High, session: "sess-q",
            };
            serde_json::from_slice(&OpenAi::new(info).build(&p).unwrap().body).unwrap()
        };
        let (v1, v2) = (body(&first), body(&second));
        assert_eq!((&v1["prompt_cache_key"], &v2["prompt_cache_key"]), (&json!("sess-q"), &json!("sess-q")));
        assert_eq!(v1["input"][0]["role"], "developer");
        assert!(v1["input"][0]["content"][0].get("prompt_cache_breakpoint").is_none());
        assert_eq!(v1["input"][0]["content"][1]["prompt_cache_breakpoint"], json!({"mode": "explicit"}));
        for k in ["tools", "model", "reasoning", "include", "store", "max_output_tokens"] {
            assert_eq!(v1[k], v2[k], "{k} differs between consecutive requests");
        }
        let (i1, i2) = (v1["input"].as_array().unwrap(), v2["input"].as_array().unwrap());
        assert!(i2.len() > i1.len());
        assert_eq!(&i2[..i1.len()], &i1[..], "request 2 must start with request 1's input items");
    }
}
