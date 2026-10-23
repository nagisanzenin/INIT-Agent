//! `@init compensates-for` markers (selfmap.md §3.7): the value grammar and the non-Rust marker forms.
//!
//! ```abnf
//! value  = "none" [ SP "(" 1*200text ")" ] / slug [ ":" SP 1*200text ]
//! slug   = 3*40( %x61-7A / DIGIT / "-" )
//! ```
//!
//! `text` is any Unicode scalar except control characters, counted in characters (the same reading as
//! the gate's trailer grammar, `kernel-gate` `evo::grammar::is_text`).
//!
//! Rust markers are doc attributes and are found by [`crate::rust_scan`]. This module handles
//! `SKILL.md` front matter and the `<!-- @init compensates-for: … -->` … `<!-- @init end -->` blocks of
//! `GENESIS.md` and `SELF.md`.

/// The text that introduces a marker in a doc comment.
pub const DOC_PREFIX: &str = "@init compensates-for:";
/// What a line must contain to count as a marker attempt (for `marker_not_doc`).
pub const NEEDLE: &str = "@init compensates-for";

/// A parsed marker value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    /// `none` or a slug.
    pub slug: String,
    /// The text in parentheses (`none`) or after `": "` (slug).
    pub note: Option<String>,
}

fn is_slug(s: &str) -> bool {
    (3..=40).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn is_text(s: &str) -> bool {
    let n = s.chars().count();
    (1..=200).contains(&n) && !s.chars().any(char::is_control)
}

/// Parses a marker value (the text after `compensates-for:` with surrounding whitespace trimmed).
pub fn parse_value(raw: &str) -> Result<Value, String> {
    let s = raw.trim();
    if s == "none" {
        return Ok(Value {
            slug: "none".into(),
            note: None,
        });
    }
    if let Some(rest) = s.strip_prefix("none ") {
        if let Some(note) = rest.strip_prefix('(').and_then(|r| r.strip_suffix(')'))
            && is_text(note)
        {
            return Ok(Value {
                slug: "none".into(),
                note: Some(note.into()),
            });
        }
        return Err(format!(
            "`{s}`: after `none` only ` (<1-200 characters>)` may follow"
        ));
    }
    let (slug, note) = match s.split_once(':') {
        Some((a, b)) => match b.strip_prefix(' ') {
            Some(n) if is_text(n) => (a, Some(n.to_string())),
            _ => {
                return Err(format!(
                    "`{s}`: the note after `<slug>:` must be one space and 1-200 characters"
                ));
            }
        },
        None => (s, None),
    };
    if slug == "none" || !is_slug(slug) {
        return Err(format!(
            "`{s}`: expected `none [(<note>)]` or `<slug>[: <note>]` with a slug of 3-40 of a-z, 0-9, -"
        ));
    }
    Ok(Value {
        slug: slug.into(),
        note,
    })
}

/// A marker found in a Markdown file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdMarker {
    /// 1-based first and last line of the marked region.
    pub line: usize,
    pub end_line: usize,
    /// The raw value text.
    pub raw: String,
    /// The marked text (for prompt blocks: the lines from the opening to the closing comment).
    pub text: String,
}

/// `SKILL.md` front matter: the single-line `name:`, `description:` and `compensates-for:` values
/// between the leading `---` lines (body/log-context.md §3.8.2, selfmap.md §3.7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrontMatter {
    pub present: bool,
    pub name: Option<String>,
    pub description: Option<String>,
    /// Line and raw value of `compensates-for:`.
    pub compensates: Option<(usize, String)>,
}

/// Reads the front matter of a `SKILL.md`.
pub fn front_matter(text: &str) -> FrontMatter {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines().enumerate();
    let mut fm = FrontMatter::default();
    match lines.next() {
        Some((_, l)) if l.trim_end() == "---" => {}
        _ => return fm,
    }
    let mut got = FrontMatter {
        present: true,
        ..FrontMatter::default()
    };
    for (i, l) in lines {
        if l.trim_end() == "---" {
            fm = got;
            return fm;
        }
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        let v = v.trim().to_string();
        match k.trim() {
            "name" => got.name = Some(v),
            "description" => got.description = Some(v),
            "compensates-for" => got.compensates = Some((i + 1, v)),
            _ => {}
        }
    }
    fm // unterminated front matter counts as none
}

const OPEN: &str = "<!-- @init compensates-for:";
const END: &str = "<!-- @init end -->";

/// The `<!-- @init compensates-for: <value> -->` … `<!-- @init end -->` blocks of a prompt file.
/// Errors (unterminated, nested, stray end, malformed opening) are returned with their line.
pub fn prompt_blocks(text: &str) -> (Vec<MdMarker>, Vec<(usize, String)>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut errors = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix(OPEN) {
            if let Some((at, _)) = &open {
                errors.push((
                    i + 1,
                    format!("nested marker block (the block at line {at} is still open)"),
                ));
                continue;
            }
            match rest.strip_suffix("-->") {
                Some(v) => open = Some((i + 1, v.trim().to_string())),
                None => errors.push((
                    i + 1,
                    "the opening comment must end with `-->` on the same line".into(),
                )),
            }
        } else if t == END {
            match open.take() {
                Some((at, raw)) => blocks.push(MdMarker {
                    line: at,
                    end_line: i + 1,
                    raw,
                    text: lines[at - 1..=i].join("\n"),
                }),
                None => errors.push((i + 1, "`<!-- @init end -->` without an open block".into())),
            }
        } else if t.contains(NEEDLE) {
            errors.push((
                i + 1,
                "malformed marker: expected `<!-- @init compensates-for: <value> -->`".into(),
            ));
        }
    }
    if let Some((at, _)) = open {
        errors.push((
            at,
            "unterminated marker block: no `<!-- @init end -->`".into(),
        ));
    }
    (blocks, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_grammar() {
        assert_eq!(
            parse_value("none").unwrap(),
            Value {
                slug: "none".into(),
                note: None
            }
        );
        assert_eq!(
            parse_value(" none (new capability) ").unwrap(),
            Value {
                slug: "none".into(),
                note: Some("new capability".into())
            }
        );
        assert_eq!(
            parse_value("loops-on-repl: repeats the same command").unwrap(),
            Value {
                slug: "loops-on-repl".into(),
                note: Some("repeats the same command".into())
            }
        );
        assert_eq!(parse_value("abc").unwrap().slug, "abc");
        for bad in [
            "",
            "ab",
            "Upper",
            "none ()",
            "none (x",
            "slug:no-space",
            "slug: ",
            "a_b_c",
            &"x".repeat(41),
            "none: x",
            "loop:  \u{7}",
        ] {
            assert!(parse_value(bad).is_err(), "{bad:?}");
        }
        assert!(parse_value(&format!("abc: {}", "y".repeat(200))).is_ok());
        assert!(parse_value(&format!("abc: {}", "y".repeat(201))).is_err());
    }

    #[test]
    fn skill_front_matter() {
        let fm = front_matter(
            "---\nname: repl\ndescription: Drive a REPL.\ncompensates-for: loops-on-repl\n---\n# Body\n",
        );
        assert!(fm.present);
        assert_eq!(fm.name.as_deref(), Some("repl"));
        assert_eq!(fm.description.as_deref(), Some("Drive a REPL."));
        assert_eq!(fm.compensates, Some((4, "loops-on-repl".into())));
        assert!(!front_matter("# no front matter\n").present);
        assert!(!front_matter("---\nname: x\n").present, "unterminated");
    }

    #[test]
    fn prompt_block_forms() {
        let t = "intro\n<!-- @init compensates-for: loop-detection: repeats -->\nIf you repeat, stop.\n<!-- @init end -->\nend\n";
        let (b, e) = prompt_blocks(t);
        assert!(e.is_empty(), "{e:?}");
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].line, b[0].end_line), (2, 4));
        assert_eq!(b[0].raw, "loop-detection: repeats");
        assert!(b[0].text.starts_with("<!-- @init") && b[0].text.ends_with("end -->"));
        let (_, e) = prompt_blocks("<!-- @init compensates-for: abc -->\nx\n");
        assert_eq!(e[0].0, 1);
        let (_, e) = prompt_blocks("<!-- @init end -->\n");
        assert_eq!(e.len(), 1);
        let (_, e) = prompt_blocks(
            "<!-- @init compensates-for: abc -->\n<!-- @init compensates-for: abd -->\n<!-- @init end -->\n",
        );
        assert_eq!(e.len(), 1);
    }
}
