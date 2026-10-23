//! The terminal (docs/tui.md §3.4, §3.7, §5): raw mode, the keyboard flags, bracketed paste, the
//! alternate screen for the transcript view, and the restore on every exit path (normal exit,
//! signals and panics). Styles reach the terminal only through the crossterm commands built here.

use crate::render::{Color, Line, Style};
use crossterm::cursor::{Hide, MoveToColumn, Show};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use crossterm::style::{Attribute, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{self, EndSynchronizedUpdate, LeaveAlternateScreen};
use crossterm::{QueueableCommand, execute};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

static RAW: AtomicBool = AtomicBool::new(false);
static KBD: AtomicBool = AtomicBool::new(false);
static ALT: AtomicBool = AtomicBool::new(false);

/// Enters raw mode with bracketed paste and, where the terminal supports it, the kitty keyboard
/// protocol's disambiguation (Shift-Enter). Returns whether the keyboard flags were pushed.
pub fn enter() -> io::Result<bool> {
    terminal::enable_raw_mode()?;
    RAW.store(true, Ordering::SeqCst);
    let mut out = io::stdout();
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

fn color(c: Color) -> crossterm::style::Color {
    use crossterm::style::Color as C;
    match c {
        Color::Red => C::Red,
        Color::Green => C::Green,
        Color::Yellow => C::Yellow,
        Color::Cyan => C::Cyan,
        Color::Grey => C::DarkGrey,
    }
}

fn set_style<W: Write>(w: &mut W, s: Style, no_color: bool) -> io::Result<()> {
    w.queue(SetAttribute(Attribute::Reset))?;
    w.queue(ResetColor)?;
    if s.bold {
        w.queue(SetAttribute(Attribute::Bold))?;
    }
    if s.dim {
        w.queue(SetAttribute(Attribute::Dim))?;
    }
    if s.italic {
        w.queue(SetAttribute(Attribute::Italic))?;
    }
    if s.reverse {
        w.queue(SetAttribute(Attribute::Reverse))?;
    }
    if !no_color {
        if let Some(c) = s.fg {
            w.queue(SetForegroundColor(color(c)))?;
        }
        if let Some(c) = s.bg {
            w.queue(SetBackgroundColor(color(c)))?;
        }
    } else if s.bg.is_some() && !s.dim {
        w.queue(SetAttribute(Attribute::Dim))?;
    }
    Ok(())
}

/// Writes one row's spans. The text was sanitized when the row was built; it is printed through
/// crossterm's `Print`, and every escape sequence comes from the commands above.
pub fn write_line<W: Write>(w: &mut W, l: &Line, no_color: bool) -> io::Result<()> {
    let mut styled = false;
    for s in &l.0 {
        if s.style != Style::PLAIN {
            set_style(w, s.style, no_color)?;
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
