//! The terminal (docs/tui.md §3.4, §3.7, §5): raw mode, the keyboard flags, bracketed paste, the
//! alternate screen for the transcript view, and the restore on every exit path (normal exit,
//! signals and panics). Styles reach the terminal only through the crossterm commands built here.

use crate::render::palette::{self, Bg, Paint, Palette, Tier};
use crate::render::{Line, Style};
use crossterm::cursor::{Hide, MoveToColumn, Show};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use crossterm::style::{Attribute, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{self, EndSynchronizedUpdate, LeaveAlternateScreen};
use crossterm::{QueueableCommand, execute};
use std::io::{self, Write};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static RAW: AtomicBool = AtomicBool::new(false);
static KBD: AtomicBool = AtomicBool::new(false);
static ALT: AtomicBool = AtomicBool::new(false);
/// The palette, chosen once at start (§3.9a); `Palette::DEFAULT` until then.
static PALETTE: OnceLock<Palette> = OnceLock::new();

/// Enters raw mode with bracketed paste and, where the terminal supports it, the kitty keyboard
/// protocol's disambiguation (Shift-Enter). Returns whether the keyboard flags were pushed.
pub fn enter() -> io::Result<bool> {
    terminal::enable_raw_mode()?;
    RAW.store(true, Ordering::SeqCst);
    let mut out = io::stdout();
    // the background first: its reply must be read before crossterm's reader sees the input; not
    // asked when `--theme` already chose
    if PALETTE.get().is_none() {
        set_palette(detect_palette(true));
    }
    execute!(out, EnableBracketedPaste)?;
    let kbd = terminal::supports_keyboard_enhancement().unwrap_or(false);
    if kbd {
        execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
        KBD.store(true, Ordering::SeqCst);
    }
    Ok(kbd)
}

pub fn set_alt(on: bool) {
    ALT.store(on, Ordering::SeqCst);
}

/// Restores the terminal: keyboard flags popped, paste off, alternate screen left, cursor shown, raw
/// mode off. Idempotent; safe from the panic hook.
pub fn restore() {
    let mut out = io::stdout();
    let _ = out.queue(EndSynchronizedUpdate);
    if KBD.swap(false, Ordering::SeqCst) {
        let _ = out.queue(PopKeyboardEnhancementFlags);
    }
    if ALT.swap(false, Ordering::SeqCst) {
        let _ = out.queue(LeaveAlternateScreen);
    }
    if RAW.load(Ordering::SeqCst) {
        let _ = out.queue(DisableBracketedPaste);
        let _ = out.queue(SetAttribute(Attribute::Reset));
        let _ = out.queue(ResetColor);
        let _ = out.queue(Show);
        let _ = out.queue(MoveToColumn(0));
    }
    let _ = out.flush();
    if RAW.swap(false, Ordering::SeqCst) {
        let _ = terminal::disable_raw_mode();
    }
}

/// The panic hook: restore the terminal, print the message, exit 101 (§4.2), whichever thread panicked.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        restore();
        eprintln!("\ninit-tui: internal error: {info}");
        std::process::exit(101);
    }));
}

/// Sets the palette for every later write (once; later calls are ignored).
pub fn set_palette(p: Palette) {
    let _ = PALETTE.set(p);
}

/// The palette in use.
pub fn palette() -> Palette {
    PALETTE.get().copied().unwrap_or(Palette::DEFAULT)
}

/// The palette from the environment (`NO_COLOR`, `COLORTERM`, `TERM`, `COLORFGBG`, §4.1) and, when
/// `query` and the background is still unknown, the terminal's answer to an OSC 11 query. Dark
/// when nothing answers.
pub fn detect_palette(query: bool) -> Palette {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let no_color = env("NO_COLOR").is_some();
    let (tier, bg) = Palette::from_env(no_color, env("TERM").as_deref(), env("COLORTERM").as_deref(), env("COLORFGBG").as_deref());
    let bg = match bg {
        Some(b) => b,
        None if query && tier != Tier::NoColor => query_bg(Duration::from_millis(1000)).unwrap_or(Bg::Dark),
        None => Bg::Dark,
    };
    Palette { tier, bg }
}

/// Asks the terminal for its background (OSC 11), followed by a primary device attributes query
/// (DA1) that every terminal answers: the DA1 reply ends the wait, so a terminal without OSC 11
/// costs one round trip, not the timeout. Raw mode must be on and nothing else may read stdin yet.
/// Hand-written (≈ 40 lines) instead of a crate, to keep the closure at 25 (§3.14); `rustix` is
/// already in it.
fn query_bg(timeout: Duration) -> Option<Bg> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use std::io::IsTerminal;
    let stdin = io::stdin();
    if !stdin.is_terminal() || !io::stdout().is_terminal() {
        return None;
    }
    {
        let mut out = io::stdout();
        out.write_all(b"\x1b]11;?\x1b\\\x1b[c").ok()?;
        out.flush().ok()?;
    }
    let end = Instant::now() + timeout;
    let mut buf: Vec<u8> = Vec::with_capacity(128);
    let mut chunk = [0u8; 256];
    while !palette::has_da1_reply(&buf) && buf.len() < 4096 {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let ts = Timespec { tv_sec: left.as_secs() as _, tv_nsec: left.subsec_nanos() as _ };
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        match poll(&mut fds, Some(&ts)) {
            Ok(0) => break,
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => break,
        }
        match rustix::io::read(&stdin, &mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN) => continue,
            Err(_) => break,
        }
    }
    palette::bg_from_osc11(&buf)
}

fn crossterm_color(p: Paint) -> Option<crossterm::style::Color> {
    use crossterm::style::Color as C;
    match p {
        Paint::Rgb(r, g, b) => Some(C::Rgb { r, g, b }),
        Paint::Index(i) => Some(C::AnsiValue(i)),
        Paint::Default | Paint::Faint | Paint::Bold => None,
    }
}

/// The one place a style becomes escape codes: attributes, then the role's paint from the palette.
fn set_style<W: Write>(w: &mut W, s: Style, pal: Palette) -> io::Result<()> {
    w.queue(SetAttribute(Attribute::Reset))?;
    w.queue(ResetColor)?;
    let fg = s.fg.map_or(Paint::Default, |r| pal.paint(r));
    if s.bold || fg == Paint::Bold {
        w.queue(SetAttribute(Attribute::Bold))?;
    }
    if fg == Paint::Faint {
        w.queue(SetAttribute(Attribute::Dim))?;
    }
    if s.italic {
        w.queue(SetAttribute(Attribute::Italic))?;
    }
    if s.reverse {
        w.queue(SetAttribute(Attribute::Reverse))?;
    }
    if let Some(c) = crossterm_color(fg) {
        w.queue(SetForegroundColor(c))?;
    }
    if let Some(r) = s.bg {
        match crossterm_color(pal.paint(r)) {
            Some(c) => {
                w.queue(SetBackgroundColor(c))?;
            }
            // no colour for a background (NO_COLOR, or a grey that is only an attribute): faint text
            None if fg != Paint::Faint => {
                w.queue(SetAttribute(Attribute::Dim))?;
            }
            None => {}
        }
    }
    Ok(())
}

/// Writes one row's spans with the palette in use (`NO_COLOR` when `no_color`). The text was
/// sanitized when the row was built; it is printed through crossterm's `Print`, and every escape
/// sequence comes from the commands above.
pub fn write_line<W: Write>(w: &mut W, l: &Line, no_color: bool) -> io::Result<()> {
    let mut pal = palette();
    if no_color {
        pal.tier = Tier::NoColor;
    }
    write_line_with(w, l, pal)
}

/// Writes one row's spans with the palette `pal`.
pub fn write_line_with<W: Write>(w: &mut W, l: &Line, pal: Palette) -> io::Result<()> {
    let mut styled = false;
    for s in &l.0 {
        if s.style != Style::PLAIN {
            set_style(w, s.style, pal)?;
            styled = true;
        } else if styled {
            w.queue(SetAttribute(Attribute::Reset))?;
            w.queue(ResetColor)?;
            styled = false;
        }
        debug_assert!(!s.text.contains('\u{1b}'), "an unsanitized ESC reached the writer");
        w.queue(Print(&s.text))?;
    }
    if styled {
        w.queue(SetAttribute(Attribute::Reset))?;
        w.queue(ResetColor)?;
    }
    Ok(())
}

pub fn hide_cursor<W: Write>(w: &mut W) -> io::Result<()> {
    w.queue(Hide)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Role;

    fn sgr(l: &Line, tier: Tier, bg: Bg) -> String {
        let mut v = Vec::new();
        write_line_with(&mut v, l, Palette { tier, bg }).unwrap();
        String::from_utf8(v).unwrap().replace('\u{1b}', "^")
    }

    #[test]
    fn roles_become_the_tier_paint() {
        let root = Line::from_span("root ok", Style::fg(Role::Root));
        assert!(sgr(&root, Tier::TrueColor, Bg::Dark).contains("38;2;230;180;80m"), "{}", sgr(&root, Tier::TrueColor, Bg::Dark));
        assert!(sgr(&root, Tier::TrueColor, Bg::Light).contains("38;2;161;106;10m"));
        assert!(sgr(&root, Tier::Ansi256, Bg::Dark).contains("38;5;179m"));
        assert!(sgr(&root, Tier::Ansi256, Bg::Light).contains("38;5;130m"));
        assert!(sgr(&root, Tier::Ansi16, Bg::Dark).contains("38;5;11m"));
        let nc = sgr(&root, Tier::NoColor, Bg::Dark);
        assert!(!nc.contains("38;"), "{nc}");
        // the greys are faint where no exact colour exists; warn is bold, never a colour
        assert!(sgr(&Line::from_span("x", Style::dim()), Tier::Ansi16, Bg::Dark).contains("^[2m"));
        assert!(sgr(&Line::from_span("x", Style::dim()), Tier::Ansi256, Bg::Dark).contains("38;5;246m"));
        let warn = sgr(&Line::from_span("x", Style::fg(Role::Warn)), Tier::Ansi256, Bg::Dark);
        assert!(warn.contains("^[1m") && !warn.contains("38;"), "{warn}");
        // a background role with no colour (NO_COLOR) falls back to faint text
        let code = Style { bg: Some(Role::Line), ..Style::PLAIN };
        assert!(sgr(&Line::from_span("x", code), Tier::NoColor, Bg::Dark).contains("^[2m"));
        assert!(sgr(&Line::from_span("x", code), Tier::Ansi256, Bg::Dark).contains("48;5;238m"));
    }

    #[test]
    fn plain_text_writes_no_escape() {
        let s = sgr(&Line::from_span("hello", Style::PLAIN), Tier::TrueColor, Bg::Dark);
        assert_eq!(s, "hello");
    }
}
