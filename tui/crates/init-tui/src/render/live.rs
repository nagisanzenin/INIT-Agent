//! Blocks to rows, committing (printing once above the live region), and the live region itself:
//! the uncommitted blocks, the completion popup or key overlay, the input box, the status bar and the
//! hint line (docs/tui.md §3.4–§3.7).

use super::md::{self, MdState};
use super::{Role, Line, Region, Style, sanitize, status, wrap};
use crate::input::Editor;
use crate::keys::HELP;
use crate::model::{Activity, Info, Kind, OwnerSt, Step, Tool, Transcript, Tray, TrayItem, TraySt};
use crate::notice::Class;
use std::time::{Duration, Instant};

/// The one spinner on screen: the liveness row's (TUI-BRAND). Rows of calls in flight carry a
/// still outline `◇` in the rewrite colour instead, so only one thing moves.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Everything a render reads.
pub struct Ui<'a> {
    pub t: &'a Transcript,
    /// STEER: lines typed while the agent works, until the body reads them (§3.7.1).
    pub tray: &'a Tray,
    pub ed: &'a Editor,
    pub info: &'a Info,
    pub verbose: bool,
    pub overlay: bool,
    pub hint: Option<&'a str>,
    pub now: Instant,
    pub now_ms: i64,
    pub no_color: bool,
    pub spin: usize,
}

/// Rows of a text block's logical lines `[from, to)`, starting in Markdown state `st`.
pub fn text_rows(lines: &[String], from: usize, to: usize, mut st: MdState, width: usize) -> (Vec<Line>, MdState) {
    let mut rows = Vec::new();
    for l in &lines[from..to] {
        let (line, next, indent) = md::line(l, st);
        st = next;
        rows.extend(wrap::wrap(&line, width, indent));
    }
    (rows, st)
}

fn wrapped(text: &str, prefix: &str, style: Style, pstyle: Style, width: usize) -> Vec<Line> {
    let pw = wrap::str_width(prefix);
    let mut rows = Vec::new();
    for (i, l) in sanitize::lines(text, style, pw).into_iter().enumerate() {
        let mut line = Line::new();
        line.push(if i == 0 { prefix.to_string() } else { " ".repeat(pw) }, pstyle);
        line.extend(l);
        rows.extend(wrap::wrap(&line, width, pw));
    }
    rows
}

/// A tool call's collapsed row: marker, name, the body's summary of the call, and its state.
pub fn tool_row(t: &Tool, width: usize, now: Instant, _spin: usize) -> Line {
    let (marker, right) = match (t.ok, t.dur) {
        (Some(true), d) => ("◇", {
            let mut r = Line::new();
            r.push("✓ ", Style::dim());
            r.extend(sanitize::inline(t.summary.as_deref().unwrap_or(""), Style::dim()));
            r.push(format!(" · {}", crate::time::dur(d.unwrap_or_default())), Style::dim());
            r
        }),
        (Some(false), d) => ("◇", {
            let mut r = Line::new();
            r.push("✕ ", Style::fg(Role::Harm));
            r.extend(sanitize::inline(t.summary.as_deref().unwrap_or(""), Style::fg(Role::Harm)));
            r.push(format!(" · {}", crate::time::dur(d.unwrap_or_default())), Style::dim());
            r
        }),
        (None, Some(d)) => ("◇", Line::from_span(format!("? no result · {}", crate::time::dur(d)), Style::fg(Role::Warn))),
        (None, None) => ("◇", Line::from_span(format!("running · {}", crate::time::dur(now.duration_since(t.started))), Style::dim())),
    };
    let mut left = Line::new();
    left.push(format!("{marker} "), if t.running() { Style::fg(Role::Rewrite) } else { Style::mute() });
    let name = sanitize::inline(&t.name, Style::PLAIN);
    let nw = name.width();
    left.extend(name);
    left.push(" ".repeat(7usize.saturating_sub(nw).max(1)), Style::PLAIN);
    left.extend(sanitize::inline(&t.start_text, Style::PLAIN));
    let right = right.truncate(width / 2);
    let rw = right.width();
    let avail = width.saturating_sub(rw + 2);
    let mut row = left.truncate(avail);
    let pad = width.saturating_sub(row.width() + rw);
    row.push(" ".repeat(pad.max(1)), Style::PLAIN);
    row.extend(right);
    row.truncate(width)
}

/// A tool call expanded (transcript view, §3.6).
fn tool_expanded(t: &Tool, width: usize, now: Instant, spin: usize) -> Vec<Line> {
    let mut rows = vec![tool_row(t, width, now, spin)];
    let field = |k: &str, v: &str, st: Style| wrapped(v, &format!("    {k}: "), st, Style::dim(), width);
    rows.extend(field("call_id", &t.call_id, Style::PLAIN));
    rows.extend(field("call", &t.start_text, Style::PLAIN));
    let ok = match t.ok {
        Some(true) => "ok",
        Some(false) => "error",
        None => "running",
    };
    rows.extend(field("result", &format!("{ok} · {}", t.summary.as_deref().unwrap_or("")), Style::PLAIN));
    let d = t.dur.map_or_else(|| format!("{} so far", crate::time::dur(now.duration_since(t.started))), crate::time::dur);
    rows.extend(field("time", &format!("started {} · {d}", crate::time::utc_hms(t.start_ms)), Style::PLAIN));
    if let Some(det) = &t.detail {
        rows.extend(field("detail", det, Style::dim()));
    }
    rows
}

/// Finished calls an activity shows while it runs; older ones fold into `… +N more` (failures never fold).
pub const KEEP_DONE: usize = 3;
/// TUI-OBSERVE: output lines shown under a failed call (the body's `detail`, at most 10): all of them
/// once the activity is frozen, the last 3 while it still runs (Ctrl-O always has all).
pub const FAIL_TAIL: usize = 10;
pub const FAIL_TAIL_LIVE: usize = 3;

/// TUI-OBSERVE: under a failed call, the last lines of its output (`detail`, §3.6), so "the output
/// above says why" is true on screen. Nothing for a call that succeeded or sent no detail.
pub fn fail_tail(t: &Tool, width: usize, indent_by: usize, max: usize) -> Vec<Line> {
    let Some(d) = t.detail.as_deref().filter(|_| t.failed()) else { return Vec::new() };
    let lines: Vec<&str> = d.lines().collect();
    let skip = lines.len().saturating_sub(max);
    lines[skip..]
        .iter()
        .map(|l| {
            let mut r = Line::from_span(format!("{}│ ", " ".repeat(indent_by)), Style::fg(Role::Line));
            r.extend(sanitize::inline(l, Style::dim()));
            r.truncate(width)
        })
        .collect()
}
/// A running call with no output for this long shows `still working…` (§3.6).
pub const HEARTBEAT: Duration = Duration::from_secs(10);

fn indent(l: Line, n: usize) -> Line {
    let mut r = Line::from_span(" ".repeat(n), Style::PLAIN);
    r.extend(l);
    r
}

/// `left` truncated, then `right` flush right (at most half the width).
fn split_row(left: Line, right: Line, width: usize) -> Line {
    let right = right.truncate(width / 2);
    let rw = right.width();
    let mut row = left.truncate(width.saturating_sub(rw + 2));
    let pad = width.saturating_sub(row.width() + rw);
    row.push(" ".repeat(pad.max(1)), Style::PLAIN);
    row.extend(right);
    row.truncate(width)
}

/// Under a running call: its elapsed time (`with_time`), its newest output line (dim), and the
/// heartbeat `still working…` after 10 s with no output (§3.6), so a long `cargo build` never
/// looks hung.
fn pulse(t: &Tool, width: usize, now: Instant, with_time: bool) -> Option<Line> {
    let run = now.duration_since(t.started);
    let quiet = now.duration_since(t.out_at.unwrap_or(t.started));
    let mut head = Line::from_span("    ", Style::PLAIN);
    if with_time {
        head.push(format!("running {}", crate::time::elapsed(run)), Style::dim());
    }
    let sep = |l: &mut Line| {
        if with_time {
            l.push(" · ", Style::dim());
        }
    };
    let mut tail = Line::new();
    match &t.out {
        Some(o) => {
            if quiet > HEARTBEAT {
                tail.push(format!(" · still working… no output for {}", crate::time::span(quiet.as_secs())), Style::dim());
            }
            // the output line gives way first: the time and the heartbeat always show
            let room = width.saturating_sub(head.width() + tail.width() + 5);
            if room >= 8 {
                sep(&mut head);
                head.push("│ ", Style::fg(Role::Line));
                head.extend(sanitize::inline(o.trim(), Style::dim()).truncate(room));
            } else if quiet > HEARTBEAT {
                tail = Line::from_span(format!(" · no output for {}", crate::time::span(quiet.as_secs())), Style::dim());
            }
        }
        None if quiet > HEARTBEAT => {
            sep(&mut head);
            head.push("still working… no output yet", Style::dim());
        }
        None if !with_time => return None,
        None => {}
    }
    head.extend(tail);
    Some(head.truncate(width))
}

/// Under a `delegate` row: its sub-task's calls, collapsed to one line (`parent_call_id`):
/// `↳ 3 calls · bash ×2 · read · ✕ 1 failed`, then the running inner call while it runs.
/// None for a call that ran no sub-task calls.
fn nested(a: &Activity, t: &Tool, width: usize, _spin: usize) -> Option<Line> {
    let kids: Vec<&Tool> = a.children(&t.call_id).collect();
    if kids.is_empty() {
        return None;
    }
    let mut l = Line::from_span("    ↳ ", Style::dim());
    let s = if kids.len() == 1 { "" } else { "s" };
    l.push(format!("{} call{s}", kids.len()), Style::dim());
    let mut names: Vec<(&str, usize)> = Vec::new();
    for k in &kids {
        match names.iter_mut().find(|(n, _)| *n == k.name) {
            Some(e) => e.1 += 1,
            None => names.push((&k.name, 1)),
        }
    }
    names.sort_by_key(|e| std::cmp::Reverse(e.1));
    let names: Vec<String> = names.into_iter().map(|(n, k)| if k > 1 { format!("{n} ×{k}") } else { n.to_string() }).collect();
    l.extend(sanitize::inline(&format!(" · {}", names.join(" · ")), Style::dim()));
    let failed = kids.iter().filter(|k| k.failed()).count();
    if failed > 0 {
        l.push(" · ", Style::dim());
        l.push(format!("✕ {failed} failed"), Style::fg(Role::Harm));
    }
    if let Some(k) = kids.iter().rev().find(|k| k.running()) {
        l.push(" · ◇ ", Style::fg(Role::Rewrite));
        l.extend(sanitize::inline(&k.name, Style::PLAIN));
        l.push(" ", Style::PLAIN);
        l.extend(sanitize::inline(&k.start_text, Style::PLAIN));
    }
    Some(l.truncate(width))
}

/// `6 tool calls · 1 running · 0 failed · 1 min 12 s`, or the short form for narrow screens.
fn counters(a: &Activity, elapsed: Duration, short: bool) -> Line {
    let c = a.counts();
    let mut l = Line::new();
    let s = if c.calls == 1 { "" } else { "s" };
    if short {
        l.push(format!("{} call{s}", c.calls), Style::dim());
        if c.failed > 0 {
            l.push(" · ", Style::dim());
            l.push(format!("{} failed", c.failed), Style::fg(Role::Harm));
        }
    } else {
        l.push(format!("{} tool call{s} · {} running · ", c.calls, c.running), Style::dim());
        l.push(format!("{} failed", c.failed), if c.failed > 0 { Style::fg(Role::Harm) } else { Style::dim() });
        if c.no_result > 0 {
            l.push(format!(" · {} no result", c.no_result), Style::fg(Role::Warn));
        }
    }
    l.push(format!(" · {}", crate::time::elapsed(elapsed)), Style::dim());
    l
}

/// The header of a running activity: spinner, the current action, counters and elapsed time.
fn activity_header(a: &Activity, width: usize, now: Instant, _spin: usize) -> Line {
    let mut left = Line::new();
    left.push("◇ ", Style::fg(Role::Rewrite));
    match a.tools().rev().find(|t| t.running()) {
        Some(t) => {
            left.extend(sanitize::inline(&t.name, Style::PLAIN));
            left.push(" ", Style::PLAIN);
            left.extend(sanitize::inline(&t.start_text, Style::PLAIN));
        }
        None if a.thinking.is_some() => left.push("thinking…", Style::dim()),
        None => left.push("waiting for the model…", Style::dim()),
    }
    let el = now.duration_since(a.started);
    let mut right = counters(a, el, false);
    if right.width() > width / 2 {
        right = counters(a, el, true);
    }
    split_row(left, right, width)
}

/// The frozen summary: `◇ ran 9 tools in 1 min 12 s · bash ×7 · read ×2      ✕ 1 failed`.
fn activity_summary(a: &Activity, width: usize) -> Line {
    let c = a.counts();
    let mut left = Line::from_span("◇ ", Style::mute());
    let s = if c.calls == 1 { "" } else { "s" };
    left.push(format!("ran {} tool{s} in {}", c.calls, crate::time::elapsed(a.frozen.unwrap_or_default())), Style::PLAIN);
    let names: Vec<String> = a.names().into_iter().map(|(n, k)| if k > 1 { format!("{n} ×{k}") } else { n }).collect();
    if !names.is_empty() {
        left.extend(sanitize::inline(&format!(" · {}", names.join(" · ")), Style::dim()));
    }
    let mut right = Line::new();
    if c.failed > 0 {
        right.push(format!("✕ {} failed", c.failed), Style::fg(Role::Harm));
    }
    if c.no_result > 0 {
        if !right.0.is_empty() {
            right.push(" · ", Style::dim());
        }
        right.push(format!("? {} no result", c.no_result), Style::fg(Role::Warn));
    }
    if right.0.is_empty() {
        right.push("✓ all ok", Style::dim());
    }
    split_row(left, right, width)
}

/// Every step of an activity (the transcript view's expansion and `/details`).
fn activity_full(a: &Activity, width: usize, now: Instant, spin: usize) -> Vec<Line> {
    let mut head = Line::from_span("◇ ", Style::mute());
    let c = a.counts();
    let s = if c.calls == 1 { "" } else { "s" };
    let el = a.frozen.map_or_else(|| format!("{} so far", crate::time::elapsed(now.duration_since(a.started))), crate::time::elapsed);
    head.push(format!("{} tool call{s} · {} failed · {el} · started {}", c.calls, c.failed, crate::time::utc_hms(a.start_ms)), Style::PLAIN);
    let mut rows = vec![head.truncate(width)];
    let w = width.saturating_sub(2);
    for st in &a.steps {
        match st {
            // a sub-task's calls follow their `delegate` call, one level deeper
            Step::Tool(t) if t.parent.is_some() => {}
            Step::Tool(t) => {
                rows.extend(tool_expanded(t, w, now, spin).into_iter().map(|l| indent(l, 2)));
                let ww = width.saturating_sub(4);
                for k in a.children(&t.call_id) {
                    rows.extend(tool_expanded(k, ww, now, spin).into_iter().map(|l| indent(l, 4)));
                }
            }
            Step::Tokens(x) => rows.extend(wrapped(x, "  · ", Style::dim(), Style::dim(), width)),
            Step::Thought { text, dur } => {
                let d = dur.map_or_else(|| "thinking…".to_string(), |d| format!("thought for {}", crate::time::dur(d)));
                rows.push(Line::from_span(format!("  · {d}"), Style::dim()));
                let st = Style::dim().with_italic();
                rows.extend(wrapped(text.trim(), "    ", st, Style::PLAIN, width));
            }
        }
    }
    rows
}

/// The rows of an activity block (§3.6): running, frozen, or in full.
pub fn activity_rows(a: &Activity, width: usize, now: Instant, o: &RowOpts) -> Vec<Line> {
    if a.full || o.expanded {
        return activity_full(a, width, now, o.spin);
    }
    let tools: Vec<&Tool> = a.tools().collect();
    let w = width.saturating_sub(2);
    if a.frozen.is_some() {
        if let [t] = tools.as_slice() {
            let mut rows = vec![tool_row(t, width, now, o.spin)];
            rows.extend(fail_tail(t, width, 2, FAIL_TAIL));
            rows.extend(nested(a, t, width, o.spin));
            return rows;
        }
        let mut rows = vec![activity_summary(a, width)];
        for t in tools.iter().filter(|t| t.failed() || t.no_result()) {
            rows.push(indent(tool_row(t, w, now, o.spin), 2));
            rows.extend(fail_tail(t, width, 4, FAIL_TAIL));
        }
        return rows;
    }
    // the current call (the newest running one) is the header's action; its time and output follow
    let cur = (0..tools.len()).rev().find(|&i| tools[i].running());
    let mut rows = vec![activity_header(a, width, now, o.spin)];
    if let Some(p) = cur.and_then(|i| pulse(tools[i], width, now, true)) {
        rows.push(p);
    }
    if let Some(n) = cur.and_then(|i| nested(a, tools[i], width, o.spin)) {
        rows.push(n);
    }
    if o.verbose
        && a.thinking.is_some()
        && let Some(Step::Thought { text, .. }) = a.steps.last()
    {
        let st = Style::dim().with_italic();
        let lines = wrapped(text.trim(), "    ", st, Style::PLAIN, width);
        let skip = lines.len().saturating_sub(3);
        rows.extend(lines.into_iter().skip(skip));
    }
    let done: Vec<usize> = (0..tools.len()).filter(|&i| !tools[i].running() && !tools[i].failed() && !tools[i].no_result()).collect();
    let keep = &done[done.len().saturating_sub(KEEP_DONE)..];
    let folded = done.len() - keep.len();
    if folded > 0 {
        rows.push(Line::from_span(format!("  … +{folded} more"), Style::dim()));
    }
    for (i, t) in tools.iter().enumerate() {
        if Some(i) == cur {
            continue;
        }
        if keep.contains(&i) || t.running() || t.failed() || t.no_result() {
            rows.push(indent(tool_row(t, w, now, o.spin), 2));
            rows.extend(fail_tail(t, width, 4, FAIL_TAIL_LIVE));
            if t.running()
                && let Some(p) = pulse(t, width, now, false)
            {
                rows.push(p);
            }
            rows.extend(nested(a, t, width, o.spin));
        }
    }
    rows
}

pub struct RowOpts {
    pub verbose: bool,
    pub expanded: bool,
    pub spin: usize,
    pub first: bool,
}

/// The rows of one block (not a streaming text block; see [`text_rows`]).
pub fn block_rows(kind: &Kind, width: usize, now: Instant, o: &RowOpts) -> Vec<Line> {
    match kind {
        Kind::Owner { text, st, .. } => {
            let style = match st {
                OwnerSt::Pending => Style::dim(),
                _ => Style::PLAIN,
            };
            let mut rows = Vec::new();
            if !o.first {
                rows.push(Line::new());
            }
            let mut body = wrapped(text, "› ", style, Style::dim(), width);
            let suffix = match st {
                OwnerSt::Queued => Some(("· queued", Style::dim())),
                OwnerSt::Seen => Some(("· seen", Style::dim())),
                OwnerSt::NotQueued => Some(("· not queued", Style::fg(Role::Harm))),
                OwnerSt::Unconfirmed => Some(("· no confirmation seen (it may have been delivered before the Root restarted); check before resending", Style::fg(Role::Warn))),
                _ => None,
            };
            if let Some((s, st)) = suffix {
                let last = body.last_mut().expect("at least one row");
                if last.width() + 1 + wrap::str_width(s) <= width {
                    last.push(" ", Style::PLAIN);
                    last.push(s, st);
                } else {
                    body.extend(wrap::wrap(&Line::from_span(format!("  {s}"), st), width, 2));
                }
            }
            rows.extend(body);
            rows
        }
        Kind::Text { lines, .. } => {
            let mut n = lines.len();
            while n > 0 && lines[n - 1].trim().is_empty() {
                n -= 1;
            }
            text_rows(lines, 0, n, MdState::default(), width).0
        }
        Kind::Thinking { text, done, started, dur } => {
            let mut rows = Vec::new();
            let head = if *done {
                format!("· thought for {}", crate::time::dur(dur.unwrap_or_default()))
            } else {
                format!("· thinking… {}", crate::time::dur(now.duration_since(*started)))
            };
            rows.push(Line::from_span(head, Style::dim()));
            if o.verbose || o.expanded {
                let st = Style::dim().with_italic();
                rows.extend(wrapped(text, "  ", st, Style::PLAIN, width));
            }
            rows
        }
        Kind::Activity(a) => activity_rows(a, width, now, o),
        // per-request token lines: the status bar sums them; the transcript view shows them
        Kind::TurnEnd(_) => Vec::new(),
        Kind::Info(text) => wrapped(text, "· ", Style::dim(), Style::dim(), width),
        Kind::Error(text) => wrapped(text, "✕ ", Style::PLAIN, Style::fg(Role::Harm), width),
        Kind::Notice { class, level, text, src } => notice_rows(*class, level, text, src.as_deref(), width),
        Kind::Repeat { n, .. } => vec![Line::from_span(format!("  ↳ ×{n} in all"), Style::mute())],
        Kind::Table { title, rows } => super::table::rows(Line::from_span("·", Style::dim()), title, rows, width),
        Kind::Identity { level } => vec![identity(*level, width)],
        Kind::Gate(r) => super::gates::committed_rows(r, width, o.expanded),
        Kind::Divider(text) => {
            let s = format!("── {text} ──");
            wrap::wrap(&sanitize::inline(&s, Style::dim()), width, 3)
        }
        Kind::Retry => vec![Line::from_span("↺ retry: the partial reply above was discarded", Style::dim())],
    }
}

/// TUI-BRAND: the identity line at the first attach: the Cell mark (three cells and one outlined
/// in the rewrite colour), `INIT` and the version, the level, and `root ok` in the Root's colour,
/// the only amber on the line. No banner, no wordmark, no animation.
pub fn identity(level: Option<i64>, width: usize) -> Line {
    let mut l = Line::from_span("■■■", Style::PLAIN);
    l.push("□", Style::fg(Role::Rewrite));
    l.push(" INIT", Style::PLAIN);
    l.push(format!(" {}", env!("CARGO_PKG_VERSION")), Style::dim());
    l.push(" · ", Style::mute());
    l.push(level.map_or_else(|| "L?".into(), |n| format!("L{n}")), Style::PLAIN);
    l.push(" · ", Style::mute());
    l.push("root ok", Style::fg(Role::Root));
    l.truncate(width)
}

/// The level in the Root's greeting (`[root] attached: slot running, level L0, 0 queued. …`).
pub fn greeting_level(text: &str) -> Option<i64> {
    let at = text.find("level L")? + "level L".len();
    let digits: String = text[at..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// A notice (§3.10): the state glyph in its signal colour (`✓` pass, `✕` harm), the source tag (the
/// Root's `[root]` in the Root's colour, only on a frame the Root stamped `src:"root"`; any other tag
/// muted, so a kernel's `[root]` look-alike stays grey), and the text in ink, or dim for routine
/// information. PANIC stays a full-width reverse-video harm line.
pub fn notice_rows(class: Class, level: &str, text: &str, src: Option<&str>, width: usize) -> Vec<Line> {
    let (glyph, gst, st) = match class {
        Class::Promotion | Class::LevelUp => ("✓ ", Style::fg(Role::Pass), Style::PLAIN),
        Class::Rollback | Class::Demotion | Class::Ceiling | Class::Halt | Class::GateFail => ("✕ ", Style::fg(Role::Harm), Style::PLAIN),
        Class::Panic => {
            let st = Style::fg(Role::Harm).with_reverse().with_bold();
            return wrapped(text, "", st, st, width).into_iter().map(|l| l.pad(width, st)).collect();
        }
        Class::SafeMode => ("", Style::PLAIN, Style::fg(Role::Warn)),
        Class::Plain => match level {
            "warn" => ("", Style::PLAIN, Style::fg(Role::Warn)),
            "alert" => ("✕ ", Style::fg(Role::Harm), Style::PLAIN),
            _ => ("", Style::PLAIN, Style::dim()),
        },
    };
    // the TUI's own ceiling notice carries its old glyph
    let text = text.strip_prefix("■ ").unwrap_or(text);
    // TUI-BRAND: the Root's JSON answers (`/root status`, `/root ledger`) as tables
    if src == Some("root")
        && class == Class::Plain
        && let Some((title, rows)) = text.strip_prefix("[root] ").and_then(super::table::root_answer)
    {
        return super::table::rows(Line::from_span("[root]", Style::fg(Role::Root)), title, &rows, width);
    }
    tagged(glyph, gst, text, st, src == Some("root"), width)
}

/// `glyph [tag] text`, wrapped under the glyph: a leading `[name]` tag is set apart, in the Root's
/// colour when `root` (the Root stamped the frame) and the tag is `[root]`, else muted.
fn tagged(glyph: &str, gst: Style, text: &str, st: Style, root: bool, width: usize) -> Vec<Line> {
    let pw = wrap::str_width(glyph);
    let (tag, rest) = match text.split_once(' ') {
        Some((t, r)) if t.len() <= 12 && t.starts_with('[') && t.ends_with(']') && !t[1..].contains('[') => {
            let tst = if root && t == "[root]" { Style::fg(Role::Root) } else { Style::mute() };
            (Some(sanitize::inline(t, tst)), r)
        }
        _ => (None, text),
    };
    let mut first = Line::from_span(glyph, gst);
    if let Some(t) = tag {
        first.extend(t);
        first.push(" ", Style::PLAIN);
    }
    let mut rows = Vec::new();
    for (i, l) in sanitize::lines(rest, st, first.width()).into_iter().enumerate() {
        let mut line = if i == 0 { std::mem::take(&mut first) } else { Line::from_span(" ".repeat(pw), Style::PLAIN) };
        line.extend(l);
        rows.extend(wrap::wrap(&line, width, pw));
    }
    if rows.is_empty() {
        rows.push(first.truncate(width));
    }
    rows
}

/// Prints finished blocks once (in order), and complete lines of a streaming text block when the
/// live region would not fit in `max_live` rows. Returns the rows to print above the live region.
pub fn commit(t: &mut Transcript, width: usize, max_live: usize, verbose: bool, now: Instant, spin: usize) -> Vec<Line> {
    let mut out = Vec::new();
    loop {
        let i = t.committed;
        if i >= t.len() {
            break;
        }
        let first = i == 0;
        let Some(b) = t.get(i) else {
            t.committed += 1;
            continue;
        };
        // (rows to print, new text_committed, new md state, the block is done)
        let text_step = if let Kind::Text { lines, done } = &b.kind {
            let total = lines.len();
            let upto = if *done {
                let mut n = total;
                while n > t.text_committed && lines[n - 1].trim().is_empty() {
                    n -= 1;
                }
                n
            } else {
                // a completed paragraph: up to the last blank line among the complete lines
                (t.text_committed..total.saturating_sub(1)).rev().find(|&k| lines[k].trim().is_empty()).map_or(t.text_committed, |k| k + 1)
            };
            let (rows, st) = if upto > t.text_committed { text_rows(lines, t.text_committed, upto, t.md_at_commit, width) } else { (Vec::new(), t.md_at_commit) };
            Some((rows, upto.max(t.text_committed), st, *done))
        } else {
            None
        };
        if let Some((rows, upto, st, done)) = text_step {
            out.extend(rows);
            t.text_committed = upto;
            t.md_at_commit = st;
            if done {
                t.committed += 1;
                t.text_committed = 0;
                t.md_at_commit = MdState::default();
                continue;
            }
            break;
        }
        if !b.finished() {
            break;
        }
        let o = RowOpts { verbose, expanded: false, spin, first };
        out.extend(block_rows(&b.kind, width, now, &o));
        t.committed += 1;
    }
    // overflow: the live region must fit. Row counts are computed once and subtracted as blocks
    // or lines go up, so a long backlog costs O(n), not O(n²).
    // a long backlog of complete text lines (a big replay): every line but the last `max_live` goes up
    // in one batch without being rendered twice
    if let Some(Kind::Text { lines, .. }) = t.get(t.committed).map(|b| &b.kind) {
        let complete = lines.len().saturating_sub(1);
        let upto = complete.saturating_sub(max_live);
        if upto > t.text_committed {
            let (rows, st) = text_rows(lines, t.text_committed, upto, t.md_at_commit, width);
            out.extend(rows);
            t.text_committed = upto;
            t.md_at_commit = st;
        }
    }
    let mut total = live_rows(t, width, verbose, now, spin).len();
    while total > max_live {
        let i = t.committed;
        let Some(b) = t.get(i) else { break };
        match &b.kind {
            Kind::Text { lines, .. } => {
                let complete = lines.len().saturating_sub(1);
                if t.text_committed >= complete {
                    break; // only the growing last line is left: the live region shows its tail
                }
                let (rows, st) = text_rows(lines, t.text_committed, t.text_committed + 1, t.md_at_commit, width);
                total = total.saturating_sub(rows.len());
                out.extend(rows);
                t.text_committed += 1;
                t.md_at_commit = st;
            }
            k => {
                let rows = block_rows(k, width, now, &RowOpts { verbose, expanded: false, spin, first: i == 0 });
                total = total.saturating_sub(rows.len());
                out.extend(rows);
                t.committed += 1;
            }
        }
    }
    out
}

/// The rows of the uncommitted blocks.
pub fn live_rows(t: &Transcript, width: usize, verbose: bool, now: Instant, spin: usize) -> Vec<Line> {
    let mut rows = Vec::new();
    for (i, b) in t.live() {
        if let Kind::Text { lines, .. } = &b.kind {
            let from = if i == t.committed { t.text_committed } else { 0 };
            let st = if i == t.committed { t.md_at_commit } else { MdState::default() };
            let mut to = lines.len();
            while to > from && lines[to - 1].is_empty() && to == lines.len() && to - 1 > from {
                to -= 1;
            }
            if lines[from..to].iter().all(String::is_empty) {
                continue;
            }
            rows.extend(text_rows(lines, from, to, st, width).0);
            continue;
        }
        let o = RowOpts { verbose, expanded: false, spin, first: i == 0 };
        rows.extend(block_rows(&b.kind, width, now, &o));
    }
    rows
}

/// The input box: borders, the wrapped buffer (at most `cap` rows, scrolled to the caret), and the
/// caret's (column, row) inside the returned rows.
pub fn input_box(ed: &Editor, width: usize, cap: usize, placeholder: &str) -> (Vec<Line>, (usize, usize)) {
    let inner = width.saturating_sub(6).max(1);
    let border = Style::fg(Role::Line);
    let mut rows: Vec<Line> = vec![Line::new()];
    let mut caret = (0usize, 0usize);
    let mut col = 0;
    let text = ed.text();
    let cur = ed.cursor();
    let mut found = false;
    for (i, g) in unicode_segmentation::UnicodeSegmentation::grapheme_indices(text, true) {
        if i == cur {
            caret = (col, rows.len() - 1);
            found = true;
        }
        if g == "\n" || g == "\r\n" {
            rows.push(Line::new());
            col = 0;
            continue;
        }
        let w = wrap::g_width(g);
        if col + w > inner {
            rows.push(Line::new());
            col = 0;
            if i == cur {
                caret = (0, rows.len() - 1);
            }
        }
        // the sanitizer for the owner's own text: a pasted escape stays inert
        let safe = sanitize::inline(g, Style::PLAIN);
        rows.last_mut().expect("never empty").extend(safe);
        col += w;
    }
    if !found {
        if col >= inner {
            rows.push(Line::new());
            col = 0;
        }
        caret = (col, rows.len() - 1);
    }
    let cap = cap.max(1);
    let top = if caret.1 >= cap { caret.1 + 1 - cap } else { 0 };
    let shown: Vec<Line> = rows.iter().skip(top).take(cap).cloned().collect();
    let mut out = Vec::with_capacity(shown.len() + 2);
    out.push(Line::from_span(format!("┌{}┐", "─".repeat(width.saturating_sub(2))), border));
    for (r, l) in shown.into_iter().enumerate() {
        let mut row = Line::from_span("│ ", border);
        let prompt = if r + top == 0 { "› " } else { "  " };
        row.push(prompt, Style::mute());
        if text.is_empty() && r == 0 {
            row.extend(Line::from_span(placeholder, Style::mute()).truncate(inner));
        } else {
            row.extend(l.truncate(inner));
        }
        let row = row.pad(width.saturating_sub(2), Style::PLAIN);
        let mut row = row.truncate(width.saturating_sub(2));
        row.push(" │", border);
        out.push(row);
    }
    out.push(Line::from_span(format!("└{}┘", "─".repeat(width.saturating_sub(2))), border));
    let caret = (4 + caret.0, caret.1 - top + 1);
    (out, caret)
}

pub const HINT: &str = "Enter send · Shift-Enter newline · Ctrl-C interrupt · Ctrl-O details · ? keys";
/// STEER: the hint while lines wait in the tray.
pub const TRAY_HINT: &str = "Esc interrupt and send it now · ↑ take back the newest · Tab queue the next task";
/// STEER: the input box's placeholder while the agent works.
pub const BUSY_PLACEHOLDER: &str = "Enter steers · Tab queues the next task · Esc interrupts";
/// STEER: tray rows shown at most; older ones fold into `… +N more`.
pub const TRAY_ROWS: usize = 4;
/// Completion popup rows shown at once (a window that follows the selection).
pub const POPUP_ROWS: usize = 8;

/// STEER: when a pending line will be read: after the running call (the batch's results come
/// first), after the reply being written, or why it waits.
pub fn tray_when(it: &TrayItem, t: &Transcript, info: &Info, now: Instant) -> String {
    match it.st {
        TraySt::Retracting => "taking back…".into(),
        TraySt::Sending if it.follow_up => "next task · sending…".into(),
        TraySt::Sending => "sending…".into(),
        TraySt::Held => "queued while INIT restarts".into(),
        TraySt::Pending if it.follow_up => "next task".into(),
        TraySt::Pending => {
            let slot = info.state.as_ref().and_then(|s| s["slot"].as_str());
            if slot.is_some_and(|s| s != "running") {
                "held while INIT updates".into()
            } else if !info.busy() {
                "starting a run".into()
            } else if let Some(tool) = t.running_tool() {
                let call = tool.start_text.lines().next().unwrap_or("").trim();
                let call = Line::from_span(call, Style::PLAIN).truncate(18).text();
                format!("after {}: {call} · {}", tool.name, crate::time::span(now.duration_since(tool.started).as_secs()))
            } else {
                "after this reply".into()
            }
        }
    }
}

/// STEER: the tray above the input box: one row per pending line, newest last, with when it will
/// be read on the right.
pub fn tray_rows(tray: &Tray, t: &Transcript, info: &Info, width: usize, now: Instant) -> Vec<Line> {
    let mut rows = Vec::new();
    let n = tray.items.len();
    if n > TRAY_ROWS {
        rows.push(Line::from_span(format!("  … +{} more waiting", n - TRAY_ROWS), Style::dim()));
    }
    for it in tray.items.iter().skip(n.saturating_sub(TRAY_ROWS)) {
        let mut left = Line::from_span("› ", Style::dim());
        let first = it.text.lines().next().unwrap_or("");
        left.extend(sanitize::inline(first, Style::dim()));
        if it.text.trim_end().contains('\n') {
            left.push(" …", Style::dim());
        }
        let st = Style::dim().with_italic();
        let right = Line::from_span(tray_when(it, t, info, now), st);
        rows.push(split_row(left, right, width));
    }
    rows
}

/// The whole live region for a `width` × `height` terminal.
pub fn region(ui: &Ui, width: usize, height: usize) -> Region {
    if width < 20 || height < 5 {
        let mut rows = vec![Line::from_span("terminal too small", Style::dim())];
        let tail: String = ui.ed.text().chars().rev().take(width.saturating_sub(3)).collect::<Vec<_>>().into_iter().rev().collect();
        let mut input = Line::from_span("› ", Style::mute());
        input.extend(sanitize::inline(&tail, Style::PLAIN));
        let cx = input.width().min(width.saturating_sub(1));
        rows.push(input.truncate(width));
        rows.push(status::bar(ui.info, width, ui.now, ui.now_ms, ui.spin, ui.no_color));
        return Region { rows, cursor: Some((cx, 1)) };
    }
    let cap = 10.min(height * 2 / 5).max(1);
    let placeholder = if ui.info.busy() {
        BUSY_PLACEHOLDER
    } else {
        "Type a message · / for commands · ? for keys"
    };
    // TUI-PICKER: an open picker takes the input box's place
    let (input, caret) = match &ui.ed.picker { Some(p) => super::picker::rows(p, width), None => input_box(ui.ed, width, cap, placeholder) };
    // the popup or the overlay
    let mut extra: Vec<Line> = Vec::new();
    if ui.overlay {
        extra.push(Line::from_span("KEYS", Style::mute()));
        for (k, v) in HELP {
            let mut l = Line::from_span(format!("  {k:<44}"), Style::PLAIN);
            l.push(*v, Style::dim());
            extra.push(l.truncate(width));
        }
    } else {
        let c = ui.ed.completions();
        let sel = ui.ed.sel.min(c.len().saturating_sub(1));
        // a window of 8 that follows the selection (the stopping commands sit at the end of the list)
        let first = (sel + 1).saturating_sub(POPUP_ROWS);
        for (i, (cmd, desc)) in c.iter().enumerate().skip(first).take(POPUP_ROWS) {
            let st = if i == sel { Style::PLAIN.with_reverse() } else { Style::PLAIN };
            let mut l = Line::from_span(format!("  {cmd:<14}"), st);
            // TUI-FIX: the commands that stop the agent read as attention, not as routine
            let quiet = if crate::proto::STOPPING.contains(cmd) { Style::fg(Role::Warn) } else { Style::dim() };
            l.push(format!(" {desc}"), if i == sel { st } else { quiet });
            extra.push(l.truncate(width));
        }
    }
    let hint: Option<Line> = match ui.hint {
        Some(h) => Some(Line::from_span(format!(" {h}"), Style::PLAIN).truncate(width)),
        None if ui.ed.picker.is_some() || ui.ed.waiting.is_some() => super::picker::hint_row(ui.ed, width), // TUI-PICKER
        None if ui.ed.is_empty() && !ui.tray.is_empty() => Some(Line::from_span(format!(" {TRAY_HINT}"), Style::mute()).truncate(width)),
        None if ui.ed.is_empty() => Some(Line::from_span(format!(" {HINT}"), Style::mute()).truncate(width)),
        None => None,
    };
    extra.extend(tray_rows(ui.tray, ui.t, ui.info, width, ui.now));
    // TUI-BRAND: what the agent is doing right now, directly above the input box
    extra.extend(liveness(ui, width));
    // TUI-OBSERVE: the self-change in flight, above the popup and the tray (§3.10a)
    let gates = ui.t.gates.live_rows(width, ui.now_ms, super::gates::Track::rows_for(height));
    extra.splice(0..0, gates);
    let fixed = input.len() + extra.len() + 1 + usize::from(hint.is_some());
    let max_live = height.saturating_sub(fixed + 1);
    let mut rows = live_rows(ui.t, width, ui.verbose, ui.now, ui.spin);
    if rows.len() > max_live {
        rows.drain(..rows.len() - max_live);
    }
    let base = rows.len() + extra.len();
    rows.extend(extra);
    rows.extend(input);
    rows.push(status::bar(ui.info, width, ui.now, ui.now_ms, ui.spin, ui.no_color));
    if let Some(h) = hint {
        rows.push(h);
    }
    Region { rows, cursor: Some((caret.0, base + caret.1)) }
}

/// Rows the live blocks may use on a `height`-row terminal (§3.4: rows − input − 3).
pub fn max_live(ui: &Ui, width: usize, height: usize) -> usize {
    let cap = 10.min(height * 2 / 5).max(1);
    let (input, _) = match &ui.ed.picker { Some(p) => super::picker::rows(p, width), None => input_box(ui.ed, width, cap, "") };
    let extra = if ui.overlay { HELP.len() + 1 } else { ui.ed.completions().len().min(POPUP_ROWS) };
    let tray = ui.tray.items.len().min(TRAY_ROWS + 1);
    let gates = ui.t.gates.live_rows(width, ui.now_ms, super::gates::Track::rows_for(height)).len();
    let busy = usize::from(liveness(ui, width).is_some());
    height.saturating_sub(input.len() + extra + tray + gates + busy + 3)
}

/// A stream with no new text for this long reads `no new text for N s` on the liveness row.
pub const STALL: Duration = Duration::from_secs(3);

/// TUI-BRAND: the liveness row (§3.4). While the agent works it is always one row directly above the
/// input box, never dropped for width: what the agent is doing (waiting for the model, thinking,
/// writing, a tool by name, or a stream that went quiet) and for how long, and on the right the
/// run's time. The spinner is telemetry beside something in flight; the words carry the state for
/// `NO_COLOR` and screen readers.
pub fn liveness(ui: &Ui, width: usize) -> Option<Line> {
    // TUI-PICKER's agent question (`ask_owner`): the agent is idle until the owner answers, so the row
    // says so in ink, still (nothing is in flight); with the picker dismissed, the question follows
    let asked = ui.ed.picker.iter().chain(ui.ed.picker_next.iter()).any(|p| p.who == super::picker::Who::Agent);
    if !ui.info.busy() && (asked || ui.ed.waiting.is_some()) {
        let mut l = Line::from_span("· waiting on you", Style::PLAIN);
        if let Some(q) = ui.ed.waiting.as_deref().filter(|_| !asked) {
            l.push(" · the agent asked: ", Style::mute());
            l.extend(sanitize::inline(q.lines().next().unwrap_or(""), Style::dim()));
        }
        return Some(l.truncate(width));
    }
    let run = ui.info.busy_since.filter(|_| ui.info.busy())?;
    let (t, now) = (ui.t, ui.now);
    // the last sign of life from the body in this run
    let last = t.last_display.filter(|&l| l >= run).unwrap_or(run);
    let secs = |from: Instant| crate::time::span(now.saturating_duration_since(from).as_secs());
    let mut left = Line::from_span(format!("{} ", SPINNER[ui.spin % SPINNER.len()]), Style::PLAIN);
    if let Some(tool) = t.running_tool() {
        left.push("running ", Style::PLAIN);
        left.extend(sanitize::inline(&tool.name, Style::PLAIN));
        left.push(format!(" · {}", secs(tool.started)), Style::dim());
    } else if let Some(since) = t.thinking_since() {
        left.push("thinking", Style::PLAIN);
        left.push(format!(" · {}", secs(since)), Style::dim());
    } else if t.writing() && now.saturating_duration_since(last) < STALL {
        left.push("writing", Style::PLAIN);
    } else if t.writing() {
        left.push("writing", Style::PLAIN);
        left.push(format!(" · no new text for {}", secs(last)), Style::dim());
    } else {
        left.push("waiting for the model", Style::PLAIN);
        left.push(format!(" · {}", secs(last)), Style::dim());
    }
    let right = Line::from_span(format!("run {}", crate::time::elapsed(now.saturating_duration_since(run))), Style::mute());
    Some(split_row(left, right, width))
}

/// TUI-FIX (v0.5.1): the line printed when `init-agent` quits: what keeps running, how to come back
/// and how to stop it, and the Root ledger's total (an estimate), so quitting is never silent.
pub fn goodbye(info: &Info, width: usize, attached: bool) -> Vec<Line> {
    if !attached {
        return wrapped("init-agent closed. It never reached the Root, so nothing was attached or changed.", "· ", Style::dim(), Style::dim(), width);
    }
    let spent = info.egress.as_ref().filter(|_| !info.egress_failed).and_then(|e| e["money"]["total_spent_musd"].as_i64()).filter(|m| *m >= 0);
    let money = spent.map(|m| format!(" Metered so far: {} in all (Root ledger, an estimate).", super::usd(m))).unwrap_or_default();
    let text = format!(
        "init-agent closed. The agent keeps running under the Root (idle until it has work): run init-agent to come back; /root stop or /panic there stops it.{money}"
    );
    wrapped(&text, "· ", Style::dim(), Style::dim(), width)
}

/// For the spinner: the elapsed tenth-seconds since `t0`.
pub fn spin_at(t0: Instant, now: Instant) -> usize {
    (now.duration_since(t0).as_millis() / 100) as usize
}

