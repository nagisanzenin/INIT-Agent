//! The Rust scan (selfmap.md §3.3): syn 3.0.9 over one source file.
//!
//! - top-level items with kind, name, visibility, lines and tokens (`use` items are skipped);
//! - `#![forbid(unsafe_code)]`: an inner `forbid` attribute whose list names `unsafe_code`
//!   (`cfg_attr(…, forbid(unsafe_code))` does not count);
//! - `@init compensates-for` markers: inner doc (`//!`) markers for the file, outer doc (`///`) markers on
//!   top-level items. Doc comments reach syn as `doc` attributes, so no comment parser is needed. A line
//!   that mentions `@init compensates-for` anywhere else is reported as `marker_not_doc`.

use std::collections::BTreeSet;

use proc_macro2::Span;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{AttrStyle, Attribute, Expr, Item, Lit, Meta, Token, Type, Visibility};

use crate::markers::{DOC_PREFIX, NEEDLE};
use crate::tokens::Tokenizer;

/// One top-level item (selfmap.md §4.2 `module.items`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemInfo {
    pub kind: &'static str,
    pub name: String,
    pub vis: &'static str,
    pub line: usize,
    pub end_line: usize,
    pub tokens: u64,
}

/// A doc-comment marker, before attribution to a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustMarker {
    /// Line of the marker comment itself.
    pub marker_line: usize,
    /// The raw value after `compensates-for:`.
    pub raw: String,
    /// `None` for a file (`//!`) marker; the item's name for an item (`///`) marker.
    pub item: Option<String>,
    /// The marked region: the whole file, or the item.
    pub line: usize,
    pub end_line: usize,
    pub tokens: u64,
}

/// The scan of one file.
#[derive(Debug, Clone, Default)]
pub struct FileScan {
    pub items: Vec<ItemInfo>,
    pub forbid_unsafe: bool,
    pub markers: Vec<RustMarker>,
    /// Lines that mention `@init compensates-for` outside a file or top-level item doc comment.
    pub not_doc: Vec<usize>,
    /// Line and message of a parse error.
    pub parse_error: Option<(usize, String)>,
    pub lines: usize,
    pub tokens: u64,
}

fn line_of(s: Span) -> usize {
    s.start().line
}

/// Lines `l0..=l1` (1-based) of `text`, joined with `\n`.
pub fn slice_lines(text: &str, l0: usize, l1: usize) -> String {
    text.lines()
        .skip(l0.saturating_sub(1))
        .take(l1.saturating_sub(l0) + 1)
        .collect::<Vec<_>>()
        .join("\n")
}

fn vis_of(v: &Visibility) -> &'static str {
    match v {
        Visibility::Public(_) => "pub",
        Visibility::Restricted(r) if r.in_token.is_none() && r.path.is_ident("crate") => "crate",
        Visibility::Restricted(_) => "restricted",
        Visibility::Inherited => "private",
    }
}

fn path_text(p: &syn::Path) -> String {
    p.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn type_text(t: &Type) -> String {
    match t {
        Type::Path(p) => path_text(&p.path),
        Type::Reference(r) => format!("&{}", type_text(&r.elem)),
        Type::Slice(s) => format!("[{}]", type_text(&s.elem)),
        Type::Array(a) => format!("[{}; _]", type_text(&a.elem)),
        Type::Tuple(t) if t.elems.is_empty() => "()".into(),
        Type::Paren(p) => type_text(&p.elem),
        _ => "_".into(),
    }
}

/// `(kind, name, vis, attrs)`; `None` for `use` items, which are not inventoried.
fn describe_item(it: &Item) -> Option<(&'static str, String, &'static str, &[Attribute])> {
    Some(match it {
        Item::Fn(f) => ("fn", f.sig.ident.to_string(), vis_of(&f.vis), &f.attrs),
        Item::Struct(s) => ("struct", s.ident.to_string(), vis_of(&s.vis), &s.attrs),
        Item::Enum(e) => ("enum", e.ident.to_string(), vis_of(&e.vis), &e.attrs),
        Item::Trait(t) => ("trait", t.ident.to_string(), vis_of(&t.vis), &t.attrs),
        Item::Union(u) => ("union", u.ident.to_string(), vis_of(&u.vis), &u.attrs),
        Item::Mod(m) => ("mod", m.ident.to_string(), vis_of(&m.vis), &m.attrs),
        Item::Const(c) => ("const", c.ident.to_string(), vis_of(&c.vis), &c.attrs),
        Item::Static(s) => ("static", s.ident.to_string(), vis_of(&s.vis), &s.attrs),
        Item::Type(t) => ("type", t.ident.to_string(), vis_of(&t.vis), &t.attrs),
        Item::Impl(i) => {
            let name = match &i.trait_ {
                Some((p, _)) => format!("impl {} for {}", path_text(p), type_text(&i.self_ty)),
                None => format!("impl {}", type_text(&i.self_ty)),
            };
            ("impl", name, "private", &i.attrs)
        }
        Item::Macro(m) => {
            let name = match &m.ident {
                Some(id) => id.to_string(),
                None => format!("{}!", path_text(&m.mac.path)),
            };
            ("macro", name, "private", &m.attrs)
        }
        Item::Use(_) => return None,
        Item::ExternCrate(e) => (
            "other",
            format!("extern crate {}", e.ident),
            vis_of(&e.vis),
            &e.attrs,
        ),
        Item::ForeignMod(f) => ("other", "extern block".into(), "private", &f.attrs),
        Item::TraitAlias(t) => ("other", t.ident.to_string(), vis_of(&t.vis), &t.attrs),
        _ => ("other", "verbatim".into(), "private", &[]),
    })
}

/// `(line, raw value)` of every marker line in the `doc` attributes of `attrs` with the given style.
fn doc_markers(attrs: &[Attribute], inner: bool) -> Vec<(usize, String)> {
    let mut v = Vec::new();
    for a in attrs {
        if matches!(a.style, AttrStyle::Inner(_)) != inner || !a.path().is_ident("doc") {
            continue;
        }
        let Meta::NameValue(nv) = &a.meta else {
            continue;
        };
        let Expr::Lit(lit) = &nv.value else { continue };
        let Lit::Str(s) = &lit.lit else { continue };
        let base = line_of(a.span());
        for (i, l) in s.value().lines().enumerate() {
            if let Some(rest) = l.trim().strip_prefix(DOC_PREFIX) {
                v.push((base + i, rest.trim().to_string()));
            }
        }
    }
    v
}

/// True if `attrs` hold an inner `#![forbid(...)]` whose list names `unsafe_code`.
pub fn has_forbid_unsafe(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        matches!(a.style, AttrStyle::Inner(_))
            && a.path().is_ident("forbid")
            && match &a.meta {
                Meta::List(list) => list
                    .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                    .map(|metas| {
                        metas
                            .iter()
                            .any(|m| matches!(m, Meta::Path(p) if p.is_ident("unsafe_code")))
                    })
                    .unwrap_or(false),
                _ => false,
            }
    })
}

/// Scans one Rust file. A parse error is returned in `parse_error` (G1 `selfmap_error`).
pub fn scan_rust(text: &str, tok: &Tokenizer) -> FileScan {
    let mut fs = FileScan {
        lines: text.lines().count(),
        tokens: tok.count(text),
        ..FileScan::default()
    };
    let file = match syn::parse_file(text) {
        Ok(f) => f,
        Err(e) => {
            fs.parse_error = Some((line_of(e.span()), e.to_string()));
            return fs;
        }
    };
    fs.forbid_unsafe = has_forbid_unsafe(&file.attrs);
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    for (line, raw) in doc_markers(&file.attrs, true) {
        seen.insert(line);
        fs.markers.push(RustMarker {
            marker_line: line,
            raw,
            item: None,
            line: 1,
            end_line: fs.lines.max(1),
            tokens: fs.tokens,
        });
    }
    for it in &file.items {
        let Some((kind, name, vis, attrs)) = describe_item(it) else {
            continue;
        };
        let sp = it.span();
        let (l0, l1) = (sp.start().line, sp.end().line);
        let tokens = tok.count(&slice_lines(text, l0, l1));
        for (line, raw) in doc_markers(attrs, false) {
            seen.insert(line);
            fs.markers.push(RustMarker {
                marker_line: line,
                raw,
                item: Some(name.clone()),
                line: l0,
                end_line: l1,
                tokens,
            });
        }
        fs.items.push(ItemInfo {
            kind,
            name,
            vis,
            line: l0,
            end_line: l1,
            tokens,
        });
    }
    fs.items.sort_by_key(|i| (i.line, i.end_line));
    for (i, l) in text.lines().enumerate() {
        if l.contains(NEEDLE) && !seen.contains(&(i + 1)) {
            fs.not_doc.push(i + 1);
        }
    }
    fs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(t: &str) -> FileScan {
        scan_rust(t, &Tokenizer::o200k())
    }

    /// S-05: present, absent, with another lint, inside `cfg_attr` (not accepted).
    #[test]
    fn s05_forbid_unsafe_detection() {
        assert!(scan("#![forbid(unsafe_code)]\nfn main() {}\n").forbid_unsafe);
        assert!(!scan("fn main() {}\n").forbid_unsafe);
        assert!(scan("#![forbid(unsafe_code, missing_docs)]\nfn main() {}\n").forbid_unsafe);
        assert!(
            !scan("#![cfg_attr(not(test), forbid(unsafe_code))]\nfn main() {}\n").forbid_unsafe
        );
        assert!(!scan("#![deny(unsafe_code)]\n").forbid_unsafe);
        assert!(!scan("// #![forbid(unsafe_code)]\n").forbid_unsafe);
        assert!(
            scan("//! doc\n#![allow(dead_code)]\n#![forbid(unsafe_code, reason = \"x\")]\n")
                .forbid_unsafe
        );
    }

    #[test]
    fn items_kinds_vis_lines_tokens() {
        let src = "\
use std::fmt;

/// A thing.
pub struct A { x: u32 }

pub(crate) fn f() -> u32 {
    1
}

impl fmt::Display for A {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, \"a\") }
}
impl A {}
pub(super) const C: u32 = 1;
static S: u8 = 0;
macro_rules! m { () => {} }
mod inner { fn g() {} }
type T = Vec<u8>;
enum E { X }
pub trait Tr {}
";
        let s = scan(src);
        assert!(s.parse_error.is_none());
        let got: Vec<(&str, &str, &str, usize, usize)> = s
            .items
            .iter()
            .map(|i| (i.kind, i.name.as_str(), i.vis, i.line, i.end_line))
            .collect();
        assert_eq!(
            got,
            [
                ("struct", "A", "pub", 3, 4),
                ("fn", "f", "crate", 6, 8),
                ("impl", "impl fmt::Display for A", "private", 10, 12),
                ("impl", "impl A", "private", 13, 13),
                ("const", "C", "restricted", 14, 14),
                ("static", "S", "private", 15, 15),
                ("macro", "m", "private", 16, 16),
                ("mod", "inner", "private", 17, 17),
                ("type", "T", "private", 18, 18),
                ("enum", "E", "private", 19, 19),
                ("trait", "Tr", "pub", 20, 20),
            ]
        );
        let t = Tokenizer::o200k();
        assert_eq!(
            s.items[1].tokens,
            t.count("pub(crate) fn f() -> u32 {\n    1\n}")
        );
        assert_eq!(s.lines, 20);
    }

    #[test]
    fn markers_file_item_and_not_doc() {
        let src = "\
//! The loop guard.
//! @init compensates-for: loops-on-repl: repeats the same command

/// @init compensates-for: loops-on-repl
pub fn guard() {}

// @init compensates-for: other-slug
fn plain() {}

impl X {
    /// @init compensates-for: nested-one
    fn m() {}
}
";
        let s = scan(src);
        assert!(s.parse_error.is_none());
        assert_eq!(s.markers.len(), 2);
        assert_eq!(s.markers[0].marker_line, 2);
        assert_eq!(s.markers[0].item, None);
        assert_eq!(s.markers[0].raw, "loops-on-repl: repeats the same command");
        assert_eq!((s.markers[0].line, s.markers[0].end_line), (1, 13));
        assert_eq!(s.markers[1].marker_line, 4);
        assert_eq!(s.markers[1].item.as_deref(), Some("guard"));
        assert_eq!((s.markers[1].line, s.markers[1].end_line), (4, 5));
        // A `//` comment marker and a nested item's doc marker are not seen as markers.
        assert_eq!(s.not_doc, [7, 11]);
    }

    #[test]
    fn parse_error_has_a_line() {
        let s = scan("fn ok() {}\n\nfn broken( {\n");
        let (line, msg) = s.parse_error.unwrap();
        assert!(line >= 3, "{line}: {msg}");
    }
}
