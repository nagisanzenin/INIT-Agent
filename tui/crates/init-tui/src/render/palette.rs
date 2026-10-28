//! The semantic palette (docs/tui.md §3.9a): the INIT brand's grammar of colour, as roles. The
//! renderer names a role ("this is the Root", "this change is in flight"), never a colour; the one
//! place where styles become escape codes (`crate::term`) asks the [`Palette`] what a role is on
//! this terminal. A colour that does not mean something is not there.
//!
//! Tiers: truecolor (`COLORTERM=truecolor|24bit`), xterm-256 (`TERM=*256color*`; Terminal.app before
//! macOS 26 has no truecolor), 16 colours, and `NO_COLOR` (meaning carried by glyphs and words). Each
//! colour tier has a variant for dark and for light backgrounds; the background comes from
//! `COLORFGBG`, else an OSC 11 query (`crate::term`), else dark. No file is read (T-5).
//!
//! Contrast (WCAG, computed): every signal is ≥ 5.7:1 on the brand night `#0D0E11` and ≥ 4.5:1 on
//! white in its light variant. Tables: research_notes `tui_audit.md` §2; the values chosen are
//! listed in [`Palette::paint`] and docs/tui.md §3.9a.

use super::Style;

/// What a span means. The renderer uses only these; `Style::fg(role)` builds a style.
#[allow(dead_code)] // `Ink` is the named form of the plain style (`Style::fg(Role::Ink) == Style::PLAIN`)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Role {
    /// Primary text (brand `bone`): the terminal's own foreground, in every tier.
    Ink,
    /// Secondary text: receipts, finished routine rows, labels.
    Dim,
    /// Tertiary text: separators, hints, times, the quietest words.
    Mute,
    /// Hairlines: the input frame, rules, the code-block background.
    Line,
    /// The Root, and only the Root: its state on the status bar (`root ok`), the `[root]` tag, the
    /// Root's restore and rollback markers, `/panic /stop /undo`. Never an accent, never animated.
    Root,
    /// A change in flight: a running gate, a promote, a hot swap; outline glyphs (`◇` `□`). Never a
    /// prompt, a key name or inline code.
    Rewrite,
    /// Kept: a gate passed, a change promoted, tests reported passing. Not every `✓`.
    Pass,
    /// Harm: a failure that matters, a refusal, a rollback, PANIC, a ceiling hit, HALTED.
    Harm,
    /// Attention that is not harm: a ceiling at 80 %, a call with no result, a provisional body, a
    /// `warn` notice, a line not confirmed. Added beside the brand's eight because these exist and
    /// must not borrow yellow (the Root's) or red (harm). It has **no hue in any tier**: primary ink
    /// in bold, and the words or glyph beside it carry the meaning.
    Warn,
}

#[allow(dead_code)] // `ALL` and `mask` serve the goldens; `style` is API for the other render modules
impl Role {
    pub const ALL: [Role; 9] = [Role::Ink, Role::Dim, Role::Mute, Role::Line, Role::Root, Role::Rewrite, Role::Pass, Role::Harm, Role::Warn];

    /// The style for this role as a foreground (`Ink` is the plain style).
    pub const fn style(self) -> Style {
        Style::fg(self)
    }

    /// The golden-mask letter (TUI-1): `.` ink, `d` dim, `m` mute, `l` line, `a` root (amber),
    /// `w` rewrite, `p` pass, `h` harm, `n` warn.
    pub const fn mask(self) -> char {
        match self {
            Role::Ink => '.',
            Role::Dim => 'd',
            Role::Mute => 'm',
            Role::Line => 'l',
            Role::Root => 'a',
            Role::Rewrite => 'w',
            Role::Pass => 'p',
            Role::Harm => 'h',
            Role::Warn => 'n',
        }
    }
}

/// How many colours the terminal shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    TrueColor,
    Ansi256,
    Ansi16,
    /// `NO_COLOR`: attributes only (faint, bold, reverse); meaning is in glyphs and words.
    NoColor,
}

/// The terminal's background.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bg {
    Dark,
    Light,
}

/// What a role becomes on the terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Paint {
    /// The terminal's own foreground (no colour written).
    Default,
    Rgb(u8, u8, u8),
    /// An xterm palette index (0–255).
    Index(u8),
    /// SGR 2 (faint), for the grey roles where no exact colour is available.
    Faint,
    /// SGR 1 (bold), no colour.
    Bold,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    pub tier: Tier,
    pub bg: Bg,
}

impl Default for Palette {
    fn default() -> Palette {
        Palette::DEFAULT
    }
}

const fn rgb(hex: u32) -> Paint {
    Paint::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Palette {
    /// Before detection, and when nothing is known: xterm-256 on a dark background.
    pub const DEFAULT: Palette = Palette { tier: Tier::Ansi256, bg: Bg::Dark };

    /// The paint of `role` in this tier and background (docs/tui.md §3.9a has the same table).
    pub const fn paint(self, role: Role) -> Paint {
        use Bg::{Dark, Light};
        use Role::*;
        use Tier::*;
        match (self.tier, self.bg, role) {
            (_, _, Ink) => Paint::Default,
            (_, _, Warn) => Paint::Bold,
            // truecolor: the brand's night tokens on dark (n-mute lifted to 4.5:1, the hairline lifted
            // so it shows on common terminal backgrounds, not only on #0D0E11); darker text variants
            // (≥ 4.5:1 on white) on light
            (TrueColor, Dark, Dim) => rgb(0x8A96A3),
            (TrueColor, Dark, Mute) => rgb(0x6F7D8B),
            (TrueColor, Dark, Line) => rgb(0x3A3F47),
            (TrueColor, Dark, Root) => rgb(0xE6B450),
            (TrueColor, Dark, Rewrite) => rgb(0x56D4DD),
            (TrueColor, Dark, Pass) => rgb(0x3FB950),
            (TrueColor, Dark, Harm) => rgb(0xF85149),
            (TrueColor, Light, Dim) => rgb(0x5A6470),
            (TrueColor, Light, Mute) => rgb(0x75757D),
            (TrueColor, Light, Line) => rgb(0xC8CCD2),
            (TrueColor, Light, Root) => rgb(0xA16A0A),
            (TrueColor, Light, Rewrite) => rgb(0x0B8191),
            (TrueColor, Light, Pass) => rgb(0x178646),
            (TrueColor, Light, Harm) => rgb(0xDD353A),
            // xterm-256: the nearest indices (redmean), contrast on #0D0E11 / on white in the comments
            (Ansi256, Dark, Dim) => Paint::Index(246),     // #949494 6.36
            (Ansi256, Dark, Mute) => Paint::Index(244),    // #808080 4.89
            (Ansi256, Dark, Line) => Paint::Index(238),    // #444444 1.98 (hairline)
            (Ansi256, Dark, Root) => Paint::Index(179),    // #d7af5f 9.36
            (Ansi256, Dark, Rewrite) => Paint::Index(80),  // #5fd7d7 11.20
            (Ansi256, Dark, Pass) => Paint::Index(71),     // #5faf5f 7.15
            (Ansi256, Dark, Harm) => Paint::Index(203),    // #ff5f5f 6.48
            (Ansi256, Light, Dim) => Paint::Index(241),    // #626262 6.10
            (Ansi256, Light, Mute) => Paint::Index(243),   // #767676 4.54
            (Ansi256, Light, Line) => Paint::Index(251),   // #c6c6c6 1.71 (hairline)
            (Ansi256, Light, Root) => Paint::Index(130),   // #af5f00 4.71
            (Ansi256, Light, Rewrite) => Paint::Index(30), // #008787 4.36
            (Ansi256, Light, Pass) => Paint::Index(29),    // #00875f 4.53
            (Ansi256, Light, Harm) => Paint::Index(160),   // #d70000 5.40
            // 16 colours: the theme decides the RGB. Yellow is the Root's alone; bright slots on dark,
            // normal slots on light (bright yellow/cyan/green are near 1:1 on white)
            (Ansi16, _, Dim | Mute) => Paint::Faint,
            (Ansi16, Dark, Line) => Paint::Index(8),
            (Ansi16, Light, Line) => Paint::Index(7),
            (Ansi16, Dark, Root) => Paint::Index(11),
            (Ansi16, Dark, Rewrite) => Paint::Index(14),
            (Ansi16, Dark, Pass) => Paint::Index(10),
            (Ansi16, Dark, Harm) => Paint::Index(9),
            (Ansi16, Light, Root) => Paint::Index(3),
            (Ansi16, Light, Rewrite) => Paint::Index(6),
            (Ansi16, Light, Pass) => Paint::Index(2),
            (Ansi16, Light, Harm) => Paint::Index(1),
            // NO_COLOR: the greys are faint, harm is bold; the rest is the words and glyphs
            (NoColor, _, Dim | Mute | Line) => Paint::Faint,
            (NoColor, _, Harm) => Paint::Bold,
            (NoColor, _, Root | Rewrite | Pass) => Paint::Default,
        }
    }

    /// The tier and, when the environment says so, the background, from the environment alone:
    /// `NO_COLOR` (non-empty), `COLORTERM`, `TERM`, `COLORFGBG`.
    pub fn from_env(no_color: bool, term: Option<&str>, colorterm: Option<&str>, colorfgbg: Option<&str>) -> (Tier, Option<Bg>) {
        let tier = if no_color {
            Tier::NoColor
        } else if colorterm.is_some_and(|c| c.eq_ignore_ascii_case("truecolor") || c.eq_ignore_ascii_case("24bit")) {
            Tier::TrueColor
        } else if term.is_some_and(|t| t.contains("256color") || t.contains("direct")) {
            Tier::Ansi256
        } else {
            Tier::Ansi16
        };
        (tier, colorfgbg.and_then(bg_from_colorfgbg))
    }
}

/// `COLORFGBG` (rxvt, Konsole, some others): `fg;bg` or `fg;default;bg`, ANSI indices. Light when
/// the background is 7 or 15 (white or light grey), or 9–14 (bright colours); dark for 0–6 and 8.
pub fn bg_from_colorfgbg(v: &str) -> Option<Bg> {
    let bg: u8 = v.rsplit(';').next()?.trim().parse().ok()?;
    match bg {
        0..=6 | 8 => Some(Bg::Dark),
        7 | 9..=15 => Some(Bg::Light),
        _ => None,
    }
}

/// The background from an OSC 11 reply (`ESC ] 11 ; rgb:RRRR/GGGG/BBBB` ended by BEL or `ESC \`;
/// 1–4 hex digits per channel). Light when the colour's relative luminance is above 0.179, where
/// black text starts to contrast better than white (WCAG). None when there is no reply in `buf`.
pub fn bg_from_osc11(buf: &[u8]) -> Option<Bg> {
    // a reply may share the buffer with other bytes; only its ASCII part matters
    let s = String::from_utf8_lossy(buf);
    let at = s.find("\u{1b}]11;")?;
    let rest = &s[at + 5..];
    let end = rest.find(['\u{7}', '\u{1b}'])?;
    let body = rest[..end].trim();
    let spec = body.strip_prefix("rgb:").or_else(|| body.strip_prefix("rgba:"))?;
    let mut ch = spec.split('/');
    let mut chan = || -> Option<f64> {
        let h = ch.next()?;
        if h.is_empty() || h.len() > 4 {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        let max = (1u32 << (4 * h.len() as u32)) - 1;
        Some(f64::from(v) / f64::from(max))
    };
    let (r, g, b) = (chan()?, chan()?, chan()?);
    let lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    let y = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
    Some(if y > 0.179 { Bg::Light } else { Bg::Dark })
}

/// Did the primary device attributes reply (`ESC [ ? … c`) arrive? It ends the OSC 11 wait:
/// terminals answer in order, so an OSC 11 reply, if any, came before it.
pub fn has_da1_reply(buf: &[u8]) -> bool {
    let mut i = 0;
    while i + 2 < buf.len() {
        if buf[i] == 0x1b && buf[i + 1] == b'[' && buf[i + 2] == b'?' {
            let mut j = i + 3;
            while j < buf.len() && (buf[j].is_ascii_digit() || buf[j] == b';') {
                j += 1;
            }
            if j < buf.len() && buf[j] == b'c' {
                return true;
            }
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_tiers() {
        assert_eq!(Palette::from_env(true, Some("xterm-256color"), Some("truecolor"), None).0, Tier::NoColor);
        assert_eq!(Palette::from_env(false, Some("xterm-256color"), Some("truecolor"), None).0, Tier::TrueColor);
        assert_eq!(Palette::from_env(false, Some("xterm-256color"), Some("24bit"), None).0, Tier::TrueColor);
        assert_eq!(Palette::from_env(false, Some("xterm-256color"), None, None).0, Tier::Ansi256);
        assert_eq!(Palette::from_env(false, Some("tmux-256color"), None, None).0, Tier::Ansi256);
        assert_eq!(Palette::from_env(false, Some("xterm"), None, None).0, Tier::Ansi16);
        assert_eq!(Palette::from_env(false, None, None, None).0, Tier::Ansi16);
        assert_eq!(Palette::from_env(false, None, None, Some("0;15")).1, Some(Bg::Light));
        assert_eq!(Palette::from_env(false, None, None, Some("15;default;0")).1, Some(Bg::Dark));
        assert_eq!(Palette::from_env(false, None, None, Some("junk")).1, None);
    }

    #[test]
    fn osc11_replies() {
        assert_eq!(bg_from_osc11(b"\x1b]11;rgb:0d0d/0e0e/1111\x1b\\"), Some(Bg::Dark));
        assert_eq!(bg_from_osc11(b"\x1b]11;rgb:ffff/ffff/ffff\x07"), Some(Bg::Light));
        assert_eq!(bg_from_osc11(b"\x1b]11;rgb:fd/f6/e3\x1b\\\x1b[?62;22c"), Some(Bg::Light)); // solarized light
        assert_eq!(bg_from_osc11(b"\x1b]11;rgb:28/2c/34\x07"), Some(Bg::Dark)); // one dark
        assert_eq!(bg_from_osc11(b"\x1b]11;rgb:7/7/7\x07"), Some(Bg::Light)); // mid grey: black text wins
        assert_eq!(bg_from_osc11(b"\x1b]11;rgba:0000/0000/0000/ffff\x07"), Some(Bg::Dark));
        assert_eq!(bg_from_osc11(b"\x1b[?62;22c"), None);
        assert_eq!(bg_from_osc11(b"\x1b]11;rgb:zz/00/00\x07"), None);
        assert_eq!(bg_from_osc11(b"\x1b]11;rgb:00/00"), None);
    }

    #[test]
    fn da1() {
        assert!(has_da1_reply(b"\x1b]11;rgb:0/0/0\x07\x1b[?62;22c"));
        assert!(has_da1_reply(b"\x1b[?1;2c"));
        assert!(!has_da1_reply(b"\x1b[?1;2"));
        assert!(!has_da1_reply(b"\x1b[?u"));
    }

    #[test]
    fn root_is_the_only_yellow_and_warn_has_no_hue() {
        for tier in [Tier::TrueColor, Tier::Ansi256, Tier::Ansi16, Tier::NoColor] {
            for bg in [Bg::Dark, Bg::Light] {
                let p = Palette { tier, bg };
                assert_eq!(p.paint(Role::Ink), Paint::Default);
                assert_eq!(p.paint(Role::Warn), Paint::Bold, "warn borrows no colour");
                for r in Role::ALL {
                    if r != Role::Root {
                        assert!(!matches!(p.paint(r), Paint::Index(3 | 11)), "{r:?} uses the Root's yellow in {tier:?}/{bg:?}");
                    }
                }
                // every role paints differently from every other colour role in the colour tiers
                if tier != Tier::NoColor && tier != Tier::Ansi16 {
                    for a in [Role::Dim, Role::Mute, Role::Line, Role::Root, Role::Rewrite, Role::Pass, Role::Harm] {
                        for b in [Role::Dim, Role::Mute, Role::Line, Role::Root, Role::Rewrite, Role::Pass, Role::Harm] {
                            assert!(a == b || p.paint(a) != p.paint(b), "{a:?} = {b:?} in {tier:?}/{bg:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn styles_and_masks() {
        assert_eq!(Role::Ink.style(), Style::PLAIN);
        assert_eq!(Role::Root.style().fg, Some(Role::Root));
        let m: String = Role::ALL.iter().map(|r| r.mask()).collect();
        assert_eq!(m, ".dmlawphn");
    }
}
