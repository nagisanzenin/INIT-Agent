//! The bounded view (log-context.md §3.7) and the projection from log to context (§3.9), grouped into
//! wire-ready turns (§3.13). The projection is a pure function of the path events and the profile.

use std::collections::BTreeMap;
use std::io;

use serde_json::{Map, Value};

use super::model::{CacheProfile, Calib};
use crate::log::clock::parse_id;
use crate::log::event::{Block, Event, EventBody, Kind, MaskEntry, MessageEv, ToolCallEv};
use crate::log::{SessionLog, icj};

/// One event of the active window, loaded from disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub ev: Event,
    /// Position on the path (root = 0).
    pub ppos: usize,
    /// Per-event token estimate (§3.14), before masking.
    pub tokens: u64,
}

impl Loaded {
    pub fn id(&self) -> u64 {
        self.ev.id
    }
    pub fn kind(&self) -> Kind {
        self.ev.kind()
    }
}

/// Mask key: (event id, part, call id for `input`).
pub type MaskKey = (u64, String, String);

pub fn mask_key(e: &MaskEntry) -> Option<MaskKey> {
    Some((
        parse_id(&e.id)?,
        e.part.clone(),
        e.call_id.clone().unwrap_or_default(),
    ))
}

/// §3.7: the live part of the session. Older events stay on disk (index offsets only).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// Boot block pinned in front of a summary (§3.12.5).
    pub prelude: Option<Loaded>,
    /// The latest compaction on the path.
    pub summary: Option<Loaded>,
    /// Path events from `first_kept` (or the path start) to the leaf, without the compaction itself.
    pub window: Vec<Loaded>,
    pub masked: BTreeMap<MaskKey, MaskEntry>,
    /// Id of the epoch-starting event (latest compaction or mask on the path), or the session root.
    pub epoch: u64,
    /// Path position of the epoch event (0 for the root).
    pub epoch_ppos: usize,
    /// Path length when built (positions of later appends continue from here).
    pub path_len: usize,
}

/// Token estimate of one event (§3.14): stored `tokens_est` for results, output tokens for assistant
/// messages whose thinking is replayed, `ceil(k·bytes)` for other projected text, 0 otherwise.
pub fn event_tokens(
    ev: &Event,
    calib: &Calib,
    profile: &CacheProfile,
    replays_thinking: bool,
) -> u64 {
    match &ev.body {
        EventBody::ToolResult(r) => r.tokens_est,
        EventBody::Message(m) if m.is_assistant() => {
            let visible = assistant_visible_bytes(m, profile);
            match (&m.usage, replays_thinking && m.has_thinking()) {
                (Some(u), true) => u.output_tokens.max(calib.tokens(visible)),
                _ => calib.tokens(visible),
            }
        }
        EventBody::Message(m) => blocks_tokens(&m.content, calib),
        EventBody::Steer(s) => blocks_tokens(&s.content, calib),
        EventBody::Command(_)
        | EventBody::ToolCall(_)
        | EventBody::Mask(_)
        | EventBody::Label(_) => 0,
        EventBody::Display(_)
        | EventBody::Error(_)
        | EventBody::SelfEdit(_)
        | EventBody::Session(_) => 0,
        EventBody::Other(..) => 0,
        b => b
            .stored_text()
            .map(|t| calib.tokens(t.len() as u64))
            .unwrap_or(0),
    }
}

fn blocks_tokens(blocks: &[Block], calib: &Calib) -> u64 {
    blocks
        .iter()
        .map(|b| match b {
            Block::Text { text } => calib.tokens(text.len() as u64),
            Block::Image { tokens_est, .. } => *tokens_est,
            _ => 0,
        })
        .sum()
}

/// Visible content bytes of an assistant message: text, tool arguments (raw when replayed raw).
pub fn assistant_visible_bytes(m: &MessageEv, profile: &CacheProfile) -> u64 {
    m.content
        .iter()
        .map(|b| match b {
            Block::Text { text } => text.len() as u64,
            Block::ToolUse {
                input,
                raw_arguments,
                name,
                ..
            } => {
                let args = match (raw_arguments, profile.replay_raw_arguments) {
                    (Some(r), true) => r.len() as u64,
                    _ => icj::to_vec(&Value::Object(input.clone()))
                        .map(|v| v.len() as u64)
                        .unwrap_or(0),
                };
                args + name.len() as u64
            }
            _ => 0,
        })
        .sum()
}

/// Whether the projection replays thinking of this assistant message to the target.
pub fn thinking_replayed(m: &MessageEv, target_provider: &str, target_model: &str) -> bool {
    m.content.iter().any(|b| {
        matches!(b, Block::Thinking { provider, model, .. }
        if provider == target_provider && model == target_model)
    })
}

/// Builds the view from the current path (§3.6 steps 7–8).
pub fn build_view(
    log: &SessionLog,
    calib: &Calib,
    profile: &CacheProfile,
    provider: &str,
    model: &str,
) -> io::Result<View> {
    let path = log.path();
    let ents = log.index.ents();
    let c_pp = path.iter().rposition(|&p| ents[p].kind == Kind::Compaction);
    let mut v = View {
        path_len: path.len(),
        ..Default::default()
    };
    let mut start = 0;
    if let Some(cp) = c_pp {
        let c = log.load(path[cp])?;
        if let EventBody::Compaction(cc) = &c.body {
            let fk = parse_id(&cc.first_kept_id);
            start = path[..cp]
                .iter()
                .position(|&p| Some(ents[p].id) == fk)
                .unwrap_or(cp + 1);
        }
        let tokens = event_tokens(&c, calib, profile, false);
        v.summary = Some(Loaded {
            ev: c,
            ppos: cp,
            tokens,
        });
        if let Some(bp) = path[..start]
            .iter()
            .rposition(|&p| ents[p].kind == Kind::Boot)
        {
            let b = log.load(path[bp])?;
            let tokens = event_tokens(&b, calib, profile, false);
            v.prelude = Some(Loaded {
                ev: b,
                ppos: bp,
                tokens,
            });
        }
    }
    // Sized once: an epoch's rebuild never reallocates (and copies) the window as it fills it.
    v.window.reserve_exact(path.len().saturating_sub(start));
    for (pp, &pos) in path.iter().enumerate().skip(start) {
        if Some(pp) == c_pp {
            continue;
        }
        let ev = log.load(pos)?;
        push_loaded(&mut v, ev, pp, calib, profile, provider, model);
    }
    let m_pp = path.iter().rposition(|&p| ents[p].kind == Kind::Mask);
    let e_pp = match (c_pp, m_pp) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    v.epoch_ppos = e_pp.unwrap_or(0);
    v.epoch = path.get(v.epoch_ppos).map(|&p| ents[p].id).unwrap_or(0);
    // A mask event comes after the events it masks, so the masked parts are known only now. Every
    // mask epoch rebuilds the view (engine `mask_epoch`), so this is the one place they are dropped.
    let View { window, masked, .. } = &mut v;
    for l in window.iter_mut() {
        slim_masked(&mut l.ev, masked);
    }
    Ok(v)
}

/// Adds an event appended at the leaf (or loaded) to the window, collecting mask entries.
pub fn push_loaded(
    v: &mut View,
    mut ev: Event,
    ppos: usize,
    calib: &Calib,
    profile: &CacheProfile,
    provider: &str,
    model: &str,
) {
    if let EventBody::Mask(m) = &mut ev.body {
        for mut e in std::mem::take(&mut m.masked) {
            if let Some(k) = mask_key(&e) {
                // The key holds the event id and the part; readers use the rest (§3.7 bound).
                e.id = String::new();
                e.part = String::new();
                v.masked.insert(k, e);
            }
        }
    }
    let replays =
        matches!(&ev.body, EventBody::Message(m) if thinking_replayed(m, provider, model));
    let tokens = event_tokens(&ev, calib, profile, replays);
    slim_unread(&mut ev);
    v.window.push(Loaded { ev, ppos, tokens });
    v.path_len = v.path_len.max(ppos + 1);
}

/// §3.7 memory bound: the view keeps only what its readers use (the projection, the estimates,
/// masking, the cut and the compaction's owner messages and artifacts). The log keeps the full
/// event, and a reader that needs more loads it from disk by offset. Called after the event's
/// token estimate is taken (the only reader of `usage`). This drops:
/// - a message's per-response metadata (`usage`, `request`, `stop_details`, `response_id`,
///   `stop_reason`, `provider`, `api`, `model`: thinking replay reads the blocks' own
///   provider and model);
/// - the signal's numbers (only its text is projected);
/// - the entries of a `mask` event (they move into `View::masked`);
/// - a `tool_call` marker's fields (only the log's open-call scan reads them, from disk; the
///   marker itself stays, so window positions and the compaction's `through_id` are unchanged);
/// - the timestamp, except on owner messages and steers (the compaction's owner-message list).
///
/// Between compactions the window keeps every masked step's events, so these bytes per step set
/// M-5's slope (FIX8, M5 lane).
fn slim_unread(ev: &mut Event) {
    let keep_ts = match &mut ev.body {
        EventBody::Message(m) => {
            m.usage = None;
            m.request = None;
            m.stop_details = None;
            m.response_id = None;
            m.stop_reason = None;
            m.provider = None;
            m.api = None;
            m.model = None;
            m.is_owner()
        }
        EventBody::Steer(_) => true,
        EventBody::Signal(s) => {
            s.extra = Map::new();
            false
        }
        EventBody::ToolCall(c) => {
            *c = ToolCallEv {
                call_id: String::new(),
                name: String::new(),
                message_id: String::new(),
                idempotency_key: None,
            };
            false
        }
        _ => false,
    };
    if !keep_ts {
        ev.ts = String::new();
    }
}

/// Drops the content that a mask replaces. The placeholder (and a masked input) is stored in the
/// `mask` entry (P2), so the projection never reads it from the event: a masked output becomes
/// empty, and a masked `write`/`edit` input becomes its masked form (the projection replays that
/// form). Without this the window would hold every masked output until the next compaction,
/// which masking postpones by design (§3.11), so memory would grow with the steps, not the tokens.
fn slim_masked(ev: &mut Event, masked: &BTreeMap<MaskKey, MaskEntry>) {
    if masked.is_empty() {
        return;
    }
    let id = ev.id;
    match &mut ev.body {
        EventBody::ToolResult(r) => {
            if !r.output.is_empty() && masked.contains_key(&(id, "output".into(), String::new())) {
                r.output = Vec::new();
            }
        }
        EventBody::Message(m) if m.is_assistant() => {
            for b in &mut m.content {
                if let Block::ToolUse {
                    call_id,
                    input,
                    raw_arguments,
                    ..
                } = b
                    && let Some(mi) = masked
                        .get(&(id, "input".into(), call_id.clone()))
                        .and_then(|e| e.input.as_ref())
                    && input != mi
                {
                    *input = mi.clone();
                    if let Some(r) = raw_arguments {
                        *r = String::new();
                    }
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// Projection and grouping
// ---------------------------------------------------------------------------------------------

/// One result of a `ToolResults` group.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolRes {
    pub event: u64,
    pub call_id: String,
    pub name: String,
    pub content: Vec<Block>,
    pub is_error: bool,
    pub tokens: u64,
}

/// The projected, grouped context (mirrors body/provider.md `Message`).
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    /// Owner text, steers, harness texts (boot block, summary, branch summary, cut-off note).
    User { blocks: Vec<Block>, tokens: u64 },
    /// One model response; blocks after masking and epoch thinking rules.
    Assistant {
        event: u64,
        blocks: Vec<Block>,
        tokens: u64,
    },
    /// Results for the preceding assistant message, in its `tool_use` block order.
    ToolResults(Vec<ToolRes>),
    /// The group's notes (signals, reload, notices), concatenated with `\n`.
    Note { text: String, tokens: u64 },
}

impl Msg {
    pub fn tokens(&self) -> u64 {
        match self {
            Msg::User { tokens, .. } | Msg::Assistant { tokens, .. } | Msg::Note { tokens, .. } => {
                *tokens
            }
            Msg::ToolResults(r) => r.iter().map(|x| x.tokens).sum(),
        }
    }
    pub fn is_assistant(&self) -> bool {
        matches!(self, Msg::Assistant { .. })
    }
}

enum Item {
    User(Vec<Block>, u64),
    Assistant(Msg, Vec<String>),
    Result(ToolRes),
    Note(String, u64),
}

/// `[init:masked] {desc} → ~{tokens} tokens, see event {id}` placeholder replay of masked outputs.
fn masked_output(v: &View, id: u64, output: &[Block]) -> Option<Vec<Block>> {
    if let Some(e) = v.masked.get(&(id, "output".into(), String::new())) {
        return Some(vec![Block::text(e.placeholder.clone())]);
    }
    if let Some(e) = v.masked.get(&(id, "image".into(), String::new())) {
        let mut out = Vec::new();
        let mut placed = false;
        for b in output {
            match b {
                Block::Image { .. } => {
                    if !placed {
                        out.push(Block::text(e.placeholder.clone()));
                        placed = true;
                    }
                }
                other => out.push(other.clone()),
            }
        }
        return Some(out);
    }
    None
}

fn assistant_blocks(v: &View, ev_id: u64, m: &MessageEv, strip_thinking: bool) -> Vec<Block> {
    let mut out = Vec::with_capacity(m.content.len());
    for b in &m.content {
        match b {
            Block::Thinking { .. } if strip_thinking => {}
            Block::ToolUse {
                call_id,
                name,
                raw_arguments,
                ..
            } => {
                match v
                    .masked
                    .get(&(ev_id, "input".into(), call_id.clone()))
                    .and_then(|e| e.input.clone())
                {
                    Some(mi) => {
                        let raw = raw_arguments.as_ref().map(|_| {
                            icj::to_string(&Value::Object(mi.clone())).unwrap_or_default()
                        });
                        out.push(Block::ToolUse {
                            call_id: call_id.clone(),
                            name: name.clone(),
                            input: mi,
                            raw_arguments: raw,
                        });
                    }
                    None => out.push(b.clone()),
                }
            }
            other => out.push(other.clone()),
        }
    }
    if out.is_empty() {
        out.push(Block::text("[no visible output]"));
    }
    out
}

fn text_item(text: &str, calib: &Calib) -> Item {
    Item::User(vec![Block::text(text)], calib.tokens(text.len() as u64))
}

/// §3.9: the projection, then §3.13 grouping.
pub fn project(v: &View, profile: &CacheProfile, calib: &Calib) -> Vec<Msg> {
    project_mode(v, profile, calib, false)
}

/// The projection as the estimate (§3.14) reads it, without copying the content it does not read:
/// every message keeps its token count, its call ids and (notes) its text, but only the messages
/// after the last assistant message keep their blocks, which the running estimate measures.
/// `estimate(&project_for_estimate(..)) == estimate(&project(..))` (tested); it saves two full
/// copies of the context's content per request (M-5: allocator churn and footprint).
pub fn project_for_estimate(v: &View, profile: &CacheProfile, calib: &Calib) -> Vec<Msg> {
    project_mode(v, profile, calib, true)
}

fn project_mode(v: &View, profile: &CacheProfile, calib: &Calib, light: bool) -> Vec<Msg> {
    // In light mode, window events before this index project without their blocks.
    let tail_from = if light {
        v.window
            .iter()
            .rposition(|l| matches!(&l.ev.body, EventBody::Message(m) if m.is_assistant()))
            .map(|i| i + 1)
            .unwrap_or(0)
    } else {
        0
    };
    let mut items: Vec<Item> = Vec::with_capacity(v.window.len() + 2);
    if let Some(c) = &v.summary {
        if let Some(p) = &v.prelude
            && let Some(t) = p.ev.body.stored_text()
        {
            items.push(text_item(t, calib));
        }
        if let Some(t) = c.ev.body.stored_text() {
            items.push(Item::User(vec![Block::text(t)], c.tokens));
        }
    }
    if tail_from > 0 {
        for it in &mut items {
            if let Item::User(blocks, _) = it {
                *blocks = Vec::new();
            }
        }
    }
    for (i, l) in v.window.iter().enumerate() {
        let id = l.id();
        let bare = i < tail_from;
        match &l.ev.body {
            EventBody::Message(m) if m.is_assistant() && bare => {
                let order: Vec<String> = m.tool_uses().map(|(c, _, _)| c.to_string()).collect();
                let tokens = masked_tokens(v, l, calib);
                items.push(Item::Assistant(
                    Msg::Assistant {
                        event: id,
                        blocks: Vec::new(),
                        tokens,
                    },
                    order,
                ));
            }
            EventBody::Message(m) if bare && !m.is_assistant() => {
                items.push(Item::User(Vec::new(), l.tokens))
            }
            EventBody::Steer(_) if bare => items.push(Item::User(Vec::new(), l.tokens)),
            EventBody::ToolResult(r) if bare => {
                let tokens = masked_tokens(v, l, calib);
                items.push(Item::Result(ToolRes {
                    event: id,
                    call_id: r.call_id.clone(),
                    name: String::new(),
                    content: Vec::new(),
                    is_error: r.is_error,
                    tokens,
                }));
            }
            EventBody::Boot(_) | EventBody::Branch(_) if bare => {
                if let Some(t) = l.ev.body.stored_text() {
                    items.push(Item::User(Vec::new(), calib.tokens(t.len() as u64)));
                }
            }
            EventBody::Message(m) if m.is_assistant() => {
                let strip = profile.strip_thinking_before_epoch && l.ppos < v.epoch_ppos;
                let blocks = assistant_blocks(v, id, m, strip);
                let order: Vec<String> = m.tool_uses().map(|(c, _, _)| c.to_string()).collect();
                let tokens = masked_tokens(v, l, calib);
                items.push(Item::Assistant(
                    Msg::Assistant {
                        event: id,
                        blocks,
                        tokens,
                    },
                    order,
                ));
            }
            EventBody::Message(m) => items.push(Item::User(m.content.clone(), l.tokens)),
            EventBody::Steer(s) => items.push(Item::User(s.content.clone(), l.tokens)),
            EventBody::ToolResult(r) => {
                let content = masked_output(v, id, &r.output).unwrap_or_else(|| r.output.clone());
                let tokens = masked_tokens(v, l, calib);
                items.push(Item::Result(ToolRes {
                    event: id,
                    call_id: r.call_id.clone(),
                    name: r.name.clone(),
                    content,
                    is_error: r.is_error,
                    tokens,
                }));
            }
            EventBody::Boot(b) => items.push(text_item(&b.text, calib)),
            EventBody::Branch(b) => {
                if let Some(t) = &b.text {
                    items.push(text_item(t, calib));
                }
            }
            EventBody::Gate(g) if g.is_async => {
                if let Some(t) = &g.text {
                    items.push(Item::Note(t.clone(), l.tokens));
                }
            }
            EventBody::Notice(n) if n.projected => items.push(Item::Note(n.text.clone(), l.tokens)),
            EventBody::Signal(_)
            | EventBody::Reload(_)
            | EventBody::UpdateFailed(_)
            | EventBody::ThinningDone(_) => {
                if let Some(t) = l.ev.body.stored_text() {
                    items.push(Item::Note(t.to_string(), l.tokens));
                }
            }
            _ => {}
        }
    }
    group(items, calib)
}

/// Token estimate of a window event after masking.
pub fn masked_tokens(v: &View, l: &Loaded, calib: &Calib) -> u64 {
    let id = l.id();
    let mut t = l.tokens;
    for ((mid, part, _), e) in v.masked.range((id, String::new(), String::new())..) {
        if *mid != id {
            break;
        }
        let ph = calib.tokens(e.placeholder.len() as u64);
        match part.as_str() {
            "output" => return ph,
            "input" => {
                let after = e
                    .input
                    .as_ref()
                    .and_then(|i| icj::to_vec(&Value::Object(i.clone())).ok())
                    .map(|b| b.len() as u64);
                t = t
                    .saturating_sub(e.tokens)
                    .saturating_add(calib.tokens(after.unwrap_or(e.placeholder.len() as u64)));
            }
            _ => t = t.saturating_sub(e.tokens).saturating_add(ph),
        }
    }
    t
}

fn group(items: Vec<Item>, calib: &Calib) -> Vec<Msg> {
    let mut out = Vec::with_capacity(items.len());
    let mut order: Vec<String> = Vec::new(); // tool_use order of the preceding assistant message
    let mut results: Vec<ToolRes> = Vec::new();
    let mut users: Vec<Msg> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let flush = |out: &mut Vec<Msg>,
                 order: &[String],
                 results: &mut Vec<ToolRes>,
                 users: &mut Vec<Msg>,
                 notes: &mut Vec<String>| {
        // Results whose call is not in the preceding assistant message cannot be sent; drop them.
        results.retain(|r| order.contains(&r.call_id));
        results.sort_by_key(|r| {
            order
                .iter()
                .position(|c| *c == r.call_id)
                .unwrap_or(usize::MAX)
        });
        if !results.is_empty() {
            out.push(Msg::ToolResults(std::mem::take(results)));
        }
        out.append(users);
        if !notes.is_empty() {
            let text = notes.join("\n");
            let tokens = calib.tokens(text.len() as u64);
            out.push(Msg::Note { text, tokens });
            notes.clear();
        }
    };
    for it in items {
        match it {
            Item::Assistant(m, o) => {
                flush(&mut out, &order, &mut results, &mut users, &mut notes);
                out.push(m);
                order = o;
            }
            Item::Result(r) => results.push(r),
            Item::User(blocks, tokens) => users.push(Msg::User { blocks, tokens }),
            Item::Note(t, _) => notes.push(t),
        }
    }
    flush(&mut out, &order, &mut results, &mut users, &mut notes);
    out
}

/// Content bytes and image tokens of a message list (§3.14 "content bytes").
pub fn content_bytes(msgs: &[Msg], profile: &CacheProfile) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut images = 0u64;
    let blocks = |bs: &[Block], bytes: &mut u64, images: &mut u64| {
        for b in bs {
            match b {
                Block::Text { text } => *bytes += text.len() as u64,
                Block::Image { tokens_est, .. } => *images += tokens_est,
                Block::Thinking { text, .. } => *bytes += text.len() as u64,
                Block::ToolUse {
                    input,
                    raw_arguments,
                    name,
                    ..
                } => {
                    *bytes += name.len() as u64
                        + match (raw_arguments, profile.replay_raw_arguments) {
                            (Some(r), true) => r.len() as u64,
                            _ => icj::to_vec(&Value::Object(input.clone()))
                                .map(|v| v.len() as u64)
                                .unwrap_or(0),
                        }
                }
                _ => {}
            }
        }
    };
    for m in msgs {
        match m {
            Msg::User { blocks: bs, .. } | Msg::Assistant { blocks: bs, .. } => {
                blocks(bs, &mut bytes, &mut images)
            }
            Msg::ToolResults(rs) => rs
                .iter()
                .for_each(|r| blocks(&r.content, &mut bytes, &mut images)),
            Msg::Note { text, .. } => bytes += text.len() as u64,
        }
    }
    (bytes, images)
}

/// Replaces long string values of a `write`/`edit` input with `[init:masked ~{tokens} tokens, see event
/// {id}]` (§3.11). Returns the masked input when anything was replaced.
pub fn mask_input(
    name: &str,
    input: &Map<String, Value>,
    event: u64,
    calib: &Calib,
    min_tokens: u64,
) -> Option<Map<String, Value>> {
    let ph = |s: &str| {
        format!(
            "[init:masked ~{} tokens, see event {}]",
            calib.tokens(s.len() as u64),
            crate::log::clock::id_hex(event)
        )
    };
    let long = |s: &str| calib.tokens(s.len() as u64) > min_tokens;
    let mut out = input.clone();
    let mut changed = false;
    match name {
        "write" => {
            if let Some(Value::String(s)) = input.get("content")
                && long(s)
            {
                out.insert("content".into(), Value::String(ph(s)));
                changed = true;
            }
        }
        "edit" => {
            if let Some(Value::Array(edits)) = input.get("edits") {
                let mut new_edits = Vec::with_capacity(edits.len());
                for e in edits {
                    let mut e2 = e.clone();
                    if let Value::Object(o) = &mut e2 {
                        for k in ["old_text", "new_text"] {
                            if let Some(Value::String(s)) = o.get(k)
                                && long(s)
                            {
                                let p = ph(s);
                                o.insert(k.into(), Value::String(p));
                                changed = true;
                            }
                        }
                    }
                    new_edits.push(e2);
                }
                out.insert("edits".into(), Value::Array(new_edits));
            }
        }
        _ => {}
    }
    if changed { Some(out) } else { None }
}
