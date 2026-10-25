//! Request layout per provider (log-context.md §3.10; body/provider.md §3.8 field rules).
//! DeepSeek Chat Completions first (also `mock`). Bytes are ICJ; each wire message's bytes are cached
//! per epoch, and a request is the concatenation of cached bytes plus new ones (§6 rank 7, T-CTX-4).

use std::path::Path;

use serde_json::{Map, Value, json};

use super::head::Head;
use super::model::{Api, Target};
use super::project::Msg;
use crate::log::event::Block;
use crate::log::icj::{self, IcjError};

/// `messages[0]` content: `core + "\n" + self_`.
pub fn system_text(head: &Head) -> String {
    format!("{}\n{}", head.core, head.self_)
}

/// A registry `ToolSpec` → Chat Completions tool object.
pub fn chat_tool(spec: &Value) -> Value {
    json!({"function": {"description": spec["description"], "name": spec["name"], "parameters": spec["input_schema"]},
           "type": "function"})
}

/// Everything except `max_tokens` and `messages`: the head params, model, stream flag and tools.
pub fn head_fields(head: &Head, t: &Target) -> Result<Map<String, Value>, IcjError> {
    match t.api {
        Api::ChatCompletions => {
            let mut m = Map::new();
            m.insert("model".into(), t.model.clone().into());
            if matches!(t.provider.as_str(), "deepseek" | "mock") {
                m.insert("reasoning_effort".into(), t.effort.clone().into());
                m.insert("thinking".into(), json!({"type": "enabled"}));
            }
            m.insert("stream".into(), true.into());
            m.insert(
                "tools".into(),
                Value::Array(head.tools.iter().map(chat_tool).collect()),
            );
            Ok(m)
        }
        other => Err(IcjError::Serialize(format!(
            "render: wire format {} not built yet (WP10)",
            other.as_str()
        ))),
    }
}

/// Native Messages/Responses requests use the existing provider adapters, including their
/// tool, reasoning and image handling. The chat byte cache remains unchanged. The adapter's
/// `wire` step still runs in Client, so all I/O/auth stays on the Root-owned proxy route.
pub fn native_request(
    head: &Head,
    target: &Target,
    home: &Path,
    session: &str,
    messages: &[Msg],
    max_tokens: u32,
) -> Result<Vec<u8>, IcjError> {
    use crate::model::{Effort, Origin, ToolSpec};
    use crate::provider::{self, Prompt, PromptHead, handoff, models};
    let error = |s: String| IcjError::Serialize(s);
    let info = models::lookup(&target.provider, &target.model)
        .filter(|info| info.api == target.api)
        .ok_or_else(|| error(format!("no matching adapter model for {}/{}", target.provider, target.model)))?;
    let effort = Effort::parse(&target.effort)
        .ok_or_else(|| error(format!("invalid effort {:?}", target.effort)))?;
    let tools = head.tools.iter().map(|t| {
        Ok(ToolSpec {
            name: t["name"].as_str().ok_or_else(|| error("tool name is missing".into()))?.into(),
            description: t["description"].as_str().ok_or_else(|| error("tool description is missing".into()))?.into(),
            input_schema: t["input_schema"].clone(),
        })
    }).collect::<Result<Vec<_>, IcjError>>()?;
    let origin = Origin {
        provider: target.provider.clone(), api: target.api,
        model: target.model.clone(), response_id: None,
    };
    let messages = handoff::from_msgs(messages, Some(home), &origin);
    let prompt = Prompt {
        head: PromptHead { core: &head.core, self_: &head.self_ },
        tools: &tools, messages: &messages, b3: None,
        max_tokens, effort, session,
    };
    provider::for_model(info).build(&prompt).map(|r| r.body).map_err(|e| error(e.to_string()))
}

/// The system message, as its own wire message.
pub fn system_message(head: &Head) -> Value {
    json!({"content": system_text(head), "role": "system"})
}

fn media_ext(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    }
}

/// Standard base64 with padding (RFC 4648), std only.
pub fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        s.push(A[(n >> 18) as usize & 63] as char);
        s.push(A[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            A[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            A[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

/// Renders messages for one target. `home` resolves image blobs (`state/blobs/<hex>.<ext>`).
pub struct Renderer<'a> {
    pub target: &'a Target,
    pub home: &'a Path,
}

impl Renderer<'_> {
    fn image_part(&self, media_type: &str, blob: &str) -> Value {
        if !self.target.images {
            return json!({"text": format!("[image omitted: {} has no image input]", self.target.model), "type": "text"});
        }
        let hex = blob.strip_prefix("sha256:").unwrap_or(blob);
        let p = self
            .home
            .join("state")
            .join("blobs")
            .join(format!("{hex}.{}", media_ext(media_type)));
        match std::fs::read(&p) {
            Ok(bytes) => {
                json!({"image_url": {"url": format!("data:{media_type};base64,{}", base64(&bytes))}, "type": "image_url"})
            }
            Err(_) => {
                json!({"text": format!("[image unavailable: blob {blob} is missing]"), "type": "text"})
            }
        }
    }

    /// `content`: a string when only text (joined with `sep`), else a content-part array.
    fn content(&self, blocks: &[Block], sep: &str) -> Value {
        let has_image = blocks.iter().any(|b| matches!(b, Block::Image { .. }));
        if !has_image || !self.target.images {
            let parts: Vec<String> = blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Text { text } => Some(text.clone()),
                    Block::Image { .. } => Some(format!(
                        "[image omitted: {} has no image input]",
                        self.target.model
                    )),
                    _ => None,
                })
                .collect();
            return Value::String(parts.join(sep));
        }
        Value::Array(
            blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Text { text } => Some(json!({"text": text, "type": "text"})),
                    Block::Image {
                        media_type, blob, ..
                    } => Some(self.image_part(media_type, blob)),
                    _ => None,
                })
                .collect(),
        )
    }

    /// One `Msg` → its Chat Completions wire messages.
    pub fn chat_messages(&self, m: &Msg) -> Vec<Value> {
        let t = self.target;
        match m {
            Msg::User { blocks, .. } => vec![json!({"content": self.content(blocks, "\n\n"), "role": "user"})],
            Msg::Note { text, .. } => vec![json!({"content": text, "role": "user"})],
            Msg::Assistant { blocks, .. } => {
                let mut text = String::new();
                let mut reasoning: Option<String> = None;
                let mut calls = Vec::new();
                for b in blocks {
                    match b {
                        Block::Text { text: x } => text.push_str(x),
                        Block::Thinking { provider, model, text: r, .. } if *provider == t.provider && *model == t.model => {
                            reasoning.get_or_insert_with(String::new).push_str(r);
                        }
                        Block::ToolUse { call_id, name, input, raw_arguments } => {
                            let args = match (raw_arguments, t.profile.replay_raw_arguments) {
                                (Some(r), true) => r.clone(),
                                _ => icj::to_string(&Value::Object(input.clone())).unwrap_or_default(),
                            };
                            calls.push(json!({"function": {"arguments": args, "name": name}, "id": call_id, "type": "function"}));
                        }
                        _ => {} // thinking of another model, opaque blocks of another api: dropped
                    }
                }
                let mut o = Map::new();
                o.insert("content".into(), text.into());
                o.insert("role".into(), "assistant".into());
                // DeepSeek requires the field on every assistant message in tool mode (provider.md
                // §3.8.1): a message with none of this model's thinking (a pause before the first
                // token, an empty-reasoning reply, a foreign turn) carries "" (live V-DS-3).
                if matches!(t.provider.as_str(), "deepseek" | "mock") {
                    o.insert("reasoning_content".into(), reasoning.unwrap_or_default().into());
                }
                if !calls.is_empty() {
                    o.insert("tool_calls".into(), Value::Array(calls));
                }
                vec![Value::Object(o)]
            }
            Msg::ToolResults(rs) => rs
                .iter()
                .map(|r| json!({"content": self.content(&r.content, "\n"), "role": "tool", "tool_call_id": r.call_id}))
                .collect(),
        }
    }

    /// ICJ bytes of one `Msg`'s wire messages, comma-joined (no surrounding brackets).
    pub fn msg_bytes(&self, m: &Msg) -> Result<Vec<u8>, IcjError> {
        let mut out = Vec::new();
        for (i, v) in self.chat_messages(m).iter().enumerate() {
            if i > 0 {
                out.push(b',');
            }
            out.extend_from_slice(&icj::to_vec(v)?);
        }
        Ok(out)
    }
}

/// Assembles a request body from the head fields, `max_tokens` and pre-rendered message pieces
/// (`system` first). Keys stay sorted: `messages` is spliced in byte-wise order.
pub fn assemble(
    fields: &Map<String, Value>,
    max_tokens: u32,
    pieces: &[&[u8]],
) -> Result<Vec<u8>, IcjError> {
    let mut all = fields.clone();
    all.insert("max_tokens".into(), max_tokens.into());
    let mut out = Vec::with_capacity(pieces.iter().map(|p| p.len() + 1).sum::<usize>() + 4096);
    out.push(b'{');
    let mut first = true;
    let mut messages_done = false;
    let put_messages = |out: &mut Vec<u8>, first: &mut bool| {
        if !*first {
            out.push(b',');
        }
        *first = false;
        out.extend_from_slice(b"\"messages\":[");
        let mut f = true;
        for p in pieces {
            if p.is_empty() {
                continue;
            }
            if !f {
                out.push(b',');
            }
            f = false;
            out.extend_from_slice(p);
        }
        out.push(b']');
    };
    for (k, v) in &all {
        if !messages_done && k.as_bytes() > b"messages".as_slice() {
            put_messages(&mut out, &mut first);
            messages_done = true;
        }
        if !first {
            out.push(b',');
        }
        first = false;
        out.extend_from_slice(&icj::to_vec(&Value::String(k.clone()))?);
        out.push(b':');
        out.extend_from_slice(&icj::to_vec(v)?);
    }
    if !messages_done {
        put_messages(&mut out, &mut first);
    }
    out.push(b'}');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_vectors() {
        // RFC 4648 §10.
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    #[test]
    fn assemble_equals_full_serialization() {
        let mut f = Map::new();
        f.insert("model".into(), "m".into());
        f.insert("stream".into(), true.into());
        f.insert("tools".into(), json!([]));
        let a = icj::to_vec(&json!({"content":"s","role":"system"})).unwrap();
        let b = icj::to_vec(&json!({"content":"u","role":"user"})).unwrap();
        let got = assemble(&f, 7, &[&a, &b]).unwrap();
        let full = json!({"max_tokens":7,"messages":[{"content":"s","role":"system"},{"content":"u","role":"user"}],"model":"m","stream":true,"tools":[]});
        assert_eq!(got, icj::to_vec(&full).unwrap());
    }
}
