//! The event envelope and every event type of format 1 (log-context.md §3.3, §4.2).
//!
//! Writers build a typed body, convert it to a `serde_json::Map` (sorted keys), and the writer adds the
//! envelope (`id`, `parent_id`, `ts`, `type`). Readers parse each line into a `Map`, read the envelope,
//! and parse the body on demand; an unknown `type`, or a known type that does not parse, becomes
//! `EventBody::Other` and is preserved and ignored (rollback safety, §3.3).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::clock::{id_hex, parse_id};
use super::icj::{self, IcjError};

/// Event type tag. `Other` covers unknown types (kept, never projected).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Kind {
    Session,
    Message,
    Command,
    ToolCall,
    ToolResult,
    Steer,
    Boot,
    SelfEdit,
    Gate,
    UpdateFailed,
    ThinningDone,
    Notice,
    Reload,
    Signal,
    Mask,
    Compaction,
    Branch,
    Label,
    Display,
    Error,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 20] = [
        Kind::Session,
        Kind::Message,
        Kind::Command,
        Kind::ToolCall,
        Kind::ToolResult,
        Kind::Steer,
        Kind::Boot,
        Kind::SelfEdit,
        Kind::Gate,
        Kind::UpdateFailed,
        Kind::ThinningDone,
        Kind::Notice,
        Kind::Reload,
        Kind::Signal,
        Kind::Mask,
        Kind::Compaction,
        Kind::Branch,
        Kind::Label,
        Kind::Display,
        Kind::Error,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Session => "session",
            Kind::Message => "message",
            Kind::Command => "command",
            Kind::ToolCall => "tool_call",
            Kind::ToolResult => "tool_result",
            Kind::Steer => "steer",
            Kind::Boot => "boot",
            Kind::SelfEdit => "self_edit",
            Kind::Gate => "gate",
            Kind::UpdateFailed => "update_failed",
            Kind::ThinningDone => "thinning_done",
            Kind::Notice => "notice",
            Kind::Reload => "reload",
            Kind::Signal => "signal",
            Kind::Mask => "mask",
            Kind::Compaction => "compaction",
            Kind::Branch => "branch",
            Kind::Label => "label",
            Kind::Display => "display",
            Kind::Error => "error",
            Kind::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Kind {
        Kind::ALL
            .iter()
            .copied()
            .find(|k| k.as_str() == s)
            .unwrap_or(Kind::Other)
    }
}

/// `^[a-z][a-z0-9_]{0,31}$`
pub fn valid_type_tag(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// `^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{3}Z$`
pub fn valid_ts(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 24
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'.',
            23 => *c == b'Z',
            _ => c.is_ascii_digit(),
        })
}

// ---------------------------------------------------------------------------------------------
// Content blocks (§4.2 block_*)
// ---------------------------------------------------------------------------------------------

/// A content block as stored in the log. `Other` keeps a block type this body does not know.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Text {
        text: String,
    },
    Image {
        media_type: String,
        blob: String,
        path: Option<String>,
        tokens_est: u64,
    },
    Thinking {
        provider: String,
        model: String,
        text: String,
        native: Option<Value>,
    },
    ToolUse {
        call_id: String,
        name: String,
        input: Map<String, Value>,
        raw_arguments: Option<String>,
    },
    Opaque {
        api: String,
        raw: Value,
    },
    Other(Value),
}

impl Block {
    pub fn text(s: impl Into<String>) -> Block {
        Block::Text { text: s.into() }
    }

    pub fn from_value(v: &Value) -> Block {
        let o = match v.as_object() {
            Some(o) => o,
            None => return Block::Other(v.clone()),
        };
        let s = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
        let parsed = match o.get("type").and_then(Value::as_str) {
            Some("text") => s("text").map(|text| Block::Text { text }),
            Some("image") => match (
                s("media_type"),
                s("blob"),
                o.get("tokens_est").and_then(Value::as_u64),
            ) {
                (Some(media_type), Some(blob), Some(tokens_est)) => Some(Block::Image {
                    media_type,
                    blob,
                    path: s("path"),
                    tokens_est,
                }),
                _ => None,
            },
            Some("thinking") => match (s("provider"), s("model"), s("text")) {
                (Some(provider), Some(model), Some(text)) => Some(Block::Thinking {
                    provider,
                    model,
                    text,
                    native: o.get("native").cloned(),
                }),
                _ => None,
            },
            Some("tool_use") => match (
                s("call_id"),
                s("name"),
                o.get("input").and_then(Value::as_object),
            ) {
                (Some(call_id), Some(name), Some(input)) => Some(Block::ToolUse {
                    call_id,
                    name,
                    input: input.clone(),
                    raw_arguments: s("raw_arguments"),
                }),
                _ => None,
            },
            Some("opaque") => match (s("api"), o.get("raw")) {
                (Some(api), Some(raw)) => Some(Block::Opaque {
                    api,
                    raw: raw.clone(),
                }),
                _ => None,
            },
            _ => None,
        };
        parsed.unwrap_or_else(|| Block::Other(v.clone()))
    }

    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        let put = |m: &mut Map<String, Value>, k: &str, v: Value| {
            m.insert(k.to_string(), v);
        };
        match self {
            Block::Text { text } => {
                put(&mut m, "type", "text".into());
                put(&mut m, "text", text.clone().into());
            }
            Block::Image {
                media_type,
                blob,
                path,
                tokens_est,
            } => {
                put(&mut m, "type", "image".into());
                put(&mut m, "media_type", media_type.clone().into());
                put(&mut m, "blob", blob.clone().into());
                if let Some(p) = path {
                    put(&mut m, "path", p.clone().into());
                }
                put(&mut m, "tokens_est", (*tokens_est).into());
            }
            Block::Thinking {
                provider,
                model,
                text,
                native,
            } => {
                put(&mut m, "type", "thinking".into());
                put(&mut m, "provider", provider.clone().into());
                put(&mut m, "model", model.clone().into());
                put(&mut m, "text", text.clone().into());
                if let Some(n) = native {
                    put(&mut m, "native", n.clone());
                }
            }
            Block::ToolUse {
                call_id,
                name,
                input,
                raw_arguments,
            } => {
                put(&mut m, "type", "tool_use".into());
                put(&mut m, "call_id", call_id.clone().into());
                put(&mut m, "name", name.clone().into());
                put(&mut m, "input", Value::Object(input.clone()));
                if let Some(r) = raw_arguments {
                    put(&mut m, "raw_arguments", r.clone().into());
                }
            }
            Block::Opaque { api, raw } => {
                put(&mut m, "type", "opaque".into());
                put(&mut m, "api", api.clone().into());
                put(&mut m, "raw", raw.clone());
            }
            Block::Other(v) => return v.clone(),
        }
        Value::Object(m)
    }

    /// The text of a `Text` block.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Block::Text { text } => Some(text),
            _ => None,
        }
    }
}

impl Serialize for Block {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Block {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Block::from_value(&Value::deserialize(d)?))
    }
}

/// Concatenated text of the `Text` blocks.
pub fn joined_text(blocks: &[Block], sep: &str) -> String {
    blocks
        .iter()
        .filter_map(Block::as_text)
        .collect::<Vec<_>>()
        .join(sep)
}

// ---------------------------------------------------------------------------------------------
// Event bodies (§4.2 $defs)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    pub reason: String, // cold | new | fork
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_event: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionEv {
    pub format: u64,
    pub session_id: String,
    pub origin: Origin,
    pub sha: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageEv {
    pub input_tokens: u64, // uncached input
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<Value>,
}

impl UsageEv {
    /// All prompt tokens (uncached + read + write).
    pub fn prompt_total(&self) -> u64 {
        self.input_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RequestMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub est_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k_ppm: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<String>,
    /// `x-init-task` as sent; `Some(Value::Null)` when the path had no owner message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageEv {
    pub role: String, // user | assistant
    pub content: Vec<Block>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>, // owner | harness
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `Some(Value::Null)` writes an explicit null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_details: Option<Value>,
    /// Boxed, like `request`: assistant messages only, and the context's view drops both after
    /// the token estimate (log-context.md §3.7), so inline they would only pad every message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Box<UsageEv>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<Box<RequestMeta>>,
}

impl From<MessageEv> for EventBody {
    fn from(m: MessageEv) -> EventBody {
        EventBody::Message(Box::new(m))
    }
}

impl MessageEv {
    pub fn is_assistant(&self) -> bool {
        self.role == "assistant"
    }
    pub fn is_owner(&self) -> bool {
        self.role == "user" && self.source.as_deref() == Some("owner")
    }
    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &Map<String, Value>)> {
        self.content.iter().filter_map(|b| match b {
            Block::ToolUse {
                call_id,
                name,
                input,
                ..
            } => Some((call_id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }
    pub fn has_thinking(&self) -> bool {
        self.content
            .iter()
            .any(|b| matches!(b, Block::Thinking { .. }))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandEv {
    pub input_id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCallEv {
    pub call_id: String,
    pub name: String,
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResultEv {
    pub call_id: String,
    pub name: String,
    pub output: Vec<Block>,
    pub is_error: bool,
    #[serde(default)]
    pub spill_path: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
    pub tokens_est: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<String>, // interrupted | not_started
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SteerEv {
    pub content: Vec<Block>,
    pub input_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LogRepair {
    pub newline_appended: bool,
    pub skipped: Vec<Skipped>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    pub offset: u64,
    pub len: u64,
}

/// The `boot` event. Required fields are typed; the optional ones (lkg_sha, provisional, branch, cause,
/// crash, update_failed, …: §4.2) travel in `extra`. body/boot.md and awareness.md fill it (WP13).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BootEv {
    pub boot_id: String,
    pub reason: String,
    pub mode: String,
    pub sha: String,
    pub from_sha: Option<String>,
    pub model: String,
    pub provider: String,
    pub effort: String,
    pub context_window: u64,
    pub effective_window: u64,
    pub body_tokens: u64,
    pub prefix_hash: String,
    pub env: Value,
    pub facts: Value,
    pub log_repair: LogRepair,
    pub text: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelfEditEv {
    pub tier: String,
    pub sha: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>, // branch, change_type, trailers
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GateEv {
    pub gate_id: String,
    pub sha: String,
    pub stage: String,
    pub pass: Option<bool>,
    #[serde(rename = "async")]
    pub is_async: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>, // reason, score, cost, wall, cache_hit, note_ref
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateFailedEv {
    pub sha: String,
    pub stage: String,
    pub reason: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>, // from_sha, job, exit_code, signal, log_tail, report
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThinningDoneEv {
    pub removed: Vec<String>,
    pub kept: Vec<String>,
    pub expired: Vec<String>,
    pub tokens_saved: i64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoticeEv {
    pub source: String, // kernel | body
    pub code: String,
    pub projected: bool,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReloadEv {
    pub tier: String, // T0 | T1
    pub ok: bool,
    pub components: Vec<String>,
    pub prefix_changed: bool,
    pub head_hash_before: String,
    pub head_hash_after: String,
    #[serde(default)]
    pub error: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignalEv {
    pub ctx_tokens: u64,
    pub ctx_permille: u64,
    pub text: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>, // spend_today_musd, cap_today_musd, cache_*_permille, uptime_s, …
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskEntry {
    pub id: String,
    pub part: String, // output | input | image
    pub tokens: u64,
    pub placeholder: String,
    /// Part `input` only: the tool call whose input is masked, and the masked input object replayed
    /// in its place (P2: computed once at the mask epoch). Extension of §4.2 (unknown fields allowed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskEv {
    pub masked: Vec<MaskEntry>,
    pub tokens_before: u64,
    pub tokens_after_est: u64,
    pub n: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompactionEv {
    pub summary: String,
    pub text: String,
    pub first_kept_id: String,
    pub through_id: String,
    pub tokens_before: u64,
    pub tokens_after_est: u64,
    pub n: u64,
    pub trigger: String, // auto | manual | overflow
    #[serde(default)]
    pub focus: Option<String>,
    pub artifacts: Value,
    #[serde(default)]
    pub owner_msgs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summarizer: Option<Value>,
    #[serde(default)]
    pub retries: u64,
    #[serde(default)]
    pub repaired_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchEv {
    pub from_leaf: String,
    pub reason: String, // owner | handoff_failed | agent
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabelEv {
    pub target_id: String,
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayEv {
    pub level: String, // info | warn | error
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorEv {
    #[serde(rename = "where")]
    pub where_: String,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A typed event body. The large bodies are boxed: the context's window holds one `EventBody` per
/// event until the next compaction (log-context.md §3.7, FIX8/M-5: up to about 1,000 steps of about
/// 4 events each), and an inline `MessageEv` made every event, `tool_call` markers and signals too,
/// as large as a message. `size_of::<EventBody>()` is pinned by a test in `context/tests.rs`.
#[derive(Clone, Debug, PartialEq)]
pub enum EventBody {
    Session(SessionEv),
    Message(Box<MessageEv>),
    Command(CommandEv),
    ToolCall(ToolCallEv),
    ToolResult(ToolResultEv),
    Steer(SteerEv),
    Boot(Box<BootEv>),
    SelfEdit(SelfEditEv),
    Gate(GateEv),
    UpdateFailed(UpdateFailedEv),
    ThinningDone(ThinningDoneEv),
    Notice(NoticeEv),
    Reload(ReloadEv),
    Signal(SignalEv),
    Mask(MaskEv),
    Compaction(Box<CompactionEv>),
    Branch(BranchEv),
    Label(LabelEv),
    Display(DisplayEv),
    Error(ErrorEv),
    /// Unknown type (or a known type that did not parse): the type tag and the whole object.
    Other(String, Map<String, Value>),
}

impl EventBody {
    pub fn kind(&self) -> Kind {
        match self {
            EventBody::Session(_) => Kind::Session,
            EventBody::Message(_) => Kind::Message,
            EventBody::Command(_) => Kind::Command,
            EventBody::ToolCall(_) => Kind::ToolCall,
            EventBody::ToolResult(_) => Kind::ToolResult,
            EventBody::Steer(_) => Kind::Steer,
            EventBody::Boot(_) => Kind::Boot,
            EventBody::SelfEdit(_) => Kind::SelfEdit,
            EventBody::Gate(_) => Kind::Gate,
            EventBody::UpdateFailed(_) => Kind::UpdateFailed,
            EventBody::ThinningDone(_) => Kind::ThinningDone,
            EventBody::Notice(_) => Kind::Notice,
            EventBody::Reload(_) => Kind::Reload,
            EventBody::Signal(_) => Kind::Signal,
            EventBody::Mask(_) => Kind::Mask,
            EventBody::Compaction(_) => Kind::Compaction,
            EventBody::Branch(_) => Kind::Branch,
            EventBody::Label(_) => Kind::Label,
            EventBody::Display(_) => Kind::Display,
            EventBody::Error(_) => Kind::Error,
            EventBody::Other(..) => Kind::Other,
        }
    }

    /// The `type` tag written for this body.
    pub fn type_tag(&self) -> &str {
        match self {
            EventBody::Other(t, _) => t,
            b => b.kind().as_str(),
        }
    }

    /// Body fields (no envelope) as a sorted map.
    pub fn to_fields(&self) -> Result<Map<String, Value>, IcjError> {
        let v = match self {
            EventBody::Session(x) => icj::to_value(x)?,
            EventBody::Message(x) => icj::to_value(x.as_ref())?,
            EventBody::Command(x) => icj::to_value(x)?,
            EventBody::ToolCall(x) => icj::to_value(x)?,
            EventBody::ToolResult(x) => icj::to_value(x)?,
            EventBody::Steer(x) => icj::to_value(x)?,
            EventBody::Boot(x) => icj::to_value(x.as_ref())?,
            EventBody::SelfEdit(x) => icj::to_value(x)?,
            EventBody::Gate(x) => icj::to_value(x)?,
            EventBody::UpdateFailed(x) => icj::to_value(x)?,
            EventBody::ThinningDone(x) => icj::to_value(x)?,
            EventBody::Notice(x) => icj::to_value(x)?,
            EventBody::Reload(x) => icj::to_value(x)?,
            EventBody::Signal(x) => icj::to_value(x)?,
            EventBody::Mask(x) => icj::to_value(x)?,
            EventBody::Compaction(x) => icj::to_value(x.as_ref())?,
            EventBody::Branch(x) => icj::to_value(x)?,
            EventBody::Label(x) => icj::to_value(x)?,
            EventBody::Display(x) => icj::to_value(x)?,
            EventBody::Error(x) => icj::to_value(x)?,
            EventBody::Other(_, m) => {
                let mut m = m.clone();
                for k in ENVELOPE {
                    m.remove(k);
                }
                return Ok(m);
            }
        };
        match v {
            Value::Object(m) => Ok(m),
            _ => Err(IcjError::Serialize("event body is not an object".into())),
        }
    }

    /// Parses a body from a full event object. Never fails: anything unparsable is `Other`.
    pub fn parse(ty: &str, obj: &Map<String, Value>) -> EventBody {
        fn p<T: serde::de::DeserializeOwned>(obj: &Map<String, Value>) -> Option<T> {
            let mut m = obj.clone();
            for k in ENVELOPE {
                m.remove(k);
            }
            serde_json::from_value(Value::Object(m)).ok()
        }
        let b = match Kind::parse(ty) {
            Kind::Session => p(obj).map(EventBody::Session),
            Kind::Message => p(obj).map(|m| EventBody::Message(Box::new(m))),
            Kind::Command => p(obj).map(EventBody::Command),
            Kind::ToolCall => p(obj).map(EventBody::ToolCall),
            Kind::ToolResult => p(obj).map(EventBody::ToolResult),
            Kind::Steer => p(obj).map(EventBody::Steer),
            Kind::Boot => p(obj).map(|b| EventBody::Boot(Box::new(b))),
            Kind::SelfEdit => p(obj).map(EventBody::SelfEdit),
            Kind::Gate => p(obj).map(EventBody::Gate),
            Kind::UpdateFailed => p(obj).map(EventBody::UpdateFailed),
            Kind::ThinningDone => p(obj).map(EventBody::ThinningDone),
            Kind::Notice => p(obj).map(EventBody::Notice),
            Kind::Reload => p(obj).map(EventBody::Reload),
            Kind::Signal => p(obj).map(EventBody::Signal),
            Kind::Mask => p(obj).map(EventBody::Mask),
            Kind::Compaction => p(obj).map(|c| EventBody::Compaction(Box::new(c))),
            Kind::Branch => p(obj).map(EventBody::Branch),
            Kind::Label => p(obj).map(EventBody::Label),
            Kind::Display => p(obj).map(EventBody::Display),
            Kind::Error => p(obj).map(EventBody::Error),
            Kind::Other => None,
        };
        b.unwrap_or_else(|| EventBody::Other(ty.to_string(), obj.clone()))
    }

    /// The stored `text` that the projection replays (P2), for the types that carry one.
    pub fn stored_text(&self) -> Option<&str> {
        match self {
            EventBody::Boot(b) => Some(&b.text),
            EventBody::Gate(g) => g.text.as_deref(),
            EventBody::UpdateFailed(u) => Some(&u.text),
            EventBody::ThinningDone(t) => Some(&t.text),
            EventBody::Notice(n) => Some(&n.text),
            EventBody::Reload(r) => Some(&r.text),
            EventBody::Signal(s) => Some(&s.text),
            EventBody::Compaction(c) => Some(&c.text),
            EventBody::Branch(b) => b.text.as_deref(),
            EventBody::Display(d) => Some(&d.text),
            _ => None,
        }
    }
}

pub const ENVELOPE: [&str; 4] = ["id", "parent_id", "ts", "type"];

/// One parsed event.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub id: u64,
    pub parent: Option<u64>,
    pub ts: String,
    pub body: EventBody,
}

impl Event {
    pub fn kind(&self) -> Kind {
        self.body.kind()
    }
    pub fn id_hex(&self) -> String {
        id_hex(self.id)
    }

    /// Parses one line (without its LF). `None` if the envelope is invalid.
    pub fn parse_line(line: &[u8]) -> Option<(Event, Map<String, Value>)> {
        let v: Value = serde_json::from_slice(line).ok()?;
        let obj = match v {
            Value::Object(o) => o,
            _ => return None,
        };
        let (id, parent, ts, ty) = envelope(&obj)?;
        let body = EventBody::parse(&ty, &obj);
        Some((
            Event {
                id,
                parent,
                ts,
                body,
            },
            obj,
        ))
    }

    /// The full ICJ object of this event (envelope + body).
    pub fn to_object(&self) -> Result<Map<String, Value>, IcjError> {
        let mut m = self.body.to_fields()?;
        m.insert("id".into(), id_hex(self.id).into());
        m.insert(
            "parent_id".into(),
            self.parent
                .map(id_hex)
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
        m.insert("ts".into(), self.ts.clone().into());
        m.insert("type".into(), self.body.type_tag().to_string().into());
        Ok(m)
    }
}

/// Validates and reads the envelope: (id, parent, ts, type).
pub fn envelope(obj: &Map<String, Value>) -> Option<(u64, Option<u64>, String, String)> {
    let id = parse_id(obj.get("id")?.as_str()?)?;
    let parent = match obj.get("parent_id")? {
        Value::Null => None,
        Value::String(s) => Some(parse_id(s)?),
        _ => return None,
    };
    let ts = obj.get("ts")?.as_str()?;
    let ty = obj.get("type")?.as_str()?;
    if !valid_ts(ts) || !valid_type_tag(ty) {
        return None;
    }
    Some((id, parent, ts.to_string(), ty.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn kinds_roundtrip() {
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.as_str()), k);
            assert!(valid_type_tag(k.as_str()));
        }
        assert_eq!(Kind::parse("future_thing"), Kind::Other);
    }

    #[test]
    fn blocks_roundtrip_and_unknown_kept() {
        let vs = [
            json!({"type":"text","text":"hi"}),
            json!({"type":"image","media_type":"image/png","blob":"sha256:00","tokens_est":1600}),
            json!({"type":"thinking","provider":"deepseek","model":"m","text":"r"}),
            json!({"type":"tool_use","call_id":"c","name":"bash","input":{"command":"ls"},"raw_arguments":"{\"command\": \"ls\"}"}),
            json!({"type":"opaque","api":"messages","raw":{"type":"server_tool_use"}}),
            json!({"type":"brand_new","x":1}),
        ];
        for v in vs {
            assert_eq!(Block::from_value(&v).to_value(), v);
        }
    }

    #[test]
    fn unknown_type_and_fields_survive() {
        let line = br#"{"future":1,"id":"00000000000000aa","parent_id":null,"ts":"2026-10-19T20:30:15.123Z","type":"wormhole"}"#;
        let (ev, _) = Event::parse_line(line).unwrap();
        assert_eq!(ev.kind(), Kind::Other);
        assert_eq!(
            icj::to_vec(&Value::Object(ev.to_object().unwrap())).unwrap(),
            line.to_vec()
        );
        // A known type with an unknown field still parses, ignoring the field.
        let line = br#"{"id":"00000000000000ab","level":"info","parent_id":"00000000000000aa","shiny":true,"text":"x","ts":"2026-10-19T20:30:15.123Z","type":"display"}"#;
        let (ev, _) = Event::parse_line(line).unwrap();
        assert_eq!(ev.kind(), Kind::Display);
        // Broken envelopes are rejected.
        assert!(Event::parse_line(br#"{"id":"0000000000000000","parent_id":null,"ts":"2026-10-19T20:30:15.123Z","type":"x"}"#).is_none());
        assert!(
            Event::parse_line(
                br#"{"id":"00000000000000aa","parent_id":null,"ts":"2026-10-19 20:30","type":"x"}"#
            )
            .is_none()
        );
        assert!(
            Event::parse_line(
                br#"{"id":"00000000000000aa","ts":"2026-10-19T20:30:15.123Z","type":"x"}"#
            )
            .is_none()
        );
        assert!(Event::parse_line(b"\xff\xfe").is_none());
    }
}
