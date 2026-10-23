#![forbid(unsafe_code)]
//! T-AW-4 (docs/awareness.md §3.9, `SELF.md` row): SELF.md stays a true, small map of the body.
//! ≤ 100 lines; every path-like backtick span exists; every `src/…` module named exists; the
//! testable absences hold; plus the volatility lint (T-AW-3) on SELF.md and GENESIS.md.
//!
//! Paths resolve against the crate root (= `body/`; a leading `body/` is stripped), or against
//! `$INIT_HOME` when that variable is set. Spans under the INIT_HOME-only top directories are
//! skipped when it is not set.

#[allow(dead_code)]
#[path = "../src/boot/awareness/lint.rs"]
mod lint;

use std::path::{Path, PathBuf};

/// Top directories that exist only under `$INIT_HOME`, never in the crate.
const INIT_HOME_ONLY: [&str; 7] = [
    "state/", "work/", "kernel/", "bin/", "lineage/", "run/", "vault/",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The text of every single-backtick span (spans may cross lines).
fn backtick_spans(text: &str) -> Vec<&str> {
    text.split('`').skip(1).step_by(2).collect()
}

/// No whitespace, no `<`, `>` or `$`, and either a `/` or a known file ending.
fn path_like(s: &str) -> bool {
    !s.is_empty()
        && !s
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '<' | '>' | '$'))
        && (s.contains('/')
            || [".rs", ".md", ".json", ".toml"]
                .iter()
                .any(|e| s.ends_with(e)))
}

/// Where `span` resolves, or None when it is to be skipped (INIT_HOME-only and no INIT_HOME).
fn resolve(span: &str, init_home: Option<&Path>) -> Option<Vec<PathBuf>> {
    let rel = span.strip_prefix("body/").unwrap_or(span);
    let mut candidates = vec![root().join(rel)];
    if let Some(h) = init_home {
        candidates.push(h.join(span));
    } else if INIT_HOME_ONLY.iter().any(|d| span.starts_with(d)) {
        return None;
    }
    Some(candidates)
}

#[test]
fn self_md_is_at_most_100_lines() {
    let n = read("SELF.md").lines().count();
    assert!(n <= 100, "SELF.md has {n} lines");
}

#[test]
fn self_md_path_spans_exist() {
    let text = read("SELF.md");
    let home = std::env::var_os("INIT_HOME").map(PathBuf::from);
    let mut checked = 0;
    let mut missing = Vec::new();
    for span in backtick_spans(&text).into_iter().filter(|s| path_like(s)) {
        let Some(cands) = resolve(span, home.as_deref()) else {
            continue;
        };
        checked += 1;
        if !cands.iter().any(|p| p.exists()) {
            missing.push(span);
        }
    }
    assert!(
        missing.is_empty(),
        "SELF.md names paths that do not exist: {missing:?}"
    );
    assert!(
        checked >= 5,
        "only {checked} path spans checked: the parser is probably broken"
    );
}

/// Every `src/…` token anywhere in the file (inside backticks or not) names a module that exists.
#[test]
fn self_md_src_modules_exist() {
    let text = read("SELF.md");
    let b = text.as_bytes();
    let mut names = Vec::new();
    for (i, _) in text.match_indices("src/") {
        let prev = i.checked_sub(1).map(|j| b[j]);
        if prev.is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'.' | b'_' | b'-'))
        {
            continue; // part of a longer path (e.g. another crate's src/)
        }
        let end = text[i..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-')))
            .map_or(text.len(), |n| i + n);
        names.push(text[i..end].trim_end_matches('.'));
    }
    assert!(names.len() >= 8, "found only {names:?}");
    let missing: Vec<&&str> = names.iter().filter(|n| !root().join(n).exists()).collect();
    assert!(
        missing.is_empty(),
        "SELF.md names modules that do not exist: {missing:?}"
    );
}

#[test]
fn main_rs_forbids_unsafe_code_first() {
    let main = read("src/main.rs");
    assert_eq!(main.lines().next(), Some("#![forbid(unsafe_code)]"));
}

/// serde_json's `preserve_order` would break ICJ's sorted keys. Comments may name it.
#[test]
fn cargo_toml_has_no_preserve_order() {
    let toml = read("Cargo.toml");
    for (n, line) in toml.lines().enumerate() {
        let mut in_str = false;
        let mut code = line;
        for (i, c) in line.char_indices() {
            match c {
                '"' => in_str = !in_str,
                '#' if !in_str => {
                    code = &line[..i];
                    break;
                }
                _ => {}
            }
        }
        assert!(
            !code.contains("preserve_order"),
            "Cargo.toml line {}: {line}",
            n + 1
        );
    }
}

/// No `HashMap` import in the head and projection modules (no HashMap reaches a prompt).
#[test]
fn head_and_projection_import_no_hashmap() {
    for rel in ["src/context/head.rs", "src/context/project.rs"] {
        let src = read(rel);
        for (n, line) in src.lines().enumerate() {
            let t = line.trim_start();
            let import = t.starts_with("use ")
                || t.starts_with("pub use ")
                || t.starts_with("pub(crate) use ");
            assert!(
                !(import && t.contains("HashMap")) && !t.contains("collections::HashMap"),
                "{rel} line {}: {line}",
                n + 1
            );
        }
    }
}

#[test]
fn self_md_and_genesis_pass_the_volatility_lint() {
    for rel in ["SELF.md", "GENESIS.md"] {
        let hits = lint::volatility_hits(&read(rel), true);
        assert!(hits.is_empty(), "{rel}: {hits:?}");
    }
}

#[test]
fn checker_helpers() {
    assert!(path_like("src/main.rs") && path_like("Cargo.toml") && path_like("src/tools/"));
    assert!(!path_like("init-body") && !path_like("initctl reload") && !path_like("work/wt-<id>"));
    assert!(!path_like("$INIT_HOME/body") && !path_like(""));
    assert_eq!(backtick_spans("a `x` b `y\nz` c"), ["x", "y\nz"]);
    assert_eq!(resolve("state/kernel/boot.json", None), None);
    assert_eq!(
        resolve("body/src/main.rs", None),
        Some(vec![root().join("src/main.rs")])
    );
}

/// G0 (kernel/gate.md, `gate.g0.forbid_missing`) rejects any body target root without
/// `#![forbid(unsafe_code)]`; a seed that fails its own G0 can never promote. Every target root
/// (src/main.rs, src/lib.rs, tests/, benches/, examples/) must open with the attribute.
#[test]
fn every_target_root_forbids_unsafe() {
    let base = root();
    let mut roots: Vec<PathBuf> = ["src/main.rs", "src/lib.rs"].iter().map(|p| base.join(p)).filter(|p| p.is_file()).collect();
    for dir in ["tests", "benches", "examples"] {
        if let Ok(entries) = std::fs::read_dir(base.join(dir)) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "rs") {
                    roots.push(p);
                }
            }
        }
    }
    assert!(!roots.is_empty(), "no target roots found under {}", base.display());
    let missing: Vec<String> = roots
        .iter()
        .filter(|p| std::fs::read_to_string(p).map(|t| t.lines().next() != Some("#![forbid(unsafe_code)]")).unwrap_or(true))
        .map(|p| p.display().to_string())
        .collect();
    assert!(missing.is_empty(), "target roots without `#![forbid(unsafe_code)]` on line 1: {missing:?}");
}
