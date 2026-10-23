//! The Markdown subset for `text` frames (docs/tui.md §3.5), line by line so that streaming stays
//! O(new bytes): fenced code blocks (indented, dim background), inline code, `**bold**`, `#`
//! headings and `-`/`*`/`1.` lists. Links print as plain text; no images, tables or HTML.
//! Every piece of text still passes the sanitizer.

use super::{Color, Line, Style, sanitize};

/// The state carried from one logical line to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MdState {
    pub in_code: bool,
}

const CODE: Style = Style { fg: None, bg: Some(Color::Grey), bold: false, dim: false, reverse: false, italic: false };
const INLINE_CODE: Style = Style { fg: Some(Color::Cyan), bg: None, bold: false, dim: false, reverse: false, italic: false };

/// Renders one logical line (no `\n` inside) and returns the state for the next one, plus the
/// continuation indent for wrapping.
pub fn line(src: &str, st: MdState) -> (Line, MdState, usize) {
    let trimmed = src.trim_start();
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        let mut l = Line::new();
        l.push("  ", Style::PLAIN);
        l.extend(sanitize::inline(trimmed, Style::dim()));
        return (l, MdState { in_code: !st.in_code }, 2);
    }
    if st.in_code {
        let mut l = Line::new();
        l.push("  ", Style::PLAIN);
        l.extend(sanitize::inline(src, CODE));
        return (l, st, 2);
    }
    // headings
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
        let mut l = Line::new();
        inline_md(trimmed[hashes + 1..].trim(), Style::bold(), &mut l);
        return (l, st, 0);
    }
    // lists
    let lead = src.len() - trimmed.len();
    let indent = &src[..lead];
    let indent_w = sanitize::inline(indent, Style::PLAIN).width();
    if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
        let mut l = Line::new();
        l.push(" ".repeat(indent_w), Style::PLAIN);
        l.push("• ", Style::PLAIN);
        inline_md(rest, Style::PLAIN, &mut l);
        return (l, st, indent_w + 2);
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && digits <= 9 && trimmed[digits..].starts_with(". ") {
        let mut l = Line::new();
        l.push(" ".repeat(indent_w), Style::PLAIN);
        l.push(&trimmed[..digits + 2], Style::PLAIN);
        inline_md(&trimmed[digits + 2..], Style::PLAIN, &mut l);
        return (l, st, indent_w + digits + 2);
    }
    let mut l = Line::new();
    inline_md(src, Style::PLAIN, &mut l);
    (l, st, 0)
}

/// Inline code and `**bold**`; an unclosed marker is literal text.
fn inline_md(s: &str, base: Style, out: &mut Line) {
    let mut rest = s;
    let mut col = out.width();
    while !rest.is_empty() {
        let tick = rest.find('`');
        let star = rest.find("**");
        let (pos, kind) = match (tick, star) {
            (Some(t), Some(b)) if b < t => (b, 2),
            (Some(t), _) => (t, 1),
            (None, Some(b)) => (b, 2),
            (None, None) => break,
        };
        let marker = if kind == 1 { "`" } else { "**" };
        let after = &rest[pos + marker.len()..];
        let Some(close) = after.find(marker).filter(|&c| c > 0) else {
            // unclosed: emit up to and including the marker literally, continue after it
            let lit = &rest[..pos + marker.len()];
            push(lit, base, out, &mut col);
            rest = after;
            continue;
        };
        push(&rest[..pos], base, out, &mut col);
        let inner = &after[..close];
        let st = if kind == 1 { INLINE_CODE } else { base.with_bold() };
        push(inner, st, out, &mut col);
        rest = &after[close + marker.len()..];
    }
    push(rest, base, out, &mut col);
}

fn push(s: &str, st: Style, out: &mut Line, col: &mut usize) {
    if s.is_empty() {
        return;
    }
    for (i, l) in sanitize::lines(s, st, *col).into_iter().enumerate() {
        if i > 0 {
            out.push("␤", st.with_dim());
        }
        *col += l.width();
        out.extend(l);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> (String, String) {
        let (l, _, _) = line(s, MdState::default());
        (l.text(), l.mask())
    }

    #[test]
    fn subset() {
        assert_eq!(r("# Title"), ("Title".into(), "bbbbb".into()));
        assert_eq!(r("- item `x` **y**"), ("• item x y".into(), ".......c.b".into()));
        assert_eq!(r("  * nested"), ("  • nested".into(), "..........".into()));
        assert_eq!(r("12. twelve"), ("12. twelve".into(), "..........".into()));
        assert_eq!(r("a `unclosed"), ("a `unclosed".into(), "...........".into()));
        assert_eq!(r("see [x](http://e.com)"), ("see [x](http://e.com)".into(), ".....................".into()));
        assert_eq!(r("**b** and **"), ("b and **".into(), "b.......".into()));
    }

    #[test]
    fn fences() {
        let (a, s1, _) = line("```rust", MdState::default());
        assert!(s1.in_code);
        assert_eq!(a.text(), "  ```rust");
        let (b, s2, _) = line("let x = `1`;", s1);
        assert_eq!(b.text(), "  let x = `1`;");
        assert_eq!(b.mask(), "..kkkkkkkkkkkk");
        assert!(s2.in_code);
        let (_, s3, _) = line("```", s2);
        assert!(!s3.in_code);
    }

    #[test]
    fn escapes_inside_markdown_are_sanitized() {
        let (l, _, _) = line("**\u{1b}[31mred**", MdState::default());
        assert!(!l.text().contains('\u{1b}'));
        assert!(l.text().starts_with('␛'));
    }
}
