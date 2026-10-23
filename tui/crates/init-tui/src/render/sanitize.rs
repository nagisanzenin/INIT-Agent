//! The text sanitizer (docs/tui.md §3.13). Every string taken from a frame passes here before any
//! terminal write: control characters become visible text, so no byte from the agent zone can start
//! an escape sequence, move the cursor or reorder text (Trojan Source).

use super::{Line, Style, wrap};

/// A piece of sanitized input.
#[derive(Debug, PartialEq)]
enum Piece<'a> {
    Text(&'a str),
    /// A visible replacement for a control character, shown dim.
    Ctl(String),
    Tab,
    Newline,
}

fn is_bidi(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}')
}

/// Is `c` a character the sanitizer replaces (other than `\t`, `\n`, `\r`)?
pub fn is_unsafe(c: char) -> bool {
    let u = c as u32;
    u < 0x20 || u == 0x7F || (0x80..=0x9F).contains(&u) || is_bidi(c)
}

fn replacement(c: char) -> String {
    match c {
        '\u{1B}' => "␛".into(),
        '\r' => "␍".into(),
        c if (c as u32) <= 0xFF => format!("<U+{:04X}>", c as u32),
        c => format!("<U+{:04X}>", c as u32),
    }
}

fn pieces(s: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let special = c == '\t' || c == '\n' || c == '\r' || is_unsafe(c);
        if !special {
            continue;
        }
        if start < i {
            out.push(Piece::Text(&s[start..i]));
        }
        let mut end = i + c.len_utf8();
        match c {
            '\t' => out.push(Piece::Tab),
            '\n' => out.push(Piece::Newline),
            '\r' if it.peek().is_some_and(|&(_, n)| n == '\n') => {
                it.next();
                end += 1;
                out.push(Piece::Newline);
            }
            c => out.push(Piece::Ctl(replacement(c))),
        }
        start = end;
    }
    if start < s.len() {
        out.push(Piece::Text(&s[start..]));
    }
    out
}

/// Sanitized logical lines (split at `\n` and `\r\n`); tabs go to the next multiple of 4 columns,
/// counted from `col0` on the first line and from 0 on the others.
pub fn lines(s: &str, style: Style, col0: usize) -> Vec<Line> {
    let mut out = vec![Line::new()];
    let mut col = col0;
    for p in pieces(s) {
        let cur = out.last_mut().expect("never empty");
        match p {
            Piece::Text(t) => {
                col += wrap::str_width(t);
                cur.push(t, style);
            }
            Piece::Ctl(r) => {
                col += wrap::str_width(&r);
                cur.push(r, style.with_dim());
            }
            Piece::Tab => {
                let n = 4 - col % 4;
                col += n;
                cur.push(" ".repeat(n), style);
            }
            Piece::Newline => {
                out.push(Line::new());
                col = 0;
            }
        }
    }
    out
}

/// One row: like [`lines`], with line breaks shown as a dim `␤`.
pub fn inline(s: &str, style: Style) -> Line {
    let mut out = Line::new();
    for (i, l) in lines(s, style, 0).into_iter().enumerate() {
        if i > 0 {
            out.push("␤", style.with_dim());
        }
        out.extend(l);
    }
    out
}

/// Plain text for line mode: control characters replaced, tabs expanded, `\n` kept. `col0` is the
/// column the text starts at.
pub fn plain(s: &str, col0: usize) -> String {
    let mut out = String::new();
    let mut col = col0;
    for p in pieces(s) {
        match p {
            Piece::Text(t) => {
                col += wrap::str_width(t);
                out.push_str(t);
            }
            Piece::Ctl(r) => {
                col += wrap::str_width(&r);
                out.push_str(&r);
            }
            Piece::Tab => {
                let n = 4 - col % 4;
                col += n;
                out.extend(std::iter::repeat_n(' ', n));
            }
            Piece::Newline => {
                out.push('\n');
                col = 0;
            }
        }
    }
    out
}

/// `--raw`: a received JSON line with U+007F–U+009F and the bidi controls re-escaped as `\uXXXX`
/// (serde_json already escapes C0). These characters can only occur inside JSON strings, so the
/// result is the same JSON value.
pub fn raw_json(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for c in line.chars() {
        let u = c as u32;
        if (0x7F..=0x9F).contains(&u) || is_bidi(c) {
            out.push_str(&format!("\\u{u:04x}"));
        } else if u < 0x20 {
            // a raw C0 cannot be valid JSON inside a string; keep the output inert anyway
            out.push_str(&format!("\\u{u:04x}"));
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txt(ls: &[Line]) -> Vec<String> {
        ls.iter().map(Line::text).collect()
    }

    #[test]
    fn controls_become_visible() {
        let l = lines("a\u{1b}[2Jb\u{7}c\u{9b}d\u{202e}e", Style::PLAIN, 0);
        assert_eq!(txt(&l), ["a␛[2Jb<U+0007>c<U+009B>d<U+202E>e"]);
        assert_eq!(l[0].mask(), ".d....dddddddd.dddddddd.dddddddd.");
        assert!(!l[0].text().contains('\u{1b}'));
    }

    #[test]
    fn newlines_tabs_and_cr() {
        assert_eq!(txt(&lines("a\r\nb\nc", Style::PLAIN, 0)), ["a", "b", "c"]);
        assert_eq!(txt(&lines("a\rb", Style::PLAIN, 0)), ["a␍b"]);
        assert_eq!(txt(&lines("ab\tc\td", Style::PLAIN, 0)), ["ab  c   d"]);
        assert_eq!(txt(&lines("\tx", Style::PLAIN, 2)), ["  x"]);
        assert_eq!(inline("a\nb", Style::PLAIN).text(), "a␤b");
        assert_eq!(plain("x\u{1b}]52;c;AAAA\u{7}\ty\n", 0), "x␛]52;c;AAAA<U+0007>    y\n");
    }

    #[test]
    fn raw_json_escapes_c1_and_bidi() {
        let v = serde_json::json!({"text": "a\u{9b}b\u{202e}c\u{7f}"});
        let s = serde_json::to_string(&v).unwrap();
        let r = raw_json(&s);
        assert!(r.is_ascii(), "{r}");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&r).unwrap(), v);
    }

    #[test]
    fn every_unsafe_code_point_is_replaced() {
        let all: String = (0u32..0xA0).filter_map(char::from_u32).chain(['\u{202A}', '\u{2069}', '\u{200E}', '\u{061C}']).collect();
        for l in lines(&all, Style::PLAIN, 0) {
            assert!(!l.text().chars().any(|c| is_unsafe(c) || c == '\t' || c == '\r'), "{:?}", l.text());
        }
    }
}
