//! The kernel inventory `kernel-self.json` (selfmap.md §3.13, §4.2a): `{self_v: 2, generator, kernel:
//! <kernel_tree>, warnings}`, built as for the body (§3.2–§3.4) and drift-checked at K0 with the body's
//! semantics ([`crate::drift`]).
//!
//! Components (generator table v2): the first top-level module directory or file under
//! `crates/init-kernel/src/` maps through [`TABLE`]. This repository splits the kernel into lane crates
//! (`crates/kernel-<area>/`, BUILD_PLAN "Shape"), so the same table is applied to them: first the crate's
//! own first module (`kernel-gate/src/evo/…` → `kernel/evolution`), then the area name
//! (`kernel-gate/src/g1.rs` → `kernel/gate`). `crates/initctl/` and `crates/init-proto/` (the initctl wire
//! types) are `kernel/initctl`. Anything else counted is `kernel/other`.
//!
//! `@init compensates-for` markers are ignored in the kernel tree, each reported in `warnings`
//! (thinning applies to body scaffolds only).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Value, json};

use crate::body::{lock_dependencies, module_value};
use crate::diag::{self, Diag};
use crate::icj::to_pretty;
use crate::inventory::{FileSetMode, glob, list_files, source_digest};
use crate::markers::NEEDLE;
use crate::rust_scan::scan_rust;
use crate::tokens::Tokenizer;

/// Module (or crate area) name → component id.
pub const TABLE: [(&str, &str); 9] = [
    ("supervise", "kernel/supervise"),
    ("proxy", "kernel/proxy"),
    ("gate", "kernel/gate"),
    ("selfmap", "kernel/selfmap"),
    ("serve", "kernel/initctl"),
    ("initctl", "kernel/initctl"),
    ("evo", "kernel/evolution"),
    ("sandbox", "kernel/sandbox"),
    ("config", "kernel/config"),
];

/// Component id → the cross-doc contracts it must keep (selfmap.md §3.13).
pub fn contracts(id: &str) -> Vec<&'static str> {
    match id {
        "kernel/gate" => vec!["kernel/gate.md §4.1a"],
        "kernel/proxy" => vec!["kernel/proxy.md §7"],
        "kernel/selfmap" => vec!["kernel/selfmap.md §4.2"],
        "kernel/initctl" => vec!["kernel/initctl.md wire protocol"],
        "kernel/supervise" => vec!["kernel/supervise.md §4.1-§4.5"],
        "kernel/evolution" => vec!["evolution.md §3.8"],
        "kernel/sandbox" => vec!["kernel/sandbox.md §4"],
        "kernel/config" => vec!["kernel/config.md §3.1"],
        _ => vec![],
    }
}

/// Counted in the kernel budget (§3.13): `**/*.rs`, `**/Cargo.toml`, `**/build.rs`, `**/*.toml`;
/// never `vendor/**`, `target/**`, `**/tests/**`, `wit/**`.
pub fn in_kernel_budget(path: &str) -> bool {
    ["**/*.rs", "**/Cargo.toml", "**/build.rs", "**/*.toml"]
        .iter()
        .any(|p| glob(p, path))
        && !["vendor/**", "target/**", "wit/**"]
            .iter()
            .any(|p| glob(p, path))
        && !is_kernel_test(path)
}

/// `**/tests/**` (reported as `budget.tests_tokens`).
pub fn is_kernel_test(path: &str) -> bool {
    glob("**/tests/**", path)
}

fn table(name: &str) -> Option<&'static str> {
    TABLE.iter().find(|(m, _)| *m == name).map(|(_, id)| *id)
}

fn first_module(rest: &str) -> &str {
    let first = rest.split('/').next().unwrap_or("");
    first.strip_suffix(".rs").unwrap_or(first)
}

/// The component of a counted kernel file.
pub fn component_of(path: &str) -> &'static str {
    let Some(rest) = path.strip_prefix("crates/") else {
        return "kernel/other";
    };
    let (krate, inner) = rest.split_once('/').unwrap_or((rest, ""));
    if krate == "initctl" || krate == "init-proto" {
        return "kernel/initctl";
    }
    let src = inner.strip_prefix("src/");
    if krate == "init-kernel" {
        return src
            .and_then(|s| table(first_module(s)))
            .unwrap_or("kernel/other");
    }
    if let Some(area) = krate.strip_prefix("kernel-") {
        if let Some(id) = src.and_then(|s| table(first_module(s))) {
            return id;
        }
        return table(area).unwrap_or("kernel/other");
    }
    "kernel/other"
}

/// The kernel inventory and its diagnostics.
pub struct KernelGenerated {
    pub value: Value,
    pub bytes: Vec<u8>,
    pub diags: Vec<Diag>,
}

/// Generates `kernel-self.json` for the kernel tree at `root`.
pub fn generate(root: &Path, mode: FileSetMode) -> KernelGenerated {
    let tok = Tokenizer::o200k();
    let mut diags = Vec::new();
    let (files, w) = match list_files(root, mode, "kernel-self.json", &tok) {
        Ok(x) => x,
        Err(e) => {
            diags.push(Diag::new(
                diag::SELFMAP_ERROR,
                format!("cannot list the kernel tree: {e}"),
            ));
            (Vec::new(), Vec::new())
        }
    };
    let mut warnings: BTreeSet<String> = w.into_iter().collect();
    let counted = |f: &crate::inventory::SourceFile| {
        !f.symlink && f.tokens.is_some() && in_kernel_budget(&f.path)
    };
    let (mut tokens, mut bytes, mut nfiles, mut tests) = (0u64, 0u64, 0u64, 0u64);
    let mut comps: BTreeMap<&'static str, (u64, BTreeSet<String>)> = BTreeMap::new();
    let mut modules = Vec::new();
    for f in &files {
        let t = f.tokens.unwrap_or(0);
        if counted(f) {
            tokens += t;
            bytes += f.bytes;
            nfiles += 1;
            let c = comps.entry(component_of(&f.path)).or_default();
            c.0 += t;
            c.1.insert(f.path.clone());
            if f.path.ends_with(".rs") {
                let s = scan_rust(f.text.as_deref().unwrap_or(""), &tok);
                if let Some((line, msg)) = &s.parse_error {
                    diags.push(
                        Diag::new(diag::SELFMAP_ERROR, format!("{}:{line}: {msg}", f.path))
                            .at(&f.path, Some(*line)),
                    );
                    continue;
                }
                modules.push(module_value(&f.path, &s));
            }
        } else if is_kernel_test(&f.path) {
            tests += t;
        }
        if let Some(text) = &f.text
            && (f.path.ends_with(".rs") || f.path.ends_with(".md"))
        {
            for (i, l) in text.lines().enumerate() {
                if l.contains(NEEDLE) {
                    warnings.insert(format!("marker_ignored: {}:{}", f.path, i + 1));
                }
            }
        }
    }
    let components: Vec<Value> = comps
        .iter()
        .map(|(id, (t, paths))| {
            json!({"contracts": contracts(id), "evolvable_from": "L3", "id": id, "paths": paths, "tokens": t})
        })
        .collect();
    let lock = files
        .iter()
        .find(|f| f.path == "Cargo.lock")
        .and_then(|f| f.text.as_deref());
    let dependencies = lock_dependencies(lock).unwrap_or_else(|e| {
        diags.push(Diag::new(diag::SELFMAP_ERROR, e).at("Cargo.lock", None));
        Vec::new()
    });
    let files_v: Vec<Value> = files
        .iter()
        .map(|f| json!({"bytes": f.bytes, "counted": counted(f), "path": f.path, "sha256": f.sha256, "tokens": f.tokens}))
        .collect();
    let value = json!({
        "generator": crate::generator_value(),
        "kernel": {
            "budget": {"bytes": bytes, "files": nfiles, "tests_tokens": tests, "tokens": tokens},
            "components": components,
            "dependencies": dependencies,
            "files": files_v,
            "modules": modules,
            "source_digest": source_digest(&files),
        },
        "self_v": crate::SELF_V,
        "warnings": warnings.into_iter().collect::<Vec<_>>(),
    });
    let bytes = to_pretty(&value);
    KernelGenerated {
        value,
        bytes,
        diags,
    }
}

/// `live.kernel.components` summary of a `kernel-self.json` value: `{id, tokens, contracts}` by id.
pub fn component_summary(kernel_self: &Value) -> Vec<Value> {
    kernel_self
        .pointer("/kernel/components")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|c| json!({"contracts": c["contracts"], "id": c["id"], "tokens": c["tokens"]}))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{put, remove_home, test_home};

    #[test]
    fn component_mapping() {
        assert_eq!(
            component_of("crates/init-kernel/src/gate/mod.rs"),
            "kernel/gate"
        );
        assert_eq!(
            component_of("crates/init-kernel/src/proxy.rs"),
            "kernel/proxy"
        );
        assert_eq!(
            component_of("crates/init-kernel/src/serve/x.rs"),
            "kernel/initctl"
        );
        assert_eq!(
            component_of("crates/init-kernel/src/misc.rs"),
            "kernel/other"
        );
        assert_eq!(
            component_of("crates/init-kernel/src/main.rs"),
            "kernel/other"
        );
        assert_eq!(component_of("crates/initctl/src/main.rs"), "kernel/initctl");
        assert_eq!(
            component_of("crates/kernel-gate/src/evo/mod.rs"),
            "kernel/evolution"
        );
        assert_eq!(component_of("crates/kernel-gate/src/g1.rs"), "kernel/gate");
        assert_eq!(component_of("crates/kernel-gate/Cargo.toml"), "kernel/gate");
        assert_eq!(
            component_of("crates/kernel-serve/src/lib.rs"),
            "kernel/initctl"
        );
        assert_eq!(component_of("Cargo.toml"), "kernel/other");
        assert!(in_kernel_budget(".cargo/config.toml") && !in_kernel_budget("Cargo.lock"));
        assert!(!in_kernel_budget("crates/x/tests/a.rs") && is_kernel_test("crates/x/tests/a.rs"));
    }

    /// S-18: modules gate, proxy, serve, misc and an `@init` marker.
    #[test]
    fn s18_kernel_inventory() {
        let home = test_home("s18");
        put(
            &home,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\n",
        );
        put(
            &home,
            "crates/init-kernel/Cargo.toml",
            "[package]\nname = \"init-kernel\"\nversion = \"0.3.0\"\n",
        );
        put(
            &home,
            "crates/init-kernel/src/main.rs",
            "mod gate;\nmod proxy;\nmod serve;\nmod misc;\nfn main() {}\n",
        );
        put(
            &home,
            "crates/init-kernel/src/gate/mod.rs",
            "//! @init compensates-for: weak-gate\npub fn g1() -> u32 { 1 }\n",
        );
        put(&home, "crates/init-kernel/src/proxy.rs", "pub fn p() {}\n");
        put(
            &home,
            "crates/init-kernel/src/serve/mod.rs",
            "pub fn s() {}\n",
        );
        put(&home, "crates/init-kernel/src/misc.rs", "pub fn m() {}\n");
        put(
            &home,
            "crates/init-kernel/tests/t.rs",
            "#[test] fn t() {}\n",
        );
        let g = generate(&home, FileSetMode::Walk);
        assert!(g.diags.is_empty(), "{:?}", g.diags);
        let ids: Vec<&str> = g.value["kernel"]["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            [
                "kernel/gate",
                "kernel/initctl",
                "kernel/other",
                "kernel/proxy"
            ]
        );
        let gate = &g.value["kernel"]["components"][0];
        assert_eq!(gate["contracts"], json!(["kernel/gate.md §4.1a"]));
        assert_eq!(gate["evolvable_from"], "L3");
        assert_eq!(
            g.value["kernel"]["components"][3]["contracts"],
            json!(["kernel/proxy.md §7"])
        );
        assert_eq!(
            g.value["warnings"],
            json!(["marker_ignored: crates/init-kernel/src/gate/mod.rs:1"])
        );
        assert!(
            g.value["kernel"]["budget"]["tests_tokens"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert!(g.value.get("live").is_none());
        // --check after editing gate/mod.rs reports drift naming that file.
        let committed = g.bytes.clone();
        put(
            &home,
            "crates/init-kernel/src/gate/mod.rs",
            "//! @init compensates-for: weak-gate\npub fn g1() -> u32 { 2 }\n",
        );
        let r = generate(&home, FileSetMode::Walk);
        let e = crate::drift::compare(Some(&committed), &r.bytes).unwrap_err();
        assert_eq!(e.code, "self_json_drift");
        assert!(
            e.detail
                .contains("changed: crates/init-kernel/src/gate/mod.rs"),
            "{}",
            e.detail
        );
        remove_home(&home);
    }
}
