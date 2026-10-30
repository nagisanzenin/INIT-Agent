//! The input box's editor (docs/tui.md §3.7): a multi-line buffer edited on grapheme clusters, with
//! display widths for the cursor, this process's history, and the owner-command completion.

use crate::proto::COMMANDS;
use crate::render::wrap::g_width;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Default)]
pub struct Editor {
    buf: String,
    /// Byte offset, always on a grapheme boundary.
    cur: usize,
    history: Vec<String>,
    /// Position in `history` while browsing (None: editing a fresh draft).
    hpos: Option<usize>,
    draft: String,
    /// The completion popup's selection; the popup itself is shown while `popup_open()`.
    pub sel: usize,
    pub popup_dismissed: bool,
    /// TUI-PICKER (§3.7.2): the open picker. It takes the box's place, and keys go to it.
    pub picker: Option<crate::render::picker::Picker>,
    /// TUI-PICKER: an agent question that arrived while another picker (setup) was open.
    pub picker_next: Option<crate::render::picker::Picker>,
    /// TUI-PICKER: the agent's question after its picker was dismissed, until the owner sends a line.
    pub waiting: Option<String>,
}

impl Editor {
    pub fn text(&self) -> &str {
        &self.buf
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
    pub fn cursor(&self) -> usize {
        self.cur
    }
    pub fn set(&mut self, s: &str) {
        self.buf = s.to_string();
        self.cur = self.buf.len();
        self.popup_dismissed = false;
        self.sel = 0;
    }
    pub fn clear(&mut self) {
        self.set("");
        self.hpos = None;
    }

    fn edited(&mut self) {
        self.hpos = None;
        self.sel = 0;
        if !self.buf.starts_with('/') {
            self.popup_dismissed = false;
        }
    }

    pub fn insert(&mut self, s: &str) {
        self.buf.insert_str(self.cur, s);
        self.cur += s.len();
        self.edited();
    }

    /// Bracketed paste: verbatim, CRLF and lone CR → LF.
    pub fn paste(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        self.insert(&s);
    }

    pub fn newline(&mut self) {
        self.insert("\n");
    }

    /// `\` then Enter: the backslash before the cursor becomes a newline. True if it applied.
    pub fn backslash_newline(&mut self) -> bool {
        if self.cur > 0 && self.buf[..self.cur].ends_with('\\') {
            self.buf.replace_range(self.cur - 1..self.cur, "\n");
            self.edited();
            return true;
        }
        false
    }

    fn prev_boundary(&self, at: usize) -> usize {
        self.buf[..at].grapheme_indices(true).next_back().map_or(0, |(i, _)| i)
    }
    fn next_boundary(&self, at: usize) -> usize {
        self.buf[at..].graphemes(true).next().map_or(at, |g| at + g.len())
    }

    /// Removes exactly one grapheme before the cursor.
    pub fn backspace(&mut self) {
        if self.cur == 0 {
            return;
        }
        let p = self.prev_boundary(self.cur);
        self.buf.replace_range(p..self.cur, "");
        self.cur = p;
        self.edited();
    }
    pub fn delete(&mut self) {
        let n = self.next_boundary(self.cur);
        self.buf.replace_range(self.cur..n, "");
        self.edited();
    }
    pub fn left(&mut self) {
        self.cur = self.prev_boundary(self.cur);
    }
    pub fn right(&mut self) {
        self.cur = self.next_boundary(self.cur);
    }
    fn line_start(&self) -> usize {
        self.buf[..self.cur].rfind('\n').map_or(0, |i| i + 1)
    }
    fn line_end(&self) -> usize {
        self.buf[self.cur..].find('\n').map_or(self.buf.len(), |i| self.cur + i)
    }
    pub fn home(&mut self) {
        self.cur = self.line_start();
    }
    pub fn end(&mut self) {
        self.cur = self.line_end();
    }
    fn word_left_pos(&self) -> usize {
        let mut p = self.cur;
        let gs: Vec<(usize, &str)> = self.buf[..self.cur].grapheme_indices(true).collect();
        let mut i = gs.len();
        while i > 0 && gs[i - 1].1.chars().all(char::is_whitespace) {
            i -= 1;
            p = gs[i].0;
        }
        while i > 0 && !gs[i - 1].1.chars().all(char::is_whitespace) {
            i -= 1;
            p = gs[i].0;
        }
        p
    }
    pub fn word_left(&mut self) {
        self.cur = self.word_left_pos();
    }
    pub fn word_right(&mut self) {
        let rest = &self.buf[self.cur..];
        let mut p = self.cur;
        let mut seen_word = false;
        for (i, g) in rest.grapheme_indices(true) {
            let ws = g.chars().all(char::is_whitespace);
            if ws && seen_word {
                p = self.cur + i;
                self.cur = p;
                return;
            }
            seen_word |= !ws;
            p = self.cur + i + g.len();
        }
        self.cur = p;
    }
    /// Ctrl-W: deletes the word before the cursor.
    pub fn delete_word(&mut self) {
        let p = self.word_left_pos();
        self.buf.replace_range(p..self.cur, "");
        self.cur = p;
        self.edited();
    }
    /// Ctrl-U: deletes to the start of the line.
    pub fn kill_to_start(&mut self) {
        let s = self.line_start();
        let s = if s == self.cur && s > 0 { s - 1 } else { s };
        self.buf.replace_range(s..self.cur, "");
        self.cur = s;
        self.edited();
    }
    /// Ctrl-K: deletes to the end of the line.
    pub fn kill_to_end(&mut self) {
        let e = self.line_end();
        let e = if e == self.cur && e < self.buf.len() { e + 1 } else { e };
        self.buf.replace_range(self.cur..e, "");
        self.edited();
    }

    /// The cursor's (line, display column) in logical lines.
    pub fn line_col(&self) -> (usize, usize) {
        let before = &self.buf[..self.cur];
        let line = before.matches('\n').count();
        let col = before[self.line_start()..].graphemes(true).map(g_width).sum();
        (line, col)
    }

    fn lines(&self) -> Vec<(usize, &str)> {
        let mut out = Vec::new();
        let mut start = 0;
        for (i, c) in self.buf.char_indices() {
            if c == '\n' {
                out.push((start, &self.buf[start..i]));
                start = i + 1;
            }
        }
        out.push((start, &self.buf[start..]));
        out
    }

    fn goto_line(&mut self, line: usize, col: usize) {
        let lines = self.lines();
        let (start, text) = lines[line];
        let mut w = 0;
        let mut p = start;
        for (i, g) in text.grapheme_indices(true) {
            if w + g_width(g) > col {
                break;
            }
            w += g_width(g);
            p = start + i + g.len();
        }
        if text.is_empty() {
            p = start;
        }
        self.cur = p;
    }

    /// Up: the previous line, or on the first line the previous history entry.
    pub fn up(&mut self) {
        let (line, col) = self.line_col();
        if line > 0 {
            return self.goto_line(line - 1, col);
        }
        if self.history.is_empty() {
            return;
        }
        let pos = match self.hpos {
            None => {
                self.draft = self.buf.clone();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(p) => p - 1,
        };
        self.hpos = Some(pos);
        self.buf = self.history[pos].clone();
        self.cur = self.buf.len();
    }

    /// Down: the next line, or on the last line the next history entry (then the draft).
    pub fn down(&mut self) {
        let (line, col) = self.line_col();
        if line + 1 < self.lines().len() {
            return self.goto_line(line + 1, col);
        }
        let Some(p) = self.hpos else { return };
        if p + 1 < self.history.len() {
            self.hpos = Some(p + 1);
            self.buf = self.history[p + 1].clone();
        } else {
            self.hpos = None;
            self.buf = std::mem::take(&mut self.draft);
        }
        self.cur = self.buf.len();
    }

    pub fn push_history(&mut self, s: &str) {
        if self.history.last().is_none_or(|l| l != s) {
            self.history.push(s.to_string());
        }
        if self.history.len() > 1000 {
            self.history.remove(0);
        }
    }

    /// The completion candidates for the box: shown while it is one line starting with `/`.
    pub fn completions(&self) -> Vec<(&'static str, &'static str)> {
        if self.popup_dismissed || !self.buf.starts_with('/') || self.buf.contains('\n') {
            return Vec::new();
        }
        let typed = self.buf.as_str();
        COMMANDS.iter().copied().filter(|(c, _)| c.starts_with(typed) && *c != typed).collect()
    }

    pub fn popup_open(&self) -> bool {
        !self.completions().is_empty()
    }

    /// Tab: completes to the selected candidate.
    pub fn complete(&mut self) -> bool {
        let c = self.completions();
        let Some((cmd, _)) = c.get(self.sel.min(c.len().saturating_sub(1))) else { return false };
        let cmd = cmd.to_string();
        self.set(&cmd);
        true
    }

    pub fn popup_move(&mut self, down: bool) {
        let n = self.completions().len();
        if n == 0 {
            return;
        }
        self.sel = if down { (self.sel + 1) % n } else { (self.sel + n - 1) % n };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backspace_removes_one_grapheme() {
        let mut e = Editor::default();
        // precomposed "Việt" and the combining form of "ệ"
        e.insert("Việt");
        e.backspace();
        assert_eq!(e.text(), "Việ");
        e.backspace();
        assert_eq!(e.text(), "Vi");
        e.clear();
        e.insert("Vie\u{0323}\u{0302}t");
        e.backspace();
        e.backspace();
        assert_eq!(e.text(), "Vi", "the combining sequence is one grapheme");
        // Telex correction: type "a", the IME sends Backspace then "â"
        e.clear();
        e.insert("ca");
        e.backspace();
        e.insert("â");
        assert_eq!(e.text(), "câ");
    }

    #[test]
    fn wide_characters_move_the_cursor_two_columns() {
        let mut e = Editor::default();
        e.insert("日本👍x");
        assert_eq!(e.line_col(), (0, 7));
        e.left();
        assert_eq!(e.line_col(), (0, 6));
        e.left();
        assert_eq!(e.line_col(), (0, 4));
    }

    #[test]
    fn lines_history_and_kills() {
        let mut e = Editor::default();
        e.insert("abc");
        e.newline();
        e.insert("de");
        assert_eq!(e.line_col(), (1, 2));
        e.up();
        assert_eq!(e.line_col(), (0, 2));
        e.end();
        assert_eq!(e.line_col(), (0, 3));
        e.down();
        assert_eq!(e.line_col(), (1, 2));
        e.push_history("first");
        e.push_history("second");
        e.clear();
        e.insert("draft");
        e.up();
        assert_eq!(e.text(), "second");
        e.up();
        assert_eq!(e.text(), "first");
        e.down();
        e.down();
        assert_eq!(e.text(), "draft");
        e.set("one two three");
        e.delete_word();
        assert_eq!(e.text(), "one two ");
        e.home();
        e.kill_to_end();
        assert_eq!(e.text(), "");
        e.set("x\\");
        assert!(e.backslash_newline());
        assert_eq!(e.text(), "x\n");
        e.clear();
        e.paste("a\r\nb\rc");
        assert_eq!(e.text(), "a\nb\nc");
    }

    #[test]
    fn completion() {
        let mut e = Editor::default();
        e.insert("/ro");
        let c = e.completions();
        // v0.5.4: `/rollback` is listed too, after the `/root` lines, so Tab on `/ro` still completes `/root status`
        assert!(c.iter().all(|(c, _)| c.starts_with("/root") || *c == "/rollback") && c.iter().any(|(c, _)| *c == "/rollback"));
        assert!(e.complete());
        assert_eq!(e.text(), "/root status");
        e.set("/later x");
        assert!(!e.popup_open());
        e.set("/");
        assert!(e.completions().len() > 10);
        assert!(!e.completions().iter().any(|(c, _)| ["/approve", "/deny", "/clear-safe"].contains(c)));
    }

    /// TUI-FIX (v0.5.1 user session): `/` then Tab completed to `/panic`. Now it completes to a safe
    /// command, and the commands that stop the agent are the last candidates of a bare `/`.
    #[test]
    fn tab_on_a_bare_slash_is_never_a_stop() {
        let mut e = Editor::default();
        e.set("/");
        assert!(e.complete());
        assert_eq!(e.text(), "/usage");
        e.set("/");
        let c = e.completions();
        assert_eq!(c.last().map(|x| x.0), Some("/panic"));
        let first_stop = c.iter().position(|x| crate::proto::STOPPING.contains(&x.0)).unwrap();
        assert!(first_stop >= c.len() - crate::proto::STOPPING.len(), "{c:?}");
        // `/r` offers the Root's read-only lines before /resume and /root stop
        e.set("/r");
        assert!(e.complete());
        assert_eq!(e.text(), "/root status");
    }
}
