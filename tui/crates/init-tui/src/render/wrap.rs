//! Width and wrapping over grapheme clusters (`unicode-segmentation`) and display widths
//! (`unicode-width`): wide CJK and emoji take two columns, combining marks none.

use super::{Line, Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Display width of a sanitized string.
pub fn str_width(s: &str) -> usize {
    s.graphemes(true).map(g_width).sum()
}

/// Display width of one grapheme cluster (at most 2).
pub fn g_width(g: &str) -> usize {
    UnicodeWidthStr::width(g).min(2)
}

/// Word-wraps a logical line into rows of at most `width` columns. Continuation rows start with
/// `indent` spaces (in `indent_style`). Breaks after spaces where possible, else inside a word.
pub fn wrap(line: &Line, width: usize, indent: usize) -> Vec<Line> {
    let width = width.max(1);
    let indent = if indent >= width { 0 } else { indent };
    let cells: Vec<(&str, Style, usize)> =
        line.0.iter().flat_map(|s| s.text.graphemes(true).map(move |g| (g, s.style, g_width(g)))).collect();
    let mut rows = Vec::new();
    let mut i = 0;
    let mut first = true;
    loop {
        let avail = if first { width } else { width - indent };
        // the cells of this row: [i, end)
        let mut used = 0;
        let mut end = i;
        let mut last_space: Option<usize> = None;
        while end < cells.len() {
            let w = cells[end].2;
            if used + w > avail {
                break;
            }
            used += w;
            if cells[end].0 == " " {
                last_space = Some(end);
            }
            end += 1;
        }
        let mut next = end;
        if end < cells.len() {
            // a break opportunity: after the last space in the row, if it leaves something on the row
            if cells[end].0 == " " {
                next = end + 1;
            } else if let Some(sp) = last_space.filter(|&sp| sp > i) {
                end = sp; // drop the space at the break
                next = sp + 1;
            }
            if end == i && next == i {
                // a single cell wider than the row: take it anyway
                end = i + 1;
                next = i + 1;
            }
        }
        let mut row = Line::new();
        if !first && indent > 0 {
            row.push(" ".repeat(indent), Style::PLAIN);
        }
        for c in &cells[i..end] {
            row.push(c.0, c.1);
        }
        rows.push(row);
        first = false;
        i = next;
        if i >= cells.len() {
            break;
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str, width: usize) -> Vec<String> {
        wrap(&Line::from_span(s, Style::PLAIN), width, 0).iter().map(Line::text).collect()
    }

    #[test]
    fn widths() {
        assert_eq!(str_width("abc"), 3);
        assert_eq!(str_width("日本"), 4);
        assert_eq!(str_width("e\u{0302}\u{0301}"), 1, "combining marks take no column");
        assert_eq!(str_width("ế"), 1);
        assert_eq!(str_width("👍"), 2);
    }

    #[test]
    fn wraps_words_and_long_runs() {
        assert_eq!(w("hello world foo", 11), ["hello world", "foo"]);
        assert_eq!(w("hello world foo", 8), ["hello", "world", "foo"]);
        assert_eq!(w("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(w("", 4), [""]);
        assert_eq!(w("日本語です", 5), ["日本", "語で", "す"]);
        let r = wrap(&Line::from_span("aaa bbb ccc", Style::PLAIN), 5, 2);
        assert_eq!(r.iter().map(Line::text).collect::<Vec<_>>(), ["aaa", "  bbb", "  ccc"]);
        for row in wrap(&Line::from_span("x".repeat(100), Style::PLAIN), 7, 0) {
            assert!(row.width() <= 7);
        }
    }
}
