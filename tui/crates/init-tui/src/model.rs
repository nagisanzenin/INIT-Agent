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
    /// STEER: a steer the body has logged (its ack, via the kernel's `state.steer_seen`).
    Seen,
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
    /// STEER: the last kernel echo matched a line sent as `/later <text>` (a follow-up).
    pub last_follow_up: bool,
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
            self.last_follow_up = false;
            if let Some(i) = self.awaiting.iter().position(|a| a.kind == LineKind::Input && proto::echo_matches(&a.text, text)) {
                let a = self.awaiting.remove(i).expect("index");
                self.last_follow_up = a.text.trim().starts_with("/later ");
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
// The pending tray (STEER, §3.7.1): lines typed while the agent works, until the body reads them
// -------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraySt {
    /// Sent; no echo yet.
    Sending,
    /// The Root holds it (no kernel ran); the kernel's echo will follow.
    Held,
    /// Durable in the kernel's inbox (`seq` known); the body has not logged it yet.
    Pending,
    /// A retract was sent; waiting for the kernel's answer.
    Retracting,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrayItem {
    /// The owner-line id (the same id the echo merge uses).
    pub id: u64,
    /// The text as typed (a follow-up without its `/later ` prefix).
    pub text: String,
    /// A follow-up (`/later`, Tab): read when the run ends, as the next task.
    pub follow_up: bool,
    pub seq: Option<u64>,
    pub st: TraySt,
    pub sent: Instant,
}

/// What leaves the tray.
#[derive(Clone, Debug, PartialEq)]
pub enum TrayOut {
    /// The body logged it (`state.steer_seen`): into the transcript, marked seen.
    Seen(TrayItem),
    /// The kernel took it back (`state.steer_retracted`): never read.
    Retracted(TrayItem),
    /// An older kernel that does not report acks: shown as delivered at its echo, as before.
    Delivered(TrayItem),
}

#[derive(Debug, Default)]
pub struct Tray {
    pub items: Vec<TrayItem>,
    /// The kernel reports `steer_pending` (seen in a `state` frame): items wait for `steer_seen`.
    pub tracked: bool,
}

/// A `state` frame's seq list (absent: None).
fn seqs(v: &Value, k: &str) -> Option<Vec<u64>> {
    Some(v.get(k)?.as_array()?.iter().filter_map(Value::as_u64).collect())
}

impl Tray {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn add(&mut self, id: u64, text: &str, follow_up: bool, now: Instant) {
        self.items.push(TrayItem { id, text: text.to_string(), follow_up, seq: None, st: TraySt::Sending, sent: now });
    }

    pub fn has(&self, id: u64) -> bool {
        self.items.iter().any(|i| i.id == id)
    }

    pub fn remove(&mut self, id: u64) -> Option<TrayItem> {
        let i = self.items.iter().position(|x| x.id == id)?;
        Some(self.items.remove(i))
    }

    /// The echo of item `id`: Root-held, or durable in the kernel with `seq`. With an older kernel
    /// (no `steer_pending` seen) the item leaves the tray as delivered, as before STEER.
    pub fn on_echo(&mut self, id: u64, st: OwnerSt, seq: Option<u64>) -> Option<TrayOut> {
        let i = self.items.iter().position(|x| x.id == id)?;
        match (st, seq) {
            (OwnerSt::Queued, _) => self.items[i].st = TraySt::Held,
            (OwnerSt::Delivered, Some(s)) => {
                if !self.tracked {
                    return Some(TrayOut::Delivered(self.items.remove(i)));
                }
                self.items[i].seq = Some(s);
                if self.items[i].st != TraySt::Retracting {
                    self.items[i].st = TraySt::Pending;
                }
            }
            (OwnerSt::NotQueued, _) => return Some(TrayOut::Retracted(self.items.remove(i))),
            _ => {}
        }
        None
    }

    /// A `state` frame: items whose seq the body acked leave as seen, retracted ones as retracted.
    /// A seq no longer pending and in neither list (it fell off the kernel's ring) counts as seen.
    pub fn on_state(&mut self, v: &Value) -> Vec<TrayOut> {
        let Some(pending) = seqs(v, "steer_pending") else { return Vec::new() };
        self.tracked = true;
        let seen = seqs(v, "steer_seen").unwrap_or_default();
        let retracted = seqs(v, "steer_retracted").unwrap_or_default();
        let mut out = Vec::new();
        let mut keep = Vec::new();
        for it in self.items.drain(..) {
            match it.seq {
                Some(s) if retracted.contains(&s) => out.push(TrayOut::Retracted(it)),
                Some(s) if seen.contains(&s) || !pending.contains(&s) => out.push(TrayOut::Seen(it)),
                _ => keep.push(it),
            }
        }
        self.items = keep;
        out
    }

    /// The run ended under an older kernel: what it echoed is delivered (no ack is reported).
    pub fn flush_untracked(&mut self) -> Vec<TrayOut> {
        if self.tracked {
            return Vec::new();
        }
        let (gone, keep): (Vec<_>, Vec<_>) = self.items.drain(..).partition(|i| i.seq.is_some());
        self.items = keep;
        gone.into_iter().map(TrayOut::Delivered).collect()
    }

    /// A steer (not a follow-up) the body has not read yet: Esc then interrupts *and* steers.
    pub fn has_pending_steer(&self) -> bool {
        self.items.iter().any(|i| !i.follow_up && i.st != TraySt::Retracting)
    }

    /// Up: the newest item to take back. `Ok(seq)` marks it retracting; `Err` says why not.
    pub fn retract_newest(&mut self) -> Result<u64, &'static str> {
        let it = self.items.iter_mut().rev().find(|i| i.st != TraySt::Retracting).ok_or("nothing to take back")?;
        match (it.st, it.seq) {
            (TraySt::Pending, Some(s)) => {
                it.st = TraySt::Retracting;
                Ok(s)
            }
            (TraySt::Held, _) => Err("held by the Root while INIT restarts: it cannot be taken back yet"),
            _ => Err("not confirmed by the kernel yet: try again in a moment"),
        }
    }

    /// The kernel said `seq` was already read: it stays, and will show as seen.
    pub fn retract_refused(&mut self, seq: u64) -> bool {
        match self.items.iter_mut().find(|i| i.seq == Some(seq) && i.st == TraySt::Retracting) {
            Some(it) => {
                it.st = TraySt::Pending;
                true
            }
            None => false,
        }
    }
}

/// `[kernel] taken back: message 7 …` / `[kernel] not taken back: message 7 …`: (taken, seq).
pub fn retract_notice(text: &str) -> Option<(bool, u64)> {
    let (taken, rest) = if let Some(r) = text.strip_prefix("[kernel] taken back: message ") {
        (true, r)
    } else {
        (false, text.strip_prefix("[kernel] not taken back: message ")?)
    };
    let n: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((taken, n.parse().ok()?))
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
    /// The newest output line of a running call (`display{kind:"tool_progress"}`, §3.6), and when it came.
    pub out: Option<String>,
    pub out_at: Option<Instant>,
    /// The `delegate` call this one ran inside (`parent_call_id`); None for the main loop's calls.
    pub parent: Option<String>,
}

impl Tool {
    pub fn running(&self) -> bool {
        self.dur.is_none()
    }
    pub fn failed(&self) -> bool {
        self.ok == Some(false)
    }
    /// Ended without a `tool_end` (the turn ended or the body restarted).
    pub fn no_result(&self) -> bool {
        self.ok.is_none() && self.dur.is_some()
    }
}

/// One step of a tool activity, in arrival order (the details view shows them all).
#[derive(Clone, Debug)]
pub enum Step {
    Tool(Tool),
    /// A per-request token line (`turn_end`): shown only in the details.
    Tokens(String),
    /// Thinking between calls: folded into the activity; its text is in the details.
    Thought { text: String, dur: Option<Duration> },
}

/// The tool calls between two assistant messages, as one block that updates in place (§3.6): a
/// header with the current action and counters while it runs, then one summary line once frozen.
#[derive(Clone, Debug)]
pub struct Activity {
    pub steps: Vec<Step>,
    pub started: Instant,
    pub start_ms: i64,
    /// Thinking now, since.
    pub thinking: Option<Instant>,
    /// The total elapsed time, once frozen (the turn's text arrived, or the run ended).
    pub frozen: Option<Duration>,
    /// A `/details` copy: printed in full in the main screen.
    pub full: bool,
}

/// Counters of an activity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub calls: usize,
    pub running: usize,
    pub failed: usize,
    pub no_result: usize,
}

impl Activity {
    fn new(now: Instant, now_ms: i64) -> Activity {
        Activity { steps: Vec::new(), started: now, start_ms: now_ms, thinking: None, frozen: None, full: false }
    }
    /// Every call, sub-task calls included (the details view, `/details`).
    pub fn all_tools(&self) -> impl DoubleEndedIterator<Item = &Tool> {
        self.steps.iter().filter_map(|s| if let Step::Tool(t) = s { Some(t) } else { None })
    }
    /// The main loop's calls: a sub-task's calls are nested under their `delegate` row instead.
    pub fn tools(&self) -> impl DoubleEndedIterator<Item = &Tool> {
        self.all_tools().filter(|t| t.parent.is_none())
    }
    /// The calls a `delegate` call ran, in arrival order.
    pub fn children<'a>(&'a self, call_id: &'a str) -> impl DoubleEndedIterator<Item = &'a Tool> + 'a {
        self.all_tools().filter(move |t| t.parent.as_deref() == Some(call_id))
    }
    fn tools_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut Tool> {
        self.steps.iter_mut().filter_map(|s| if let Step::Tool(t) = s { Some(t) } else { None })
    }
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for t in self.tools() {
            c.calls += 1;
            c.running += usize::from(t.running());
            c.failed += usize::from(t.failed());
            c.no_result += usize::from(t.no_result());
        }
        c
    }
    /// `bash ×7 · read ×2`: the tool names by count, most used first (ties in first-use order).
    pub fn names(&self) -> Vec<(String, usize)> {
        let mut v: Vec<(String, usize)> = Vec::new();
        for t in self.tools() {
            match v.iter_mut().find(|(n, _)| *n == t.name) {
                Some(e) => e.1 += 1,
                None => v.push((t.name.clone(), 1)),
            }
        }
        v.sort_by_key(|e| std::cmp::Reverse(e.1));
        v
    }
    fn end_thinking(&mut self, now: Instant) {
        if let Some(since) = self.thinking.take()
            && let Some(Step::Thought { dur, .. }) = self.steps.iter_mut().rev().find(|s| matches!(s, Step::Thought { dur: None, .. }))
        {
            *dur = Some(now.duration_since(since));
        }
    }
    /// Freezes: calls still running end as `no result`.
    fn freeze(&mut self, now: Instant) {
        self.end_thinking(now);
        for t in self.tools_mut().filter(|t| t.dur.is_none()) {
            t.dur = Some(now.duration_since(t.started));
        }
        self.frozen = Some(now.duration_since(self.started));
    }
    fn bytes(&self) -> usize {
        self.steps
            .iter()
            .map(|s| match s {
                Step::Tool(t) => t.start_text.len() + t.summary.as_ref().map_or(0, String::len) + t.detail.as_ref().map_or(0, String::len),
                Step::Tokens(x) => x.len(),
                Step::Thought { text, .. } => text.len(),
            })
            .sum()
    }
}

#[derive(Clone, Debug)]
pub enum Kind {
    Owner { id: u64, text: String, st: OwnerSt },
    /// Logical lines; the last one may still be growing while `!done`.
    Text { lines: Vec<String>, done: bool },
    Thinking { text: String, done: bool, started: Instant, dur: Option<Duration> },
    Activity(Activity),
    /// A per-request token line outside an activity: hidden in the main screen (the status bar sums
    /// it), shown in the transcript view.
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
            Kind::Activity(a) => a.frozen.is_some(),
            _ => true,
        }
    }
    fn bytes(&self) -> usize {
        match &self.kind {
            Kind::Owner { text, .. } | Kind::TurnEnd(text) | Kind::Info(text) | Kind::Error(text) | Kind::Divider(text) => text.len(),
            Kind::Notice { text, .. } => text.len(),
            Kind::Text { lines, .. } => lines.iter().map(|l| l.len() + 1).sum(),
            Kind::Thinking { text, .. } => text.len(),
            Kind::Activity(a) => a.bytes(),
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
    /// The activity block taking tool calls (§3.6), while it is not frozen.
    open_act: Option<usize>,
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
        // the owner's next line, an error, a divider or the reply's text end the tool activity (§3.6)
        if matches!(kind, Kind::Error(_) | Kind::Divider(_)) {
            self.end_activity(Instant::now());
        }
        // STEER: an owner line never ends a call that is still running (it keeps running in the
        // body, and its result is still coming): the activity freezes only once no call runs
        if matches!(kind, Kind::Owner { .. }) && self.running_tool().is_none() {
            self.end_activity(Instant::now());
        }
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
                self.bytes = self.bytes.saturating_sub(b.bytes());
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
            if let Kind::Activity(a) = &mut b.kind {
                for t in a.tools_mut().filter(|t| t.dur.is_none()) {
                    t.dur = Some(now.duration_since(t.started));
                    changed = true;
                }
            }
        }
        self.dirty |= changed;
    }

    /// The open activity, if it is still in the live region (an overflow may have printed it).
    fn open_activity(&mut self) -> Option<&mut Activity> {
        let i = self.open_act?;
        if i < self.committed || self.get(i).is_none() {
            self.open_act = None;
            return None;
        }
        self.dirty = true;
        match self.blocks.get_mut(i - self.base).map(|b| &mut b.kind) {
            Some(Kind::Activity(a)) if a.frozen.is_none() => Some(a),
            _ => None,
        }
    }

    /// The open activity, or a new one.
    fn activity(&mut self, now: Instant, now_ms: i64) -> &mut Activity {
        if self.open_activity().is_none() {
            let i = self.push(Kind::Activity(Activity::new(now, now_ms)), now_ms);
            self.open_act = Some(i);
        }
        self.open_activity().expect("just opened")
    }

    /// Freezes the open activity into its one-line summary (§3.6).
    pub fn end_activity(&mut self, now: Instant) {
        if let Some(a) = self.open_activity() {
            a.freeze(now);
        }
        self.open_act = None;
    }

    /// `/details`: a copy of the last tool activity with every call expanded and the token line of
    /// each request, pushed as a block, so it is printed into the main screen (the scrollback).
    pub fn details(&mut self, now_ms: i64) -> Result<(), &'static str> {
        let i = self.last_activity().ok_or("no tool activity yet")?;
        let Some(Kind::Activity(mut a)) = self.get(i).map(|b| b.kind.clone()) else { return Err("no tool activity yet") };
        if a.frozen.is_none() {
            return Err("the tool activity is still running: Ctrl-O shows its details now");
        }
        // the token line of the request that started the activity sits just before it
        if let Some(Kind::TurnEnd(x)) = i.checked_sub(1).and_then(|p| self.get(p)).map(|b| &b.kind) {
            a.steps.insert(0, Step::Tokens(x.clone()));
        }
        a.full = true;
        self.push(Kind::Activity(a), now_ms);
        Ok(())
    }

    /// STEER: the call a steer waits behind, the oldest still running in the open activity (the
    /// batch's results all come back before the body reads a steer).
    pub fn running_tool(&self) -> Option<&Tool> {
        let i = self.open_act.filter(|&i| i >= self.committed)?;
        match &self.get(i)?.kind {
            Kind::Activity(a) if a.frozen.is_none() => a.tools().filter(|t| t.running()).min_by_key(|t| t.started),
            _ => None,
        }
    }

    /// Is a tool activity running (for the spinner)?
    pub fn activity_live(&self) -> bool {
        self.open_act.and_then(|i| self.get(i)).is_some_and(|b| matches!(&b.kind, Kind::Activity(a) if a.frozen.is_none()))
    }

    /// The newest activity block (for Ctrl-O and `/details`).
    pub fn last_activity(&self) -> Option<usize> {
        (self.base..self.len()).rev().take(10_000).find(|&i| matches!(self.get(i).map(|b| &b.kind), Some(Kind::Activity(a)) if !a.full))
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
                if !d.text.trim().is_empty() {
                    self.end_activity(now);
                }
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
                // thinking between tool calls folds into the activity
                if let Some(a) = self.open_activity() {
                    match a.steps.last_mut() {
                        Some(Step::Thought { text, dur: None }) => text.push_str(&d.text),
                        _ => a.steps.push(Step::Thought { text: d.text.clone(), dur: None }),
                    }
                    a.thinking.get_or_insert(now);
                    self.bytes += d.text.len();
                    return;
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
                self.bytes += d.text.len();
                let a = self.activity(now, now_ms);
                a.end_thinking(now);
                a.steps.push(Step::Tool(Tool { call_id, name, start_text: d.text.clone(), ok: None, summary: None, detail: d.detail.clone(), started: now, start_ms: now_ms, dur: None, out: None, out_at: None, parent: d.parent.clone() }));
            }
            DisplayKind::ToolProgress => {
                let cid = d.call_id.clone().unwrap_or_default();
                let last = d.text.lines().rev().map(str::trim_end).find(|l| !l.trim().is_empty()).map(str::to_string);
                if let Some(t) = self.open_activity().and_then(|a| a.tools_mut().rev().find(|t| t.call_id == cid && t.dur.is_none())) {
                    if last.is_some() {
                        t.out = last;
                    }
                    t.out_at = Some(now);
                }
            }
            DisplayKind::ToolEnd => {
                self.close_open(now);
                let cid = d.call_id.clone().unwrap_or_default();
                self.bytes += d.text.len();
                // the most recent unfinished call with this id, among the last 1,000 blocks
                let found = self.blocks.iter_mut().rev().take(1000).find_map(|b| match &mut b.kind {
                    Kind::Activity(a) => a.tools_mut().rev().find(|t| t.call_id == cid && t.dur.is_none()),
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
                        let a = self.activity(now, now_ms);
                        a.end_thinking(now);
                        a.steps.push(Step::Tool(Tool { call_id: cid, name, start_text: String::new(), ok: Some(d.ok.unwrap_or(false)), summary: Some(d.text.clone()), detail: d.detail.clone(), started: now, start_ms: now_ms, dur: Some(Duration::ZERO), out: None, out_at: None, parent: d.parent.clone() }));
                    }
                }
            }
            DisplayKind::TurnEnd => {
                self.close_open(now);
                self.close_tools(now);
                // `tokens: …; n display deltas dropped`: the drop stays visible
                let (tokens, dropped) = match d.text.split_once("; ") {
                    Some((a, b)) if parse_tokens(a).is_some() => (a.to_string(), Some(b.to_string())),
                    _ => (d.text.clone(), None),
                };
                if let Some(a) = self.open_activity() {
                    a.end_thinking(now);
                    a.steps.push(Step::Tokens(tokens));
                } else {
                    self.push(Kind::TurnEnd(tokens), now_ms);
                }
                if let Some(x) = dropped {
                    self.push(Kind::Info(x), now_ms);
                }
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
// Tokens and cache (§3.9): the per-request `turn_end` lines summed per turn and per session
// -------------------------------------------------------------------------------------------

/// One request's usage from its `turn_end` text: `tokens: in 118497 (cached 117760), out 961`.
/// `input` counts every prompt token, cached ones included (seed `Usage.input`); `cached` is None
/// when the line has no cache field (the provider did not report one), which is not 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub cached: Option<u64>,
    pub output: u64,
}

fn digits(s: &str) -> Option<(u64, &str)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    Some((s[..n].parse().ok()?, &s[n..]))
}

pub fn parse_tokens(text: &str) -> Option<Usage> {
    let r = text.trim().strip_prefix("tokens: in ")?;
    let (input, r) = digits(r)?;
    let (cached, r) = match r.strip_prefix(" (cached ") {
        Some(c) => {
            let (c, r) = digits(c)?;
            (Some(c), r.strip_prefix(')')?)
        }
        None => (None, r),
    };
    let (output, _) = digits(r.strip_prefix(", out ")?)?;
    Some(Usage { input, cached, output })
}

/// The cache-hit rate, token-weighted: Σ cached / Σ input over the requests that reported a cache
/// field. `Partial` when some requests did not report one; `Unknown` when none did (never 0%).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CacheRate {
    Unknown,
    Exact(f64),
    Partial(f64),
}

impl CacheRate {
    /// `99.8%` (rounded down to 0.1%, so a rate is never overstated), `~99.8%`, `?`.
    pub fn show(&self) -> String {
        let pct = |r: f64| format!("{:.1}%", (r * 1000.0).floor() / 10.0);
        match self {
            CacheRate::Unknown => "?".into(),
            CacheRate::Exact(r) => pct(*r),
            CacheRate::Partial(r) => format!("~{}", pct(*r)),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    /// Σ input of the requests that reported a cache field (the rate's denominator).
    pub cache_input: u64,
    /// Requests without a cache field.
    pub cache_unknown: u64,
}

impl Tally {
    pub fn add(&mut self, u: &Usage) {
        self.requests += 1;
        self.input += u.input;
        self.output += u.output;
        match u.cached {
            Some(c) => {
                self.cached += c.min(u.input);
                self.cache_input += u.input;
            }
            None => self.cache_unknown += 1,
        }
    }
    pub fn cache(&self) -> CacheRate {
        if self.cache_input == 0 {
            return CacheRate::Unknown;
        }
        let r = self.cached as f64 / self.cache_input as f64;
        if self.cache_unknown > 0 { CacheRate::Partial(r) } else { CacheRate::Exact(r) }
    }
}

/// Token counts: `961`, `12.3k`, `238k`, `1.24M`.
pub fn tok(n: u64) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..100_000 => format!("{:.1}k", n as f64 / 1e3),
        100_000..1_000_000 => format!("{}k", n / 1000),
        _ => format!("{:.2}M", n as f64 / 1e6),
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
    /// Tokens of the current (or last) turn, and of everything this TUI has seen.
    pub turn: Tally,
    pub session: Tally,
    /// The Root's `total_spent_musd` when the turn began (for the turn's estimated cost).
    turn_start_musd: Option<i64>,
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
            turn: Tally::default(),
            session: Tally::default(),
            turn_start_musd: None,
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
            (true, None) => {
                self.busy_since = Some(now);
                self.begin_turn();
            }
            (false, _) => self.busy_since = None,
            _ => {}
        }
        self.state = Some(v);
    }

    pub fn busy(&self) -> bool {
        self.busy_since.is_some()
    }

    fn spent(&self) -> Option<i64> {
        self.egress.as_ref().filter(|_| !self.egress_failed)?["money"]["total_spent_musd"].as_i64().filter(|m| *m >= 0)
    }

    /// A new turn: the run started (`state{busy:true}`), or an owner line was delivered while idle.
    pub fn begin_turn(&mut self) {
        self.turn = Tally::default();
        self.turn_start_musd = self.spent();
    }

    /// An owner line reached the kernel: a new turn unless it steers the running one.
    pub fn on_owner_delivered(&mut self) {
        if !self.busy() {
            self.begin_turn();
        }
    }

    /// A `turn_end` line: its tokens are added to the turn and the session.
    pub fn on_tokens(&mut self, text: &str) -> Option<Usage> {
        let u = parse_tokens(text)?;
        if self.turn.requests == 0 && self.turn_start_musd.is_none() {
            self.turn_start_musd = self.spent();
        }
        self.turn.add(&u);
        self.session.add(&u);
        Some(u)
    }

    /// The turn's estimated cost: the change in the Root's ledger total since the turn began
    /// (estimates from usage and tariffs, not an invoice; it includes any other spend in that time).
    pub fn turn_musd(&self) -> Option<i64> {
        Some((self.spent()? - self.turn_start_musd?).max(0))
    }

    /// `in 238k out 1.0k · cache 99.8% · $0.004`, the turn's line for the status bar and line mode.
    pub fn turn_line(&self) -> String {
        let cost = self.turn_musd().map_or_else(|| "$?".into(), crate::render::usd_fine);
        format!("in {} out {} · cache {} · {cost}", tok(self.turn.input), tok(self.turn.output), self.turn.cache().show())
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
        let d = |kind, text: &str| Display { kind, text: text.into(), call_id: None, name: None, ok: None, boot_seq: Some(1), detail: None, parent: None };
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
        assert!(matches!(&t.get(1).unwrap().kind, Kind::Activity(a) if a.tools().next().is_some_and(|tool| tool.ok == Some(true) && tool.dur == Some(Duration::from_millis(300)))));
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

    fn tool_d(kind: DisplayKind, id: &str, name: &str, text: &str, ok: Option<bool>) -> Display {
        Display { kind, text: text.into(), call_id: Some(id.into()), name: Some(name.into()), ok, boot_seq: Some(1), detail: None, parent: None }
    }

    fn plain(kind: DisplayKind, text: &str) -> Display {
        Display { kind, text: text.into(), call_id: None, name: None, ok: None, boot_seq: Some(1), detail: None, parent: None }
    }

    #[test]
    fn tool_calls_between_texts_are_one_activity_that_freezes_on_text() {
        let now = Instant::now();
        let mut t = Transcript::default();
        t.display(&plain(DisplayKind::Text, "Let me look."), now, 0);
        t.display(&plain(DisplayKind::TurnEnd, "tokens: in 100 (cached 90), out 5"), now, 0);
        for i in 0..5 {
            let id = format!("c{i}");
            t.display(&tool_d(DisplayKind::ToolStart, &id, "bash", "ls", None), now, 0);
            t.display(&plain(DisplayKind::Thinking, "hmm"), now, 0);
            t.display(&tool_d(DisplayKind::ToolEnd, &id, "bash", "exit 0", Some(i != 2)), now + Duration::from_secs(1), 0);
            t.display(&plain(DisplayKind::TurnEnd, "tokens: in 200 (cached 190), out 6; 2 display deltas dropped"), now, 0);
        }
        // one text block, one hidden token line, one activity, and the visible drop notes
        let acts: Vec<&Activity> = t.blocks.iter().filter_map(|b| if let Kind::Activity(a) = &b.kind { Some(a) } else { None }).collect();
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].counts(), Counts { calls: 5, running: 0, failed: 1, no_result: 0 });
        assert_eq!(acts[0].steps.iter().filter(|s| matches!(s, Step::Tokens(x) if !x.contains("dropped"))).count(), 5);
        assert!(acts[0].frozen.is_none());
        assert!(t.blocks.iter().any(|b| matches!(&b.kind, Kind::Info(x) if x == "2 display deltas dropped")));
        assert!(!t.blocks.iter().any(|b| matches!(b.kind, Kind::Thinking { .. })), "thinking folds into the activity");
        // whitespace text does not end it; the reply's text does
        t.display(&plain(DisplayKind::Text, "\n"), now, 0);
        assert!(t.activity_live());
        t.display(&plain(DisplayKind::Text, "Done."), now + Duration::from_secs(9), 0);
        assert!(!t.activity_live());
        let a = t.blocks.iter().find_map(|b| if let Kind::Activity(a) = &b.kind { Some(a) } else { None }).unwrap();
        assert_eq!(a.frozen, Some(Duration::from_secs(9)));
        assert_eq!(t.last_activity().map(|i| matches!(t.get(i).unwrap().kind, Kind::Activity(_))), Some(true));
        // the next tool call starts a new activity
        t.display(&tool_d(DisplayKind::ToolStart, "d1", "read", "x", None), now, 0);
        assert!(t.activity_live());
        // STEER: an owner line typed while a call runs does not end it (no false `? no result`):
        // the call keeps running and its result still lands in the activity ...
        t.push(Kind::Owner { id: 9, text: "stop".into(), st: OwnerSt::Delivered }, 0);
        let last = t.last_activity().unwrap();
        assert!(matches!(&t.get(last).unwrap().kind, Kind::Activity(a) if a.counts() == Counts { calls: 1, running: 1, failed: 0, no_result: 0 } && a.frozen.is_none()));
        assert_eq!(t.running_tool().map(|x| x.call_id.as_str()), Some("d1"));
        t.display(&tool_d(DisplayKind::ToolEnd, "d1", "read", "12 lines", Some(true)), now + Duration::from_secs(2), 0);
        assert!(matches!(&t.get(last).unwrap().kind, Kind::Activity(a) if a.counts() == Counts { calls: 1, running: 0, failed: 0, no_result: 0 }));
        // ... and the next owner line (the steer, once read) freezes it, all ok
        t.push(Kind::Owner { id: 10, text: "and test it".into(), st: OwnerSt::Seen }, 0);
        assert!(matches!(&t.get(last).unwrap().kind, Kind::Activity(a) if a.counts().no_result == 0 && a.frozen.is_some()));
        assert!(t.running_tool().is_none());
    }

    fn st(pending: &[u64], seen: &[u64], retracted: &[u64]) -> Value {
        serde_json::json!({"busy": true, "t": "state", "steer_pending": pending, "steer_seen": seen, "steer_retracted": retracted})
    }

    #[test]
    fn tray_items_leave_when_seen_or_retracted() {
        let now = Instant::now();
        let mut tr = Tray::default();
        tr.on_state(&st(&[], &[], &[]));
        assert!(tr.tracked);
        tr.add(1, "fix the test name", false, now);
        tr.add(2, "then run clippy", true, now);
        assert!(tr.has_pending_steer());
        assert_eq!(tr.retract_newest(), Err("not confirmed by the kernel yet: try again in a moment"));
        assert_eq!(tr.on_echo(1, OwnerSt::Delivered, Some(7)), None);
        assert_eq!(tr.on_echo(2, OwnerSt::Delivered, Some(8)), None);
        assert_eq!((tr.items[0].st, tr.items[0].seq), (TraySt::Pending, Some(7)));
        // a state before the ack changes nothing
        assert!(tr.on_state(&st(&[7, 8], &[5, 6], &[])).is_empty());
        // Up takes back the newest (the follow-up); the kernel confirms
        assert_eq!(tr.retract_newest(), Ok(8));
        assert_eq!(tr.items[1].st, TraySt::Retracting);
        let out = tr.on_state(&st(&[7], &[5, 6], &[8]));
        assert!(matches!(&out[..], [TrayOut::Retracted(i)] if i.id == 2));
        // the steer is acked: seen
        let out = tr.on_state(&st(&[], &[5, 6, 7], &[8]));
        assert!(matches!(&out[..], [TrayOut::Seen(i)] if i.id == 1 && i.text == "fix the test name"));
        assert!(tr.is_empty() && !tr.has_pending_steer());
    }

    #[test]
    fn tray_retract_refused_and_old_kernels() {
        let now = Instant::now();
        let mut tr = Tray { tracked: true, ..Tray::default() };
        tr.add(1, "a", false, now);
        tr.on_echo(1, OwnerSt::Delivered, Some(3));
        assert_eq!(tr.retract_newest(), Ok(3));
        assert!(!tr.has_pending_steer(), "a line being taken back no longer steers on Esc");
        assert!(tr.retract_refused(3));
        assert_eq!(tr.items[0].st, TraySt::Pending);
        // a seq that fell off the seen ring (no longer pending) still counts as seen
        assert!(matches!(&tr.on_state(&st(&[9], &[10], &[]))[..], [TrayOut::Seen(_)]));
        // Root-held lines cannot be taken back yet
        tr.add(2, "b", false, now);
        tr.on_echo(2, OwnerSt::Queued, None);
        assert_eq!(tr.items[0].st, TraySt::Held);
        assert!(tr.retract_newest().unwrap_err().contains("held by the Root"));
        // an older kernel (no steer lists): delivered at the echo, as before STEER
        let mut old = Tray::default();
        old.add(4, "c", false, now);
        assert!(old.on_state(&serde_json::json!({"busy": true, "t": "state"})).is_empty());
        assert!(matches!(old.on_echo(4, OwnerSt::Delivered, Some(1)), Some(TrayOut::Delivered(i)) if i.id == 4));
        assert_eq!(retract_notice("[kernel] taken back: message 12 will not be read"), Some((true, 12)));
        assert_eq!(retract_notice("[kernel] not taken back: message 3 was already read by the agent"), Some((false, 3)));
        assert_eq!(retract_notice("[kernel] promoted"), None);
    }

    #[test]
    fn progress_updates_the_running_call() {
        let now = Instant::now();
        let mut t = Transcript::default();
        t.display(&tool_d(DisplayKind::ToolStart, "c1", "bash", "cargo build", None), now, 0);
        t.display(&tool_d(DisplayKind::ToolProgress, "c1", "bash", "   Compiling a v1\n   Compiling b v2\n\n", None), now + Duration::from_secs(3), 0);
        let a = t.blocks.iter().find_map(|b| if let Kind::Activity(a) = &b.kind { Some(a) } else { None }).unwrap();
        let tool = a.tools().next().unwrap();
        assert_eq!(tool.out.as_deref(), Some("   Compiling b v2"));
        assert_eq!(tool.out_at, Some(now + Duration::from_secs(3)));
    }

    #[test]
    fn token_lines_parse_and_cache_is_token_weighted() {
        assert_eq!(parse_tokens("tokens: in 119586 (cached 119424), out 79"), Some(Usage { input: 119_586, cached: Some(119_424), output: 79 }));
        assert_eq!(parse_tokens("tokens: in 5, out 6"), Some(Usage { input: 5, cached: None, output: 6 }));
        assert_eq!(parse_tokens("tokens: in 5 (cached 1), out 6; 3 display deltas dropped").map(|u| u.output), Some(6));
        assert_eq!(parse_tokens("retrying (1/3)"), None);
        // the owner's question: 119424 / 119586 is one request's rate (99.86%, shown rounded down)
        let mut one = Tally::default();
        one.add(&parse_tokens("tokens: in 119586 (cached 119424), out 79").unwrap());
        assert_eq!(one.cache().show(), "99.8%");
        // weighted, not the mean of rates: (90 + 0) / (100 + 900) = 9%, not (90% + 0%) / 2
        let mut t = Tally::default();
        t.add(&Usage { input: 100, cached: Some(90), output: 1 });
        t.add(&Usage { input: 900, cached: Some(0), output: 1 });
        assert_eq!(t.cache(), CacheRate::Exact(0.09));
        assert_eq!(t.cache().show(), "9.0%");
        // a missing cache field is unknown, never 0%
        let mut u = Tally::default();
        u.add(&Usage { input: 5, cached: None, output: 6 });
        assert_eq!(u.cache(), CacheRate::Unknown);
        assert_eq!(u.cache().show(), "?");
        u.add(&Usage { input: 10, cached: Some(5), output: 1 });
        assert_eq!(u.cache().show(), "~50.0%", "partial: only requests that reported a cache field");
        assert_eq!(Tally::default().cache().show(), "?");
        assert_eq!((tok(961), tok(12_345), tok(238_102), tok(1_240_000)), ("961".into(), "12.3k".into(), "238k".into(), "1.24M".into()));
    }

    #[test]
    fn turn_and_session_tallies_and_cost() {
        let now = Instant::now();
        let mut i = Info::default();
        let money = |m: i64| json!({"money": {"enforced": false, "policy": "observe", "total_spent_musd": m}});
        i.on_egress(money(1_000_000));
        i.on_state(json!({"busy": true}), now);
        i.on_tokens("tokens: in 1000 (cached 900), out 10");
        i.on_egress(money(1_004_200));
        i.on_tokens("tokens: in 3000 (cached 2900), out 20");
        assert_eq!(i.turn.requests, 2);
        assert_eq!(i.turn_musd(), Some(4_200));
        assert_eq!(i.turn_line(), "in 4.0k out 30 · cache 95.0% · $0.004");
        // a steer while busy is the same turn; the next run is a new one
        i.on_owner_delivered();
        assert_eq!(i.turn.requests, 2);
        i.on_state(json!({"busy": false}), now);
        i.on_state(json!({"busy": true}), now);
        assert_eq!(i.turn.requests, 0);
        i.on_tokens("tokens: in 10, out 1");
        assert_eq!(i.session.requests, 3);
        assert_eq!(i.session.cache().show(), "~95.0%");
        assert_eq!(i.turn.cache().show(), "?");
        // no ledger: the cost is unknown, not $0
        i.egress_failed = true;
        assert!(i.turn_line().ends_with("· $?"), "{}", i.turn_line());
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
