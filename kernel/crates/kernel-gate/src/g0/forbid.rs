//! The `#![forbid(unsafe_code)]` check on a target root (gate.md §3.3 step 3; CONVENTIONS §5).
//!
//! A hand-written lexer, as rustc reads the start of a file: skip a UTF-8 BOM, an optional shebang
//! line (`#!` not followed by `[`, rustc_lexer's `strip_shebang` rule), then whitespace and comments
//! (line, doc, and block comments, which nest). The first inner attribute must then be
//! `#![forbid(...)]` whose list contains `unsafe_code`. Anything else fails closed:
//! `#![cfg_attr(.., forbid(unsafe_code))]`, a forbid inside a comment, `#![deny(unsafe_code)]`, or an
//! attribute that comes after another one.

/// `Ok(())` if `src` starts with an inner `#![forbid(..)]` listing `unsafe_code`; else the reason.
pub fn check(src: &str) -> Result<(), String> {
    let s = src.strip_prefix('\u{feff}').unwrap_or(src);
    let mut lx = Lexer {
        s: s.as_bytes(),
        text: s,
        i: 0,
    };
    lx.skip_shebang()?;
    lx.skip_trivia()?;
    if lx.at_end() {
        return Err("no `#![forbid(unsafe_code)]`: the file has no items".into());
    }
    lx.expect(b'#', "the file does not start with an inner attribute")?;
    lx.skip_trivia()?;
    if lx.peek() != Some(b'!') {
        return Err(
            "the first attribute is an outer attribute, not `#![forbid(unsafe_code)]`".into(),
        );
    }
    lx.i += 1;
    lx.skip_trivia()?;
    lx.expect(b'[', "malformed inner attribute")?;
    lx.skip_trivia()?;
    let name = lx.ident();
    if name != "forbid" {
        return Err(format!(
            "the first inner attribute is `#![{}..]`, not `#![forbid(unsafe_code)]`",
            if name.is_empty() { "?" } else { name }
        ));
    }
    lx.skip_trivia()?;
    lx.expect(b'(', "malformed `#![forbid(..)]`")?;
    let mut lints: Vec<String> = Vec::new();
    loop {
        lx.skip_trivia()?;
        match lx.peek() {
            Some(b')') => {
                lx.i += 1;
                break;
            }
            None => return Err("unterminated `#![forbid(..)]`".into()),
            _ => {}
        }
        let path = lx.path()?;
        lx.skip_trivia()?;
        if lx.peek() == Some(b'=') {
            // `reason = "..."` (lint reasons, stable since Rust 1.81)
            lx.i += 1;
            lx.skip_trivia()?;
            lx.string_literal()?;
            lx.skip_trivia()?;
        } else {
            lints.push(path);
        }
        match lx.peek() {
            Some(b',') => lx.i += 1,
            Some(b')') => {}
            _ => return Err("malformed `#![forbid(..)]` list".into()),
        }
    }
    lx.skip_trivia()?;
    lx.expect(b']', "malformed `#![forbid(..)]`")?;
    if lints.iter().any(|l| l == "unsafe_code") {
        Ok(())
    } else {
        Err(format!(
            "the first inner attribute is `#![forbid({})]`, which does not list `unsafe_code`",
            lints.join(", ")
        ))
    }
}

struct Lexer<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
}

impl<'a> Lexer<'a> {
    fn at_end(&self) -> bool {
        self.i >= self.s.len()
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn rest(&self) -> &[u8] {
        &self.s[self.i.min(self.s.len())..]
    }

    fn expect(&mut self, b: u8, err: &str) -> Result<(), String> {
        if self.peek() == Some(b) {
            self.i += 1;
            Ok(())
        } else {
            Err(err.to_string())
        }
    }

    /// rustc_lexer `strip_shebang`: `#!` starts a shebang line unless the next token that is not
    /// whitespace or a non-doc comment is `[`.
    fn skip_shebang(&mut self) -> Result<(), String> {
        if !self.rest().starts_with(b"#!") {
            return Ok(());
        }
        let mut probe = Lexer {
            s: self.s,
            text: self.text,
            i: self.i + 2,
        };
        let is_attr = match probe.skip(false) {
            Ok(()) => probe.peek() == Some(b'['),
            Err(_) => false,
        };
        if !is_attr {
            while let Some(c) = self.peek() {
                if c == b'\n' {
                    break;
                }
                self.i += 1;
            }
        }
        Ok(())
    }

    /// Skips whitespace and every kind of comment.
    fn skip_trivia(&mut self) -> Result<(), String> {
        self.skip(true)
    }

    /// Skips whitespace and comments; doc comments only if `docs`.
    fn skip(&mut self, docs: bool) -> Result<(), String> {
        loop {
            let r = self.rest();
            if let Some(n) = whitespace_len(self.text.get(self.i..).unwrap_or("")) {
                self.i += n;
            } else if r.starts_with(b"//") {
                let doc =
                    (r.starts_with(b"///") && !r.starts_with(b"////")) || r.starts_with(b"//!");
                if doc && !docs {
                    return Ok(());
                }
                while let Some(c) = self.peek() {
                    if c == b'\n' {
                        break;
                    }
                    self.i += 1;
                }
            } else if r.starts_with(b"/*") {
                let doc =
                    (r.starts_with(b"/**") && !r.starts_with(b"/***") && !r.starts_with(b"/**/"))
                        || r.starts_with(b"/*!");
                if doc && !docs {
                    return Ok(());
                }
                self.block_comment()?;
            } else {
                return Ok(());
            }
        }
    }

    /// Skips a (nested) block comment starting at `/*`.
    fn block_comment(&mut self) -> Result<(), String> {
        self.i += 2;
        let mut depth = 1usize;
        while depth > 0 {
            let r = self.rest();
            if r.is_empty() {
                return Err("unterminated block comment".into());
            }
            if r.starts_with(b"/*") {
                depth += 1;
                self.i += 2;
            } else if r.starts_with(b"*/") {
                depth -= 1;
                self.i += 2;
            } else {
                self.i += 1;
            }
        }
        Ok(())
    }

    /// An ASCII identifier (possibly empty). Lint names are ASCII.
    fn ident(&mut self) -> &'a str {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.i += 1;
            } else {
                break;
            }
        }
        &self.text[start..self.i]
    }

    /// A lint path `a::b`, with trivia allowed around `::`; returned without whitespace.
    fn path(&mut self) -> Result<String, String> {
        let mut out = String::new();
        loop {
            let id = self.ident();
            if id.is_empty() {
                return Err("malformed `#![forbid(..)]` list".into());
            }
            out.push_str(id);
            let save = self.i;
            self.skip_trivia()?;
            if self.rest().starts_with(b"::") {
                self.i += 2;
                self.skip_trivia()?;
                out.push_str("::");
            } else {
                self.i = save;
                return Ok(out);
            }
        }
    }

    /// A string literal: `"..."` with escapes, or raw `r"..."` / `r#"..."#`.
    fn string_literal(&mut self) -> Result<(), String> {
        let err = || "malformed `reason = ..` in `#![forbid(..)]`".to_string();
        match self.peek() {
            Some(b'"') => {
                self.i += 1;
                loop {
                    match self.peek() {
                        None => return Err(err()),
                        Some(b'\\') => self.i += 2,
                        Some(b'"') => {
                            self.i += 1;
                            return Ok(());
                        }
                        Some(_) => self.i += 1,
                    }
                }
            }
            Some(b'r') => {
                self.i += 1;
                let mut hashes = 0usize;
                while self.peek() == Some(b'#') {
                    hashes += 1;
                    self.i += 1;
                }
                if self.peek() != Some(b'"') {
                    return Err(err());
                }
                self.i += 1;
                let mut close = vec![b'"'];
                close.extend(std::iter::repeat_n(b'#', hashes));
                loop {
                    if self.at_end() {
                        return Err(err());
                    }
                    if self.rest().starts_with(&close) {
                        self.i += close.len();
                        return Ok(());
                    }
                    self.i += 1;
                }
            }
            _ => Err(err()),
        }
    }
}

/// Length in bytes of one Rust whitespace character (`Pattern_White_Space`) at the start of `s`.
fn whitespace_len(s: &str) -> Option<usize> {
    let c = s.chars().next()?;
    matches!(
        c,
        '\t' | '\n'
            | '\u{000B}'
            | '\u{000C}'
            | '\r'
            | ' '
            | '\u{0085}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{2028}'
            | '\u{2029}'
    )
    .then(|| c.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::check;

    fn ok(s: &str) {
        assert_eq!(check(s), Ok(()), "should pass: {s:?}");
    }

    fn bad(s: &str) -> String {
        match check(s) {
            Ok(()) => panic!("should fail: {s:?}"),
            Err(e) => e,
        }
    }

    #[test]
    fn plain_and_spacing() {
        ok("#![forbid(unsafe_code)]\nfn main() {}\n");
        ok("#![forbid(unsafe_code)]");
        ok("# ! [ forbid ( unsafe_code ) ]\n");
        ok("#!\n[\nforbid\n(\nunsafe_code\n,\n)\n]");
        ok("\t\r\n  #![forbid(missing_docs, unsafe_code)]");
        ok("#![forbid(unsafe_code, missing_docs)]");
        ok("#![forbid(unsafe_code,)]");
        ok("#![forbid(clippy::all, unsafe_code)]");
        ok("#![forbid(unsafe_code, reason = \"body (A6) ) ] \\\" safe\")]");
        ok("#![forbid(unsafe_code, reason = r#\"raw \" ) ]\"#)]");
        ok("#![forbid(/* inline */ unsafe_code /* x */)]");
        ok("#![forbid(clippy :: all, unsafe_code)]");
    }

    #[test]
    fn bom_and_shebang() {
        ok("\u{feff}#![forbid(unsafe_code)]");
        ok("#!/usr/bin/env run-cargo-script\n#![forbid(unsafe_code)]\n");
        ok("\u{feff}#!/bin/sh\n#![forbid(unsafe_code)]\n");
        // `#!` followed by `[` (after whitespace or a plain comment) is an attribute, not a shebang
        ok("#! [forbid(unsafe_code)]");
        ok("#!/* c */[forbid(unsafe_code)]");
        ok("#!// c\n[forbid(unsafe_code)]");
        // a shebang line hides whatever is on it
        bad("#!/bin/sh #![forbid(unsafe_code)]\nfn main() {}\n");
        bad("#!/bin/sh\nfn main() {}\n");
        // BOM only at the very start
        bad("  \u{feff}#![forbid(unsafe_code)]");
    }

    #[test]
    fn comments_are_skipped() {
        ok("// line\n#![forbid(unsafe_code)]");
        ok("//! crate docs\n//! more\n#![forbid(unsafe_code)]");
        ok("/// outer doc\n#![forbid(unsafe_code)]");
        ok("/* block */#![forbid(unsafe_code)]");
        ok("/*! inner block doc */\n#![forbid(unsafe_code)]");
        ok("/** outer block doc */\n#![forbid(unsafe_code)]");
        ok("/* a /* nested */ still comment */\n#![forbid(unsafe_code)]");
        ok("/* /* /* deep */ */ */#![forbid(unsafe_code)]");
        ok("/**/#![forbid(unsafe_code)]");
        ok("/***/\n#![forbid(unsafe_code)]");
        ok("// a\n/* b */ //! c\n  /*! d /* e */ */\n#![forbid(unsafe_code)]");
    }

    #[test]
    fn forbid_in_a_comment_fails() {
        bad("// #![forbid(unsafe_code)]\nfn main() {}");
        bad("//! #![forbid(unsafe_code)]\nfn main() {}");
        bad("/* #![forbid(unsafe_code)] */ fn main() {}");
        // the nesting matters: after the first `*/` we are still inside the outer comment
        let e = bad("/* /* */ #![forbid(unsafe_code)] */");
        assert!(e.contains("no items"), "{e}");
        bad("/* /* */ #![forbid(unsafe_code)]");
        bad("/*! #![forbid(unsafe_code)] */");
    }

    #[test]
    fn wrong_attributes_fail() {
        bad("");
        bad("\n\n");
        bad("fn main() {}");
        let e = bad("#![cfg_attr(all(), forbid(unsafe_code))]");
        assert!(e.contains("cfg_attr"), "{e}");
        let e = bad("#![deny(unsafe_code)]");
        assert!(e.contains("deny"), "{e}");
        bad("#![forbid(missing_docs)]");
        bad("#![forbid(unsafe_codes)]");
        bad("#![forbid(clippy::unsafe_code)]");
        bad("#![forbid()]");
        bad("#![forbid]");
        bad("#![forbid = \"unsafe_code\"]");
        bad("#[forbid(unsafe_code)]\nfn main() {}");
        bad("#![allow(dead_code)]\n#![forbid(unsafe_code)]");
        bad("#![doc = \"x\"]\n#![forbid(unsafe_code)]");
        bad("#![r#forbid(unsafe_code)]");
        bad("#![forbid(unsafe_code)");
        bad("#![forbid(unsafe_code");
        bad("#![forbid(unsafe_code reason)]");
        bad("#![forbid(unsafe_code, reason = )]");
        bad("#![forbid(unsafe_code, reason = \"open)]");
        bad("\u{a0}#![forbid(unsafe_code)]");
    }

    #[test]
    fn unterminated_comment_fails() {
        let e = bad("/* never closed #![forbid(unsafe_code)]");
        assert!(e.contains("unterminated"), "{e}");
        bad("/* /* */");
    }
}
