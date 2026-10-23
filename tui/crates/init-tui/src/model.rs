//! The TUI's state (docs/tui.md §3.5, §3.6, §3.9, §3.11): the feed (replay, epochs and the overlap
//! dedup, so no frame is shown twice), the echo merge of owner lines (never resent), the transcript
//! blocks and the status-bar data.

use crate::notice::{self, Class};
use crate::proto::{self, Display, DisplayKind, Frame, LineKind};
use serde_json::Value;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

// -------------------------------------------------------------------------------------------
// The feed: replay, epochs, no duplicates (§3.11)
// -------------------------------------------------------------------------------------------

/// Bounds of the kept frames: the Root's ring bounds (root/root.md §3.15).
pub const KEEP_FRAMES: usize = 2000;
pub const KEEP_BYTES: usize = 256 * 1024;
const OVERLAP: usize = 64;

pub const DIV_MISSING: &str = "reconnected; some output may be missing";

#[derive(Debug)]
enum Phase {
    Idle,
    Replay(Vec<(String, Value)>),
    Greeting,
    Live,
}

/// What the feed passes on to the front end.
#[derive(Debug, PartialEq)]
pub enum FeedOut {
    Frame(Value),
    /// A raw frame for `--raw` (the frame as received, after the dedup).
    Divider(String),
    /// The Root's `[root] attached: …` greeting at the first attach.
    Greeting(Value),
    /// The greeting at a later attach: shown as `reconnected` in the status bar instead.
    Reconnected,
}

#[derive(Debug)]
pub struct Feed {
    /// The epoch of what has been shown (`root.started_at/root.pid`), when known.
    pub epoch: Option<String>,
    kept: VecDeque<String>,
    kept_bytes: usize,
    /// The largest `rseq` shown in this epoch (§9 Q1), when the Root numbers its frames.
    pub last_rseq: Option<u64>,
    phase: Phase,
    pub attaches: u32,
    attempt_epoch: Option<String>,
    attempt_started: Option<String>,
    /// The raw lines accepted, for `--raw` (filled only when `want_raw`).
    pub want_raw: bool,
    pub raw_out: Vec<String>,
}

impl Default for Feed {
    fn default() -> Self {
        Feed::new()
    }
}

impl Feed {
    pub fn new() -> Feed {
        Feed {
            epoch: None,
            kept: VecDeque::new(),
            kept_bytes: 0,
            last_rseq: None,
            phase: Phase::Idle,
            attaches: 0,
            attempt_epoch: None,
            attempt_started: None,
            want_raw: false,
            raw_out: Vec::new(),
        }
    }

    /// `since` for the next attach (§9 Q1): only once the Root has shown that it numbers its frames.
    pub fn since(&self) -> Option<(u64, String)> {
        Some((self.last_rseq?, self.epoch.clone()?))
    }

    pub fn is_live(&self) -> bool {
        matches!(self.phase, Phase::Live)
    }

    /// A new attach is starting; `epoch` and `started_at` come from the `status` asked just before.
    pub fn on_attach(&mut self, epoch: Option<String>, started_at: Option<String>) {
        self.phase = Phase::Replay(Vec::new());
        self.attempt_epoch = epoch;
        self.attempt_started = started_at;
    }

    pub fn on_disconnect(&mut self) {
        self.phase = Phase::Idle;
    }

    fn keep(&mut self, raw: &str) {
        self.kept.push_back(raw.to_string());
        self.kept_bytes += raw.len();
        while self.kept.len() > KEEP_FRAMES || self.kept_bytes > KEEP_BYTES {
            let Some(x) = self.kept.pop_front() else { break };
            self.kept_bytes -= x.len();
        }
    }

    fn accept(&mut self, raw: String, v: Value, out: &mut Vec<FeedOut>) {
        if let Some(r) = proto::rseq(&v) {
            self.last_rseq = Some(self.last_rseq.map_or(r, |l| l.max(r)));
        }
        self.keep(&raw);
        if self.want_raw {
            self.raw_out.push(raw);
        }
        out.push(FeedOut::Frame(v));
    }

    /// One frame from the attach connection.
    pub fn on_frame(&mut self, raw: &str, v: Value) -> Vec<FeedOut> {
        let raw = raw.trim_end_matches(['\n', '\r']).to_string();
        let mut out = Vec::new();
        match &mut self.phase {
            Phase::Idle => {} // a late frame of a closed connection
            Phase::Replay(buf) => {
                if let Frame::ReplayEnd { epoch, gap } = proto::classify(&v) {
                    let buf = std::mem::take(buf);
                    if self.want_raw {
                        self.raw_out.push(raw.clone());
                    }
                    self.end_replay(buf, epoch, gap, &mut out);
                    self.phase = Phase::Greeting;
                } else {
                    buf.push((raw, v));
                }
            }
            Phase::Greeting => {
                self.phase = Phase::Live;
                let greeting = matches!(proto::classify(&v), Frame::Notice { ref text, .. } if notice::is_greeting(text, proto::src(&v)))
                    && proto::rseq(&v).is_none();
                if greeting {
                    if self.want_raw {
                        self.raw_out.push(raw);
                    }
                    out.push(if self.attaches == 1 { FeedOut::Greeting(v) } else { FeedOut::Reconnected });
                } else {
                    return self.on_frame_live(raw, v);
                }
            }
            Phase::Live => return self.on_frame_live(raw, v),
        }
        out
    }

    fn on_frame_live(&mut self, raw: String, v: Value) -> Vec<FeedOut> {
        let mut out = Vec::new();
        if let (Some(r), Some(l)) = (proto::rseq(&v), self.last_rseq)
            && r <= l
        {
            return out; // already shown (§9 Q1)
        }
        self.accept(raw, v, &mut out);
        out
    }

    fn end_replay(&mut self, r: Vec<(String, Value)>, epoch: Option<String>, gap: bool, out: &mut Vec<FeedOut>) {
        let epoch_now = epoch.or_else(|| self.attempt_epoch.clone());
        let first = self.attaches == 0;
        self.attaches += 1;
        let new_epoch = !first && matches!((&self.epoch, &epoch_now), (Some(a), Some(b)) if a != b);
        if epoch_now.is_some() {
            self.epoch = epoch_now;
        }
        if first {
            for (raw, v) in r {
                self.accept(raw, v, out);
            }
            return;
        }
        if new_epoch {
            self.kept.clear();
            self.kept_bytes = 0;
            self.last_rseq = None;
            let at = self.attempt_started.as_deref().and_then(crate::time::parse_rfc3339).map(crate::time::utc_hms).unwrap_or_else(|| "?".into());
            out.push(FeedOut::Divider(format!("Root restarted ({at}); output from while it was down is in the session log, not replayed")));
            for (raw, v) in r {
                self.accept(raw, v, out);
            }
            return;
        }
        if r.iter().any(|(_, v)| proto::rseq(v).is_some()) && self.last_rseq.is_some() {
            // numbered frames (§9 Q1): the Root sent only newer ones; keep the check anyway
            let last = self.last_rseq.unwrap_or(0);
            let fresh: Vec<_> = r.into_iter().filter(|(_, v)| proto::rseq(v).is_none_or(|x| x > last)).collect();
            let lost = gap || fresh.first().and_then(|(_, v)| proto::rseq(v)).is_some_and(|x| x > last + 1) && !self.kept.is_empty();
            if lost {
                out.push(FeedOut::Divider(DIV_MISSING.into()));
            }
            for (raw, v) in fresh {
                self.accept(raw, v, out);
            }
            return;
        }
        // as built: overlap matching over the exact bytes (§3.11)
        let n = r.len();
        if self.kept.is_empty() {
            for (raw, v) in r {
                self.accept(raw, v, out);
            }
            return;
        }
        let k = self.kept.len();
        let mut matches = Vec::new();
        for j in 1..=n {
            let m = OVERLAP.min(k).min(j);
            if (0..m).all(|i| r[j - m + i].0 == self.kept[k - m + i]) {
                matches.push(j);
            }
        }
        let start = match matches.last() {
            None => {
                if n > 0 {
                    out.push(FeedOut::Divider(DIV_MISSING.into()));
                }
                0
            }
            Some(&j) => {
                if matches.len() > 1 {
                    out.push(FeedOut::Divider(DIV_MISSING.into()));
                }
                j
            }
        };
        for (raw, v) in r.into_iter().skip(start) {
            self.accept(raw, v, out);
        }
    }
}

// -------------------------------------------------------------------------------------------
// Owner lines and their echoes (§3.11)
// -------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerSt {
    /// Sent, no echo yet (dim).
    Pending,
    /// The Root queued it (no kernel ran); the kernel's echo will follow.
    Queued,
    Delivered,
    NotQueued,
    Unconfirmed,
    /// A Root or kernel command: no echo.
    Command,
}

#[derive(Debug)]
struct Awaiting {
    id: u64,
    text: String,
    st: OwnerSt,
    kind: LineKind,
    at: Instant,
}

/// The result of an echo: which line it belongs to, and its new state.
#[derive(Debug, PartialEq)]
pub enum EchoOut {
    /// A line this TUI sent (or an earlier foreign Root echo) changed state.
    Update(u64, OwnerSt),
    /// An echo that matches no pending line: shown as an ordinary owner line.
    New(u64, OwnerSt),
    /// A Root echo of a command line: absorbed.
    Absorbed(u64),
}

#[derive(Debug, Default)]
pub struct Echoes {
    awaiting: VecDeque<Awaiting>,
    next_id: u64,
    /// Kernel echoes seen (for `--wait-idle`).
    pub kernel_echoes: u64,
}

impl Echoes {
    pub fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// A line was sent on the attach connection. Returns its id and its initial state.
    pub fn sent(&mut self, text: &str, kind: LineKind, now: Instant) -> (u64, OwnerSt) {
        let id = self.new_id();
        let st = match kind {
            LineKind::Input => OwnerSt::Pending,
            _ => OwnerSt::Command,
        };
        if kind != LineKind::RootCommand {
            self.awaiting.push_back(Awaiting { id, text: text.to_string(), st: OwnerSt::Pending, kind, at: now });
        }
        (id, st)
    }

    pub fn on_echo(&mut self, text: &str, qid: Option<&str>, seq: Option<u64>, now: Instant) -> EchoOut {
        let kernel = seq.is_some() || qid.is_none();
        if kernel {
            self.kernel_echoes += 1;
            if let Some(i) = self.awaiting.iter().position(|a| a.kind == LineKind::Input && proto::echo_matches(&a.text, text)) {
                let a = self.awaiting.remove(i).expect("index");
                return EchoOut::Update(a.id, OwnerSt::Delivered);
            }
            let id = self.new_id();
            return EchoOut::New(id, OwnerSt::Delivered);
        }
        // a Root echo: the line is in the Root's durable queue
        if let Some(i) = self.awaiting.iter().position(|a| a.st == OwnerSt::Pending && a.text == text) {
            let a = &mut self.awaiting[i];
            if a.kind == LineKind::KernelCommand {
                let id = a.id;
                self.awaiting.remove(i);
                return EchoOut::Absorbed(id);
            }
            a.st = OwnerSt::Queued;
            return EchoOut::Update(a.id, OwnerSt::Queued);
        }
        let id = self.new_id();
        if proto::line_kind(text, false) == LineKind::Input {
            self.awaiting.push_back(Awaiting { id, text: text.to_string(), st: OwnerSt::Queued, kind: LineKind::Input, at: now });
        }
        EchoOut::New(id, OwnerSt::Queued)
    }

    /// The Root refused the newest line (`the owner queue is full`).
    pub fn queue_full(&mut self) -> Option<u64> {
        let i = self.awaiting.iter().rposition(|a| a.st == OwnerSt::Pending)?;
        Some(self.awaiting.remove(i).expect("index").id)
    }

    /// Lines still unconfirmed 30 s after the kernel is ready again and the Root's queue is empty.
    pub fn sweep(&mut self, now: Instant, ready_since: Option<Instant>, root_queued: Option<u64>) -> Vec<u64> {
        let mut gone = Vec::new();
        self.awaiting.retain(|a| {
            if a.kind == LineKind::KernelCommand && now.duration_since(a.at) > Duration::from_secs(30) {
                return false;
            }
            let (Some(r), Some(0)) = (ready_since, root_queued) else { return true };
            if a.kind == LineKind::Input && now.duration_since(a.at.max(r)) >= Duration::from_secs(30) {
                gone.push(a.id);
                return false;
            }
            true
        });
        gone
    }

    /// Lines of this TUI still waiting for a kernel echo.
    pub fn unconfirmed_inputs(&self) -> usize {
        self.awaiting.iter().filter(|a| a.kind == LineKind::Input).count()
    }
}

// -------------------------------------------------------------------------------------------
// Transcript blocks (§3.5, §3.6)
// -------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Tool {
    pub call_id: String,
    pub name: String,
    pub start_text: String,
    pub ok: Option<bool>,
    pub summary: Option<String>,
    pub detail: Option<String>,
    pub started: Instant,
    pub start_ms: i64,
    pub dur: Option<Duration>,
}

#[derive(Clone, Debug)]
pub enum Kind {
    Owner { id: u64, text: String, st: OwnerSt },
    /// Logical lines; the last one may still be growing while `!done`.
    Text { lines: Vec<String>, done: bool },
    Thinking { text: String, done: bool, started: Instant, dur: Option<Duration> },
    Tool(Tool),
    TurnEnd(String),
    Info(String),
    Error(String),
    Notice { class: Class, level: String, text: String },
    Divider(String),
    Retry,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub kind: Kind,
    pub at_ms: i64,
    pub expanded: bool,
}

impl Block {
    /// Can this block be printed once and never redrawn?
    pub fn finished(&self) -> bool {
        match &self.kind {
            Kind::Owner { st, .. } => *st != OwnerSt::Pending,
            Kind::Text { done, .. } => *done,
            Kind::Thinking { done, .. } => *done,
            Kind::Tool(t) => t.ok.is_some() || t.dur.is_some(),
            _ => true,
        }
    }
    fn bytes(&self) -> usize {
        match &self.kind {
            Kind::Owner { text, .. } | Kind::TurnEnd(text) | Kind::Info(text) | Kind::Error(text) | Kind::Divider(text) => text.len(),
            Kind::Notice { text, .. } => text.len(),
            Kind::Text { lines, .. } => lines.iter().map(|l| l.len() + 1).sum(),
            Kind::Thinking { text, .. } => text.len(),
            Kind::Tool(t) => t.start_text.len() + t.summary.as_ref().map_or(0, String::len) + t.detail.as_ref().map_or(0, String::len),
            Kind::Retry => 0,
        }
    }
}

/// The transcript view keeps at most this many blocks or bytes (§3.6).
pub const MAX_BLOCKS: usize = 50_000;
pub const MAX_BYTES: usize = 32 << 20;

#[derive(Debug, Default)]
pub struct Transcript {
    pub blocks: VecDeque<Block>,
    /// Absolute index of `blocks[0]` (blocks dropped from the view are gone).
    pub base: usize,
    /// Absolute count of blocks already printed above the live region.
    pub committed: usize,
    /// Logical lines of the block at `committed` already printed (a streaming text block).
    pub text_committed: usize,
    /// The Markdown state at that point.
    pub md_at_commit: crate::render::md::MdState,
    open_text: Option<usize>,
    open_thinking: Option<usize>,
    last_boot: Option<u64>,
    bytes: usize,
    /// Set when a new block was added or one changed (for the renderer).
    pub dirty: bool,
}

impl Transcript {
    pub fn len(&self) -> usize {
        self.base + self.blocks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
    pub fn get(&self, abs: usize) -> Option<&Block> {
        self.blocks.get(abs.checked_sub(self.base)?)
    }
    pub fn get_mut(&mut self, abs: usize) -> Option<&mut Block> {
        self.dirty = true;
        self.blocks.get_mut(abs.checked_sub(self.base)?)
    }

    pub fn push(&mut self, kind: Kind, at_ms: i64) -> usize {
        let b = Block { kind, at_ms, expanded: false };
        self.bytes += b.bytes();
        self.blocks.push_back(b);
        self.dirty = true;
        self.trim();
        self.len() - 1
    }

    fn trim(&mut self) {
        if self.blocks.len() <= MAX_BLOCKS && self.bytes <= MAX_BYTES {
            return;
        }
        // drop the oldest tenth, but never a block that is not yet printed
        let n = (self.blocks.len() / 10).max(1).min(self.committed.saturating_sub(self.base));
        for _ in 0..n {
            if let Some(b) = self.blocks.pop_front() {
                self.bytes -= b.bytes();
                self.base += 1;
            }
        }
    }

    /// Tool calls still running when the turn ends or the body restarts never get a `tool_end`:
    /// they end here as `no result`, so they no longer hold the blocks after them in the live region.
    fn close_tools(&mut self, now: Instant) {
        let start = self.committed.max(self.base) - self.base;
        let mut changed = false;
        for b in self.blocks.iter_mut().skip(start) {
            if let Kind::Tool(t) = &mut b.kind
                && t.dur.is_none()
            {
                t.dur = Some(now.duration_since(t.started));
                changed = true;
            }
        }
        self.dirty |= changed;
    }

    fn close_open(&mut self, now: Instant) {
        if let Some(i) = self.open_text.take()
            && let Some(b) = self.get_mut(i)
            && let Kind::Text { done, .. } = &mut b.kind
        {
            *done = true;
        }
        if let Some(i) = self.open_thinking.take()
            && let Some(b) = self.get_mut(i)
            && let Kind::Thinking { done, dur, started, .. } = &mut b.kind
        {
            *done = true;
            *dur = Some(now.duration_since(*started));
        }
    }

    /// The owner line of id `id`, if still in the view.
    pub fn owner_mut(&mut self, id: u64) -> Option<&mut Block> {
        self.dirty = true;
        self.blocks.iter_mut().rev().find(|b| matches!(b.kind, Kind::Owner { id: i, .. } if i == id))
    }

    pub fn set_owner(&mut self, id: u64, new: OwnerSt) {
        if let Some(b) = self.owner_mut(id)
            && let Kind::Owner { st, .. } = &mut b.kind
        {
            *st = new;
        }
    }

    pub fn display(&mut self, d: &Display, now: Instant, now_ms: i64) {
        if let Some(b) = d.boot_seq {
            if self.last_boot.is_some_and(|p| p != b) {
                self.close_open(now);
                self.close_tools(now);
                self.push(Kind::Divider(format!("body restarted (boot {b})")), now_ms);
            }
            self.last_boot = Some(b);
        }
        match d.kind {
            DisplayKind::Text => {
                if let Some(i) = self.open_thinking.take() {
                    self.open_thinking = Some(i);
                    let t = self.open_text.take();
                    self.close_open(now);
                    self.open_text = t;
                }
                let i = match self.open_text {
                    Some(i) if self.get(i).is_some() => i,
                    _ => {
                        let i = self.push(Kind::Text { lines: vec![String::new()], done: false }, now_ms);
                        self.open_text = Some(i);
                        i
                    }
                };
                self.bytes += d.text.len();
                if let Some(b) = self.get_mut(i)
                    && let Kind::Text { lines, .. } = &mut b.kind
                {
                    let mut parts = d.text.split('\n');
                    if let Some(first) = parts.next() {
                        lines.last_mut().expect("never empty").push_str(first);
                    }
                    for p in parts {
                        lines.push(p.to_string());
                    }
                }
            }
            DisplayKind::Thinking => {
                if let Some(i) = self.open_text.take()
                    && let Some(b) = self.get_mut(i)
                    && let Kind::Text { done, .. } = &mut b.kind
                {
                    *done = true;
                }
                let i = match self.open_thinking {
                    Some(i) if self.get(i).is_some() => i,
                    _ => {
                        let i = self.push(Kind::Thinking { text: String::new(), done: false, started: now, dur: None }, now_ms);
                        self.open_thinking = Some(i);
                        i
                    }
                };
                self.bytes += d.text.len();
                if let Some(b) = self.get_mut(i)
                    && let Kind::Thinking { text, .. } = &mut b.kind
                {
                    text.push_str(&d.text);
                }
            }
            DisplayKind::ToolStart => {
                self.close_open(now);
                let call_id = d.call_id.clone().unwrap_or_default();
                let name = d.name.clone().unwrap_or_else(|| "tool".into());
                self.push(Kind::Tool(Tool { call_id, name, start_text: d.text.clone(), ok: None, summary: None, detail: d.detail.clone(), started: now, start_ms: now_ms, dur: None }), now_ms);
            }
            DisplayKind::ToolEnd => {
                self.close_open(now);
                let cid = d.call_id.clone().unwrap_or_default();
                // the most recent unfinished call with this id, among the last 1,000 blocks
                let found = self.blocks.iter_mut().rev().take(1000).find_map(|b| match &mut b.kind {
                    Kind::Tool(t) if t.call_id == cid && t.dur.is_none() => Some(t),
                    _ => None,
                });
                match found {
                    Some(t) => {
                        t.ok = Some(d.ok.unwrap_or(false));
                        t.summary = Some(d.text.clone());
                        t.dur = Some(now.duration_since(t.started));
                        if d.detail.is_some() {
                            t.detail = d.detail.clone();
                        }
                        self.dirty = true;
                    }
                    None => {
                        let name = d.name.clone().unwrap_or_else(|| "tool".into());
                        self.push(
                            Kind::Tool(Tool { call_id: cid, name, start_text: String::new(), ok: Some(d.ok.unwrap_or(false)), summary: Some(d.text.clone()), detail: d.detail.clone(), started: now, start_ms: now_ms, dur: Some(Duration::ZERO) }),
                            now_ms,
                        );
                    }
                }
            }
            DisplayKind::TurnEnd => {
                self.close_open(now);
                self.close_tools(now);
                self.push(Kind::TurnEnd(d.text.clone()), now_ms);
            }
            DisplayKind::Info | DisplayKind::Other => {
                if d.kind == DisplayKind::Info && d.text.trim_start().starts_with("retrying") {
                    self.retry();
                }
                self.close_open(now);
                self.push(Kind::Info(d.text.clone()), now_ms);
            }
            DisplayKind::Error => {
                self.close_open(now);
                self.push(Kind::Error(d.text.clone()), now_ms);
            }
        }
    }

    /// A retry (body/loop.md §3.6): the partial text still live is discarded; if part of it was
    /// already printed, a dim `↺ retry` line says so.
    fn retry(&mut self) {
        let Some(i) = self.open_text.take() else { return };
        let printed = if i == self.committed { self.text_committed } else if i < self.committed { usize::MAX } else { 0 };
        if printed == 0 {
            // nothing printed: the block goes (it is the newest or close to it)
            if let Some(b) = self.get_mut(i) {
                b.kind = Kind::Text { lines: vec![String::new()], done: true };
            }
            return;
        }
        if let Some(b) = self.get_mut(i)
            && let Kind::Text { lines, done } = &mut b.kind
        {
            if printed != usize::MAX {
                lines.truncate(printed);
            }
            *done = true;
        }
        self.push(Kind::Retry, 0);
    }

    pub fn notice(&mut self, level: &str, text: &str, src: Option<&str>, code: Option<&str>, now_ms: i64) {
        let class = notice::classify(text, src, code);
        self.push(Kind::Notice { class, level: level.to_string(), text: text.to_string() }, now_ms);
    }

    /// Blocks after the printed ones: the live region's content.
    pub fn live(&self) -> impl Iterator<Item = (usize, &Block)> {
        let start = self.committed.max(self.base);
        (start..self.len()).filter_map(move |i| self.get(i).map(|b| (i, b)))
    }

    /// Is the text block at `abs` empty (a retry removed all of it)?
    #[cfg(test)]
    pub fn is_blank_text(&self, abs: usize) -> bool {
        matches!(self.get(abs).map(|b| &b.kind), Some(Kind::Text { lines, .. }) if lines.iter().all(String::is_empty))
    }
}

// -------------------------------------------------------------------------------------------
// Status-bar data (§3.9)
// -------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Conn {
    Connecting,
    Live,
    /// Disconnected; the next attempt at `retry_at`.
    Down { retry_at: Instant },
}

#[derive(Debug)]
pub struct Info {
    pub status: Option<Value>,
    pub egress: Option<Value>,
    /// `egress.stats` refused or failed: the money fields show `?`.
    pub egress_failed: bool,
    pub state: Option<Value>,
    pub busy_since: Option<Instant>,
    pub conn: Conn,
    pub day_spent_at_attach: Option<i64>,
    pub reconnected_at: Option<Instant>,
    pub ready_since: Option<Instant>,
    pub level_flash_until: Option<Instant>,
    last_level: Option<i64>,
    /// Ceiling notices raised by the TUI, by key (`day:<resets_at>`, `total`).
    pub ceilings_noticed: Vec<String>,
    /// ADR-009: the Root's `AccessView` (status `access`, or an access request's answer).
    pub access: Option<Value>,
    /// `$HOME`, to show the workspace as `~/…` (display only).
    pub home: Option<String>,
}

impl Default for Info {
    fn default() -> Self {
        Info {
            status: None,
            egress: None,
            egress_failed: false,
            state: None,
            busy_since: None,
            conn: Conn::Connecting,
            day_spent_at_attach: None,
            reconnected_at: None,
            ready_since: None,
            level_flash_until: None,
            last_level: None,
            ceilings_noticed: Vec::new(),
            access: None,
            home: None,
        }
    }
}

impl Info {
    pub fn on_status(&mut self, v: Value, now: Instant) {
        let ready = v["kernel"]["ready"] == true;
        if ready && self.ready_since.is_none() {
            self.ready_since = Some(now);
        } else if !ready {
            self.ready_since = None;
        }
        let level = v["level"].as_i64();
        if self.last_level.is_some() && level != self.last_level {
            self.level_flash_until = Some(now + Duration::from_secs(3));
        }
        self.last_level = level;
        if v["access"].is_object() {
            self.access = Some(v["access"].clone());
        }
        self.status = Some(v);
    }

    /// An `AccessView` from an access request's answer.
    pub fn on_access(&mut self, v: Value) {
        if v.is_object() {
            self.access = Some(v);
        }
    }

    pub fn on_egress(&mut self, v: Value) {
        if self.day_spent_at_attach.is_none() {
            self.day_spent_at_attach = v["money"]["day_spent_musd"].as_i64();
        }
        self.egress = Some(v);
        self.egress_failed = false;
    }

    pub fn on_state(&mut self, v: Value, now: Instant) {
        let busy = v["busy"] == true;
        match (busy, self.busy_since) {
            (true, None) => self.busy_since = Some(now),
            (false, _) => self.busy_since = None,
            _ => {}
        }
        self.state = Some(v);
    }

    pub fn busy(&self) -> bool {
        self.busy_since.is_some()
    }

    /// The Root's slot is panicked, halted or stopped (where `/resume` is the Root's).
    pub fn rootish(&self) -> bool {
        self.status.as_ref().is_some_and(|s| matches!(s["slot"].as_str(), Some("panicked" | "halted" | "stopped")))
    }

    pub fn kernel_ready(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s["kernel"]["ready"] == true)
    }

    /// The TUI's own ceiling notices (§3.10): once per owner day for the day ceiling, once for the total.
    pub fn ceiling_notices(&mut self, now_ms: i64) -> Vec<String> {
        let mut out = Vec::new();
        let Some(m) = self.egress.as_ref().map(|e| e["money"].clone()) else { return out };
        if self.egress_failed || m["enforced"] == false || m["policy"] == "observe" {
            return out;
        }
        let n = |k: &str| m[k].as_i64();
        if let (Some(spent), Some(limit)) = (n("day_spent_musd"), n("day_limit_musd")) {
            let reserved = n("day_reserved_musd").unwrap_or(0);
            let resets = m["day_resets_at"].as_str().unwrap_or("").to_string();
            let key = format!("day:{resets}");
            if limit > 0 && spent + reserved >= limit && !self.ceilings_noticed.contains(&key) {
                let when = crate::time::parse_rfc3339(&resets).map(|t| format!(" ({})", crate::time::until(t, now_ms))).unwrap_or_default();
                out.push(format!(
                    "■ daily ceiling reached: {} of {}; requests are refused until the day resets{when}",
                    crate::render::usd(spent),
                    crate::render::usd(limit)
                ));
                self.ceilings_noticed.push(key);
            }
        }
        if let (Some(spent), Some(limit)) = (n("total_spent_musd"), n("total_limit_musd"))
            && limit > 0
            && spent >= limit
            && !self.ceilings_noticed.iter().any(|k| k == "total")
        {
            out.push(format!("■ total ceiling reached: {} of {}; requests are refused until the owner raises it", crate::render::usd(spent), crate::render::usd(limit)));
            self.ceilings_noticed.push("total".into());
        }
        out
    }
}

/// The epoch of a `status` answer (§3.11): `root.epoch` (§9 Q1) or `started_at/pid`; None if absent.
pub fn epoch_of(status: &Value) -> Option<String> {
    let r = &status["root"];
    if let Some(e) = r["epoch"].as_str() {
        return Some(e.to_string());
    }
    Some(format!("{}/{}", r["started_at"].as_str()?, r["pid"].as_u64()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn f(n: u64) -> (String, Value) {
        let v = json!({"kind": "text", "t": "display", "text": format!("f{n}")});
        (proto::icj(&v), v)
    }

    fn frames(out: &[FeedOut]) -> Vec<String> {
        out.iter()
            .map(|o| match o {
                FeedOut::Frame(v) => v["text"].as_str().unwrap_or("?").to_string(),
                FeedOut::Divider(d) => format!("--{d}"),
                FeedOut::Greeting(_) => "hello".into(),
                FeedOut::Reconnected => "reconnected".into(),
            })
            .collect()
    }

    fn attach(feed: &mut Feed, epoch: &str, ring: &[(String, Value)]) -> Vec<String> {
        feed.on_attach(Some(epoch.into()), Some("2026-10-20T21:04:05Z".into()));
        let mut out = Vec::new();
        for (raw, v) in ring {
            out.extend(feed.on_frame(raw, v.clone()));
        }
        out.extend(feed.on_frame(r#"{"t":"replay_end"}"#, json!({"t": "replay_end"})));
        let g = json!({"level": "info", "t": "notice", "text": "[root] attached: slot running", "ts": "x"});
        out.extend(feed.on_frame(&proto::icj(&g), g.clone()));
        frames(&out)
    }

    #[test]
    fn first_attach_shows_the_replay_and_the_greeting() {
        let mut feed = Feed::new();
        let ring: Vec<_> = (1..=3).map(f).collect();
        assert_eq!(attach(&mut feed, "e1", &ring), ["f1", "f2", "f3", "hello"]);
    }

    #[test]
    fn same_epoch_overlap_at_every_drop_point() {
        // TUI-6: the server drops the connection after n frames, at many n
        let all: Vec<_> = (1..=40).map(f).collect();
        for n in 0..=40 {
            let mut feed = Feed::new();
            let mut shown = attach(&mut feed, "e1", &all[..n]);
            shown.retain(|s| s != "hello");
            let more = attach(&mut feed, "e1", &all);
            let mut seen: Vec<String> = shown.clone();
            seen.extend(more.iter().filter(|s| *s != "reconnected").cloned());
            let expect: Vec<String> = (1..=40).map(|i| format!("f{i}")).collect();
            assert_eq!(seen, expect, "n = {n}");
            assert_eq!(more.last().map(String::as_str), Some("reconnected"));
        }
    }

    #[test]
    fn rotated_ring_and_ambiguity_give_the_divider() {
        let all: Vec<_> = (1..=10).map(f).collect();
        let mut feed = Feed::new();
        attach(&mut feed, "e1", &all[..3]);
        // the ring rotated past f1..f5
        let out = attach(&mut feed, "e1", &all[5..]);
        assert_eq!(out[0], format!("--{DIV_MISSING}"));
        assert_eq!(&out[1..6], ["f6", "f7", "f8", "f9", "f10"]);
        // a repeated block: s s s, then the ring holds s s s s
        let s = || {
            let v = json!({"kind": "text", "t": "display", "text": "s"});
            (proto::icj(&v), v)
        };
        let mut feed = Feed::new();
        attach(&mut feed, "e1", &[s(), s(), s()]);
        let out = attach(&mut feed, "e1", &[s(), s(), s(), s()]);
        assert_eq!(out[0], format!("--{DIV_MISSING}"));
    }

    #[test]
    fn new_epoch_shows_everything_after_a_divider() {
        let all: Vec<_> = (1..=4).map(f).collect();
        let mut feed = Feed::new();
        attach(&mut feed, "e1", &all);
        let out = attach(&mut feed, "e2", &all[..2]);
        assert!(out[0].starts_with("--Root restarted (21:04:05Z)"), "{out:?}");
        assert_eq!(&out[1..3], ["f1", "f2"]);
    }

    #[test]
    fn numbered_frames_use_rseq() {
        let n = |i: u64| {
            let v = json!({"kind": "text", "rseq": i, "src": "kernel", "t": "display", "text": format!("f{i}")});
            (proto::icj(&v), v)
        };
        let mut feed = Feed::new();
        attach(&mut feed, "e1", &[n(1), n(2)]);
        assert_eq!(feed.since(), Some((2, "e1".into())));
        // the Root honoured since: only f3; a stale duplicate f2 is still dropped
        let out = attach(&mut feed, "e1", &[n(2), n(3)]);
        assert_eq!(out, ["f3", "reconnected"]);
        // live duplicates are dropped too
        let (raw, v) = n(3);
        assert!(feed.on_frame(&raw, v).is_empty());
        // a gap reported by the Root
        feed.on_attach(Some("e1".into()), None);
        let mut out = feed.on_frame(&n(9).0, n(9).1);
        out.extend(feed.on_frame("", json!({"t": "replay_end", "gap": true, "epoch": "e1"})));
        assert_eq!(frames(&out), [format!("--{DIV_MISSING}"), "f9".into()]);
    }

    #[test]
    fn echo_merge() {
        let now = Instant::now();
        let mut e = Echoes::default();
        let (a, st) = e.sent("hello", LineKind::Input, now);
        assert_eq!(st, OwnerSt::Pending);
        assert_eq!(e.on_echo("hello", Some("q1"), None, now), EchoOut::Update(a, OwnerSt::Queued));
        assert_eq!(e.on_echo("hello", None, Some(1), now), EchoOut::Update(a, OwnerSt::Delivered));
        let (b, _) = e.sent("/later do x", LineKind::Input, now);
        assert_eq!(e.on_echo("do x", None, Some(2), now), EchoOut::Update(b, OwnerSt::Delivered));
        // a foreign echo
        assert!(matches!(e.on_echo("from elsewhere", None, Some(3), now), EchoOut::New(_, OwnerSt::Delivered)));
        // a foreign Root echo followed by its kernel echo: one line
        let EchoOut::New(c, OwnerSt::Queued) = e.on_echo("queued before", Some("q2"), None, now) else { panic!() };
        assert_eq!(e.on_echo("queued before", None, Some(4), now), EchoOut::Update(c, OwnerSt::Delivered));
        // a command's Root echo is absorbed
        let (d, st) = e.sent("/stop", LineKind::KernelCommand, now);
        assert_eq!(st, OwnerSt::Command);
        assert_eq!(e.on_echo("/stop", Some("q3"), None, now), EchoOut::Absorbed(d));
        assert_eq!(e.unconfirmed_inputs(), 0);
    }

    #[test]
    fn unconfirmed_and_queue_full() {
        let now = Instant::now();
        let mut e = Echoes::default();
        let (a, _) = e.sent("one", LineKind::Input, now);
        let (b, _) = e.sent("two", LineKind::Input, now);
        assert_eq!(e.queue_full(), Some(b));
        assert!(e.sweep(now + Duration::from_secs(40), None, Some(0)).is_empty(), "no ready kernel yet");
        assert!(e.sweep(now + Duration::from_secs(40), Some(now + Duration::from_secs(20)), Some(0)).is_empty(), "ready only 20 s");
        assert_eq!(e.sweep(now + Duration::from_secs(51), Some(now + Duration::from_secs(20)), Some(0)), [a]);
    }

    #[test]
    fn transcript_tools_text_retry() {
        let now = Instant::now();
        let mut t = Transcript::default();
        let d = |kind, text: &str| Display { kind, text: text.into(), call_id: None, name: None, ok: None, boot_seq: Some(1), detail: None };
        t.display(&d(DisplayKind::Text, "Hello\nwor"), now, 0);
        t.display(&d(DisplayKind::Text, "ld"), now, 0);
        let mut s = d(DisplayKind::ToolStart, "cat SELF.md");
        s.call_id = Some("c1".into());
        s.name = Some("bash".into());
        t.display(&s, now, 0);
        let mut e = d(DisplayKind::ToolEnd, "exit 0");
        e.call_id = Some("c1".into());
        e.ok = Some(true);
        t.display(&e, now + Duration::from_millis(300), 0);
        assert!(matches!(&t.get(0).unwrap().kind, Kind::Text { lines, done: true } if lines == &["Hello", "world"]));
        assert!(matches!(&t.get(1).unwrap().kind, Kind::Tool(tool) if tool.ok == Some(true) && tool.dur == Some(Duration::from_millis(300))));
        // a retry with nothing printed blanks the partial text
        t.display(&d(DisplayKind::Text, "partial"), now, 0);
        t.display(&d(DisplayKind::Info, "retrying (1/3)"), now, 0);
        assert!(t.is_blank_text(2));
        // boot change
        let mut b = d(DisplayKind::Text, "x");
        b.boot_seq = Some(2);
        t.display(&b, now, 0);
        assert!(t.blocks.iter().any(|b| matches!(&b.kind, Kind::Divider(s) if s == "body restarted (boot 2)")));
    }

    #[test]
    fn observing_costs_does_not_claim_requests_are_refused() {
        let mut i = Info::default();
        i.on_egress(json!({"money": {"enforced": false, "day_spent_musd": 9_000_000,
            "day_limit_musd": 1, "total_spent_musd": 9_000_000, "total_limit_musd": 1}}));
        assert!(i.ceiling_notices(0).is_empty());
    }

    #[test]
    fn ceiling_notice_once_per_day() {
        let mut i = Info::default();
        i.on_egress(json!({"money": {"day_spent_musd": 4_000_000, "day_reserved_musd": 0, "day_limit_musd": 5_000_000, "day_resets_at": "1970-01-01T05:12:00Z"}}));
        assert!(i.ceiling_notices(0).is_empty());
        i.on_egress(json!({"money": {"day_spent_musd": 5_000_000, "day_reserved_musd": 0, "day_limit_musd": 5_000_000, "day_resets_at": "1970-01-01T05:12:00Z"}}));
        assert_eq!(i.ceiling_notices(0), ["■ daily ceiling reached: $5.00 of $5.00; requests are refused until the day resets (in 5 h 12 min)"]);
        assert!(i.ceiling_notices(0).is_empty());
    }

    #[test]
    fn epochs() {
        assert_eq!(epoch_of(&json!({"root": {"started_at": "t", "pid": 4}})), Some("t/4".into()));
        assert_eq!(epoch_of(&json!({"root": {"epoch": "x", "started_at": "t", "pid": 4}})), Some("x".into()));
        assert_eq!(epoch_of(&json!({"slot": "running"})), None);
    }
}
