//! The renderer (docs/tui.md §3.4–§3.9): a pure function of the state and the terminal size to rows
//! of styled spans. The terminal writer (`crate::app`) turns rows into crossterm commands; styles are
//! only ever produced here, never taken from frame bytes (§3.13).

pub mod live;
pub mod md;
pub mod sanitize;
pub mod status;
pub mod transcript;
pub mod wrap;

#[cfg(test)]
mod golden_tests;

/// The colours the TUI uses. Mapped to crossterm colours by the writer; dropped under `NO_COLOR`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Red,
    Green,
    Yellow,
    Cyan,
    Grey,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub reverse: bool,
    pub italic: bool,
}

impl Style {
    pub const PLAIN: Style = Style { fg: None, bg: None, bold: false, dim: false, reverse: false, italic: false };
    pub fn fg(c: Color) -> Style {
        Style { fg: Some(c), ..Style::PLAIN }
    }
    pub fn dim() -> Style {
        Style { dim: true, ..Style::PLAIN }
    }
    pub fn bold() -> Style {
        Style { bold: true, ..Style::PLAIN }
    }
    pub fn with_dim(mut self) -> Style {
        self.dim = true;
        self
    }
    pub fn with_bold(mut self) -> Style {
        self.bold = true;
        self
    }
    pub fn with_reverse(mut self) -> Style {
        self.reverse = true;
        self
    }
    /// One character per style for the golden style masks (TUI-1).
    #[cfg(test)]
    pub fn mask(&self) -> char {
        let c = |c: Color| match c {
            Color::Red => 'r',
            Color::Green => 'g',
            Color::Yellow => 'y',
            Color::Cyan => 'c',
            Color::Grey => 'e',
        };
        if self.reverse {
            return match self.fg {
                Some(Color::Red) => 'R',
                Some(Color::Yellow) => 'Y',
                _ => 'V',
            };
        }
        if let Some(f) = self.fg {
            let ch = c(f);
            return if self.bold { ch.to_ascii_uppercase() } else { ch };
        }
        if self.bg.is_some() {
            return 'k';
        }
        if self.bold {
            return 'b';
        }
        if self.dim || self.italic {
            return 'd';
        }
        '.'
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Span {
        Span { text: text.into(), style }
    }
}

/// One terminal row (or one logical line before wrapping).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Line(pub Vec<Span>);

impl Line {
    pub fn new() -> Line {
        Line(Vec::new())
    }
    pub fn from_span(text: impl Into<String>, style: Style) -> Line {
        Line(vec![Span::new(text, style)])
    }
    pub fn push(&mut self, text: impl Into<String>, style: Style) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.0.last_mut()
            && last.style == style
        {
            last.text.push_str(&text);
            return;
        }
        self.0.push(Span { text, style });
    }
    pub fn extend(&mut self, other: Line) {
        for s in other.0 {
            self.push(s.text, s.style);
        }
    }
    pub fn width(&self) -> usize {
        self.0.iter().map(|s| wrap::str_width(&s.text)).sum()
    }
    pub fn text(&self) -> String {
        self.0.iter().map(|s| s.text.as_str()).collect()
    }
    /// The style mask: one char per column (wide graphemes take two).
    #[cfg(test)]
    pub fn mask(&self) -> String {
        let mut m = String::new();
        for s in &self.0 {
            let w = wrap::str_width(&s.text);
            m.extend(std::iter::repeat_n(s.style.mask(), w));
        }
        m
    }
    /// Truncates to `width` columns, adding `…` when something was cut.
    pub fn truncate(&self, width: usize) -> Line {
        if self.width() <= width {
            return self.clone();
        }
        let mut out = Line::new();
        let mut used = 0;
        let limit = width.saturating_sub(1);
        'outer: for s in &self.0 {
            for g in unicode_segmentation::UnicodeSegmentation::graphemes(s.text.as_str(), true) {
                let w = wrap::str_width(g);
                if used + w > limit {
                    break 'outer;
                }
                out.push(g, s.style);
                used += w;
            }
        }
        if width > 0 {
            out.push("…", Style::dim());
        }
        out
    }
    /// Pads with plain spaces to `width` columns, using `style` for the padding.
    pub fn pad(mut self, width: usize, style: Style) -> Line {
        let w = self.width();
        if w < width {
            self.push(" ".repeat(width - w), style);
        }
        self
    }
}

/// A rendered screen region: rows plus the caret (column, row) when the cursor is shown there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Region {
    pub rows: Vec<Line>,
    pub cursor: Option<(usize, usize)>,
}

/// Golden text grid and style mask of rows (TUI-1): each row padded to `width`.
#[cfg(test)]
pub fn grid(rows: &[Line], width: usize) -> String {
    let mut s = String::new();
    for r in rows {
        let t = r.text();
        let w = r.width();
        s.push_str(&t);
        s.extend(std::iter::repeat_n(' ', width.saturating_sub(w)));
        s.push('|');
        s.push_str(&r.mask());
        s.extend(std::iter::repeat_n(' ', width.saturating_sub(w)));
        s.push('\n');
    }
    s
}

/// Money in micro-USD as `$1.23`; amounts below a cent but above zero as `<$0.01`.
pub fn usd(musd: i64) -> String {
    if musd > 0 && musd < 10_000 {
        return "<$0.01".into();
    }
    let cents = (musd + 5_000).div_euclid(10_000);
    format!("${}.{:02}", cents / 100, cents % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usd_formats() {
        assert_eq!(usd(0), "$0.00");
        assert_eq!(usd(1_230_000), "$1.23");
        assert_eq!(usd(5_000_000), "$5.00");
        assert_eq!(usd(1500), "<$0.01");
        assert_eq!(usd(44_999), "$0.04");
        assert_eq!(usd(45_000), "$0.05");
    }

    #[test]
    fn line_push_merges_and_truncates() {
        let mut l = Line::new();
        l.push("ab", Style::PLAIN);
        l.push("cd", Style::PLAIN);
        l.push("ef", Style::dim());
        assert_eq!(l.0.len(), 2);
        assert_eq!(l.width(), 6);
        assert_eq!(l.mask(), "....dd");
        let t = l.truncate(4);
        assert_eq!(t.text(), "abc…");
        let wide = Line::from_span("日本語", Style::PLAIN);
        assert_eq!(wide.width(), 6);
        assert_eq!(wide.truncate(4).text(), "日…");
    }
}
