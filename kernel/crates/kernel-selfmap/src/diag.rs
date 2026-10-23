//! Diagnostics: stderr JSON lines `{"code","message","path"?,"line"?}` (selfmap.md §4.1) and the codes of
//! selfmap.md §3.8. The gate prefixes a code with `gate.g1.` (or `gate.k0.` for the kernel tree).

use serde_json::{Map, Value};

/// Diagnostic messages are cut at this many bytes (a trap message carries a wasm backtrace).
pub const MESSAGE_MAX: usize = 4096;

/// How a diagnostic affects the exit code (selfmap.md §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A G1 check failed; the file is still generated (exit 1).
    Check,
    /// The generator could not produce a trustworthy file: parse, manifest or component (exit 4).
    Error,
}

/// One diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub code: &'static str,
    pub message: String,
    pub path: Option<String>,
    pub line: Option<usize>,
}

impl Diag {
    /// A diagnostic; the message is cut at [`MESSAGE_MAX`] bytes.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > MESSAGE_MAX {
            let mut cut = MESSAGE_MAX - 4;
            while !message.is_char_boundary(cut) {
                cut -= 1;
            }
            message.truncate(cut);
            message.push_str(" …");
        }
        Diag {
            code,
            message,
            path: None,
            line: None,
        }
    }

    pub fn at(mut self, path: &str, line: Option<usize>) -> Self {
        self.path = Some(path.to_string());
        self.line = line;
        self
    }

    /// The exit-code class of this code.
    pub fn class(&self) -> Class {
        match self.code {
            SELFMAP_ERROR | MANIFEST_ERROR | EXT_CONTRACT => Class::Error,
            _ => Class::Check,
        }
    }

    /// The stderr JSON line (ICJ, without the trailing newline).
    pub fn json_line(&self) -> String {
        let mut m = Map::new();
        m.insert("code".into(), Value::String(self.code.into()));
        m.insert("message".into(), Value::String(self.message.clone()));
        if let Some(p) = &self.path {
            m.insert("path".into(), Value::String(p.clone()));
        }
        if let Some(l) = self.line {
            m.insert("line".into(), Value::from(l as u64));
        }
        crate::icj::to_icj_string(&Value::Object(m))
    }
}

/// G1 check 1: a Rust file did not parse (or another input could not be read).
pub const SELFMAP_ERROR: &str = "selfmap_error";
/// G1 check 2: an extension component broke the contract (selfmap.md §3.6).
pub const EXT_CONTRACT: &str = "ext_contract";
/// G1 check 3.
pub const TAG_MARKER_MISMATCH: &str = "tag_marker_mismatch";
/// G1 check 4: a marker value breaks the grammar.
pub const MARKER_GRAMMAR: &str = "marker_grammar";
/// G1 check 4: two slugs in one component.
pub const MARKER_CONFLICT: &str = "marker_conflict";
/// G1 check 5.
pub const MANIFEST_ERROR: &str = "manifest_error";
/// G1 check 6: the manifest's own `tools_sha256` is not `init-tools-v1` of its `tools`.
pub const MANIFEST_TOOLS_HASH: &str = "manifest_tools_hash";

/// True if any diagnostic is of class [`Class::Error`].
pub fn has_errors(d: &[Diag]) -> bool {
    d.iter().any(|x| x.class() == Class::Error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_line_shape_and_classes() {
        let d = Diag::new(SELFMAP_ERROR, "bad").at("src/a.rs", Some(3));
        assert_eq!(
            d.json_line(),
            r#"{"code":"selfmap_error","line":3,"message":"bad","path":"src/a.rs"}"#
        );
        assert_eq!(d.class(), Class::Error);
        assert_eq!(Diag::new(MARKER_CONFLICT, "x").class(), Class::Check);
        assert_eq!(
            Diag::new(MANIFEST_TOOLS_HASH, "x").json_line(),
            r#"{"code":"manifest_tools_hash","message":"x"}"#
        );
    }
}
