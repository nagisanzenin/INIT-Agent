//! Reference reassembly of a streamed reply from its SSE events, per wire family. The DeepSeek one
//! follows body/provider.md §3.8.1's decoder rules; the Anthropic and OpenAI ones cover what the
//! stub renderers emit. Used by the self-tests (and available to scenario tests).

use super::sse::SseEvent;
use super::wire::Family;
use serde_json::Value;
use std::collections::BTreeMap;

/// One reassembled tool call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ACall {
    pub id: String,
    pub name: String,
    pub args: String,
}

/// A reassembled reply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Assembled {
    pub id: Option<String>,
    pub model: Option<String>,
    pub reasoning: String,
    pub text: String,
    pub calls: Vec<ACall>,
    /// `end_turn` | `tool_use` | `max_tokens` | `refusal` | `stream:<kind>` (a retryable stream error).
    pub stop: String,
    /// The provider's own finish value (DeepSeek `finish_reason`, Anthropic `stop_reason`, OpenAI status).
    pub finish_raw: String,
    pub usage: Option<Value>,
    /// Anthropic signature / OpenAI `encrypted_content`.
    pub signature: Option<String>,
}

/// Reassemble `events` of `family`.
pub fn assemble(family: Family, events: &[SseEvent]) -> Result<Assembled, String> {
    match family {
        Family::Chat => deepseek(events),
        Family::Messages => anthropic(events),
        Family::Responses => openai(events),
    }
}

fn parse(ev: &SseEvent) -> Result<Value, String> {
    serde_json::from_str(&ev.data).map_err(|e| format!("bad event JSON: {e}"))
}

#[derive(Default)]
struct Partial {
    id: String,
    name: String,
    args: String,
}

/// DeepSeek Chat Completions.
pub fn deepseek(events: &[SseEvent]) -> Result<Assembled, String> {
    let mut a = Assembled::default();
    let mut calls: BTreeMap<u64, Partial> = BTreeMap::new();
    let mut finish: Option<String> = None;
    let mut done = false;
    for ev in events {
        if done {
            return Err("event after [DONE]".into());
        }
        if ev.data == "[DONE]" {
            done = true;
            continue;
        }
        let c = parse(ev)?;
        if a.id.is_none() {
            a.id = c.get("id").and_then(Value::as_str).map(String::from);
            a.model = c.get("model").and_then(Value::as_str).map(String::from);
        }
        let choices = c
            .get("choices")
            .and_then(Value::as_array)
            .ok_or("chunk without choices")?;
        if choices.len() > 1 {
            return Err("more than one choice".into());
        }
        if let Some(ch) = choices.first() {
            let delta = ch.get("delta").cloned().unwrap_or(Value::Null);
            if let Some(r) = delta.get("reasoning_content").and_then(Value::as_str) {
                a.reasoning.push_str(r);
            }
            if let Some(t) = delta.get("content").and_then(Value::as_str) {
                a.text.push_str(t);
            }
            for tc in delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let idx = tc
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or("tool call delta without index")?;
                let p = calls.entry(idx).or_default();
                if let Some(id) = tc
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    if p.id.is_empty() {
                        p.id = id.into();
                    } else if p.id != id {
                        return Err(format!("call {idx}: id changed"));
                    }
                }
                let f = tc.get("function");
                if let Some(n) = f
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    if p.name.is_empty() {
                        p.name = n.into();
                    } else if p.name != n {
                        return Err(format!("call {idx}: name changed"));
                    }
                }
                if let Some(x) = f.and_then(|f| f.get("arguments")).and_then(Value::as_str) {
                    p.args.push_str(x);
                }
            }
            if let Some(fr) = ch.get("finish_reason").and_then(Value::as_str) {
                finish = Some(fr.into());
            }
        }
        if let Some(u) = c.get("usage").filter(|u| !u.is_null()) {
            a.usage = Some(u.clone());
        }
    }
    if !done {
        return Err("stream ended without [DONE]".into());
    }
    let finish = finish.ok_or("no finish_reason")?;
    for (idx, p) in calls {
        if p.id.is_empty() || p.name.is_empty() {
            return Err(format!("call {idx} without id or name"));
        }
        a.calls.push(ACall {
            id: p.id,
            name: p.name,
            args: p.args,
        });
    }
    a.stop = match finish.as_str() {
        "tool_calls" if a.calls.is_empty() => return Err("tool_calls without calls".into()),
        "tool_calls" => "tool_use".into(),
        "stop" if a.calls.is_empty() => "end_turn".into(),
        "stop" => "tool_use".into(),
        "length" => "max_tokens".into(),
        "content_filter" => "refusal".into(),
        "insufficient_system_resource" => "stream:overloaded".into(),
        "aborted" => "stream:server".into(),
        other => return Err(format!("unknown finish_reason {other}")),
    };
    a.finish_raw = finish;
    Ok(a)
}

/// Anthropic Messages.
pub fn anthropic(events: &[SseEvent]) -> Result<Assembled, String> {
    let mut a = Assembled::default();
    let mut blocks: BTreeMap<u64, (String, Partial)> = BTreeMap::new();
    let mut stopped = false;
    for ev in events {
        let v = parse(ev)?;
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        if ty != ev.event {
            return Err(format!("event name {} differs from type {ty}", ev.event));
        }
        match ty {
            "message_start" => {
                let m = v.get("message").ok_or("message_start without message")?;
                a.id = m.get("id").and_then(Value::as_str).map(String::from);
                a.model = m.get("model").and_then(Value::as_str).map(String::from);
                a.usage = m.get("usage").cloned();
            }
            "content_block_start" => {
                let idx = v
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or("block without index")?;
                let b = v
                    .get("content_block")
                    .ok_or("block start without content_block")?;
                let kind = b
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let p = Partial {
                    id: b.get("id").and_then(Value::as_str).unwrap_or("").into(),
                    name: b.get("name").and_then(Value::as_str).unwrap_or("").into(),
                    args: String::new(),
                };
                blocks.insert(idx, (kind, p));
            }
            "content_block_delta" => {
                let idx = v
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or("delta without index")?;
                let (_, p) = blocks.get_mut(&idx).ok_or("delta for an unknown block")?;
                let d = v.get("delta").ok_or("delta missing")?;
                match d.get("type").and_then(Value::as_str) {
                    Some("text_delta") => a
                        .text
                        .push_str(d.get("text").and_then(Value::as_str).unwrap_or("")),
                    Some("thinking_delta") => a
                        .reasoning
                        .push_str(d.get("thinking").and_then(Value::as_str).unwrap_or("")),
                    Some("signature_delta") => {
                        a.signature = d.get("signature").and_then(Value::as_str).map(String::from)
                    }
                    Some("input_json_delta") => p
                        .args
                        .push_str(d.get("partial_json").and_then(Value::as_str).unwrap_or("")),
                    other => return Err(format!("unknown delta {other:?}")),
                }
            }
            "content_block_stop" | "ping" => {}
            "message_delta" => {
                a.finish_raw = v
                    .pointer("/delta/stop_reason")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into();
                if let (Some(Value::Object(u)), Some(Value::Object(m))) =
                    (v.get("usage"), a.usage.as_mut())
                {
                    for (k, x) in u {
                        m.insert(k.clone(), x.clone());
                    }
                }
            }
            "message_stop" => stopped = true,
            "error" => {
                let t = v
                    .pointer("/error/type")
                    .and_then(Value::as_str)
                    .unwrap_or("error");
                a.stop = format!("stream:{t}");
                return Ok(a);
            }
            other => return Err(format!("unknown event {other}")),
        }
    }
    if !stopped {
        return Err("stream ended without message_stop".into());
    }
    for (_, (kind, p)) in blocks {
        if kind == "tool_use" {
            a.calls.push(ACall {
                id: p.id,
                name: p.name,
                args: p.args,
            });
        }
    }
    a.stop = a.finish_raw.clone();
    Ok(a)
}

/// OpenAI Responses.
pub fn openai(events: &[SseEvent]) -> Result<Assembled, String> {
    let mut a = Assembled::default();
    let mut calls: BTreeMap<String, (u64, Partial)> = BTreeMap::new();
    let mut refusal = false;
    let mut terminal = false;
    for (seq, ev) in events.iter().enumerate() {
        if terminal {
            return Err("event after the terminal event".into());
        }
        let v = parse(ev)?;
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        if ty != ev.event {
            return Err(format!("event name {} differs from type {ty}", ev.event));
        }
        if v.get("sequence_number").and_then(Value::as_u64) != Some(seq as u64) {
            return Err(format!("sequence_number gap at {ty}"));
        }
        match ty {
            "response.created" => {
                a.id = v
                    .pointer("/response/id")
                    .and_then(Value::as_str)
                    .map(String::from);
                a.model = v
                    .pointer("/response/model")
                    .and_then(Value::as_str)
                    .map(String::from);
            }
            "response.output_item.added" => {
                let item = v.get("item").ok_or("item missing")?;
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    let oi = v.get("output_index").and_then(Value::as_u64).unwrap_or(0);
                    let id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let p = Partial {
                        id: item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .into(),
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .into(),
                        args: String::new(),
                    };
                    calls.insert(id, (oi, p));
                }
            }
            "response.output_item.done" => {
                let item = v.get("item").ok_or("item missing")?;
                if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                    a.signature = item
                        .get("encrypted_content")
                        .and_then(Value::as_str)
                        .map(String::from);
                    for s in item
                        .get("summary")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        a.reasoning
                            .push_str(s.get("text").and_then(Value::as_str).unwrap_or(""));
                    }
                }
            }
            "response.output_text.delta" => a
                .text
                .push_str(v.get("delta").and_then(Value::as_str).unwrap_or("")),
            "response.refusal.delta" => {
                refusal = true;
                a.text
                    .push_str(v.get("delta").and_then(Value::as_str).unwrap_or(""));
            }
            "response.function_call_arguments.delta" => {
                let id = v.get("item_id").and_then(Value::as_str).unwrap_or("");
                let (_, p) = calls.get_mut(id).ok_or("arguments for an unknown item")?;
                p.args
                    .push_str(v.get("delta").and_then(Value::as_str).unwrap_or(""));
            }
            "response.content_part.added"
            | "response.content_part.done"
            | "response.output_text.done"
            | "response.refusal.done"
            | "response.function_call_arguments.done" => {}
            "response.completed" | "response.incomplete" | "response.failed" => {
                terminal = true;
                a.finish_raw = v
                    .pointer("/response/status")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into();
                a.usage = v.pointer("/response/usage").cloned();
                a.stop = match ty {
                    "response.failed" => {
                        format!(
                            "stream:{}",
                            v.pointer("/response/error/code")
                                .and_then(Value::as_str)
                                .unwrap_or("error")
                        )
                    }
                    "response.incomplete" => "max_tokens".into(),
                    _ if refusal => "refusal".into(),
                    _ if !calls.is_empty() => "tool_use".into(),
                    _ => "end_turn".into(),
                };
            }
            other => return Err(format!("unknown event {other}")),
        }
    }
    if !terminal {
        return Err("stream ended without a terminal event".into());
    }
    let mut cs: Vec<(u64, Partial)> = calls.into_values().collect();
    cs.sort_by_key(|(oi, _)| *oi);
    a.calls = cs
        .into_iter()
        .map(|(_, p)| ACall {
            id: p.id,
            name: p.name,
            args: p.args,
        })
        .collect();
    Ok(a)
}
