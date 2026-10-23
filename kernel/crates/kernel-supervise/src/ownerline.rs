//! `owner.out` lines that always fit the control link (K-LINE).
//!
//! Each `owner.out` event is one control-link line, and the Root ends the link on a line over
//! 1 MiB (root/root.md §3.6.3: "a line over 1 MiB"), which SIGKILLs the kernel and its bodies and
//! counts as a crash. A body `display` line may itself be up to 1 MiB (§4.5), so its relay (the
//! frame plus `boot_seq` and the event envelope) could pass the limit. Every relayed frame is
//! kept within [`LINE_BUDGET`], which leaves the Root room for the `rseq` and `src` it adds:
//!
//! - `text` and `thinking` frames that do not fit are split into several frames of at most
//!   [`PIECE`] bytes, cut on char boundaries. The TUI and `init-root attach` append consecutive
//!   deltas, so the owner sees the whole text, byte for byte.
//! - Any other kind (`tool_start`, `tool_end`, `turn_end`, `info`, `error`) and the kernel's own
//!   `echo` and `notice` keep their first [`KEEP`] bytes plus a visible marker, which gives the
//!   size and points to the session log for the full record.
//! - `call_id` and `name` keep at most [`ID_CAP`] bytes, in every frame, so both ends of a tool
//!   call are cut alike and the TUI still pairs them.
//! - The known fields win. Fields that the relay does not know (`detail`, …) are dropped first, and
//!   a marker names them. The text is split or cut only when it does not fit on its own; the
//!   unknown fields then ride on the first frame if they fit.
//!
//! A frame that fits is relayed unchanged; the common small delta costs one size estimate and no
//! extra encoding.
//!
//! The Root also ends the link past 10,000 control frames a minute (§3.6.3), and a body streams
//! one `display` frame per model delta. [`Pacer`] keeps `owner.out` within [`RATE`] frames a
//! second ([`BURST`] at once): under the rate every frame goes out at once; over it, frames wait
//! in order, consecutive `text`/`thinking` deltas merge (up to [`PIECE`]), and a newer `state`
//! replaces a waiting one. Past [`HOLD`] waiting body frames, a delta joins the newest waiting
//! frame of its kind (no text is lost; only the interleaving of text and thinking coarsens), and a
//! body frame of another kind is dropped and counted, with a notice. The kernel's own frames
//! (`echo`, `notice`, `state`) are never dropped.

use init_proto::envelope::{Event, MAX_RESPONSE_LINE};
use init_proto::icj::{self, V1};
use init_proto::root_link::{AlertLevel, OwnerDisplay, OwnerOutFrame};
use init_proto::stdio::{Display, DisplayKind};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

/// The longest `owner.out` line the kernel sends (newline included): 1 MiB − 1 KiB.
pub const LINE_BUDGET: usize = MAX_RESPONSE_LINE - 1024;
/// A split `text` or `thinking` frame carries at most this many bytes of text. Even if JSON
/// escaping grows every byte sixfold (`\u0001`), a piece fits in [`LINE_BUDGET`].
pub const PIECE: usize = 128 * 1024;
/// A bounded frame of another kind keeps this many bytes of its text, before the marker.
pub const KEEP: usize = 64 * 1024;
/// A bounded frame's `call_id` and `name` keep at most this many bytes.
pub const ID_CAP: usize = 256;
/// More than the envelope plus a display frame's fixed fields need (`boot_seq`, `kind`, `ok`, `t`).
const FIXED: usize = 512;
/// The keys only the relay sets (init-proto `OwnerDisplay`; tui.md §9 Q1/Q2): a body's copies are dropped.
const RELAY_KEYS: [&str; 5] = ["boot_seq", "rseq", "src", "t", "v"];

#[derive(Serialize)]
struct Data<'a> {
    frame: &'a OwnerOutFrame,
}

/// The length of the `owner.out` line that carries `frame`, newline included (`usize::MAX` when
/// the frame is not ICJ).
pub fn line_len(frame: &OwnerOutFrame) -> usize {
    let envelope = icj::to_line(&Event { v: V1, ev: "owner.out".into(), data: Value::Null }).map_or(0, |l| l.len() - "null".len());
    icj::to_vec(&Data { frame }).map_or(usize::MAX, |d| d.len() + envelope)
}

fn fits(frame: &OwnerOutFrame) -> bool {
    line_len(frame) <= LINE_BUDGET
}

/// The longest prefix of `s` of at most `max` bytes that ends on a char boundary.
fn prefix(s: &str, max: usize) -> &str {
    let mut cut = max.min(s.len());
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    &s[..cut]
}

/// `s` cut into pieces of at most `max` bytes (`max` ≥ 4), on char boundaries.
fn pieces(s: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::with_capacity(s.len() / max + 1);
    let mut rest = s;
    while !rest.is_empty() {
        let p = prefix(rest, max);
        out.push(p);
        rest = &rest[p.len()..];
    }
    out
}

fn cap_id(id: &mut Option<String>) {
    if let Some(s) = id
        && s.len() > ID_CAP
    {
        let n = prefix(s, ID_CAP).len();
        s.truncate(n);
    }
}

/// The marker a bounded frame ends with (ASCII, so escaping never grows it).
fn cut_marker(kept: usize, total: usize) -> String {
    format!(" [kernel: showing {kept} of {total} bytes, cut to fit the owner line; the session log has the full record]")
}

fn dropped_marker(names: &[String], bytes: usize) -> String {
    let names: Vec<&str> = names.iter().map(|n| prefix(n, 64)).take(8).collect();
    format!(" [kernel: dropped the fields {} ({bytes} bytes) to fit the owner line]", names.join(", "))
}

fn display(boot_seq: u64, display: Display, extra: BTreeMap<String, Value>) -> OwnerOutFrame {
    OwnerOutFrame::Display(OwnerDisplay { boot_seq, display, extra })
}

/// The `owner.out` frames that relay one body display frame, each within [`LINE_BUDGET`]
/// (module doc). `extra` holds the fields the body added that this build does not know.
pub fn display_frames(boot_seq: u64, mut d: Display, mut extra: BTreeMap<String, Value>) -> Vec<OwnerOutFrame> {
    extra.retain(|k, _| !RELAY_KEYS.contains(&k.as_str()));
    // Always, not only when the frame is too long: both ends of a tool call are cut alike.
    cap_id(&mut d.call_id);
    cap_id(&mut d.name);
    let raw = d.text.len() + d.call_id.as_ref().map_or(0, String::len) + d.name.as_ref().map_or(0, String::len);
    if extra.is_empty() && FIXED + 6 * raw <= LINE_BUDGET {
        return vec![display(boot_seq, d, extra)]; // fits for certain: no encoding needed
    }
    let whole = display(boot_seq, d, extra);
    if fits(&whole) {
        return vec![whole];
    }
    let OwnerOutFrame::Display(OwnerDisplay { display: mut d, mut extra, .. }) = whole else { unreachable!("built as a display") };
    let split = matches!(d.kind, DisplayKind::Text | DisplayKind::Thinking);
    let names: Vec<String> = extra.keys().cloned().collect();
    let extra_bytes = if extra.is_empty() { 0 } else { icj::to_vec(&extra).map_or(0, |b| b.len()) };
    // The known fields win: the unknown ones go first, and the text is split or cut only when it
    // does not fit on its own.
    if !extra.is_empty() {
        let bare = display(boot_seq, d.clone(), BTreeMap::new());
        if fits(&bare) {
            return with_note(vec![bare], dropped_marker(&names, extra_bytes), split, boot_seq);
        }
    }
    let text = std::mem::take(&mut d.text);
    let total = text.len();
    let kept = if split { text.as_str() } else { prefix(&text, KEEP) };
    let mut out: Vec<OwnerOutFrame> = pieces(kept, PIECE).into_iter().map(|p| display(boot_seq, Display { text: p.to_string(), ..d.clone() }, BTreeMap::new())).collect();
    if out.is_empty() {
        out.push(display(boot_seq, d, BTreeMap::new()));
    }
    let mut note = String::new();
    if kept.len() < total {
        note.push_str(&cut_marker(kept.len(), total));
    }
    if !extra.is_empty() {
        // On the first frame, when they fit there.
        let OwnerOutFrame::Display(first) = &mut out[0] else { unreachable!() };
        std::mem::swap(&mut first.extra, &mut extra);
        if !fits(&out[0]) {
            let OwnerOutFrame::Display(first) = &mut out[0] else { unreachable!() };
            first.extra.clear();
            note.push_str(&dropped_marker(&names, extra_bytes));
        }
    }
    with_note(out, note, split, boot_seq)
}

/// `out` with `note` (markers, may be empty) appended to its last frame's text, or, for `text`
/// and `thinking`, as a separate `info` frame (never inside the streamed text).
fn with_note(mut out: Vec<OwnerOutFrame>, note: String, split: bool, boot_seq: u64) -> Vec<OwnerOutFrame> {
    if !note.is_empty() {
        if split {
            let text = note.trim_start().to_string();
            out.push(display(boot_seq, Display { kind: DisplayKind::Info, text, call_id: None, name: None, ok: None }, BTreeMap::new()));
        } else if let Some(OwnerOutFrame::Display(last)) = out.last_mut() {
            last.display.text.push_str(&note);
        }
    }
    // Unreachable with the caps above (6 × PIECE + ids + markers < LINE_BUDGET); never send a
    // line the Root would end the link on.
    out.retain(fits);
    out
}

/// `frame` bounded to [`LINE_BUDGET`]: an `echo` or `notice` keeps its first [`KEEP`] bytes of
/// text plus the marker; a `display` keeps its first piece (use [`display_frames`] to split).
/// `None` when it still cannot fit (not reachable with the kernel's own frames).
pub fn bounded(frame: OwnerOutFrame) -> Option<OwnerOutFrame> {
    let sure = match &frame {
        OwnerOutFrame::Echo { text, .. } | OwnerOutFrame::Notice { text, .. } => FIXED + 6 * text.len() <= LINE_BUDGET,
        OwnerOutFrame::Display(d) => {
            let raw = d.display.text.len() + d.display.call_id.as_ref().map_or(0, String::len) + d.display.name.as_ref().map_or(0, String::len);
            d.extra.is_empty() && FIXED + 6 * raw <= LINE_BUDGET
        }
        OwnerOutFrame::State { .. } => false, // rare; `model` comes from the config
    };
    if sure || fits(&frame) {
        return Some(frame);
    }
    let cut = |text: &mut String| {
        let total = text.len();
        let n = prefix(text, KEEP).len();
        text.truncate(n);
        text.push_str(&cut_marker(n, total));
    };
    let frame = match frame {
        OwnerOutFrame::Echo { seq, mut text } => {
            cut(&mut text);
            OwnerOutFrame::Echo { seq, text }
        }
        OwnerOutFrame::Notice { level, mut text, ts } => {
            cut(&mut text);
            OwnerOutFrame::Notice { level, text, ts }
        }
        OwnerOutFrame::Display(d) => return display_frames(d.boot_seq, d.display, d.extra).into_iter().next(),
        f @ OwnerOutFrame::State { .. } => f,
    };
    fits(&frame).then_some(frame)
}

/// `owner.out` frames a second, on average. With [`BURST`], at most 6,300 in any minute: the
/// Root's limit is 10,000 control frames a minute for everything the kernel sends (§3.6.3).
pub const RATE: u32 = 100;
/// `owner.out` frames that may go out at once.
pub const BURST: u32 = 300;
/// Body frames that may wait; past this, a body frame that cannot merge is dropped.
pub const HOLD: usize = 1000;

/// Paces `owner.out` (module doc). Not thread-safe; the supervisor calls [`Pacer::push`] for
/// every frame and [`Pacer::tick`] from its periodic work.
pub struct Pacer {
    /// Tokens in thousandths of a frame.
    milli: u64,
    last: Instant,
    held: VecDeque<(OwnerOutFrame, bool)>,
    held_body: usize,
    dropped: u64,
}

/// `b` appended to `a` when they merge: two `text` (or two `thinking`) deltas of one boot with no
/// other fields, within [`PIECE`]; or two `state` frames (the newer wins).
fn merge(a: &mut OwnerOutFrame, b: &OwnerOutFrame) -> bool {
    match (a, b) {
        (OwnerOutFrame::Display(x), OwnerOutFrame::Display(y)) => {
            let plain = |d: &OwnerDisplay| matches!(d.display.kind, DisplayKind::Text | DisplayKind::Thinking) && d.extra.is_empty() && d.display.call_id.is_none() && d.display.name.is_none() && d.display.ok.is_none();
            if plain(x) && plain(y) && x.boot_seq == y.boot_seq && x.display.kind == y.display.kind && x.display.text.len() + y.display.text.len() <= PIECE {
                x.display.text.push_str(&y.display.text);
                return true;
            }
            false
        }
        (a @ OwnerOutFrame::State { .. }, b @ OwnerOutFrame::State { .. }) => {
            *a = b.clone();
            true
        }
        _ => false,
    }
}

impl Pacer {
    pub fn new(now: Instant) -> Pacer {
        Pacer { milli: u64::from(BURST) * 1000, last: now, held: VecDeque::new(), held_body: 0, dropped: 0 }
    }

    fn refill(&mut self, now: Instant) {
        let ms = now.saturating_duration_since(self.last).as_millis() as u64;
        if ms > 0 {
            self.milli = (self.milli + ms * u64::from(RATE)).min(u64::from(BURST) * 1000);
            self.last = now;
        }
    }

    fn take(&mut self) -> bool {
        if self.milli >= 1000 {
            self.milli -= 1000;
            return true;
        }
        false
    }

    /// The frames waiting to go out.
    pub fn waiting(&self) -> usize {
        self.held.len()
    }

    /// Offer `frame` (`body`: a relayed body display, which may be dropped; else a kernel frame)
    /// and get the frames to send now, in order.
    pub fn push(&mut self, frame: OwnerOutFrame, body: bool, now: Instant) -> Vec<OwnerOutFrame> {
        self.refill(now);
        if self.held.is_empty() && self.take() {
            return vec![frame];
        }
        let mut merged = self.held.back_mut().is_some_and(|(last, _)| merge(last, &frame));
        if !merged && body && self.held_body >= HOLD {
            // Full: a delta joins the newest waiting frame of its kind (text never drops; only
            // the interleaving of text and thinking coarsens).
            merged = self.held.iter_mut().rev().any(|(f, _)| merge(f, &frame));
        }
        if !merged {
            if body && self.held_body >= HOLD {
                self.dropped += 1;
            } else {
                self.held_body += usize::from(body);
                self.held.push_back((frame, body));
            }
        }
        self.drain()
    }

    /// The waiting frames whose turn has come (call it at least every 100 ms).
    pub fn tick(&mut self, now: Instant) -> Vec<OwnerOutFrame> {
        self.refill(now);
        self.drain()
    }

    fn drain(&mut self) -> Vec<OwnerOutFrame> {
        let mut out = Vec::new();
        while !self.held.is_empty() && self.take() {
            let (f, body) = self.held.pop_front().expect("not empty");
            self.held_body -= usize::from(body);
            out.push(f);
        }
        if self.held.is_empty() && self.dropped > 0 && self.take() {
            let text = format!("[kernel] {} display frames dropped: the body sent more than {RATE} a second", self.dropped);
            out.push(OwnerOutFrame::Notice { level: AlertLevel::Warn, text, ts: crate::clock::now_rfc3339() });
            self.dropped = 0;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use init_proto::root_link::{KernelEvent, OwnerOutData};
    use std::time::Duration;
    use serde_json::json;

    /// Text that JSON escaping grows: quotes, a backslash, a tab, a newline, U+0001 (six bytes),
    /// plus two- and four-byte chars, so pieces must cut on char boundaries.
    fn nasty(n: usize) -> String {
        const P: &str = "lorem \"q\" \\ \t\n\u{1} é 😀 ipsum ";
        let mut s = P.repeat(n / P.len() + 1);
        let cut = prefix(&s, n).len();
        s.truncate(cut);
        s
    }

    fn d(kind: DisplayKind, text: String) -> Display {
        Display { kind, text, call_id: None, name: None, ok: None }
    }

    fn real_line(f: &OwnerOutFrame) -> Vec<u8> {
        let ev = KernelEvent::OwnerOut(OwnerOutData { frame: f.clone() }).to_event().unwrap();
        icj::to_line(&ev).unwrap()
    }

    fn text_of(f: &OwnerOutFrame) -> &str {
        match f {
            OwnerOutFrame::Display(d) => &d.display.text,
            OwnerOutFrame::Echo { text, .. } | OwnerOutFrame::Notice { text, .. } => text,
            OwnerOutFrame::State { .. } => "",
        }
    }

    #[test]
    fn line_len_is_the_real_line() {
        let f = display(7, Display { kind: DisplayKind::ToolEnd, text: nasty(5000), call_id: Some("c1".into()), name: Some("bash".into()), ok: Some(true) }, BTreeMap::from([("detail".into(), json!({"lines": [1, 2]}))]));
        assert_eq!(line_len(&f), real_line(&f).len());
        let n = OwnerOutFrame::Notice { level: AlertLevel::Warn, text: "x".into(), ts: "2026-10-20T00:00:00Z".into() };
        assert_eq!(line_len(&n), real_line(&n).len());
    }

    /// The bug this module fixes: the largest display line a body may send (1 MiB, stdio §4.5),
    /// relayed as one frame, is an `owner.out` line over the Root's 1 MiB limit.
    #[test]
    fn one_frame_relay_of_a_full_body_line_passes_the_limit() {
        use init_proto::stdio::{encode_from_body, FromBody, MAX_LINE};
        let mut text = "a".repeat(MAX_LINE);
        let body_line = loop {
            let l = encode_from_body(&FromBody::Display(d(DisplayKind::Text, text.clone()))).unwrap();
            if l.len() <= MAX_LINE {
                break l;
            }
            text.truncate(text.len() - (l.len() - MAX_LINE));
        };
        assert_eq!(body_line.len(), MAX_LINE, "a valid body line");
        let old = OwnerOutFrame::Display(OwnerDisplay::new(1, d(DisplayKind::Text, text.clone()), BTreeMap::new()));
        assert!(real_line(&old).len() > MAX_RESPONSE_LINE + 1, "the old relay: {} bytes", real_line(&old).len());
        let new = display_frames(1, d(DisplayKind::Text, text), BTreeMap::new());
        assert!(new.len() > 1 && new.iter().all(|f| real_line(f).len() <= LINE_BUDGET));
    }

    #[test]
    fn small_frames_pass_unchanged() {
        let x = BTreeMap::from([("detail".into(), json!("y".repeat(1000))), ("src".into(), json!("root"))]);
        let out = display_frames(3, d(DisplayKind::Text, "hello".into()), x);
        assert_eq!(out.len(), 1);
        let OwnerOutFrame::Display(od) = &out[0] else { panic!() };
        assert_eq!((od.boot_seq, od.display.text.as_str()), (3, "hello"));
        assert_eq!(od.extra.keys().collect::<Vec<_>>(), ["detail"], "the relay's own keys are dropped, the rest kept");
        // Just under the budget: still one frame, untouched.
        let body = "a".repeat(LINE_BUDGET - 200);
        let out = display_frames(3, d(DisplayKind::ToolEnd, body.clone()), BTreeMap::new());
        assert_eq!(out.len(), 1);
        assert_eq!(text_of(&out[0]), body);
    }

    #[test]
    fn long_text_and_thinking_split_whole_and_fit() {
        for kind in [DisplayKind::Text, DisplayKind::Thinking] {
            for n in [LINE_BUDGET, MAX_RESPONSE_LINE, 2 * MAX_RESPONSE_LINE + 3] {
                let text = nasty(n);
                let out = display_frames(9, d(kind, text.clone()), BTreeMap::new());
                assert!(out.len() > 1, "{kind:?} {n}");
                let mut joined = String::new();
                for f in &out {
                    let line = real_line(f);
                    assert!(line.len() <= LINE_BUDGET, "{} > budget", line.len());
                    let OwnerOutFrame::Display(od) = f else { panic!() };
                    assert_eq!((od.display.kind, od.boot_seq), (kind, 9));
                    assert!(od.display.text.len() <= PIECE);
                    joined.push_str(&od.display.text);
                }
                assert_eq!(joined, text, "every byte, in order, no marker");
            }
        }
    }

    #[test]
    fn other_kinds_are_cut_with_a_marker() {
        let long_id = "i".repeat(10_000);
        for kind in [DisplayKind::ToolStart, DisplayKind::ToolEnd, DisplayKind::TurnEnd, DisplayKind::Info, DisplayKind::Error] {
            let text = nasty(MAX_RESPONSE_LINE - 100);
            let src = Display { kind, text: text.clone(), call_id: Some(long_id.clone()), name: Some("bash".into()), ok: Some(false) };
            let out = display_frames(4, src, BTreeMap::new());
            assert_eq!(out.len(), 1, "{kind:?}");
            assert!(real_line(&out[0]).len() <= LINE_BUDGET);
            let OwnerOutFrame::Display(od) = &out[0] else { panic!() };
            let shown = &od.display.text;
            let kept = prefix(&text, KEEP);
            assert!(shown.starts_with(kept), "the start of the text is kept");
            assert!(shown.ends_with(&cut_marker(kept.len(), text.len())), "{}", &shown[kept.len()..]);
            assert!(shown.contains("session log"));
            assert_eq!(od.display.call_id.as_deref(), Some(prefix(&long_id, ID_CAP)));
            assert_eq!((od.display.name.as_deref(), od.display.ok), (Some("bash"), Some(false)));
        }
        // A small frame with the same id gets the same cut id, so start and end still pair.
        let small = Display { kind: DisplayKind::ToolStart, text: "ls".into(), call_id: Some(long_id.clone()), name: None, ok: None };
        let out = display_frames(4, small, BTreeMap::new());
        let OwnerOutFrame::Display(od) = &out[0] else { panic!() };
        assert_eq!((od.display.call_id.as_deref(), od.display.text.as_str()), (Some(prefix(&long_id, ID_CAP)), "ls"));
    }

    #[test]
    fn huge_extra_fields_are_dropped_and_named() {
        let big = BTreeMap::from([("detail".to_string(), json!("x".repeat(MAX_RESPONSE_LINE)))]);
        let out = display_frames(2, Display { kind: DisplayKind::ToolEnd, text: "exit 0".into(), call_id: Some("c1".into()), name: Some("bash".into()), ok: Some(true) }, big.clone());
        assert_eq!(out.len(), 1);
        let OwnerOutFrame::Display(od) = &out[0] else { panic!() };
        assert!(od.extra.is_empty());
        assert!(od.display.text.starts_with("exit 0 [kernel: dropped the fields detail ("), "{}", od.display.text);
        // The known fields win: a text that fits alone stays whole, and the detail goes.
        let mid = BTreeMap::from([("detail".to_string(), json!("x".repeat(600 * 1024)))]);
        let text = "y".repeat(500 * 1024);
        let out = display_frames(2, d(DisplayKind::ToolEnd, text.clone()), mid);
        assert_eq!(out.len(), 1);
        let OwnerOutFrame::Display(od) = &out[0] else { panic!() };
        assert!(od.extra.is_empty() && od.display.text.starts_with(&text) && od.display.text[text.len()..].starts_with(" [kernel: dropped the fields detail ("));
        // On a text frame the note is a separate info line, never inside the streamed text.
        let out = display_frames(2, d(DisplayKind::Text, "hi".into()), big);
        assert_eq!(out.len(), 2);
        assert_eq!(text_of(&out[0]), "hi");
        let OwnerOutFrame::Display(info) = &out[1] else { panic!() };
        assert_eq!(info.display.kind, DisplayKind::Info);
        assert!(info.display.text.starts_with("[kernel: dropped the fields detail ("), "{}", info.display.text);
        // Extras that fit next to a split text ride on its first piece.
        let small = BTreeMap::from([("lang".to_string(), json!("md"))]);
        let out = display_frames(2, d(DisplayKind::Text, nasty(2 * MAX_RESPONSE_LINE)), small);
        let OwnerOutFrame::Display(first) = &out[0] else { panic!() };
        assert_eq!(first.extra.get("lang"), Some(&json!("md")));
        assert!(out[1..].iter().all(|f| matches!(f, OwnerOutFrame::Display(o) if o.extra.is_empty() && o.display.kind == DisplayKind::Text)));
    }

    #[test]
    fn echo_and_notice_are_bounded() {
        let text = nasty(MAX_RESPONSE_LINE);
        let e = bounded(OwnerOutFrame::Echo { seq: 5, text: text.clone() }).unwrap();
        assert!(real_line(&e).len() <= LINE_BUDGET);
        let OwnerOutFrame::Echo { seq, text: shown } = &e else { panic!() };
        assert_eq!(*seq, 5);
        assert!(shown.starts_with(prefix(&text, KEEP)) && shown.contains("[kernel: showing"));
        let n = bounded(OwnerOutFrame::Notice { level: AlertLevel::Info, text, ts: "t".into() }).unwrap();
        assert!(real_line(&n).len() <= LINE_BUDGET);
        let small = OwnerOutFrame::Echo { seq: 1, text: "hi".into() };
        assert_eq!(bounded(small.clone()), Some(small));
    }

    fn delta(kind: DisplayKind, text: &str) -> OwnerOutFrame {
        display(1, d(kind, text.into()), BTreeMap::new())
    }

    #[test]
    fn pacer_passes_frames_under_the_rate() {
        let t0 = Instant::now();
        let mut p = Pacer::new(t0);
        for i in 0..1000u64 {
            let now = t0 + Duration::from_millis(i * 20); // 50 a second
            let f = delta(DisplayKind::Text, "x");
            assert_eq!(p.push(f.clone(), true, now), vec![f], "frame {i} goes out at once");
        }
        assert_eq!(p.waiting(), 0);
    }

    /// 20,000 deltas in one second: the text all arrives, in order, and in no 60 s window do more
    /// than BURST + 60 × RATE frames go out (the Root's limit is 10,000).
    #[test]
    fn pacer_merges_a_delta_flood_without_losing_text() {
        let t0 = Instant::now();
        let mut p = Pacer::new(t0);
        let mut sent: Vec<(Instant, OwnerOutFrame)> = Vec::new();
        let mut want = String::new();
        for i in 0..20_000u64 {
            let now = t0 + Duration::from_micros(i * 50);
            let kind = if i % 3 == 0 { DisplayKind::Thinking } else { DisplayKind::Text };
            let text = format!("<{i}>");
            if kind == DisplayKind::Text {
                want.push_str(&text);
            }
            sent.extend(p.push(delta(kind, &text), true, now).into_iter().map(|f| (now, f)));
            if i == 10_000 {
                let echo = OwnerOutFrame::Echo { seq: 7, text: "hi".into() };
                sent.extend(p.push(echo, false, now).into_iter().map(|f| (now, f)));
            }
        }
        let mut now = t0 + Duration::from_secs(1);
        while p.waiting() > 0 {
            now += Duration::from_millis(100);
            sent.extend(p.tick(now).into_iter().map(|f| (now, f)));
        }
        let got: String = sent.iter().filter_map(|(_, f)| match f {
            OwnerOutFrame::Display(o) if o.display.kind == DisplayKind::Text => Some(o.display.text.as_str()),
            _ => None,
        }).collect();
        assert_eq!(got, want);
        assert_eq!(sent.iter().filter(|(_, f)| matches!(f, OwnerOutFrame::Echo { .. })).count(), 1, "kernel frames are never dropped");
        assert!(sent.iter().all(|(_, f)| fits(f)));
        let per_min = sent.iter().filter(|(at, _)| *at < t0 + Duration::from_secs(60)).count();
        assert!(per_min <= (BURST + 60 * RATE) as usize, "{per_min} frames in a minute");
        assert!(sent.len() < 20_000 / 10, "merged: {}", sent.len());
    }

    #[test]
    fn pacer_drops_unmergeable_body_floods_and_says_so() {
        let t0 = Instant::now();
        let mut p = Pacer::new(t0);
        let mut out = Vec::new();
        for i in 0..5_000u32 {
            let f = display(1, Display { kind: DisplayKind::ToolStart, text: format!("{i}"), call_id: Some(format!("c{i}")), name: Some("bash".into()), ok: None }, BTreeMap::new());
            out.extend(p.push(f, true, t0));
        }
        let st = |busy| OwnerOutFrame::State { busy, level: 0, members: 0, mode: "normal".into(), model: None, queued: 0, sha12: "x".into(), slot: "running".into(), task_musd: None, evolution_checks_today_musd: None };
        out.extend(p.push(st(true), false, t0));
        out.extend(p.push(st(false), false, t0)); // replaces the waiting one
        assert_eq!(out.len(), BURST as usize);
        assert_eq!(p.waiting(), HOLD + 1);
        let mut now = t0;
        while p.waiting() > 0 || out.iter().all(|f| !matches!(f, OwnerOutFrame::Notice { .. })) {
            now += Duration::from_millis(100);
            out.extend(p.tick(now));
        }
        let states: Vec<_> = out.iter().filter(|f| matches!(f, OwnerOutFrame::State { .. })).collect();
        assert_eq!(states, [&st(false)]);
        let OwnerOutFrame::Notice { text, .. } = out.last().unwrap() else { panic!("{:?}", out.last()) };
        assert_eq!(text, &format!("[kernel] {} display frames dropped: the body sent more than {RATE} a second", 5_000 - BURST as usize - HOLD));
    }
}
