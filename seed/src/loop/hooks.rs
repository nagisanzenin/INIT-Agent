//! `ContextHooks` (loop.md §4.2): the loop's view of log-context, and `LogContext`, the thin
//! wrapper that maps the loop's and provider's types onto WP12's `Context` (build/status/WP12.md).

use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::{Map, Value, json};

use super::frames::Notice;
use super::status::Status;
use crate::context::model::Thresholds;
use crate::context::{
    AssistantIn, BoundaryIn, BoundaryOut, CallStart, Context, ContextError, NoticeIn, OwnerIn,
    ReloadIn, RequestOut, ResultIn, SignalSource,
};
use crate::log::event::{Block, ErrorEv, SignalEv};
use crate::log::session::store_blob;
use crate::model::{AssistantMessage, ContentBlock, ModelInfo, StopReason, ToolResult};

/// Owner text with its frame identity (`id` = `input_id`, `seq` for the ack).
#[derive(Clone, Debug, PartialEq)]
pub struct OwnerInput {
    pub id: String,
    pub seq: u64,
    pub text: String,
}

/// What the loop hands to log-context steps 1–4 (loop.md §3.3 `BoundaryInput`).
#[derive(Clone, Debug, Default)]
pub struct BoundaryInput {
    /// `(tool name, result)` in call order; empty at the start of a run.
    pub results: Vec<(String, ToolResult)>,
    /// Owner text that starts a run, or a delivered follow-up.
    pub owner: Vec<OwnerInput>,
    pub steers: Vec<OwnerInput>,
    /// `/compact /new /branch /label` (log-context §4.5).
    pub commands: Vec<OwnerInput>,
    /// One entry per reload tier (T0, T1).
    pub reload: Vec<ReloadIn>,
    pub notices: Vec<Notice>,
    /// The cut-off note (§3.6).
    pub harness_text: Option<String>,
    /// Run-ending errors and provider warnings.
    pub errors: Vec<ErrorEv>,
}

impl BoundaryInput {
    pub fn is_empty(&self) -> bool {
        self.results.is_empty()
            && self.owner.is_empty()
            && self.steers.is_empty()
            && self.commands.is_empty()
            && self.reload.is_empty()
            && self.notices.is_empty()
            && self.harness_text.is_none()
            && self.errors.is_empty()
    }
}

/// Timing of one model reply (logged in `message.request`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Timing {
    pub wall_ms: Option<u64>,
    pub ttft_ms: Option<u64>,
}

/// loop.md §4.2, plus `has_input` (duplicate classification, §3.4) and `session_id` (it changes
/// on `/new`). `boundary_append` returns WP12's `BoundaryOut` (duplicates, command outcomes,
/// reload prefix changes).
pub trait ContextHooks {
    fn on_response(
        &mut self,
        msg: &AssistantMessage,
        started: &[CallStart],
        t: Timing,
    ) -> io::Result<()>;
    fn boundary_append(&mut self, input: BoundaryInput) -> io::Result<BoundaryOut>;
    fn next_request(&mut self, m: &ModelInfo, max_tokens: u32) -> Result<RequestOut, ContextError>;
    fn pause(&mut self) -> io::Result<(u64, String)>;
    fn resume(&mut self, log_len: u64, leaf: &str, failure: Option<Value>) -> io::Result<()>;
    fn has_pending_work(&self) -> bool;
    fn request_emergency_compaction(&mut self);
    fn leaf(&self) -> String;
    fn has_input(&self, id: &str) -> bool;
    fn session_id(&self) -> String;
}

/// Renders a kernel notice for the model (awareness.md owns the real texts; WP13 replaces this).
pub type NoticeRenderer = Box<dyn Fn(&Notice) -> String>;

pub fn default_notice_text(n: &Notice) -> String {
    if let Some(t) = n.data.get("text").and_then(Value::as_str) {
        return t.to_string();
    }
    let data = crate::log::icj::to_vec(&n.data)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    format!("[init:notice] {}: {data}", n.kind)
}

/// A minimal self-signal (awareness.md §3.8 owns the content; WP13 supplies the real one).
pub struct BasicSignals {
    pub status: Arc<Status>,
}

impl SignalSource for BasicSignals {
    fn signal(&mut self, ctx_tokens: u64, ctx_permille: u64, th: &Thresholds) -> SignalEv {
        let turn = self.status.turn.load(Ordering::Relaxed);
        SignalEv {
            ctx_tokens,
            ctx_permille,
            text: format!(
                "[init:signals] context {}% ({}k of {}k) · turn {turn}",
                ctx_permille / 10,
                ctx_tokens / 1000,
                th.w_eff / 1000
            ),
            extra: Map::new(),
        }
    }
}

/// WP12's `Context` behind `ContextHooks`.
pub struct LogContext {
    pub ctx: Context,
    pub signals: Box<dyn SignalSource>,
    pub notice_text: NoticeRenderer,
}

impl LogContext {
    pub fn new(ctx: Context, signals: Box<dyn SignalSource>) -> LogContext {
        LogContext {
            ctx,
            signals,
            notice_text: Box::new(default_notice_text),
        }
    }

    fn blocks(&self, content: &[ContentBlock]) -> io::Result<Vec<Block>> {
        content.iter().map(|b| self.block(b)).collect()
    }

    fn block(&self, b: &ContentBlock) -> io::Result<Block> {
        Ok(match b {
            ContentBlock::Text { text } => Block::text(text.clone()),
            ContentBlock::Image {
                media_type,
                data_b64,
            } => {
                let bytes = base64_decode(data_b64).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "image block is not valid base64",
                    )
                })?;
                let blob = store_blob(&self.ctx.log.home, media_type, &bytes)?;
                Block::Image {
                    media_type: media_type.clone(),
                    blob,
                    path: None,
                    tokens_est: IMAGE_TOKENS_DEFAULT,
                }
            }
            ContentBlock::Thinking(t) => Block::Thinking {
                provider: t.provider.clone(),
                model: t.model.clone(),
                text: t.text.clone(),
                native: t.native.clone(),
            },
            ContentBlock::ToolCall(c) => Block::ToolUse {
                call_id: c.id.clone(),
                name: c.name.clone(),
                input: c.input.clone(),
                raw_arguments: c.raw_arguments.clone(),
            },
            ContentBlock::Opaque { api, raw } => Block::Opaque {
                api: api.as_str().to_string(),
                raw: raw.clone(),
            },
        })
    }
}

/// log-context's default image estimate (provider.md §4.1: DeepSeek and OpenAI publish none).
pub const IMAGE_TOKENS_DEFAULT: u64 = 1_600;

impl ContextHooks for LogContext {
    fn on_response(
        &mut self,
        msg: &AssistantMessage,
        started: &[CallStart],
        t: Timing,
    ) -> io::Result<()> {
        let stop_details = match &msg.stop {
            StopReason::Refusal { category: Some(c) } => Some(json!({"category": c})),
            _ => None,
        };
        let a = AssistantIn {
            content: self.blocks(&msg.content)?,
            provider: msg.origin.provider.clone(),
            api: msg.origin.api.as_str().to_string(),
            model: msg.origin.model.clone(),
            response_id: msg.origin.response_id.clone(),
            stop_reason: msg.stop.log_str().to_string(),
            stop_details,
            usage: msg.usage.to_log(None),
            wall_ms: t.wall_ms,
            ttft_ms: t.ttft_ms,
        };
        self.ctx.on_response(a, started).map(|_| ())
    }

    fn boundary_append(&mut self, input: BoundaryInput) -> io::Result<BoundaryOut> {
        let mut results = Vec::with_capacity(input.results.len());
        for (name, r) in input.results {
            results.push(ResultIn {
                call_id: r.call_id,
                name,
                output: self.blocks(&r.content)?,
                is_error: r.is_error,
                spill_path: r.meta.spill_path,
                exit_code: r.meta.exit_code.map(i64::from),
                signal: r.meta.signal.map(i64::from),
                wall_ms: Some(r.meta.wall_ms),
            });
        }
        let owner_in = |o: OwnerInput| OwnerIn {
            input_id: o.id,
            text: o.text,
            channel: None,
        };
        let notices = input
            .notices
            .into_iter()
            .map(|n| NoticeIn {
                text: (self.notice_text)(&n),
                id: n.id,
                kind: n.kind,
                data: n.data,
            })
            .collect();
        self.ctx.boundary_append(BoundaryIn {
            results,
            harness_text: input.harness_text,
            owner: input.owner.into_iter().map(owner_in).collect(),
            steers: input.steers.into_iter().map(owner_in).collect(),
            commands: input.commands.into_iter().map(owner_in).collect(),
            reload: input.reload,
            notices,
            errors: input.errors,
        })
    }

    fn next_request(
        &mut self,
        _m: &ModelInfo,
        max_tokens: u32,
    ) -> Result<RequestOut, ContextError> {
        self.ctx.next_request(&mut *self.signals, max_tokens)
    }

    fn pause(&mut self) -> io::Result<(u64, String)> {
        self.ctx.pause()
    }

    fn resume(&mut self, log_len: u64, leaf: &str, failure: Option<Value>) -> io::Result<()> {
        self.ctx.resume(log_len, leaf, failure)
    }

    fn has_pending_work(&self) -> bool {
        self.ctx.has_pending_work()
    }

    fn request_emergency_compaction(&mut self) {
        self.ctx.request_emergency_compaction()
    }

    fn leaf(&self) -> String {
        self.ctx.leaf()
    }

    fn has_input(&self, id: &str) -> bool {
        self.ctx.log.index.input_ids.contains(id)
    }

    fn session_id(&self) -> String {
        self.ctx.log.sid().to_string()
    }
}

/// RFC 4648 base64 decoding (standard alphabet, padding optional), std only.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    }
    let b: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let b = match b.iter().position(|&c| c == b'=') {
        Some(i) => {
            if b[i..].iter().any(|&c| c != b'=') {
                return None;
            }
            &b[..i]
        }
        None => &b[..],
    };
    if b.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() * 3 / 4);
    for chunk in b.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}
