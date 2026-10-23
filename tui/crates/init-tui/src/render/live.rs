//! Blocks to rows, committing (printing once above the live region), and the live region itself:
//! the uncommitted blocks, the completion popup or key overlay, the input box, the status bar and the
//! hint line (docs/tui.md §3.4–§3.7).

use super::md::{self, MdState};
use super::{Color, Line, Region, Style, sanitize, status, wrap};
use crate::input::Editor;
use crate::keys::HELP;
use crate::model::{Info, Kind, OwnerSt, Tool, Transcript};
use crate::notice::Class;
use std::time::Instant;

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Everything a render reads.
pub struct Ui<'a> {
    pub t: &'a Transcript,
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
pub fn tool_row(t: &Tool, width: usize, now: Instant, spin: usize) -> Line {
    let (marker, right) = match (t.ok, t.dur) {
        (Some(true), d) => ("▸", {
            let mut r = Line::new();
            r.push("✓ ", Style::fg(Color::Green));
            r.extend(sanitize::inline(t.summary.as_deref().unwrap_or(""), Style::dim()));
            r.push(format!(" · {}", crate::time::dur(d.unwrap_or_default())), Style::dim());
            r
        }),
        (Some(false), d) => ("▸", {
            let mut r = Line::new();
            r.push("✗ ", Style::fg(Color::Red));
            r.extend(sanitize::inline(t.summary.as_deref().unwrap_or(""), Style::fg(Color::Red)));
            r.push(format!(" · {}", crate::time::dur(d.unwrap_or_default())), Style::dim());
            r
        }),
        (None, Some(d)) => ("▸", Line::from_span(format!("? no result · {}", crate::time::dur(d)), Style::dim())),
        (None, None) => (SPINNER[spin % SPINNER.len()], Line::from_span(format!("running · {}", crate::time::dur(now.duration_since(t.started))), Style::dim())),
    };
    let mut left = Line::new();
    left.push(format!("{marker} "), if t.ok.is_none() { Style::fg(Color::Cyan) } else { Style::dim() });
    let name = sanitize::inline(&t.name, Style::bold());
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
            let mut body = wrapped(text, "› ", style, Style::fg(Color::Cyan).with_bold(), width);
            let suffix = match st {
                OwnerSt::Queued => Some(("· queued", Style::dim())),
                OwnerSt::NotQueued => Some(("· not queued", Style::fg(Color::Red))),
                OwnerSt::Unconfirmed => Some(("· no confirmation seen (it may have been delivered before the Root restarted); check before resending", Style::fg(Color::Yellow))),
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
                format!("✻ thought for {}", crate::time::dur(dur.unwrap_or_default()))
            } else {
                format!("{} thinking… {}", SPINNER[o.spin % SPINNER.len()], crate::time::dur(now.duration_since(*started)))
            };
            rows.push(Line::from_span(head, Style::dim()));
            if o.verbose || o.expanded {
                let st = Style { italic: true, dim: true, ..Style::PLAIN };
                rows.extend(wrapped(text, "  ", st, Style::PLAIN, width));
            }
            rows
        }
        Kind::Tool(t) => {
            if o.expanded {
                tool_expanded(t, width, now, o.spin)
            } else {
                vec![tool_row(t, width, now, o.spin)]
            }
        }
        Kind::TurnEnd(text) => wrapped(text, "· ", Style::dim(), Style::dim(), width),
        Kind::Info(text) => wrapped(text, "· ", Style::dim(), Style::dim(), width),
        Kind::Error(text) => wrapped(text, "✗ ", Style::fg(Color::Red), Style::fg(Color::Red), width),
        Kind::Notice { class, level, text } => notice_rows(*class, level, text, width),
        Kind::Divider(text) => {
            let s = format!("── {text} ──");
            wrap::wrap(&sanitize::inline(&s, Style::dim()), width, 3)
        }
        Kind::Retry => vec![Line::from_span("↺ retry: the partial reply above was discarded", Style::dim())],
    }
}

pub fn notice_rows(class: Class, level: &str, text: &str, width: usize) -> Vec<Line> {
    let (prefix, st) = match class {
        Class::Promotion | Class::LevelUp => ("▲ ", Style::fg(Color::Green)),
        Class::Rollback => ("▼ ", Style::fg(Color::Yellow)),
        Class::Demotion => ("▼ ", Style::fg(Color::Red)),
        Class::Ceiling => (if text.starts_with('■') { "" } else { "■ " }, Style::fg(Color::Red)),
        Class::Panic => {
            let st = Style::fg(Color::Red).with_reverse().with_bold();
            return wrapped(text, "", st, st, width).into_iter().map(|l| l.pad(width, st)).collect();
        }
        Class::SafeMode => ("", Style::fg(Color::Yellow)),
        Class::Halt => ("", Style::fg(Color::Red)),
        Class::Plain => match level {
            "warn" => ("", Style::fg(Color::Yellow)),
            "alert" => ("", Style::fg(Color::Red)),
            _ => ("", Style::dim()),
        },
    };
    wrapped(text, prefix, st, st, width)
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
    let border = Style::fg(Color::Grey);
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
    out.push(Line::from_span(format!("╭{}╮", "─".repeat(width.saturating_sub(2))), border));
    for (r, l) in shown.into_iter().enumerate() {
        let mut row = Line::from_span("│ ", border);
        let prompt = if r + top == 0 { "> " } else { "  " };
        row.push(prompt, Style::fg(Color::Cyan));
        if text.is_empty() && r == 0 {
            row.extend(Line::from_span(placeholder, Style::dim()).truncate(inner));
        } else {
            row.extend(l.truncate(inner));
        }
        let row = row.pad(width.saturating_sub(2), Style::PLAIN);
        let mut row = row.truncate(width.saturating_sub(2));
        row.push(" │", border);
        out.push(row);
    }
    out.push(Line::from_span(format!("╰{}╯", "─".repeat(width.saturating_sub(2))), border));
    let caret = (4 + caret.0, caret.1 - top + 1);
    (out, caret)
}

pub const HINT: &str = "Enter send · Shift-Enter newline · Ctrl-C interrupt (twice: quit) · Ctrl-O transcript · ? keys";

/// The whole live region for a `width` × `height` terminal.
pub fn region(ui: &Ui, width: usize, height: usize) -> Region {
    if width < 20 || height < 5 {
        let mut rows = vec![Line::from_span("terminal too small", Style::dim())];
        let tail: String = ui.ed.text().chars().rev().take(width.saturating_sub(3)).collect::<Vec<_>>().into_iter().rev().collect();
        let mut input = Line::from_span("> ", Style::fg(Color::Cyan));
        input.extend(sanitize::inline(&tail, Style::PLAIN));
        let cx = input.width().min(width.saturating_sub(1));
        rows.push(input.truncate(width));
        rows.push(status::bar(ui.info, width, ui.now, ui.now_ms, ui.spin, ui.no_color));
        return Region { rows, cursor: Some((cx, 1)) };
    }
    let cap = 10.min(height * 2 / 5).max(1);
    let placeholder = if ui.info.busy() {
        "Enter steers the current run · /later <text> queues a follow-up"
    } else {
        "Type a message · / for commands · ? for keys"
    };
    let (input, caret) = input_box(ui.ed, width, cap, placeholder);
    // the popup or the overlay
    let mut extra: Vec<Line> = Vec::new();
    if ui.overlay {
        extra.push(Line::from_span("keys", Style::bold()));
        for (k, v) in HELP {
            let mut l = Line::from_span(format!("  {k:<44}"), Style::fg(Color::Cyan));
            l.push(*v, Style::PLAIN);
            extra.push(l.truncate(width));
        }
    } else {
        let c = ui.ed.completions();
        let sel = ui.ed.sel.min(c.len().saturating_sub(1));
        for (i, (cmd, desc)) in c.iter().enumerate().take(8) {
            let st = if i == sel { Style::PLAIN.with_reverse() } else { Style::PLAIN };
            let mut l = Line::from_span(format!("  {cmd:<14}"), st);
            l.push(format!(" {desc}"), if i == sel { st } else { Style::dim() });
            extra.push(l.truncate(width));
        }
    }
    let hint: Option<Line> = match ui.hint {
        Some(h) => Some(Line::from_span(format!(" {h}"), Style::fg(Color::Yellow)).truncate(width)),
        None if ui.ed.is_empty() => Some(Line::from_span(format!(" {HINT}"), Style::dim()).truncate(width)),
        None => None,
    };
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
    let (input, _) = input_box(ui.ed, width, cap, "");
    let extra = if ui.overlay { HELP.len() + 1 } else { ui.ed.completions().len().min(8) };
    height.saturating_sub(input.len() + extra + 3)
}

/// For the spinner: the elapsed tenth-seconds since `t0`.
pub fn spin_at(t0: Instant, now: Instant) -> usize {
    (now.duration_since(t0).as_millis() / 100) as usize
}

