//! The renderer (docs/tui.md §3.4–§3.9): a pure function of the state and the terminal size to rows
//! of styled spans. The terminal writer (`crate::app`) turns rows into crossterm commands; styles are
//! only ever produced here, never taken from frame bytes (§3.13).

pub mod gates; // TUI-OBSERVE: the gate track
pub mod lineage; // TUI-OBSERVE: the lineage panel in the transcript view
pub mod live;
pub mod md;
pub mod palette;
pub mod picker; // TUI-PICKER: the inline picker (setup, /model, /key, the agent's questions)
pub mod sanitize;
pub mod status;
pub mod table; // TUI-BRAND: the Root's JSON answers as tables
pub mod transcript;
pub mod wrap;

#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod vt_tests; // TUI-BRAND: the renderer's bytes through a terminal emulator (vt100)
#[cfg(test)]
mod observe_tests; // TUI-OBSERVE

pub use palette::Role;

/// A span's style: a semantic role for the foreground (and rarely the background), plus attributes.
/// The writer (`crate::term`) asks the palette what a role is on this terminal (§3.9a).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style {
    /// None is the terminal's own foreground, the same as `Role::Ink`.
    pub fg: Option<Role>,
    pub bg: Option<Role>,
    pub bold: bool,
    pub reverse: bool,
    pub italic: bool,
}

impl Style {
    pub const PLAIN: Style = Style { fg: None, bg: None, bold: false, reverse: false, italic: false };
    /// The style of a role (`Role::Ink` is `PLAIN`).
    pub const fn fg(r: Role) -> Style {
        Style { fg: if matches!(r, Role::Ink) { None } else { Some(r) }, bg: None, bold: false, reverse: false, italic: false }
    }
    /// Secondary text (`Role::Dim`).
    pub const fn dim() -> Style {
        Style::fg(Role::Dim)
    }
    /// Tertiary text (`Role::Mute`).
    pub const fn mute() -> Style {
        Style::fg(Role::Mute)
    }
    pub const fn bold() -> Style {
        Style { bold: true, ..Style::PLAIN }
    }
    /// The same style in the secondary role (an inert placeholder inside a span, for example).
    pub const fn with_dim(mut self) -> Style {
        self.fg = Some(Role::Dim);
        self
    }
    pub const fn with_bold(mut self) -> Style {
        self.bold = true;
        self
    }
    pub const fn with_reverse(mut self) -> Style {
        self.reverse = true;
        self
    }
    pub const fn with_italic(mut self) -> Style {
        self.italic = true;
        self
    }
    /// One character per style for the golden style masks (TUI-1): the role's letter
    /// ([`Role::mask`]), uppercase when bold (`b` for bold ink); reverse video `R` harm, `O` root,
    /// `G` pass, `V` other; `k` a background.
    #[cfg(test)]
    pub fn mask(&self) -> char {
        if self.reverse {
            return match self.fg {
                Some(Role::Harm) => 'R',
                Some(Role::Root) => 'O',
                Some(Role::Pass) => 'G',
                _ => 'V',
            };
        }
        if self.bg.is_some() {
            return 'k';
        }
        match self.fg {
            Some(r) if self.bold => r.mask().to_ascii_uppercase(),
            Some(r) => r.mask(),
            None if self.bold => 'b',
            None => '.',
        }
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

/// Money in micro-USD, one rule on every surface (TUI-FIX, v0.5.1: the status bar said `$0.05` and
/// `/usage` `$0.04` around the same 45,850 µUSD): below a dollar to a tenth of a cent (`$0.046`,
/// `$0.004`, `$0.000`), from a dollar up to the cent (`$1.23`), both rounded half up; an amount above
/// zero but below a tenth of a cent is `<$0.001`, never `$0.000`.
pub fn usd(musd: i64) -> String {
    let musd = musd.max(0);
    if musd > 0 && musd < 500 {
        return "<$0.001".into();
    }
    if musd < 999_500 {
        let m = (musd + 500) / 1000;
        return format!("$0.{m:03}");
    }
    let cents = (musd + 5_000) / 10_000;
    format!("${}.{:02}", cents / 100, cents % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One rounding rule on every surface (TUI-FIX, v0.5.1).
    #[test]
    fn usd_formats() {
        assert_eq!(usd(0), "$0.000");
        assert_eq!(usd(-3), "$0.000");
        assert_eq!(usd(1), "<$0.001", "spend that happened is never $0.000");
        assert_eq!(usd(499), "<$0.001");
        assert_eq!(usd(500), "$0.001");
        assert_eq!(usd(4_200), "$0.004");
        assert_eq!(usd(44_999), "$0.045");
        assert_eq!(usd(45_850), "$0.046", "the user session's ledger total");
        assert_eq!(usd(123_456), "$0.123");
        assert_eq!(usd(999_499), "$0.999");
        assert_eq!(usd(999_500), "$1.00");
        assert_eq!(usd(1_230_000), "$1.23");
        assert_eq!(usd(5_000_000), "$5.00");
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
