//! The key map (docs/tui.md §3.7) and the transcript view's keys (§3.6). Pure: a key and a little
//! context in, an action out.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Insert(char),
    Newline,
    /// Enter: send the box (or turn a trailing `\` into a newline).
    Submit,
    Interrupt,
    /// Esc while busy: interrupt without arming the quit (with a steer pending: interrupt and steer).
    EscInterrupt,
    /// STEER: Tab while busy with text in the box: queue it as `/later` (the next task).
    QueueLater,
    ClosePopup,
    /// Ctrl-D on an empty box.
    Detach,
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    WordLeft,
    WordRight,
    DeleteWord,
    KillToStart,
    KillToEnd,
    Up,
    Down,
    Complete,
    Transcript,
    ToggleThinking,
    Redraw,
    Help,
    None,
}

pub struct Ctx {
    pub empty: bool,
    pub busy: bool,
    pub popup: bool,
    pub overlay: bool,
}

pub fn map(k: &KeyEvent, cx: &Ctx) -> Action {
    if k.kind == KeyEventKind::Release {
        return Action::None;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    match k.code {
        KeyCode::Char('c') if ctrl => Action::Interrupt,
        KeyCode::Char('d') if ctrl => {
            if cx.empty {
                Action::Detach
            } else {
                Action::Delete
            }
        }
        KeyCode::Char('j') if ctrl => Action::Newline,
        KeyCode::Char('a') if ctrl => Action::Home,
        KeyCode::Char('e') if ctrl => Action::End,
        KeyCode::Char('w') if ctrl => Action::DeleteWord,
        KeyCode::Char('u') if ctrl => Action::KillToStart,
        KeyCode::Char('k') if ctrl => Action::KillToEnd,
        KeyCode::Char('o') if ctrl => Action::Transcript,
        KeyCode::Char('t') if ctrl => Action::ToggleThinking,
        KeyCode::Char('l') if ctrl => Action::Redraw,
        KeyCode::Char('b') if alt => Action::WordLeft,
        KeyCode::Char('f') if alt => Action::WordRight,
        KeyCode::Char(_) if ctrl => Action::None, // Ctrl-Z and the rest: ignored
        KeyCode::Char('?') if cx.empty && !cx.overlay => Action::Help,
        KeyCode::Char(c) => Action::Insert(c),
        KeyCode::Enter if shift || alt => Action::Newline,
        KeyCode::Enter => Action::Submit,
        KeyCode::Esc => {
            if cx.popup || cx.overlay {
                Action::ClosePopup
            } else if cx.busy {
                Action::EscInterrupt
            } else {
                Action::None
            }
        }
        KeyCode::Tab => {
            if cx.popup {
                Action::Complete
            } else if cx.busy && !cx.empty {
                Action::QueueLater
            } else {
                Action::Insert('\t')
            }
        }
        KeyCode::Backspace if alt || ctrl => Action::DeleteWord,
        KeyCode::Backspace => Action::Backspace,
        KeyCode::Delete => Action::Delete,
        KeyCode::Left if alt || ctrl => Action::WordLeft,
        KeyCode::Right if alt || ctrl => Action::WordRight,
        KeyCode::Left => Action::Left,
        KeyCode::Right => Action::Right,
        KeyCode::Home => Action::Home,
        KeyCode::End => Action::End,
        KeyCode::Up => Action::Up,
        KeyCode::Down => Action::Down,
        _ => Action::None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Toggle,
    ExpandAll,
    CollapseAll,
    Close,
    None,
}

/// Keys inside the transcript view.
pub fn view(k: &KeyEvent) -> View {
    if k.kind == KeyEventKind::Release {
        return View::None;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Char('o') if ctrl => View::Close,
        KeyCode::Char('c') if ctrl => View::Close,
        KeyCode::Char('q') | KeyCode::Esc => View::Close,
        KeyCode::Char('j') | KeyCode::Down => View::Down,
        KeyCode::Char('k') | KeyCode::Up => View::Up,
        KeyCode::PageUp => View::PageUp,
        KeyCode::PageDown => View::PageDown,
        KeyCode::Char('g') | KeyCode::Home => View::Top,
        KeyCode::Char('G') | KeyCode::End => View::Bottom,
        KeyCode::Enter | KeyCode::Char(' ') => View::Toggle,
        KeyCode::Char('e') => View::ExpandAll,
        KeyCode::Char('c') => View::CollapseAll,
        _ => View::None,
    }
}

/// The key overlay (`?`), as in Claude Code.
pub const HELP: &[(&str, &str)] = &[
    ("Enter", "send"),
    ("Shift-Enter · Alt-Enter · Ctrl-J · \\ Enter", "newline"),
    ("Ctrl-C", "interrupt; twice within 2 s: quit init-agent (the agent keeps running)"),
    ("Esc", "pause while busy: your next message continues the task (a pending steer goes in now); close a popup"),
    ("Tab (while busy)", "queue the box as the next task (/later)"),
    ("Up (empty box, pending line)", "take back the newest line not yet read"),
    ("Ctrl-D", "quit on an empty box (the agent keeps running; init-agent comes back)"),
    ("Up / Down", "lines; history on the first line"),
    ("Ctrl-A/E · Alt-←/→ · Ctrl-W/U/K", "move and delete"),
    ("/", "owner commands (Tab completes)"),
    ("Ctrl-O · /details", "the transcript, last tool activity expanded · print it (a long call: its first lines)"),
    ("Ctrl-T", "show or hide thinking"),
    ("Ctrl-L", "redraw"),
    ("/stop · /root stop · /panic", "stop the run · stop the agent · emergency stop + demotion (asks first)"),
    ("self-change ■ □ · ✕", "gate step passed · running · to come or not run · failed; LKG: last known good"),
    ("/new · /clear", "start a new session"),
    ("/model · /key · /setup", "switch model · set a key (hidden) · provider setup"),
    ("in a picker: ↑↓ Enter · 1–9 · Esc", "choose; type to filter, or to answer the agent in your own words"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The overlay fits a 24-row terminal: KEYS, the list, the input box, the status bar, the hint.
    #[test]
    fn help_fits_24_rows() {
        let rows = HELP.len() + 6; // KEYS, the list, 3 input-box rows, the status bar, the hint
        assert!(rows <= 24, "{rows} rows");
    }

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }
    const CX: Ctx = Ctx { empty: false, busy: false, popup: false, overlay: false };

    #[test]
    fn newline_keys() {
        assert_eq!(map(&k(KeyCode::Enter, KeyModifiers::SHIFT), &CX), Action::Newline);
        assert_eq!(map(&k(KeyCode::Enter, KeyModifiers::ALT), &CX), Action::Newline);
        assert_eq!(map(&k(KeyCode::Char('j'), KeyModifiers::CONTROL), &CX), Action::Newline);
        assert_eq!(map(&k(KeyCode::Enter, KeyModifiers::NONE), &CX), Action::Submit);
    }

    #[test]
    fn control_keys() {
        assert_eq!(map(&k(KeyCode::Char('c'), KeyModifiers::CONTROL), &CX), Action::Interrupt);
        let empty = Ctx { empty: true, ..CX };
        assert_eq!(map(&k(KeyCode::Char('d'), KeyModifiers::CONTROL), &empty), Action::Detach);
        assert_eq!(map(&k(KeyCode::Char('?'), KeyModifiers::NONE), &empty), Action::Help);
        assert_eq!(map(&k(KeyCode::Char('?'), KeyModifiers::NONE), &CX), Action::Insert('?'));
        assert_eq!(map(&k(KeyCode::Esc, KeyModifiers::NONE), &Ctx { busy: true, ..CX }), Action::EscInterrupt);
        assert_eq!(map(&k(KeyCode::Esc, KeyModifiers::NONE), &CX), Action::None);
        assert_eq!(map(&k(KeyCode::Char('z'), KeyModifiers::CONTROL), &CX), Action::None);
    }

    #[test]
    fn tab_queues_a_follow_up_only_while_busy_with_text() {
        let tab = k(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(map(&tab, &Ctx { busy: true, ..CX }), Action::QueueLater);
        assert_eq!(map(&tab, &CX), Action::Insert('\t'));
        assert_eq!(map(&tab, &Ctx { busy: true, empty: true, ..CX }), Action::Insert('\t'));
        assert_eq!(map(&tab, &Ctx { busy: true, popup: true, ..CX }), Action::Complete);
    }
}
