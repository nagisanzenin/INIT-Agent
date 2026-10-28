//! TUI-PICKER (docs/tui.md §3.7.2): the inline picker. It takes the input box's place while open:
//! ↑/↓ move (wrapping), Enter chooses, Esc cancels, 1–9 pick by number, typing filters a long list,
//! and with `multi` Space toggles and Enter confirms. At most [`VISIBLE`] option rows show; the list
//! scrolls inside the widget. No mouse capture: the inline layout, the terminal's selection and its
//! scrollback stay the terminal's.
//!
//! Two kinds of picker exist, and nothing else:
//! - **the TUI's own** ([`Who::Tui`]): first-run setup, `/setup`, `/model`, `/key`; built here from
//!   the Root's `providers.list` answer;
//! - **the agent's question** ([`Who::Agent`]): built only from a `display` frame's `ask` object
//!   ([`Ask::from_frame`]). Display frames always come from the agent zone, so whatever the body puts
//!   in the frame renders under the TUI's own `agent asks` label, in ink and dim only, never in the
//!   Root's colour or style, with every string cleaned and capped here ([`clean`]). The owner's choice
//!   goes back as an ordinary owner line (the chosen labels as text), never as a command: a label that
//!   starts with `/` is shown and sent quoted. The owner can always ignore the picker and type.
//!
//! No picker is the Root's own, so the Root's amber never appears in one. Style: focus and selection
//! by ink weight (the focused row bold ink, the rest dim), hairline rules in `Line`, the quiet words
//! in `Mute`.

use super::{Line, Role, Style, sanitize, wrap};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde_json::Value;

/// Option rows shown at once.
pub const VISIBLE: usize = 8;
/// The TUI's caps on an agent question, whatever the frame carries (characters).
pub const QUESTION_MAX: usize = 500;
pub const HEADER_MAX: usize = 24;
pub const LABEL_MAX: usize = 80;
pub const DESC_MAX: usize = 200;
pub const OPTIONS_MAX: usize = 4;
/// Rows of the question shown in the picker (the rest is cut with `…`).
const TITLE_ROWS: usize = 4;
/// The always-present last row of an agent question.
pub const OTHER: &str = "Type your own answer";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    /// The TUI's own question (setup, `/model`, `/key`).
    Tui,
    /// The agent's question, from a display frame.
    Agent,
}

/// One option. `id` is what the picker returns (the caller's index); the number shown is the row's
/// place in the list, 1-based, stable under filtering.
#[derive(Clone, Debug, PartialEq)]
pub struct Opt {
    pub id: usize,
    pub label: String,
    pub desc: Option<String>,
    /// `recommended` or `current`, shown after the label.
    pub tag: Option<&'static str>,
}

impl Opt {
    pub fn new(id: usize, label: impl Into<String>) -> Opt {
        Opt { id, label: label.into(), desc: None, tag: None }
    }
    pub fn desc(mut self, d: impl Into<String>) -> Opt {
        let d = d.into();
        self.desc = (!d.is_empty()).then_some(d);
        self
    }
    pub fn tag(mut self, t: &'static str) -> Opt {
        self.tag = Some(t);
        self
    }
}

/// What a key did.
#[derive(Clone, Debug, PartialEq)]
pub enum PickOut {
    None,
    /// The chosen options' ids (one for a single choice; the checked ones, in list order, for multi).
    Chosen(Vec<usize>),
    /// "Type your own answer": the picker closes and the box takes the answer.
    Other,
    /// Esc: the picker closes; the caller says what that means (leave setup, model unchanged, dismiss).
    Cancel,
    /// The owner started typing (an agent question): the picker closes and the box starts with this.
    Type(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Opt(usize),
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Picker {
    pub who: Who,
    /// The committed line's key (`provider`, `model`) or the agent's header chip.
    pub key: String,
    /// The question.
    pub title: String,
    pub opts: Vec<Opt>,
    pub multi: bool,
    /// An agent question's "Type your own answer" row.
    pub other: bool,
    /// What Esc does, for the hint (`leave setup`, `keep the current model`, `dismiss`).
    pub esc: &'static str,
    focus: usize,
    top: usize,
    filter: String,
    checked: Vec<bool>,
}

impl Picker {
    pub fn new(who: Who, key: impl Into<String>, title: impl Into<String>, opts: Vec<Opt>) -> Picker {
        let n = opts.len();
        Picker {
            who,
            key: key.into(),
            title: title.into(),
            opts,
            multi: false,
            other: who == Who::Agent,
            esc: if who == Who::Agent { "dismiss" } else { "cancel" },
            focus: 0,
            top: 0,
            filter: String::new(),
            checked: vec![false; n],
        }
    }
    pub fn multi(mut self, m: bool) -> Picker {
        self.multi = m;
        self
    }
    pub fn esc(mut self, e: &'static str) -> Picker {
        self.esc = e;
        self
    }
    /// Starts with the focus on the option with this id.
    #[cfg(test)]
    pub fn focus_id(mut self, id: usize) -> Picker {
        if let Some(i) = self.opts.iter().position(|o| o.id == id) {
            self.focus = i;
            self.scroll();
        }
        self
    }

    /// Typing filters the list (the TUI's own pickers); in an agent question typing is an answer.
    fn filters(&self) -> bool {
        !self.other
    }
    /// Digits pick only while no filter is typed and every row has a one-digit number.
    fn digits_pick(&self) -> bool {
        self.filter.is_empty() && self.opts.len() + usize::from(self.other) <= 9
    }
    fn matches(&self, o: &Opt) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        let f = self.filter.to_lowercase();
        o.label.to_lowercase().contains(&f) || o.desc.as_ref().is_some_and(|d| d.to_lowercase().contains(&f))
    }
    fn rows(&self) -> Vec<Row> {
        let mut v: Vec<Row> = (0..self.opts.len()).filter(|&i| self.matches(&self.opts[i])).map(Row::Opt).collect();
        if self.other && self.filter.is_empty() {
            v.push(Row::Other);
        }
        v
    }
    fn focused(&self) -> Option<Row> {
        self.rows().get(self.focus).copied()
    }
    fn scroll(&mut self) {
        if self.focus < self.top {
            self.top = self.focus;
        } else if self.focus >= self.top + VISIBLE {
            self.top = self.focus + 1 - VISIBLE;
        }
    }
    fn refiltered(&mut self) {
        self.focus = 0;
        self.top = 0;
    }
    fn chosen(&self) -> Vec<usize> {
        (0..self.opts.len()).filter(|&i| self.checked[i]).map(|i| self.opts[i].id).collect()
    }

    /// The option ids checked so far (multi).
    #[cfg(test)]
    pub fn checked_ids(&self) -> Vec<usize> {
        self.chosen()
    }

    /// One key. Ctrl-C, Ctrl-D and the other control keys are the caller's (it checks them first).
    pub fn key(&mut self, k: &KeyEvent) -> PickOut {
        if k.kind == KeyEventKind::Release {
            return PickOut::None;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let n = self.rows().len();
        match k.code {
            KeyCode::Up | KeyCode::BackTab => self.step(n, false),
            KeyCode::Char('p') if ctrl => self.step(n, false),
            KeyCode::Down | KeyCode::Tab => self.step(n, true),
            KeyCode::Char('n') if ctrl => self.step(n, true),
            KeyCode::Home => self.jump(0),
            KeyCode::End => self.jump(n.saturating_sub(1)),
            KeyCode::PageUp => self.jump(self.focus.saturating_sub(VISIBLE)),
            KeyCode::PageDown => self.jump((self.focus + VISIBLE).min(n.saturating_sub(1))),
            KeyCode::Enter => self.enter(),
            KeyCode::Esc => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.refiltered();
                    PickOut::None
                } else {
                    PickOut::Cancel
                }
            }
            KeyCode::Backspace => {
                if self.filter.pop().is_some() {
                    self.refiltered();
                }
                PickOut::None
            }
            KeyCode::Char(' ') if self.multi && self.filter.is_empty() => {
                if let Some(Row::Opt(i)) = self.focused() {
                    self.checked[i] = !self.checked[i];
                }
                PickOut::None
            }
            KeyCode::Char(_) if ctrl => PickOut::None,
            KeyCode::Char(c @ '1'..='9') if self.digits_pick() => self.digit(c as usize - '0' as usize),
            KeyCode::Char(c) => {
                if self.filters() {
                    if c == '/' && self.filter.is_empty() {
                        return PickOut::Type(c.to_string()); // an owner command: the box takes it
                    }
                    self.filter.push(c);
                    self.refiltered();
                    PickOut::None
                } else {
                    PickOut::Type(c.to_string())
                }
            }
            _ => PickOut::None,
        }
    }

    fn step(&mut self, n: usize, down: bool) -> PickOut {
        if n > 0 {
            self.focus = if down { (self.focus + 1) % n } else { (self.focus + n - 1) % n };
            self.scroll();
        }
        PickOut::None
    }
    fn jump(&mut self, to: usize) -> PickOut {
        self.focus = to;
        self.scroll();
        PickOut::None
    }
    fn enter(&mut self) -> PickOut {
        if self.multi {
            let c = self.chosen();
            if !c.is_empty() {
                return PickOut::Chosen(c);
            }
        }
        match self.focused() {
            Some(Row::Opt(i)) => PickOut::Chosen(vec![self.opts[i].id]),
            Some(Row::Other) => PickOut::Other,
            None => PickOut::None,
        }
    }
    /// A number key: the option with that number (multi: toggle it); the number after the last
    /// option is "Type your own answer".
    fn digit(&mut self, d: usize) -> PickOut {
        let n = self.opts.len();
        if d >= 1 && d <= n {
            let i = d - 1;
            self.focus = i;
            self.scroll();
            if self.multi {
                self.checked[i] = !self.checked[i];
                return PickOut::None;
            }
            return PickOut::Chosen(vec![self.opts[i].id]);
        }
        if self.other && d == n + 1 {
            return PickOut::Other;
        }
        if self.other {
            return PickOut::Type(d.to_string()); // an answer that starts with a number
        }
        PickOut::None
    }

    /// The chosen options' labels.
    pub fn labels(&self, ids: &[usize]) -> Vec<String> {
        ids.iter().filter_map(|id| self.opts.iter().find(|o| o.id == *id)).map(|o| o.label.clone()).collect()
    }
    /// The owner line an agent question's choice sends: the labels, joined with `, `. Never a command.
    pub fn answer(&self, ids: &[usize]) -> String {
        guard(&self.labels(ids).join(", "))
    }
    /// The transcript's record of an agent question once its picker closes: `agent asked · Database: …`.
    pub fn record(&self) -> String {
        if self.key.is_empty() { format!("agent asked: {}", self.title) } else { format!("agent asked · {}: {}", self.key, self.title) }
    }
    /// The one line a closed TUI picker leaves in the transcript (after `› `): `provider · DeepSeek`.
    pub fn committed(&self, ids: &[usize]) -> String {
        format!("{} · {}", self.key, self.labels(ids).join(", "))
    }
}

/// An answer line never starts with `/`, so no choice can be read as a Root, kernel or body command.
fn guard(s: &str) -> String {
    if s.trim_start().starts_with('/') { format!("\"{}\"", s.trim()) } else { s.to_string() }
}

// ---------------------------------------------------------------------------------------------
// The agent's question
// ---------------------------------------------------------------------------------------------

/// An agent question from a display frame's `ask` object, cleaned and capped.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub question: String,
    pub header: Option<String>,
    pub multi: bool,
    /// At most [`OPTIONS_MAX`]; the recommended one first and tagged.
    pub opts: Vec<Opt>,
}

fn invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{00AD}')
}

/// One line of agent text made inert and short: escape sequences (CSI, OSC, DCS, APC, PM, SOS)
/// dropped whole, other control, bidi and zero-width characters dropped, whitespace runs (line
/// breaks included) collapsed to one space, cut to `max` characters with `…`.
pub fn clean(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    let mut space = false;
    while let Some(c) = it.next() {
        if c == '\u{1B}' || c == '\u{9B}' || c == '\u{9D}' {
            let intro = match c {
                '\u{9B}' => Some('['),
                '\u{9D}' => Some(']'),
                _ => it.next(),
            };
            match intro {
                Some('[') => {
                    for n in it.by_ref() {
                        if ('\u{40}'..='\u{7E}').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']' | 'P' | '_' | '^' | 'X') => {
                    while let Some(n) = it.next() {
                        if n == '\u{7}' || n == '\u{9C}' {
                            break;
                        }
                        if n == '\u{1B}' {
                            if it.peek() == Some(&'\\') {
                                it.next();
                            }
                            break;
                        }
                    }
                }
                _ => {} // ESC and one character
            }
            continue;
        }
        if c.is_whitespace() || c == '\u{85}' {
            space = !out.is_empty();
            continue;
        }
        if sanitize::is_unsafe(c) || invisible(c) {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    if out.chars().count() > max {
        out = out.chars().take(max.saturating_sub(1)).collect::<String>().trim_end().to_string();
        out.push('…');
    }
    out
}

impl Ask {
    /// The `ask` of a `display` frame. None when there is none, or nothing usable is left after
    /// cleaning (no question, no option); the frame then renders as the plain display it also is.
    pub fn from_frame(v: &Value) -> Option<Ask> {
        if v["t"] != "display" {
            return None;
        }
        let a = v.get("ask")?;
        let question = clean(a["question"].as_str()?, QUESTION_MAX);
        if question.is_empty() {
            return None;
        }
        let header = a["header"].as_str().map(|h| clean(h, HEADER_MAX)).filter(|h| !h.is_empty());
        let mut opts: Vec<(Opt, bool)> = Vec::new();
        for o in a["options"].as_array()?.iter().take(64) {
            let (label, desc, rec) = match o {
                Value::String(s) => (s.as_str(), None, false),
                _ => (o["label"].as_str().unwrap_or(""), o["description"].as_str(), o["recommended"] == true),
            };
            let label = guard(&clean(label, LABEL_MAX));
            if label.is_empty() || opts.iter().any(|(x, _)| x.label.to_lowercase() == label.to_lowercase()) {
                continue;
            }
            let desc = desc.map(|d| clean(d, DESC_MAX)).unwrap_or_default();
            opts.push((Opt::new(0, label).desc(desc), rec));
            if opts.len() == OPTIONS_MAX {
                break;
            }
        }
        if opts.is_empty() {
            return None;
        }
        if let Some(r) = opts.iter().position(|(_, rec)| *rec) {
            let (o, _) = opts.remove(r);
            opts.insert(0, (o.tag("recommended"), false));
        }
        let opts = opts.into_iter().enumerate().map(|(i, (mut o, _))| {
            o.id = i;
            o
        }).collect();
        Some(Ask { question, header, multi: a["multi"] == true, opts })
    }

    pub fn picker(&self) -> Picker {
        let p = Picker::new(Who::Agent, self.header.clone().unwrap_or_default(), self.question.clone(), self.opts.clone());
        p.multi(self.multi)
    }

    /// Line mode: the numbered prompt (plain text; the line writer sanitizes it again).
    pub fn line_mode(&self) -> Vec<String> {
        let mut v = vec![match &self.header {
            Some(h) => format!("[agent asks] {h}: {}", self.question),
            None => format!("[agent asks] {}", self.question),
        }];
        for (i, o) in self.opts.iter().enumerate() {
            let tag = o.tag.map(|t| format!(" ({t})")).unwrap_or_default();
            let desc = o.desc.as_ref().map(|d| format!(" — {d}")).unwrap_or_default();
            v.push(format!("  {}. {}{tag}{desc}", i + 1, o.label));
        }
        let n = self.opts.len();
        v.push(if self.multi && n > 1 {
            format!("[agent asks] reply with numbers from 1 to {n} (several: 1,2), or type your own answer")
        } else if n > 1 {
            format!("[agent asks] reply with a number from 1 to {n}, or type your own answer")
        } else {
            "[agent asks] reply 1, or type your own answer".to_string()
        });
        v
    }

    /// Line mode: a reply of option numbers (`2`, or `1,3` for multi) becomes the labels; anything
    /// else is the owner's own words, sent as typed (None).
    pub fn answer_for_line(&self, t: &str) -> Option<String> {
        let parts: Vec<&str> = t.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()).collect();
        if parts.is_empty() || (!self.multi && parts.len() > 1) {
            return None;
        }
        let mut ids = Vec::new();
        for p in parts {
            let n: usize = p.parse().ok()?;
            let o = self.opts.get(n.checked_sub(1)?)?;
            if !ids.contains(&o.id) {
                ids.push(o.id);
            }
        }
        Some(self.picker().answer(&ids))
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

/// A hairline with a label: `── agent asks · Database ──────── waiting on you ──`.
fn rule(left: Line, right: Option<Line>, width: usize) -> Line {
    let line = Style::fg(Role::Line);
    let mut l = Line::from_span("── ", line);
    l.extend(left);
    l.push(" ", line);
    let rw = right.as_ref().map_or(0, |r| r.width() + 4);
    let fill = width.saturating_sub(l.width() + rw);
    l.push("─".repeat(fill), line);
    if let Some(r) = right {
        l.push(" ", line);
        l.extend(r);
        l.push(" ──", line);
    }
    l.truncate(width)
}

/// The picker's rows (in place of the input box) and the caret (column, row).
pub fn rows(p: &Picker, width: usize) -> (Vec<Line>, (usize, usize)) {
    let width = width.max(10);
    let mut out = Vec::new();
    // the label is the TUI's own: whose question this is
    let mut left = match p.who {
        Who::Agent => Line::from_span("agent asks", Style::dim()),
        Who::Tui => Line::from_span("setup", Style::dim()),
    };
    if !p.key.is_empty() {
        left.push(" · ", Style::mute());
        left.extend(sanitize::inline(&p.key, Style::bold()));
    }
    let right = (p.who == Who::Agent).then(|| Line::from_span("waiting on you", Style::dim()));
    out.push(rule(left, right, width));
    // the question
    let title = sanitize::inline(&p.title, Style::PLAIN);
    let mut t = Line::from_span(" ", Style::PLAIN);
    t.extend(title);
    let mut trows = wrap::wrap(&t, width, 1);
    if trows.len() > TITLE_ROWS {
        trows.truncate(TITLE_ROWS);
        let last = trows.pop().unwrap_or_default();
        let mut l = last.truncate(width.saturating_sub(1));
        if l.width() == last.width() {
            l.push("…", Style::dim());
        }
        trows.push(l);
    }
    out.extend(trows);
    // the filter
    let rows_all = p.rows();
    let mut caret = None;
    if !p.filter.is_empty() {
        let mut f = Line::from_span(" filter: ", Style::mute());
        f.extend(sanitize::inline(&p.filter, Style::PLAIN));
        caret = Some((f.width(), out.len()));
        let k = rows_all.len();
        f.push(format!("   {k} of {}", p.opts.len()), Style::mute());
        out.push(f.truncate(width));
        if k == 0 {
            out.push(Line::from_span("   no match · Backspace edits the filter · Esc clears it", Style::mute()).truncate(width));
        }
    }
    // the options, scrolled
    let end = (p.top + VISIBLE).min(rows_all.len());
    if p.top > 0 {
        out.push(Line::from_span(format!("   ↑ {} more", p.top), Style::mute()));
    }
    for (vi, row) in rows_all.iter().enumerate().take(end).skip(p.top) {
        let focused = vi == p.focus;
        let (num, label, desc, tag, checked) = match *row {
            Row::Opt(i) => {
                let o = &p.opts[i];
                (i + 1, o.label.as_str(), o.desc.as_deref(), o.tag, Some(p.checked[i]))
            }
            Row::Other => (p.opts.len() + 1, OTHER, None, None, None),
        };
        let lstyle = if focused { Style::bold() } else { Style::dim() };
        let mut l = if focused { Line::from_span(" › ", Style::bold()) } else { Line::from_span("   ", Style::PLAIN) };
        l.push(format!("{num}. "), if focused { Style::dim() } else { Style::mute() });
        if p.multi {
            match checked {
                Some(true) => l.push("[x] ", Style::bold()),
                Some(false) => l.push("[ ] ", Style::mute()),
                None => l.push("    ", Style::PLAIN),
            }
        }
        if focused && caret.is_none() {
            caret = Some((1, out.len()));
        }
        l.extend(sanitize::inline(label, lstyle));
        if *row == Row::Other {
            l.push("…", lstyle);
        }
        if let Some(t) = tag {
            l.push("  ", Style::PLAIN);
            l.push(t, Style::mute());
        }
        out.push(l.truncate(width));
        if focused && let Some(d) = desc {
            let ind = if p.multi { 10 } else { 6 };
            let mut dl = Line::from_span(" ".repeat(ind), Style::PLAIN);
            dl.extend(sanitize::inline(d, Style::dim()));
            out.push(dl.truncate(width));
        }
    }
    if end < rows_all.len() {
        out.push(Line::from_span(format!("   ↓ {} more", rows_all.len() - end), Style::mute()));
    }
    out.push(Line::from_span("─".repeat(width), Style::fg(Role::Line)));
    let caret = caret.unwrap_or((1, 1));
    (out, caret)
}

/// The keys, for the hint line under the status bar (`width` columns, after its leading space).
pub fn hint(p: &Picker, width: usize) -> String {
    let n = p.opts.len() + usize::from(p.other);
    let nums = if n <= 9 && n > 1 { format!(" · 1–{n} pick") } else { String::new() };
    let (choose, typing) = (if p.multi { "Space toggle · Enter confirm" } else { "Enter choose" }, if p.other { "or just type your answer" } else { "type to filter" });
    let long = format!("↑↓ move · {choose}{nums} · {typing} · Esc {}", p.esc);
    if wrap::str_width(&long) < width {
        return long;
    }
    let (choose, typing) = (if p.multi { "Space · Enter" } else { "Enter" }, if p.other { "type to answer" } else { "type to filter" });
    let nums = if nums.is_empty() { String::new() } else { format!(" · 1–{n}") };
    format!("↑↓ {choose}{nums} · {typing} · Esc {}", p.esc)
}

/// The hint row under the status bar while a picker is open, or while the agent's question waits.
pub fn hint_row(ed: &crate::input::Editor, width: usize) -> Option<Line> {
    let h = match (&ed.picker, &ed.waiting) {
        (Some(p), _) => hint(p, width.saturating_sub(1)),
        (None, Some(q)) => waiting_hint(q),
        (None, None) => return None,
    };
    Some(Line::from_span(format!(" {h}"), Style::mute()).truncate(width))
}

/// The hint while an agent question waits after its picker was dismissed.
pub fn waiting_hint(_question: &str) -> String {
    "waiting on you · the agent asked a question (above) · type your answer, Enter sends it".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::grid;
    use serde_json::json;
    use std::path::PathBuf;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }
    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }
    fn opts(labels: &[&str]) -> Vec<Opt> {
        labels.iter().enumerate().map(|(i, l)| Opt::new(i, *l)).collect()
    }
    fn tui(labels: &[&str]) -> Picker {
        Picker::new(Who::Tui, "model", "Choose a model", opts(labels))
    }

    /// Golden compare (`UPDATE_GOLDEN=1` rewrites), as render/golden_tests.rs does.
    fn check(name: &str, got: &str) {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.txt"));
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&p, got).unwrap();
            return;
        }
        let want = std::fs::read_to_string(&p).unwrap_or_else(|_| panic!("missing golden {} (run with UPDATE_GOLDEN=1)", p.display()));
        assert!(want == got, "golden {name} differs:\n--- want\n{want}\n--- got\n{got}");
    }

    #[test]
    fn moves_wrap_and_enter_chooses() {
        let mut p = tui(&["a", "b", "c"]);
        assert_eq!(p.key(&key(KeyCode::Up)), PickOut::None);
        assert_eq!(p.key(&key(KeyCode::Enter)), PickOut::Chosen(vec![2]), "Up from the top wraps to the bottom");
        p.key(&key(KeyCode::Down));
        assert_eq!(p.key(&key(KeyCode::Enter)), PickOut::Chosen(vec![0]), "Down from the bottom wraps to the top");
        p.key(&key(KeyCode::Tab));
        assert_eq!(p.key(&key(KeyCode::Enter)), PickOut::Chosen(vec![1]));
        p.key(&key(KeyCode::End));
        assert_eq!(p.key(&key(KeyCode::Enter)), PickOut::Chosen(vec![2]));
        p.key(&key(KeyCode::Home));
        assert_eq!(p.key(&key(KeyCode::Enter)), PickOut::Chosen(vec![0]));
        let ctrl_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        p.key(&ctrl_n);
        assert_eq!(p.key(&key(KeyCode::Enter)), PickOut::Chosen(vec![1]));
        assert_eq!(p.key(&KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)), PickOut::None, "control keys are the caller's");
    }

    #[test]
    fn numbers_pick_and_esc_cancels() {
        let mut p = tui(&["a", "b", "c"]);
        assert_eq!(p.key(&ch('2')), PickOut::Chosen(vec![1]));
        assert_eq!(p.key(&ch('9')), PickOut::None, "out of range: nothing");
        assert_eq!(p.key(&key(KeyCode::Esc)), PickOut::Cancel);
        // ids are the caller's, numbers the list's
        let mut q = Picker::new(Who::Tui, "k", "t", vec![Opt::new(7, "x"), Opt::new(3, "y")]);
        assert_eq!(q.key(&ch('2')), PickOut::Chosen(vec![3]));
        assert_eq!(q.clone().focus_id(3).key(&key(KeyCode::Enter)), PickOut::Chosen(vec![3]));
    }

    #[test]
    fn typing_filters_a_long_list() {
        let models: Vec<String> = (0..12).map(|i| format!("model-{i:02}")).collect();
        let mut p = Picker::new(Who::Tui, "model", "Choose a model", models.iter().enumerate().map(|(i, m)| Opt::new(i, m.as_str())).collect());
        assert!(!p.digits_pick(), "12 options: digits filter");
        for c in "1".chars() {
            p.key(&ch(c));
        }
        // model-01, model-10, model-11
        assert_eq!(p.rows().len(), 3);
        p.key(&ch('1'));
        assert_eq!(p.rows(), vec![Row::Opt(11)], "model-11 only");
        assert_eq!(p.key(&key(KeyCode::Enter)), PickOut::Chosen(vec![11]));
        p.key(&ch('z'));
        assert!(p.rows().is_empty());
        assert_eq!(p.key(&key(KeyCode::Enter)), PickOut::None, "no match: Enter does nothing");
        p.key(&key(KeyCode::Backspace));
        assert_eq!(p.rows().len(), 1);
        assert_eq!(p.key(&key(KeyCode::Esc)), PickOut::None, "Esc clears the filter first");
        assert_eq!(p.rows().len(), 12);
        assert_eq!(p.key(&key(KeyCode::Esc)), PickOut::Cancel);
        // filtering is case-insensitive and reads the description too
        let mut q = Picker::new(Who::Tui, "provider", "t", vec![Opt::new(0, "DeepSeek").desc("live-tested"), Opt::new(1, "Local server")]);
        q.key(&ch('L'));
        q.key(&ch('I'));
        q.key(&ch('V'));
        assert_eq!(q.rows(), vec![Row::Opt(0)]);
        // `/` on an empty filter goes to the box (an owner command)
        assert_eq!(tui(&["a"]).key(&ch('/')), PickOut::Type("/".into()));
    }

    #[test]
    fn scrolls_inside_the_widget() {
        let labels: Vec<String> = (0..20).map(|i| format!("m{i}")).collect();
        let mut p = Picker::new(Who::Tui, "model", "t", labels.iter().enumerate().map(|(i, l)| Opt::new(i, l.as_str())).collect());
        for _ in 0..9 {
            p.key(&key(KeyCode::Down));
        }
        assert_eq!((p.focus, p.top), (9, 2));
        let (r, _) = rows(&p, 60);
        let text: Vec<String> = r.iter().map(Line::text).collect();
        assert!(text.iter().any(|l| l.contains("↑ 2 more")) && text.iter().any(|l| l.contains("↓ 10 more")), "{text:#?}");
        assert_eq!(text.iter().filter(|l| l.contains(". m")).count(), VISIBLE);
        p.key(&key(KeyCode::Up));
        p.key(&key(KeyCode::PageUp));
        assert_eq!((p.focus, p.top), (0, 0));
        p.key(&key(KeyCode::Up));
        assert_eq!((p.focus, p.top), (19, 12), "wraps to the end and scrolls there");
    }

    #[test]
    fn multi_select_toggles_and_confirms() {
        let mut p = tui(&["a", "b", "c"]).multi(true);
        p.key(&ch(' '));
        p.key(&key(KeyCode::Down));
        p.key(&key(KeyCode::Down));
        p.key(&ch(' '));
        assert_eq!(p.checked_ids(), vec![0, 2]);
        p.key(&ch('1'));
        assert_eq!(p.checked_ids(), vec![2], "a number toggles in multi");
        assert_eq!(p.key(&key(KeyCode::Enter)), PickOut::Chosen(vec![2]));
        // nothing checked: Enter takes the focused one
        let mut q = tui(&["a", "b"]).multi(true);
        q.key(&key(KeyCode::Down));
        assert_eq!(q.key(&key(KeyCode::Enter)), PickOut::Chosen(vec![1]));
    }

    #[test]
    fn agent_question_typing_is_an_answer() {
        let a = Ask::from_frame(&json!({"t": "display", "kind": "info", "text": "x", "ask": {"question": "Which db?", "options": [{"label": "Postgres"}, {"label": "SQLite"}]}})).unwrap();
        let mut p = a.picker();
        assert_eq!(p.key(&ch('h')), PickOut::Type("h".into()), "a letter starts the owner's own answer");
        assert_eq!(p.key(&ch('3')), PickOut::Other, "the number after the options is 'Type your own answer'");
        assert_eq!(p.key(&ch('7')), PickOut::Type("7".into()));
        p.key(&key(KeyCode::Up));
        assert_eq!(p.key(&key(KeyCode::Enter)), PickOut::Other);
        assert_eq!(p.key(&ch('2')), PickOut::Chosen(vec![1]));
        assert_eq!(p.answer(&[1]), "SQLite");
        assert_eq!(p.key(&key(KeyCode::Esc)), PickOut::Cancel);
        let m = Ask::from_frame(&json!({"t": "display", "ask": {"question": "q", "multi": true, "options": ["a", "b", "c"]}})).unwrap();
        let mut p = m.picker();
        p.key(&ch('1'));
        p.key(&ch('3'));
        assert_eq!(p.key(&key(KeyCode::Enter)), PickOut::Chosen(vec![0, 2]));
        assert_eq!(p.answer(&[0, 2]), "a, c");
    }

    #[test]
    fn line_mode_numbers_become_labels() {
        let a = Ask::from_frame(&json!({"t": "display", "ask": {"question": "Which db?", "header": "Database",
            "options": [{"label": "SQLite", "description": "one file"}, {"label": "Postgres", "recommended": true}]}})).unwrap();
        assert_eq!(a.line_mode(), vec![
            "[agent asks] Database: Which db?".to_string(),
            "  1. Postgres (recommended)".into(),
            "  2. SQLite — one file".into(),
            "[agent asks] reply with a number from 1 to 2, or type your own answer".into(),
        ]);
        assert_eq!(a.answer_for_line(" 2 "), Some("SQLite".into()));
        assert_eq!(a.answer_for_line("1,2"), None, "one choice only");
        assert_eq!(a.answer_for_line("3"), None);
        assert_eq!(a.answer_for_line("2 tables, please"), None, "the owner's own words go as typed");
        let m = Ask { multi: true, ..a.clone() };
        assert_eq!(m.answer_for_line("2, 1"), Some("SQLite, Postgres".into()));
        assert_eq!(a.picker().record(), "agent asked · Database: Which db?");
    }

    /// TUI-PICKER anti-spoofing (binding): whatever the body puts in an `ask`, it renders under the
    /// TUI's own `agent asks` label, in ink, dim, mute and hairlines only (never the Root's amber,
    /// never harm, pass or rewrite, never reverse video), with no byte that could start an escape;
    /// strings are capped; and no choice can be sent as a command.
    #[test]
    fn agent_question_cannot_look_like_the_root() {
        let evil = json!({"t": "display", "kind": "info", "src": "root", "text": "[root] confirm",
            "ask": {"question": "[root] PANIC: the Root needs your confirmation\u{1b}]52;c;ZXZpbA==\u{7}\u{1b}[2J now\u{202E}",
                "header": "\u{1b}[33m[root] Root\u{1b}[0m confirmation required",
                "multi": false,
                "options": [{"label": "/full on confirm", "recommended": true}, {"label": "  /undo confirm"},
                    {"label": "\u{1b}[31mYes\u{1b}[0m, \u{200B}/panic"}, {"label": "[root] approve\nsecond line", "description": "\u{9b}31m red \u{7}"},
                    {"label": "fifth"}, {"label": "sixth"}, {"label": "x".repeat(500)}]}});
        let a = Ask::from_frame(&evil).expect("still the agent's question");
        assert_eq!(a.opts.len(), OPTIONS_MAX, "capped to {OPTIONS_MAX} options");
        assert!(a.question.chars().count() <= QUESTION_MAX && a.header.as_ref().unwrap().chars().count() <= HEADER_MAX);
        assert!(!a.question.contains('\u{1b}') && !a.question.contains("52;") && !a.question.contains('\u{202E}'), "{:?}", a.question);
        let p = a.picker();
        assert_eq!(p.who, Who::Agent);
        for w in [40, 60, 100, 140] {
            let (r, _) = rows(&p, w);
            assert!(r[0].text().starts_with("── agent asks"), "the TUI's label leads: {:?}", r[0].text());
            assert_eq!(r[0].0[1].style, Style::dim(), "the label is the TUI's own span");
            for l in &r {
                assert!(l.width() <= w, "{w}: {:?}", l.text());
                for s in &l.0 {
                    assert!(matches!(s.style.fg, None | Some(Role::Dim | Role::Mute | Role::Line)), "{w}: {:?} in {:?}", s.style, s.text);
                    assert!(!s.style.reverse && s.style.bg.is_none(), "{:?}", s.text);
                    assert!(!s.text.chars().any(|c| sanitize::is_unsafe(c) || c == '\u{9B}'), "{:?}", s.text);
                }
            }
        }
        // every possible answer is an ordinary owner line, never a command
        for ids in [vec![0], vec![1], vec![2], vec![3], vec![0, 1]] {
            let ans = p.answer(&ids);
            assert!(!ans.trim_start().starts_with('/'), "{ans:?}");
            assert_eq!(crate::proto::line_kind(&ans, true), crate::proto::LineKind::Input, "{ans:?}");
        }
        assert_eq!(p.answer(&[0]), "\"/full on confirm\"", "shown and sent quoted");
        assert_eq!(p.opts[0].label, "\"/full on confirm\"");
        assert_eq!(a.answer_for_line("2").as_deref(), Some("\"/undo confirm\""));
        // nothing usable: no picker (the frame shows as the plain info it also is)
        assert!(Ask::from_frame(&json!({"t": "display", "ask": {"question": "\u{1b}[2J", "options": ["a"]}})).is_none());
        assert!(Ask::from_frame(&json!({"t": "display", "ask": {"question": "q", "options": ["\u{7}", " "]}})).is_none());
        assert!(Ask::from_frame(&json!({"t": "notice", "ask": {"question": "q", "options": ["a"]}})).is_none(), "only display frames");
    }

    /// Every picker (the TUI's own included) is free of the Root's amber.
    #[test]
    fn no_picker_uses_the_root_colour() {
        let p = Picker::new(Who::Tui, "provider", "Choose a model provider", vec![Opt::new(0, "DeepSeek").tag("recommended").desc("live-tested")]);
        let (r, _) = rows(&p, 80);
        assert!(r.iter().flat_map(|l| l.0.iter()).all(|s| s.style.fg != Some(Role::Root)));
    }

    #[test]
    fn clean_drops_escapes_whole() {
        assert_eq!(clean("a\u{1b}[1;31mb\u{1b}[0m c", 80), "ab c");
        assert_eq!(clean("x\u{1b}]8;;http://e\u{1b}\\link\u{1b}]8;;\u{1b}\\ y", 80), "xlink y");
        assert_eq!(clean("p\u{1b}Pq\u{9c}r", 80), "pr");
        assert_eq!(clean("  one\n\ttwo\r\n three  ", 80), "one two three");
        assert_eq!(clean("Việt 日本 👍", 80), "Việt 日本 👍");
        assert_eq!(clean("abcdef", 4), "abc…");
        assert_eq!(clean("a\u{200B}b\u{FEFF}c\u{202E}d", 80), "abcd");
    }

    fn setup_providers() -> Picker {
        Picker::new(Who::Tui, "provider", "Choose a model provider", vec![
            Opt::new(0, "DeepSeek").desc("deepseek-flash, deepseek-v4-pro · live-tested").tag("recommended"),
            Opt::new(1, "OpenAI-compatible gateway (Chat Completions)").desc("experimental (not live-tested)"),
            Opt::new(2, "Anthropic (Messages API)").desc("claude-opus-5-5, claude-sonnet-5-5 · experimental (not live-tested)"),
            Opt::new(3, "OpenAI (Responses API)").desc("gpt-6.1-sol, gpt-6-astra · experimental (not live-tested)"),
            Opt::new(4, "Local server, no key (Ollama, LM Studio, llama.cpp; Chat Completions)").desc("experimental (not live-tested)"),
        ]).esc("leave setup")
    }

    fn agent_db() -> Picker {
        Ask::from_frame(&json!({"t": "display", "ask": {"question": "Which database should the new service use? It stores about 2 GB of orders and needs concurrent writers.",
            "header": "Database", "options": [{"label": "SQLite", "description": "one file, no server; writers wait for each other"},
            {"label": "Postgres", "description": "a server; concurrent writers, the usual choice at this size", "recommended": true}]}})).unwrap().picker()
    }

    fn snap(p: &Picker, hint_too: bool) -> String {
        let mut s = String::new();
        for w in [60, 100, 140] {
            let (mut r, caret) = rows(p, w);
            if hint_too {
                r.push(Line::from_span(format!(" {}", hint(p, w - 1)), Style::mute()).truncate(w));
            }
            s.push_str(&format!("== {w} cols, caret {caret:?}\n"));
            s.push_str(&grid(&r, w));
        }
        s
    }

    /// Golden renders at 60/100/140 columns: setup's provider picker, a filtered model list, an
    /// agent question (single, with the focus moved), and a multi-select.
    #[test]
    fn golden_pickers() {
        check("picker_setup_provider", &snap(&setup_providers(), true));
        let ids = ["deepseek-flash", "deepseek-v4-pro", "claude-opus-5-5", "claude-sonnet-5-5", "gpt-6.1-sol", "gpt-6-astra", "qwen3-coder", "llama-4-70b", "mistral-large-3", "gemma-4-27b", "phi-5"];
        let mut m = Picker::new(Who::Tui, "model", "Switch model", ids.iter().enumerate().map(|(i, x)| Opt::new(i, *x).desc("$0.30 in · $1.20 out per 1M tokens")).collect())
            .focus_id(0).esc("keep the current model");
        m.opts[0].tag = Some("current");
        let unfiltered = snap(&m, true);
        m.key(&ch('g'));
        m.key(&ch('p'));
        check("picker_model_filter", &format!("{unfiltered}{}", snap(&m, true)));
        let mut a = agent_db();
        let first = snap(&a, true);
        a.key(&key(KeyCode::Down));
        check("picker_agent", &format!("{first}{}", snap(&a, true)));
        let mut mm = Ask::from_frame(&json!({"t": "display", "ask": {"question": "Which checks should run before each commit?", "multi": true,
            "options": [{"label": "cargo test"}, {"label": "clippy -D warnings", "description": "slower on a cold cache"}, {"label": "fmt --check"}]}})).unwrap().picker();
        mm.key(&ch(' '));
        mm.key(&key(KeyCode::Down));
        check("picker_multi", &snap(&mm, true));
    }
}
