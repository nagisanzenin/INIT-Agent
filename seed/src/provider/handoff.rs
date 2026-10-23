//! Cross-provider and cross-model handoff (body/provider.md §3.10): one pure filter applied to every
//! prompt after log-context's projection, plus the log-block → normalized-message conversion.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::context::project::Msg;
use crate::log::event::Block;
use crate::model::{
    Api, AssistantMessage, ContentBlock, Message, ModelInfo, Origin, StopReason, Thinking,
    ToolCall, ToolMeta, ToolResult, Usage,
};

pub const NO_VISIBLE_OUTPUT: &str = "[no visible output]";

pub fn image_omitted(model: &str) -> String {
    format!("[image omitted: {model} has no image input]")
}

/// Replaces every byte outside `[A-Za-z0-9_-]` with `_` and truncates to 64 bytes.
pub fn sanitize_id(id: &str) -> String {
    id.bytes()
        .take(64)
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
                b as char
            } else {
                '_'
            }
        })
        .collect()
}

fn unique(base: String, used: &mut BTreeSet<String>) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    for n in 2u32.. {
        let suffix = format!("_{n}");
        let mut b = base.clone();
        b.truncate(64 - suffix.len());
        let cand = format!("{b}{suffix}");
        if used.insert(cand.clone()) {
            return cand;
        }
    }
    unreachable!()
}

fn filter_media(blocks: &[ContentBlock], target: &ModelInfo) -> Vec<ContentBlock> {
    blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Image { .. } if !target.images => ContentBlock::Text {
                text: image_omitted(target.id),
            },
            other => other.clone(),
        })
        .collect()
}

/// The §3.10 table. Thinking is kept only for the block's own provider + model; `Opaque` only for
/// the same api; text and tool calls always; call ids sanitized when the message came from another
/// api (and the matching results renamed). Pure: the same list always yields the same output.
pub fn filter(messages: &[Message], target: &ModelInfo) -> Vec<Message> {
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut rename: BTreeMap<String, String> = BTreeMap::new(); // for the following ToolResults only
    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        match m {
            Message::User { content } => {
                out.push(Message::User {
                    content: filter_media(content, target),
                });
            }
            Message::Note { text } => out.push(Message::Note { text: text.clone() }),
            Message::ToolResults(rs) => {
                let rs = rs
                    .iter()
                    .map(|r| ToolResult {
                        call_id: rename
                            .get(&r.call_id)
                            .cloned()
                            .unwrap_or_else(|| r.call_id.clone()),
                        content: filter_media(&r.content, target),
                        is_error: r.is_error,
                        meta: r.meta.clone(),
                    })
                    .collect();
                out.push(Message::ToolResults(rs));
            }
            Message::Assistant(a) => {
                rename.clear();
                let foreign_api = a.origin.api != target.api;
                let mut content = Vec::with_capacity(a.content.len());
                for b in &a.content {
                    match b {
                        ContentBlock::Thinking(t) => {
                            if t.provider == target.provider && t.model == target.id {
                                content.push(b.clone());
                            }
                        }
                        ContentBlock::Opaque { api, .. } => {
                            if *api == target.api {
                                content.push(b.clone());
                            }
                        }
                        ContentBlock::ToolCall(c) => {
                            let mut c = c.clone();
                            if foreign_api {
                                let new = unique(sanitize_id(&c.id), &mut used);
                                rename.insert(c.id.clone(), new.clone());
                                c.id = new;
                            } else {
                                used.insert(c.id.clone());
                            }
                            content.push(ContentBlock::ToolCall(c));
                        }
                        ContentBlock::Image { .. } if !target.images => {
                            content.push(ContentBlock::Text {
                                text: image_omitted(target.id),
                            })
                        }
                        other => content.push(other.clone()),
                    }
                }
                if content.is_empty() {
                    content.push(ContentBlock::Text {
                        text: NO_VISIBLE_OUTPUT.into(),
                    });
                }
                out.push(Message::Assistant(AssistantMessage {
                    content,
                    stop: a.stop.clone(),
                    usage: a.usage,
                    origin: a.origin.clone(),
                }));
            }
        }
    }
    out
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

pub fn api_from_str(s: &str) -> Option<Api> {
    match s {
        "chat_completions" => Some(Api::ChatCompletions),
        "messages" => Some(Api::Messages),
        "responses" => Some(Api::Responses),
        _ => None,
    }
}

fn block(b: &Block, home: Option<&Path>) -> Option<ContentBlock> {
    Some(match b {
        Block::Text { text } => ContentBlock::Text { text: text.clone() },
        Block::Image {
            media_type, blob, ..
        } => {
            let hex = blob.strip_prefix("sha256:").unwrap_or(blob);
            let bytes = home.and_then(|h| {
                std::fs::read(
                    h.join("state")
                        .join("blobs")
                        .join(format!("{hex}.{}", media_ext(media_type))),
                )
                .ok()
            });
            match bytes {
                Some(b) => ContentBlock::Image {
                    media_type: media_type.clone(),
                    data_b64: crate::context::render::base64(&b),
                },
                None => ContentBlock::Text {
                    text: format!("[image unavailable: blob {blob} is missing]"),
                },
            }
        }
        Block::Thinking {
            provider,
            model,
            text,
            native,
        } => ContentBlock::Thinking(Thinking {
            provider: provider.clone(),
            model: model.clone(),
            text: text.clone(),
            native: native.clone(),
        }),
        Block::ToolUse {
            call_id,
            name,
            input,
            raw_arguments,
        } => ContentBlock::ToolCall(ToolCall {
            id: call_id.clone(),
            name: name.clone(),
            input: input.clone(),
            raw_arguments: raw_arguments.clone(),
        }),
        Block::Opaque { api, raw } => ContentBlock::Opaque {
            api: api_from_str(api)?,
            raw: raw.clone(),
        },
        Block::Other(_) => return None,
    })
}

/// Converts log-context's projection (`context::project::Msg`) into normalized messages, for the
/// adapters' `build` (APIs the context does not render itself) and for cross-checks. `home`
/// resolves image blobs; `origin` is stamped on every assistant message (the projection does not
/// carry it; log-context already applied its own same-model rules).
pub fn from_msgs(msgs: &[Msg], home: Option<&Path>, origin: &Origin) -> Vec<Message> {
    let conv = |bs: &[Block]| bs.iter().filter_map(|b| block(b, home)).collect::<Vec<_>>();
    msgs.iter()
        .map(|m| match m {
            Msg::User { blocks, .. } => Message::User {
                content: conv(blocks),
            },
            Msg::Note { text, .. } => Message::Note { text: text.clone() },
            Msg::Assistant { blocks, .. } => {
                let content = conv(blocks);
                let stop = if content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolCall(_)))
                {
                    StopReason::ToolUse
                } else {
                    StopReason::EndTurn
                };
                Message::Assistant(AssistantMessage {
                    content,
                    stop,
                    usage: Usage::default(),
                    origin: origin.clone(),
                })
            }
            Msg::ToolResults(rs) => Message::ToolResults(
                rs.iter()
                    .map(|r| ToolResult {
                        call_id: r.call_id.clone(),
                        content: conv(&r.content),
                        is_error: r.is_error,
                        meta: ToolMeta::default(),
                    })
                    .collect(),
            ),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::models;
    use serde_json::json;

    fn origin(provider: &str, api: Api, model: &str) -> Origin {
        Origin {
            provider: provider.into(),
            api,
            model: model.into(),
            response_id: None,
        }
    }

    fn think(provider: &str, model: &str) -> ContentBlock {
        ContentBlock::Thinking(Thinking {
            provider: provider.into(),
            model: model.into(),
            text: "t".into(),
            native: None,
        })
    }

    fn call(id: &str) -> ContentBlock {
        ContentBlock::ToolCall(ToolCall {
            id: id.into(),
            name: "bash".into(),
            input: Default::default(),
            raw_arguments: Some("{}".into()),
        })
    }

    fn asst(o: Origin, content: Vec<ContentBlock>) -> Message {
        Message::Assistant(AssistantMessage {
            content,
            stop: StopReason::ToolUse,
            usage: Usage::default(),
            origin: o,
        })
    }

    fn img() -> ContentBlock {
        ContentBlock::Image {
            media_type: "image/png".into(),
            data_b64: "AA==".into(),
        }
    }

    fn results(ids: &[&str]) -> Message {
        Message::ToolResults(
            ids.iter()
                .map(|i| ToolResult {
                    call_id: (*i).into(),
                    content: vec![img()],
                    is_error: false,
                    meta: ToolMeta::default(),
                })
                .collect(),
        )
    }

    fn content(m: &Message) -> &[ContentBlock] {
        match m {
            Message::Assistant(a) => &a.content,
            Message::User { content } => content,
            _ => panic!(),
        }
    }

    /// provider.md §8 item 9: the full §3.10 table.
    #[test]
    fn handoff_table() {
        let flash = models::lookup("deepseek", "deepseek-flash").unwrap();
        let pro = models::lookup("deepseek", "deepseek-v4-pro").unwrap();
        let opus = models::lookup("anthropic", "claude-opus-5-5").unwrap();
        let ds = origin("deepseek", Api::ChatCompletions, "deepseek-flash");
        let opaque_cc = ContentBlock::Opaque {
            api: Api::ChatCompletions,
            raw: json!({"x": 1}),
        };
        let msgs = vec![
            Message::User {
                content: vec![ContentBlock::Text { text: "u".into() }, img()],
            },
            asst(
                ds.clone(),
                vec![
                    think("deepseek", "deepseek-flash"),
                    ContentBlock::Text { text: "x".into() },
                    opaque_cc.clone(),
                    call("call_00_a"),
                ],
            ),
            results(&["call_00_a"]),
            Message::Note { text: "n".into() },
        ];
        // Same provider + model: everything kept; images kept (flash has images).
        assert_eq!(filter(&msgs, flash), msgs);
        // Different model, same api: thinking dropped; opaque, text, calls kept; images replaced (pro has none).
        let f = filter(&msgs, pro);
        assert_eq!(content(&f[1]).len(), 3);
        assert!(
            !content(&f[1])
                .iter()
                .any(|b| matches!(b, ContentBlock::Thinking(_)))
        );
        assert_eq!(
            content(&f[0])[1],
            ContentBlock::Text {
                text: image_omitted("deepseek-v4-pro")
            }
        );
        match &f[2] {
            Message::ToolResults(r) => assert_eq!(
                r[0].content[0],
                ContentBlock::Text {
                    text: image_omitted("deepseek-v4-pro")
                }
            ),
            _ => panic!(),
        }
        // Different api: thinking and opaque dropped, text and calls kept.
        let f = filter(&msgs, opus);
        assert_eq!(
            content(&f[1]),
            &[ContentBlock::Text { text: "x".into() }, call("call_00_a")]
        );
        assert_eq!(f[3], msgs[3]);
        // A message left empty gets the placeholder text.
        let only_think = vec![asst(ds, vec![think("deepseek", "deepseek-flash")])];
        assert_eq!(
            content(&filter(&only_think, opus)[0]),
            &[ContentBlock::Text {
                text: NO_VISIBLE_OUTPUT.into()
            }]
        );
    }

    #[test]
    fn call_ids_sanitized_with_collisions() {
        let flash = models::lookup("deepseek", "deepseek-flash").unwrap();
        let an = origin("anthropic", Api::Messages, "claude-opus-5-5");
        let long = "a".repeat(80);
        let msgs = vec![
            asst(
                an.clone(),
                vec![call("tool.u:1"), call("tool_u_1"), call(&long)],
            ),
            results(&["tool.u:1", "tool_u_1", &long]),
            asst(an, vec![call("tool/u/1")]),
            results(&["tool/u/1"]),
        ];
        let f = filter(&msgs, flash);
        let ids: Vec<String> = content(&f[0])
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolCall(c) => Some(c.id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, ["tool_u_1", "tool_u_1_2", &"a".repeat(64)]);
        match (&f[1], &f[3]) {
            (Message::ToolResults(r1), Message::ToolResults(r2)) => {
                assert_eq!(
                    r1.iter().map(|r| r.call_id.as_str()).collect::<Vec<_>>(),
                    ["tool_u_1", "tool_u_1_2", &"a".repeat(64)]
                );
                assert_eq!(r2[0].call_id, "tool_u_1_3");
            }
            _ => panic!(),
        }
        // Pure and idempotent.
        assert_eq!(filter(&msgs, flash), f);
        assert_eq!(filter(&f, flash), f);
    }
}
