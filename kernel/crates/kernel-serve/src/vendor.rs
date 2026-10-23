//! `vendor` manifest steps (kernel/initctl.md §4.1 `vendor`, steps 1–2), as pure functions the
//! backend's vendor job uses on the exported lineage `Cargo.toml`:
//!
//! 1. [`check_sources`]: only crates.io dependencies may be added this way. Any `git =`,
//!    `registry =` / `registry-index =`, `[patch]`, `[replace]`, or a `path =` that leaves the
//!    tree → `E_VENDOR_SOURCE` (IC-14).
//! 2. [`add_dependency`]: `<crate> = "=<version>"` under `[dependencies]` (an existing entry is
//!    replaced).
//!
//! Steps 3–8 (local registry over the egress `crates` class, offline resolution, `cargo vendor`,
//! the delta, the lineage record) need the Root's egress socket and lineage and live in the
//! backend.

use init_proto::ErrorBody;
use init_proto::errors::ErrorCode;
use std::path::{Component, Path};
use toml::{Table, Value};

fn source_err(msg: String) -> ErrorBody {
    ErrorBody::new(ErrorCode::VendorSource, msg)
}

fn parse(manifest: &str) -> Result<Table, ErrorBody> {
    manifest.parse::<Table>().map_err(|e| source_err(format!("Cargo.toml does not parse: {}", e.message())))
}

/// A `path =` stays inside the tree: relative, and no `..` that climbs above the manifest's
/// directory (`manifest_dir` is the manifest's directory relative to the tree root).
fn path_inside(manifest_dir: &Path, p: &str) -> bool {
    let p = Path::new(p);
    if p.is_absolute() {
        return false;
    }
    let mut depth: i64 = manifest_dir.components().filter(|c| matches!(c, Component::Normal(_))).count() as i64;
    for c in p.components() {
        match c {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return false,
        }
        if depth < 0 {
            return false;
        }
    }
    true
}

fn check_dep_table(manifest_dir: &Path, section: &str, deps: &Table, problems: &mut Vec<String>) {
    for (name, spec) in deps {
        let Value::Table(t) = spec else { continue };
        for k in ["git", "registry", "registry-index"] {
            if t.contains_key(k) {
                problems.push(format!("[{section}] {name} has `{k} =`"));
            }
        }
        if let Some(Value::String(p)) = t.get("path")
            && !path_inside(manifest_dir, p)
        {
            problems.push(format!("[{section}] {name} has `path = \"{p}\"` outside the tree"));
        }
    }
}

const DEP_SECTIONS: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies", "dev_dependencies", "build_dependencies"];

/// Step 1: refuse any non-crates.io source (`E_VENDOR_SOURCE`). `manifest_dir` is the
/// manifest's directory relative to the tree root (`""` for the root `Cargo.toml`, `"ext"` for
/// `ext/Cargo.toml`).
pub fn check_sources(manifest: &str, manifest_dir: &Path) -> Result<(), ErrorBody> {
    let t = parse(manifest)?;
    let mut problems = Vec::new();
    for s in ["patch", "replace"] {
        if t.contains_key(s) {
            problems.push(format!("`[{s}]` is present"));
        }
    }
    for s in DEP_SECTIONS {
        if let Some(Value::Table(d)) = t.get(*s) {
            check_dep_table(manifest_dir, s, d, &mut problems);
        }
    }
    if let Some(Value::Table(targets)) = t.get("target") {
        for (cfg, tt) in targets {
            let Value::Table(tt) = tt else { continue };
            for s in DEP_SECTIONS {
                if let Some(Value::Table(d)) = tt.get(*s) {
                    check_dep_table(manifest_dir, &format!("target.{cfg}.{s}"), d, &mut problems);
                }
            }
        }
    }
    if let Some(Value::Table(ws)) = t.get("workspace") {
        if let Some(Value::Table(d)) = ws.get("dependencies") {
            check_dep_table(manifest_dir, "workspace.dependencies", d, &mut problems);
        }
        if ws.contains_key("patch") {
            problems.push("`[workspace.patch]` is present".into());
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    let details = problems
        .iter()
        .map(|p| init_proto::ErrorDetail { code: "E_VENDOR_SOURCE".into(), detail: p.clone() })
        .collect();
    Err(source_err(format!("only crates.io dependencies can be vendored: {}", problems.join("; "))).with_details(details))
}

/// Step 2: `<crate> = "=<version>"` under `[dependencies]`; returns the new manifest text.
/// (The text is re-serialized: this copy lives only in `cache/vendor/<job>/`.)
pub fn add_dependency(manifest: &str, krate: &str, version: &str) -> Result<String, ErrorBody> {
    let mut t = parse(manifest)?;
    let deps = t.entry("dependencies").or_insert_with(|| Value::Table(Table::new()));
    let Value::Table(deps) = deps else {
        return Err(source_err("[dependencies] is not a table".into()));
    };
    deps.insert(krate.to_string(), Value::String(format!("={version}")));
    toml::to_string(&t).map_err(|e| ErrorBody::new(ErrorCode::Internal, format!("Cargo.toml: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"
[package]
name = "init-body"
version = "0.1.0"

[dependencies]
serde = { version = "=1.0.229", features = ["derive"] }
local = { path = "crates/local" }

[target.'cfg(unix)'.dependencies]
libc = "0.2"
"#;

    #[test]
    fn accepts_crates_io_and_inner_paths() {
        check_sources(OK, Path::new("")).unwrap();
        check_sources("[dependencies]\nx = { path = \"../shared\" }\n", Path::new("ext")).unwrap();
    }

    #[test]
    fn ic14_refuses_other_sources() {
        let git = format!("{OK}\n[dev-dependencies]\nevil = {{ git = \"https://example.invalid/x\" }}\n");
        let e = check_sources(&git, Path::new("")).unwrap_err();
        assert_eq!((e.code.clone(), e.exit_code()), (ErrorCode::VendorSource, 1));
        assert_eq!(e.details.len(), 1);
        for bad in [
            "[dependencies]\na = { registry = \"other\", version = \"1\" }\n",
            "[dependencies]\na = { path = \"../../outside\" }\n",
            "[dependencies]\na = { path = \"/abs\" }\n",
            "[patch.crates-io]\nserde = { path = \"x\" }\n",
            "[target.'cfg(unix)'.build-dependencies]\nb = { git = \"https://x.invalid\" }\n",
            "[workspace.dependencies]\nc = { git = \"https://x.invalid\" }\n",
        ] {
            assert_eq!(check_sources(bad, Path::new("")).unwrap_err().code, ErrorCode::VendorSource, "{bad}");
        }
        assert_eq!(check_sources("not toml [", Path::new("")).unwrap_err().code, ErrorCode::VendorSource);
    }

    #[test]
    fn adds_exact_pin() {
        let out = add_dependency(OK, "vt100", "0.16.2").unwrap();
        let t: Table = out.parse().unwrap();
        assert_eq!(t["dependencies"]["vt100"].as_str(), Some("=0.16.2"));
        assert!(t["dependencies"]["serde"].is_table());
        let again = add_dependency(&out, "vt100", "0.16.3").unwrap();
        assert_eq!(again.parse::<Table>().unwrap()["dependencies"]["vt100"].as_str(), Some("=0.16.3"));
        let fresh = add_dependency("[package]\nname = \"x\"\n", "a", "1.0.0").unwrap();
        assert!(fresh.contains("[dependencies]"));
    }
}
