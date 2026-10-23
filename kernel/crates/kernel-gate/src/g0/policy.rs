//! G0 step 2: the build-config policy (gate.md §3.3, "Why step 2 exists").
//!
//! The tree's `.cargo/config.toml` is honoured (a faster linker is a legitimate self-edit), but keys
//! that would let the body swap the compiler or silence lints are denied: `#![forbid(unsafe_code)]`
//! and `clippy -D warnings` hold only with the real compiler and real flags. Every violation is
//! reported, all under the one code [`code::BUILD_CONFIG`].
//!
//! Beyond gate.md's list (each closes a bypass reproduced with cargo 1.97.1; see the WP8 report):
//! - the legacy `.cargo/config` is checked too, since cargo reads it (and prefers it);
//! - top-level `include` (stable config includes would load an unchecked file), `paths` (local path
//!   overrides would replace dependencies without a `dep` change);
//! - `@file` arguments in any flags value (rustc reads more flags from the file);
//! - `[alias]` entries named after a subcommand the gate runs (`alias.clippy = "check"` shadows
//!   cargo-clippy with only a warning).

use std::path::{Path, PathBuf};

use toml::{Table, Value};

use super::{Finding, code};

/// The config files cargo would read from `<root>/.cargo/`, in cargo's order of preference
/// (`config` wins over `config.toml` when both exist), filtered to those that exist.
pub fn config_files(root: &Path) -> Vec<PathBuf> {
    ["config", "config.toml"]
        .iter()
        .map(|n| root.join(".cargo").join(n))
        .filter(|p| p.symlink_metadata().is_ok())
        .collect()
}

/// Result of the policy on a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyReport {
    /// A build-config file exists (gate.md §3.3 "Data recorded: `build_config_present`").
    pub present: bool,
    pub findings: Vec<Finding>,
}

/// Runs the policy on every config file cargo reads from the tree root.
pub fn check_tree(root: &Path) -> PolicyReport {
    let files = config_files(root);
    let mut findings = Vec::new();
    for f in &files {
        let label = f.strip_prefix(root).unwrap_or(f).display().to_string();
        match std::fs::read_to_string(f) {
            Ok(text) => findings.extend(check_text(&label, &text)),
            Err(e) => findings.push(finding(format!("{label}: cannot be read: {e}"))),
        }
    }
    PolicyReport {
        present: !files.is_empty(),
        findings,
    }
}

/// The build keys that replace the compiler or rustdoc.
const DENIED_BUILD_KEYS: [&str; 4] = [
    "rustc",
    "rustdoc",
    "rustc-wrapper",
    "rustc-workspace-wrapper",
];
/// `[env]` names cargo or rustc read (matched as prefixes, ASCII case-insensitively).
const DENIED_ENV_PREFIXES: [&str; 4] = ["RUSTC", "RUSTFLAGS", "CARGO", "RUSTDOC"];
/// Top-level keys that are denied outright.
const DENIED_TOP: [(&str, &str); 3] = [
    (
        "patch",
        "`[patch]` replaces dependencies outside the vendored set",
    ),
    (
        "paths",
        "`paths` overrides replace dependencies with local paths",
    ),
    (
        "include",
        "`include` loads another config file the policy has not checked",
    ),
];
/// Subcommands the gate runs; an alias with one of these names could shadow it.
const GATE_SUBCOMMANDS: [&str; 5] = ["metadata", "check", "clippy", "build", "test"];

fn finding(detail: String) -> Finding {
    Finding {
        code: code::BUILD_CONFIG,
        detail,
    }
}

/// Cargo config keys are kebab-case; also catch `snake_case` spellings, fail-closed.
fn norm(k: &str) -> String {
    k.replace('_', "-").to_ascii_lowercase()
}

/// Checks one config file's text. `label` names the file in the details.
pub fn check_text(label: &str, text: &str) -> Vec<Finding> {
    let doc: Table = match text.parse() {
        Ok(t) => t,
        Err(e) => {
            let msg = e.to_string();
            let first = msg.lines().next().unwrap_or("").trim();
            return vec![finding(format!("{label}: not valid TOML ({first})"))];
        }
    };
    let mut out = Vec::new();
    for (key, why) in DENIED_TOP {
        if doc.keys().any(|k| norm(k) == key) {
            out.push(finding(format!("{label}: {why}")));
        }
    }
    if let Some(build) = get_table(&doc, "build") {
        for k in build.keys() {
            if DENIED_BUILD_KEYS.contains(&norm(k).as_str()) {
                out.push(finding(format!(
                    "{label}: `build.{k}` is not allowed (the gate runs the real compiler)"
                )));
            }
        }
    }
    if let Some(env) = get_table(&doc, "env") {
        for k in env.keys() {
            let up = k.to_ascii_uppercase();
            if let Some(p) = DENIED_ENV_PREFIXES.iter().find(|p| up.starts_with(*p)) {
                out.push(finding(format!(
                    "{label}: `[env]` entry `{k}` is not allowed (names starting with {p})"
                )));
            }
        }
    }
    if let Some(alias) = get_table(&doc, "alias") {
        for k in alias.keys() {
            if GATE_SUBCOMMANDS.contains(&norm(k).as_str()) {
                out.push(finding(format!(
                    "{label}: `alias.{k}` would shadow `cargo {k}`, which the gate runs"
                )));
            }
        }
    }
    for (k, v) in &doc {
        if norm(k) == "source" {
            check_source(label, v, &mut out);
        }
    }
    walk_flags(label, &mut Vec::new(), &doc, &mut out);
    out
}

/// A top-level table by (normalized) name, if it is one.
fn get_table<'a>(doc: &'a Table, name: &str) -> Option<&'a Table> {
    doc.iter()
        .find(|(k, _)| norm(k) == name)
        .and_then(|(_, v)| v.as_table())
}

/// `[source]` may hold only the vendored replacement: `source.crates-io.replace-with =
/// "vendored-sources"` and `source.vendored-sources.directory = "<dir>"` (supervise's `--config`
/// flags override the directory anyway).
fn check_source(label: &str, v: &Value, out: &mut Vec<Finding>) {
    let Some(t) = v.as_table() else {
        out.push(finding(format!("{label}: `source` must be a table")));
        return;
    };
    for (name, s) in t {
        let ok = match (name.as_str(), s.as_table()) {
            ("crates-io", Some(s)) => {
                s.len() == 1
                    && s.get("replace-with").and_then(Value::as_str) == Some("vendored-sources")
            }
            ("vendored-sources", Some(s)) => {
                s.len() == 1 && s.get("directory").is_some_and(Value::is_str)
            }
            _ => false,
        };
        if !ok {
            out.push(finding(format!(
                "{label}: `[source.{name}]` is not allowed (only the vendored replacement: \
                 crates-io.replace-with = \"vendored-sources\", vendored-sources.directory)"
            )));
        }
    }
}

/// Every `rustflags` / `rustdocflags` value anywhere in the document (build, target.<triple>,
/// target.<cfg>, host, profile, ...), in string or array form.
fn walk_flags(label: &str, path: &mut Vec<String>, t: &Table, out: &mut Vec<Finding>) {
    for (k, v) in t {
        path.push(k.clone());
        let n = norm(k);
        if n == "rustflags" || n == "rustdocflags" {
            for tok in flag_tokens(v) {
                if let Some(why) = bad_flag(&tok) {
                    out.push(finding(format!(
                        "{label}: `{}` has `{tok}` ({why})",
                        path.join(".")
                    )));
                }
            }
        } else {
            walk_value(label, path, v, out);
        }
        path.pop();
    }
}

fn walk_value(label: &str, path: &mut Vec<String>, v: &Value, out: &mut Vec<Finding>) {
    match v {
        Value::Table(t) => walk_flags(label, path, t, out),
        Value::Array(a) => {
            for x in a {
                walk_value(label, path, x, out);
            }
        }
        _ => {}
    }
}

/// The tokens of a flags value: a string is split on whitespace (as cargo does); each array element
/// is checked whole and, fail-closed, split on whitespace too.
fn flag_tokens(v: &Value) -> Vec<String> {
    let mut toks = Vec::new();
    match v {
        Value::String(s) => toks.extend(s.split_whitespace().map(str::to_string)),
        Value::Array(a) => {
            for x in a {
                if let Some(s) = x.as_str() {
                    toks.push(s.to_string());
                    let parts: Vec<&str> = s.split_whitespace().collect();
                    if parts.len() > 1 || parts.first().is_some_and(|p| *p != s) {
                        toks.extend(parts.into_iter().map(str::to_string));
                    }
                }
            }
        }
        _ => {}
    }
    toks
}

/// Why a rustc flag is denied, if it is: lint-level and unstable flags, in every spelling rustc
/// accepts (separate value, `--flag=value`, joined short `-Avalue`), and `@file` argument files.
fn bad_flag(tok: &str) -> Option<&'static str> {
    const LONG: [(&str, &str); 4] = [
        (
            "--cap-lints",
            "--cap-lints caps every lint, including forbid",
        ),
        ("--allow", "--allow lowers lint levels"),
        ("--force-warn", "--force-warn overrides forbid"),
        ("--warn", "--warn lowers lint levels"),
    ];
    const SHORT: [(&str, &str); 3] = [
        ("-A", "-A lowers lint levels"),
        ("-W", "-W lowers lint levels"),
        ("-Z", "-Z enables unstable compiler options"),
    ];
    if tok.starts_with('@') {
        return Some("@file reads more rustc flags from a file the policy cannot check");
    }
    for (f, why) in LONG {
        if tok == f || tok.strip_prefix(f).is_some_and(|r| r.starts_with('=')) {
            return Some(why);
        }
    }
    SHORT
        .iter()
        .find(|(f, _)| tok.starts_with(f))
        .map(|(_, why)| *why)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(cfg: &str) {
        let f = check_text("c", cfg);
        assert!(f.is_empty(), "should pass:\n{cfg}\n{f:#?}");
    }

    fn deny(cfg: &str) -> Vec<String> {
        let f = check_text("c", cfg);
        assert!(!f.is_empty(), "should be denied:\n{cfg}");
        assert!(f.iter().all(|f| f.code == code::BUILD_CONFIG));
        f.into_iter().map(|f| f.detail).collect()
    }

    #[test]
    fn legit_configs_pass() {
        pass("");
        // the seed's own (in-repo) config
        pass(
            "[source.crates-io]\nreplace-with = \"vendored-sources\"\n[source.vendored-sources]\n\
             directory = \"../vendor\"\n[net]\noffline = true\n",
        );
        pass(
            r#"
            [build]
            jobs = 4
            rustflags = ["-C", "target-cpu=native", "-Dwarnings", "--deny=unused", "-F", "unsafe_code", "--cfg", "foo"]
            rustdocflags = "-C opt-level=1"
            incremental = false
            [target.aarch64-apple-darwin]
            linker = "clang"
            rustflags = ["-C", "link-arg=-fuse-ld=lld", "-Clink-arg=-Wl,-dead_strip"]
            runner = "./run-wrapper.sh"
            [target.'cfg(unix)']
            rustflags = "-C link-arg=-Wl,-z,relro --forbid=unsafe_code"
            [profile.dev]
            opt-level = 1
            debug = "line-tables-only"
            [profile.release]
            lto = "thin"
            codegen-units = 1
            [alias]
            b = "build"
            xtask = "run --package xtask --"
            t = ["test", "--workspace"]
            [env]
            RUST_LOG = "debug"
            MY_FLAG = { value = "1", force = true }
            [term]
            color = "never"
            [net]
            offline = true
            "#,
        );
    }

    #[test]
    fn denied_build_keys() {
        for k in [
            "rustc",
            "rustdoc",
            "rustc-wrapper",
            "rustc-workspace-wrapper",
            "rustc_wrapper",
            "RUSTC-WRAPPER",
        ] {
            let d = deny(&format!("[build]\n{k} = \"./x.sh\"\n"));
            assert_eq!(d.len(), 1, "{d:?}");
            assert!(d[0].contains(&format!("build.{k}")), "{d:?}");
        }
        deny("build.rustc-wrapper = \"./evil.sh\"\n");
        deny("build = { rustc = \"/bin/false\" }\n");
    }

    #[test]
    fn denied_env_names() {
        for k in [
            "RUSTC_BOOTSTRAP",
            "RUSTC",
            "RUSTFLAGS",
            "RUSTDOCFLAGS",
            "RUSTDOC",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_TARGET_DIR",
            "CARGO_BUILD_RUSTC_WRAPPER",
            "cargo_home",
        ] {
            let d = deny(&format!("[env]\n{k} = \"1\"\n"));
            assert!(d[0].contains(k), "{d:?}");
        }
        deny("[env]\nRUSTC_WRAPPER = { value = \"x\", force = true }\n");
        pass("[env]\nRUST_BACKTRACE = \"1\"\nRUST_MIN_STACK = \"8388608\"\n");
    }

    #[test]
    fn denied_top_level_tables() {
        let d = deny("[patch.crates-io]\nserde = { path = \"x\" }\n");
        assert!(d[0].contains("[patch]"), "{d:?}");
        let d = deny("paths = [\"../override\"]\n");
        assert!(d[0].contains("paths"), "{d:?}");
        let d = deny("include = [\"evil.toml\"]\n");
        assert!(d[0].contains("include"), "{d:?}");
        deny("include = \"evil.toml\"\n");
    }

    #[test]
    fn source_must_be_the_vendored_replacement() {
        let d = deny("[source.my-git]\ngit = \"https://example.com/x\"\n");
        assert!(d[0].contains("source.my-git"), "{d:?}");
        deny(
            "[source.crates-io]\nreplace-with = \"mine\"\n[source.mine]\ndirectory = \"elsewhere\"\n",
        );
        deny("[source.crates-io]\nreplace-with = \"vendored-sources\"\nregistry = \"x\"\n");
        deny("[source.vendored-sources]\ndirectory = \"vendor\"\nreplace-with = \"other\"\n");
        deny("[source.vendored-sources]\nregistry = \"sparse+https://example.com/\"\n");
        deny("source = 3\n");
        pass("[source.vendored-sources]\ndirectory = \"vendor\"\n");
    }

    #[test]
    fn denied_aliases_shadowing_gate_subcommands() {
        for k in GATE_SUBCOMMANDS {
            let d = deny(&format!("[alias]\n{k} = \"check\"\n"));
            assert!(d[0].contains(&format!("alias.{k}")), "{d:?}");
        }
    }

    #[test]
    fn denied_flags_every_spelling_and_place() {
        let forms: [&str; 22] = [
            r#"["--cap-lints", "allow"]"#,
            r#"["--cap-lints=allow"]"#,
            r#""--cap-lints allow""#,
            r#"["-A", "warnings"]"#,
            r#"["-Awarnings"]"#,
            r#""-Aunsafe_code""#,
            r#"["--allow=unsafe_code"]"#,
            r#"["--allow", "x"]"#,
            r#"["--force-warn", "unsafe_code"]"#,
            r#"["--force-warn=unsafe_code"]"#,
            r#"["-W", "x"]"#,
            r#"["-Wunused"]"#,
            r#"["--warn=x"]"#,
            r#"["--warn", "x"]"#,
            r#"["-Z", "unstable-options"]"#,
            r#"["-Zallow-features=asm"]"#,
            r#"["-Zfoo"]"#,
            r#"["@args.txt"]"#,
            r#""-C opt-level=1 @flags""#,
            r#"["-C opt-level=1 --cap-lints=allow"]"#,
            r#""-C opt-level=1 -Zfoo""#,
            r#"["-C", "debuginfo=0", "-A", "unsafe_code"]"#,
        ];
        let places = [
            "[build]\nrustflags = {v}\n",
            "[build]\nrustdocflags = {v}\n",
            "[target.x86_64-unknown-linux-gnu]\nrustflags = {v}\n",
            "[target.aarch64-apple-darwin]\nrustdocflags = {v}\n",
            "[target.'cfg(all())']\nrustflags = {v}\n",
            "[host]\nrustflags = {v}\n",
            "[profile.dev]\nrustflags = {v}\n",
            "[profile.dev.package.foo]\nrustflags = {v}\n",
        ];
        for p in places {
            for f in forms {
                let cfg = p.replace("{v}", f);
                deny(&cfg);
            }
        }
        let d = deny("[target.'cfg(unix)']\nrustflags = [\"--cap-lints=allow\"]\n");
        assert!(
            d[0].contains("target.cfg(unix).rustflags") && d[0].contains("--cap-lints=allow"),
            "{d:?}"
        );
    }

    #[test]
    fn every_violation_is_reported() {
        // gate.md §8 G-03's config
        let d =
            deny("[build]\nrustc-wrapper = \"./evil.sh\"\nrustflags = [\"--cap-lints=allow\"]\n");
        assert_eq!(d.len(), 2, "{d:?}");
        let d = deny(
            "include = \"x.toml\"\n[build]\nrustc = \"x\"\nrustflags = [\"-Awarnings\", \"-Zfoo\"]\n\
             [env]\nRUSTFLAGS = \"x\"\n[patch.crates-io]\na = { path = \"a\" }\n",
        );
        assert_eq!(d.len(), 6, "{d:?}");
    }

    #[test]
    fn invalid_toml_is_denied() {
        let d = deny("[build\nrustc = ");
        assert!(d[0].contains("not valid TOML"), "{d:?}");
    }

    #[test]
    fn tree_checks_both_config_files() {
        let home = crate::testutil::test_home("g0-policy");
        let root = home.join("src");
        std::fs::create_dir_all(&root).unwrap();
        let r = check_tree(&root);
        assert!(!r.present && r.findings.is_empty());

        std::fs::create_dir_all(root.join(".cargo")).unwrap();
        std::fs::write(root.join(".cargo/config.toml"), "[build]\njobs = 2\n").unwrap();
        let r = check_tree(&root);
        assert!(r.present && r.findings.is_empty(), "{r:?}");

        std::fs::write(
            root.join(".cargo/config"),
            "[build]\nrustc-wrapper = \"./evil.sh\"\n",
        )
        .unwrap();
        let r = check_tree(&root);
        assert!(r.present);
        assert_eq!(r.findings.len(), 1);
        assert!(r.findings[0].detail.starts_with(".cargo/config:"), "{r:?}");

        std::fs::remove_file(root.join(".cargo/config.toml")).unwrap();
        let r = check_tree(&root);
        assert!(r.present && r.findings.len() == 1);
        crate::testutil::remove_home(&home);
    }
}
