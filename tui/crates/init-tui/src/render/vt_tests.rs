//! TUI-BRAND: renderer property tests against a terminal emulator (`vt100`, a dev-dependency only).
//! Random transcripts at random sizes are rendered by the real renderer and written by the real
//! writer (`term::write_line_with`), with the same cursor moves the TTY loop uses (`app.rs` render:
//! commits, then the live region redrawn in place); the bytes go to vt100. Checked after every frame
//! and at the end:
//! - the emulator meets no escape sequence it does not know (except synchronized-update brackets);
//! - the bottom of the screen is exactly the live region the renderer meant (nothing wrapped, nothing
//!   left behind by a shrinking region);
//! - every cell's colour is its role's paint in the palette, so amber appears only where the
//!   renderer said `Role::Root`;
//! - the scrollback plus the screen hold every committed row once, in order (native scrollback is
//!   never corrupted or duplicated).

use super::live::{self, Ui};
use super::palette::{Bg, Paint, Palette, Tier};
use super::{Line, Role};
use crate::input::Editor;
use crate::model::{Conn, Info, Kind, OwnerSt, Transcript, Tray};
use crate::proto::{Display, DisplayKind};
use crossterm::QueueableCommand;
use crossterm::cursor::{Hide, MoveToColumn, MoveToPreviousLine, Show};
use crossterm::terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate};
use serde_json::json;
use std::io::Write;
use std::num::NonZeroU16;
use std::time::{Duration, Instant};

const NOW_MS: i64 = 1_792_530_245_000;
const PAL: Palette = Palette { tier: Tier::Ansi256, bg: Bg::Dark };
const PALETTES: [Palette; 7] = [
    PAL,
    Palette { tier: Tier::Ansi256, bg: Bg::Light },
    Palette { tier: Tier::TrueColor, bg: Bg::Dark },
    Palette { tier: Tier::TrueColor, bg: Bg::Light },
    Palette { tier: Tier::Ansi16, bg: Bg::Dark },
    Palette { tier: Tier::Ansi16, bg: Bg::Light },
    Palette { tier: Tier::NoColor, bg: Bg::Dark },
];

/// Records what the emulator did not understand.
#[derive(Default)]
struct Strict {
    unknown: Vec<String>,
}

impl vt100::Callbacks for Strict {
    fn unhandled_char(&mut self, _: &mut vt100::Screen, c: char) {
        self.unknown.push(format!("char {c:?}"));
    }
    fn unhandled_control(&mut self, _: &mut vt100::Screen, b: u8) {
        self.unknown.push(format!("control {b:#x}"));
    }
    fn unhandled_escape(&mut self, _: &mut vt100::Screen, i1: Option<u8>, i2: Option<u8>, b: u8) {
        self.unknown.push(format!("escape {i1:?} {i2:?} {b:#x}"));
    }
    fn unhandled_csi(&mut self, _: &mut vt100::Screen, i1: Option<u8>, _i2: Option<u8>, params: &[&[u16]], c: char) {
        // synchronized update (DEC mode 2026): terminals without it ignore it, as vt100 does
        if i1 == Some(b'?') && params == [&[2026u16][..]] && matches!(c, 'h' | 'l') {
            return;
        }
        self.unknown.push(format!("csi {i1:?} {params:?} {c}"));
    }
    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        self.unknown.push(format!("osc {params:?}"));
    }
}

/// The TTY loop's redraw (`app.rs` `App::render`, main screen), with the same commands.
struct Writer {
    pal: Palette,
    prev: Vec<Line>,
    prev_cursor_row: usize,
    drawn: bool,
}

impl Writer {
    fn frame(&mut self, out: &mut Vec<u8>, commits: &[Line], region: &super::Region) {
        out.queue(BeginSynchronizedUpdate).unwrap();
        out.queue(Hide).unwrap();
        if self.prev_cursor_row > 0 {
            out.queue(MoveToPreviousLine(self.prev_cursor_row as u16)).unwrap();
        } else {
            out.queue(MoveToColumn(0)).unwrap();
        }
        let full = !self.drawn || !commits.is_empty();
        self.drawn = true;
        for l in commits {
            crate::term::write_line_with(out, l, self.pal).unwrap();
            out.queue(Clear(ClearType::UntilNewLine)).unwrap();
            out.write_all(b"\r\n").unwrap();
        }
        let n = region.rows.len();
        for (i, l) in region.rows.iter().enumerate() {
            if full || i + 1 == n || self.prev.get(i) != Some(l) {
                crate::term::write_line_with(out, l, self.pal).unwrap();
                out.queue(Clear(ClearType::UntilNewLine)).unwrap();
            }
            if i + 1 < n {
                out.write_all(b"\r\n").unwrap();
            }
        }
        out.queue(Clear(ClearType::FromCursorDown)).unwrap();
        let (cx, cy) = region.cursor.unwrap_or((0, n.saturating_sub(1)));
        let up = n.saturating_sub(1).saturating_sub(cy);
        if up > 0 {
            out.queue(MoveToPreviousLine(up as u16)).unwrap();
        }
        out.queue(MoveToColumn(cx as u16)).unwrap();
        out.queue(Show).unwrap();
        out.queue(EndSynchronizedUpdate).unwrap();
        self.prev = region.rows.clone();
        self.prev_cursor_row = cy.min(n.saturating_sub(1));
    }
}

fn paint_of(p: Paint) -> vt100::Color {
    match p {
        Paint::Index(i) => vt100::Color::Idx(i),
        Paint::Rgb(r, g, b) => vt100::Color::Rgb(r, g, b),
        Paint::Default | Paint::Faint | Paint::Bold => vt100::Color::Default,
    }
}

/// Every row of history: the scrollback, oldest first, then the screen.
fn history(p: &mut vt100::Parser<Strict>, h: usize) -> Vec<String> {
    let total = p.screen().scrollback_len();
    let mut out = Vec::new();
    for off in (1..=total).rev() {
        p.screen_mut().set_scrollback(off);
        out.push(p.screen().rows(0, 400).next().unwrap_or_default());
    }
    p.screen_mut().set_scrollback(0);
    out.extend(p.screen().rows(0, 400).take(h));
    out.iter().map(|r| r.trim_end().to_string()).collect()
}

/// The screen row where the live region starts: the cursor sits at the region's caret.
fn region_top(p: &vt100::Parser<Strict>, caret_row: usize) -> usize {
    (p.screen().cursor_position().0 as usize).checked_sub(caret_row).expect("the caret is inside the region")
}

/// Checks the live region on screen against the renderer's rows, cell by cell (text and colour).
fn check_region(p: &vt100::Parser<Strict>, pal: Palette, region: &super::Region, w: usize, h: usize, ctx: &str) {
    let n = region.rows.len();
    assert!(n <= h, "{ctx}: a live region of {n} rows on a {h}-row screen");
    let screen = p.screen();
    let top = region_top(p, region.cursor.map_or(n.saturating_sub(1), |c| c.1));
    assert!(top + n <= h, "{ctx}: the live region runs off the screen");
    for (i, l) in region.rows.iter().enumerate() {
        let row = (top + i) as u16;
        let got: String = screen.rows(0, w as u16).nth(top + i).unwrap_or_default();
        if got.trim_end() != l.text().trim_end() {
            let all: Vec<String> = screen.rows(0, w as u16).collect();
            let want: Vec<String> = region.rows.iter().map(Line::text).collect();
            panic!("{ctx}: row {row} differs (top {top}, cursor {:?}, caret {:?})\nscreen:\n{}\nregion:\n{}", screen.cursor_position(), region.cursor, all.join("|\n"), want.join("|\n"));
        }
        let mut col = 0u16;
        for s in &l.0 {
            let want = s.style.fg.map_or(vt100::Color::Default, |r| paint_of(pal.paint(r)));
            let root = paint_of(pal.paint(Role::Root));
            for g in unicode_segmentation::UnicodeSegmentation::graphemes(s.text.as_str(), true) {
                let gw = super::wrap::g_width(g) as u16;
                if gw == 0 {
                    continue;
                }
                if g != " " {
                    let c = screen.cell(row, col).expect("cell");
                    assert_eq!(c.fgcolor(), want, "{ctx}: colour of {g:?} at {row},{col} in {:?}", l.text());
                    if root != vt100::Color::Default && c.fgcolor() == root {
                        assert_eq!(s.style.fg, Some(Role::Root), "{ctx}: amber off the Root");
                    }
                    if pal.tier == Tier::NoColor {
                        assert_eq!(c.fgcolor(), vt100::Color::Default, "{ctx}: a colour under NO_COLOR");
                    }
                }
                col += gw;
            }
        }
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn word(r: &mut Rng) -> String {
    const WORDS: [&str; 16] = ["root", "gate", "`code`", "**bold**", "the", "agent", "日本語", "👍", "x", "a-much-longer-token-than-most", "↳", "◇", "ok", "cache", "–", "\t"];
    WORDS[r.below(16) as usize].to_string()
}

fn text(r: &mut Rng, words: u64) -> String {
    let mut s = String::new();
    for i in 0..r.below(words) + 1 {
        if i > 0 {
            s.push(if r.below(9) == 0 { '\n' } else { ' ' });
        }
        s.push_str(&word(r));
    }
    s
}

/// One random session at `w`×`h`: frames, notices, owner lines, an editor draft; a render after
/// each event. Returns the number of frames rendered.
fn session(seed: u64, w: usize, h: usize, events: usize) -> usize {
    let mut r = Rng(seed | 1);
    let base = Instant::now();
    let mut t = Transcript::default();
    let tray = Tray::default();
    let mut ed = Editor::default();
    let mut info = Info::default();
    info.conn = Conn::Live;
    info.on_status(json!({"slot": "running", "mode": "normal", "level": 0, "cap": 1, "tuples": {"current": "t1", "lkg": "t1"}, "queued": 0}), base);
    info.on_egress(json!({"money": {"day_spent_musd": 1_230_000, "day_reserved_musd": 0, "day_limit_musd": 5_000_000, "total_spent_musd": 1_230_000}, "models": [{"model": "deepseek-flash", "routable": true}]}));
    info.on_access(json!({"full": r.below(2) == 0, "workspace": "/Users/o/proj"}));
    let mut parser = vt100::Parser::new_with_callbacks(NonZeroU16::new(h as u16).unwrap(), NonZeroU16::new(w as u16).unwrap(), 100_000, Strict::default());
    let pal = PALETTES[r.below(PALETTES.len() as u64) as usize];
    let mut wr = Writer { pal, prev: Vec::new(), prev_cursor_row: 0, drawn: false };
    let mut committed: Vec<String> = Vec::new();
    let mut frames = 0;
    let mut call = 0u64;
    for e in 0..events {
        let now = base + Duration::from_millis(e as u64 * 700);
        let d = |kind, text: String, id: Option<String>, ok| Display { kind, text, call_id: id, name: Some("bash".into()), ok, boot_seq: Some(1), detail: None, parent: None };
        match r.below(14) {
            0 => {
                t.push(Kind::Owner { id: e as u64, text: text(&mut r, 6), st: OwnerSt::Delivered }, NOW_MS);
                info.on_state(json!({"busy": true, "mode": "normal", "sha12": "3f9c0a1b2c4d"}), now);
            }
            1..=4 => t.display(&d(DisplayKind::Text, format!("{} ", text(&mut r, 12)), None, None), now, NOW_MS),
            5 => t.display(&d(DisplayKind::Thinking, text(&mut r, 5), None, None), now, NOW_MS),
            6 => {
                call += 1;
                t.display(&d(DisplayKind::ToolStart, text(&mut r, 4), Some(format!("c{call}")), None), now, NOW_MS);
            }
            7 => t.display(&d(DisplayKind::ToolEnd, "exit 0, 3 lines".into(), Some(format!("c{}", call.saturating_sub(r.below(2)))), Some(r.below(3) > 0)), now, NOW_MS),
            8 => t.display(&d(DisplayKind::TurnEnd, "tokens: in 1200 (cached 1100), out 40".into(), None, None), now, NOW_MS),
            9 => {
                let src = ["root", "kernel", "tui"][r.below(3) as usize];
                let level = ["info", "warn", "alert"][r.below(3) as usize];
                let body = match r.below(4) {
                    0 => "[root] demoted L1→L0 (ceiling_hit)".to_string(),
                    1 => "[kernel] promoted".to_string(),
                    2 => "[root] {\"cap\":0,\"entered_at\":\"2026-10-27T10:49:45.957Z\",\"entered_seq\":0,\"level\":0,\"path\":\"gated\",\"seq\":13}".to_string(),
                    _ => format!("[root] {}", text(&mut r, 8)),
                };
                t.notice(level, &body, Some(src), None, NOW_MS);
            }
            10 => {
                info.on_state(json!({"busy": false}), now);
                t.end_activity(now);
            }
            11 => ed.insert(&text(&mut r, 5)),
            12 => {
                ed = Editor::default();
            }
            _ => t.display(&d(DisplayKind::Error, text(&mut r, 6), None, None), now, NOW_MS),
        }
        let spin = e % 10;
        let ui = Ui { t: &t, tray: &tray, ed: &ed, info: &info, verbose: false, overlay: false, hint: None, now, now_ms: NOW_MS, no_color: false, spin };
        let max = live::max_live(&ui, w, h);
        let commits = live::commit(&mut t, w, max, false, now, spin);
        committed.extend(commits.iter().map(|l| l.text().trim_end().to_string()));
        let ui = Ui { t: &t, tray: &tray, ed: &ed, info: &info, verbose: false, overlay: false, hint: None, now, now_ms: NOW_MS, no_color: false, spin };
        let region = live::region(&ui, w, h);
        let mut bytes = Vec::new();
        wr.frame(&mut bytes, &commits, &region);
        parser.process(&bytes);
        let ctx = format!("seed {seed} {w}x{h} {pal:?} event {e}");
        assert!(parser.callbacks().unknown.is_empty(), "{ctx}: unknown sequences {:?}", parser.callbacks().unknown);
        check_region(&parser, pal, &region, w, h, &ctx);
        frames += 1;
    }
    // the history above the live region is exactly the committed rows: each once, in order
    let top = region_top(&parser, wr.prev_cursor_row);
    let back = parser.screen().scrollback_len();
    let hist = history(&mut parser, h);
    let above = &hist[..back + top];
    assert_eq!(above.len(), committed.len(), "seed {seed} {w}x{h}: {} history rows for {} committed", above.len(), committed.len());
    for (i, (a, b)) in above.iter().zip(committed.iter()).enumerate() {
        assert_eq!(a, b, "seed {seed} {w}x{h}: committed row {i} differs in the scrollback");
    }
    frames
}

#[test]
fn renderer_output_through_a_terminal_emulator() {
    let mut r = Rng(0x5DEECE66D);
    let mut frames = 0;
    for case in 0..24 {
        let w = 20 + r.below(121) as usize;
        let h = 6 + r.below(35) as usize;
        frames += session(0xC0FFEE + case, w, h, 120);
    }
    assert!(frames >= 24 * 120);
}

/// The brand rule at the byte level: on the 256-colour tier, amber (179) is written only for spans
/// in the Root role, and the old theme-defined slots (bright yellow 11, cyan 14, green 10) never.
#[test]
fn only_the_root_is_amber_in_written_bytes() {
    let mut l = Line::new();
    l.push("✓ ", super::Style::fg(Role::Pass));
    l.push("[root]", super::Style::fg(Role::Root));
    l.push(" demoted ", super::Style::PLAIN);
    l.push("◇", super::Style::fg(Role::Rewrite));
    l.push(" root ok", super::Style::fg(Role::Root));
    let mut bytes = Vec::new();
    crate::term::write_line_with(&mut bytes, &l, PAL).unwrap();
    let s = String::from_utf8(bytes).unwrap();
    assert_eq!(s.matches("38;5;179m").count(), 2, "{s}");
    for old in ["38;5;11m", "38;5;14m", "38;5;10m"] {
        assert!(!s.contains(old), "{old} in {s}");
    }
}
