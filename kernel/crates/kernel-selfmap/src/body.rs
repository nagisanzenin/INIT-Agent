//! The committed body file `self.json` (selfmap.md §3.2–§3.9, §4.2): committed fields only. There is no
//! `live` key (v03-decisions D12); the served view adds it ([`crate::serve`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::describe::DescribeRunner;
use crate::diag::{self, Diag};
use crate::icj::{to_icj_string, to_pretty};
use crate::inventory::{FileSetMode, SourceFile, glob, list_files, source_digest};
use crate::manifest::{
    ManifestSource, manifest_tools, parse_manifest, run_manifest, tools_object, tools_sha256,
};
use crate::markers::{self, front_matter, prompt_blocks};
use crate::rust_scan::{FileScan, scan_rust};
use crate::tokens::Tokenizer;
use crate::wit::{WitItem, wit_inventory};

/// The body budget set, generator v1 (selfmap.md §3.2).
pub const BUDGET_INCLUDE: [&str; 11] = [
    "src/**/*.rs",
    "build.rs",
    "Cargo.toml",
    ".cargo/config.toml",
    "GENESIS.md",
    "SELF.md",
    "skills/**",
    "ext/Cargo.toml",
    "ext/*/src/**/*.rs",
    "ext/*/Cargo.toml",
    "ext/*/build.rs",
];
/// Reported separately as `budget.tests_tokens`.
pub const TESTS: [&str; 2] = ["tests/**", "ext/*/tests/**"];
/// Never counted.
pub const BUDGET_EXCLUDE: [&str; 5] = [
    "vendor/**",
    "Cargo.lock",
    "ext/Cargo.lock",
    "target/**",
    "wit/**",
];

/// The seed's core tools (body/tools.md §4.1).
pub const CORE_TOOLS: [&str; 5] = ["bash", "edit", "ext", "read", "write"];

/// True if `path` is in the body budget (before the text check).
pub fn in_budget(path: &str) -> bool {
    BUDGET_INCLUDE.iter().any(|p| glob(p, path))
        && !BUDGET_EXCLUDE.iter().any(|p| glob(p, path))
        && !is_test(path)
}

/// True if `path` is a test file.
pub fn is_test(path: &str) -> bool {
    TESTS.iter().any(|p| glob(p, path))
}

/// Inputs of one generation.
#[derive(Debug, Clone)]
pub struct BodyConfig {
    pub root: PathBuf,
    pub mode: FileSetMode,
    pub manifest: ManifestSource,
    /// Directory holding the built components (`<name>.wasm`, or cargo's `<crate_with_underscores>.wasm`).
    pub components: Option<PathBuf>,
    pub describe: DescribeRunner,
    /// A directory the generator may use for empty child working directories (created and removed).
    pub scratch: PathBuf,
}

/// The result: the `self.json` value, its pretty-ICJ bytes and the diagnostics.
#[derive(Debug, Clone)]
pub struct Generated {
    pub value: Value,
    pub bytes: Vec<u8>,
    pub diags: Vec<Diag>,
}

/// Component ids and kinds, in one place.
fn file_component(path: &str, tools: &BTreeSet<String>) -> Option<String> {
    if let Some(name) = path
        .strip_prefix("src/tools/")
        .and_then(|r| r.strip_suffix(".rs"))
        && !name.contains('/')
        && tools.contains(name)
    {
        return Some(format!("tool/{name}"));
    }
    if let Some(fam) = profile_family(path) {
        return Some(format!("profile/{fam}"));
    }
    if let Some((name, _)) = path.strip_prefix("ext/").and_then(|r| r.split_once('/')) {
        return Some(format!("ext/{name}"));
    }
    if glob("src/**/*.rs", path) {
        return Some(format!("mod/{path}"));
    }
    None
}

/// `src/profiles/<family>.rs` (a `mod.rs` there is a module, not a profile).
fn profile_family(path: &str) -> Option<&str> {
    let fam = path.strip_prefix("src/profiles/")?.strip_suffix(".rs")?;
    (!fam.contains('/') && fam != "mod").then_some(fam)
}

struct Comp {
    kind: &'static str,
    core: bool,
    tokens: u64,
    paths: BTreeSet<String>,
}

struct Mark {
    component: String,
    slug: String,
    note: Option<String>,
    path: String,
    line: usize,
    end_line: usize,
    item: Option<String>,
    tokens: u64,
}

fn cargo_package(files: &BTreeMap<&str, &SourceFile>) -> Value {
    let get = |doc: &toml::Table, key: &str| -> Option<String> {
        let pkg = doc.get("package")?.as_table()?;
        match pkg.get(key)? {
            toml::Value::String(s) => Some(s.clone()),
            toml::Value::Table(t)
                if t.get("workspace").and_then(toml::Value::as_bool) == Some(true) =>
            {
                doc.get("workspace")?
                    .get("package")?
                    .get(key)?
                    .as_str()
                    .map(str::to_string)
            }
            _ => None,
        }
    };
    let doc: toml::Table = files
        .get("Cargo.toml")
        .and_then(|f| f.text.as_deref())
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();
    json!({
        "crate": get(&doc, "name").unwrap_or_default(),
        "edition": get(&doc, "edition").unwrap_or_else(|| "2015".into()),
        "rust_version": get(&doc, "rust-version"),
        "version": get(&doc, "version").unwrap_or_default(),
    })
}

/// `dependencies` from a `Cargo.lock` text: registry packages only (those with a `source`), sorted by
/// (name, version).
pub fn lock_dependencies(lock: Option<&str>) -> Result<Vec<Value>, String> {
    let Some(text) = lock else {
        return Ok(Vec::new());
    };
    let doc: toml::Table = text.parse().map_err(|e| format!("Cargo.lock: {e}"))?;
    let mut v: Vec<(String, String, Value)> = Vec::new();
    if let Some(pkgs) = doc.get("package").and_then(toml::Value::as_array) {
        for p in pkgs {
            if p.get("source").is_none() {
                continue;
            }
            let s = |k: &str| p.get(k).and_then(toml::Value::as_str).map(str::to_string);
            let (Some(name), Some(version)) = (s("name"), s("version")) else {
                continue;
            };
            v.push((
                name.clone(),
                version.clone(),
                json!({"checksum": s("checksum"), "name": name, "version": version}),
            ));
        }
    }
    v.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    Ok(v.into_iter().map(|x| x.2).collect())
}

/// The `module` object of one scanned file.
pub fn module_value(path: &str, s: &FileScan) -> Value {
    let items: Vec<Value> = s
        .items
        .iter()
        .map(|i| json!({"end_line": i.end_line, "kind": i.kind, "line": i.line, "name": i.name, "tokens": i.tokens, "vis": i.vis}))
        .collect();
    json!({"forbid_unsafe": s.forbid_unsafe, "items": items, "lines": s.lines, "path": path, "tokens": s.tokens})
}

/// An empty, fresh directory under `scratch` for a child's cwd.
fn fresh_dir(scratch: &Path, name: &str) -> std::io::Result<PathBuf> {
    let d = scratch.join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d)?;
    }
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

fn find_component(dir: &Path, name: &str, krate: &str) -> Option<PathBuf> {
    [
        format!("{name}.wasm"),
        format!("{}.wasm", krate.replace('-', "_")),
    ]
    .into_iter()
    .map(|f| dir.join(f))
    .find(|p| p.is_file())
}

/// Generates the committed `self.json` of a body tree.
pub fn generate(cfg: &BodyConfig) -> Generated {
    let tok = Tokenizer::o200k();
    let mut diags: Vec<Diag> = Vec::new();
    let mut warnings: BTreeSet<String> = BTreeSet::new();

    let (files, w) = match list_files(&cfg.root, cfg.mode, "self.json", &tok) {
        Ok(x) => x,
        Err(e) => {
            diags.push(Diag::new(
                diag::SELFMAP_ERROR,
                format!("cannot list the tree: {e}"),
            ));
            (Vec::new(), Vec::new())
        }
    };
    warnings.extend(w);
    let by_path: BTreeMap<&str, &SourceFile> = files.iter().map(|f| (f.path.as_str(), f)).collect();
    let counted = |f: &SourceFile| !f.symlink && f.tokens.is_some() && in_budget(&f.path);

    // Budget.
    let (mut b_tokens, mut b_bytes, mut b_files, mut t_tokens, mut u_tokens) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    for f in &files {
        let t = f.tokens.unwrap_or(0);
        if counted(f) {
            b_tokens += t;
            b_bytes += f.bytes;
            b_files += 1;
        } else if is_test(&f.path) {
            t_tokens += t;
        } else {
            u_tokens += t;
        }
    }

    // Manifest and tools.
    let mut manifest_obj = Map::new();
    let mut tools: Vec<Value> = Vec::new();
    let raw = match &cfg.manifest {
        ManifestSource::Json(b) => Ok(b.clone()),
        ManifestSource::Binary(bin) => fresh_dir(&cfg.scratch, "manifest-cwd")
            .map_err(|e| format!("cannot create the manifest cwd: {e}"))
            .and_then(|cwd| {
                let r = run_manifest(bin, &cwd);
                let _ = std::fs::remove_dir_all(&cwd);
                r
            }),
    };
    match raw
        .and_then(|b| parse_manifest(&b))
        .and_then(|m| manifest_tools(&m).map(|t| (m, t)))
    {
        Ok((m, t)) => {
            let ours = tools_sha256(&t);
            match m.get("tools_sha256").and_then(Value::as_str) {
                Some(theirs) if theirs == ours => {}
                other => diags.push(Diag::new(
                    diag::MANIFEST_TOOLS_HASH,
                    format!(
                        "the manifest reports tools_sha256 {}, init-tools-v1 of its tools is {ours}",
                        other.map(|s| format!("`{s}`")).unwrap_or_else(|| "(none)".into())
                    ),
                )),
            }
            manifest_obj = m;
            manifest_obj.remove("tools");
            tools = t;
        }
        Err(e) => diags.push(Diag::new(diag::MANIFEST_ERROR, e)),
    }
    let tool_names: BTreeSet<String> = tools
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();

    // Components and markers.
    let mut comps: BTreeMap<String, Comp> = BTreeMap::new();
    let mut marks: Vec<Mark> = Vec::new();
    let add_path = |comps: &mut BTreeMap<String, Comp>,
                    id: &str,
                    kind: &'static str,
                    path: &str,
                    tokens: u64| {
        let c = comps.entry(id.to_string()).or_insert(Comp {
            kind,
            core: false,
            tokens: 0,
            paths: BTreeSet::new(),
        });
        c.tokens += tokens;
        c.paths.insert(path.to_string());
    };

    // Rust modules.
    let mut modules: Vec<Value> = Vec::new();
    for f in files
        .iter()
        .filter(|f| counted(f) && f.path.ends_with(".rs"))
    {
        let s = scan_rust(f.text.as_deref().unwrap_or(""), &tok);
        if let Some((line, msg)) = &s.parse_error {
            diags.push(
                Diag::new(diag::SELFMAP_ERROR, format!("{}:{line}: {msg}", f.path))
                    .at(&f.path, Some(*line)),
            );
            continue;
        }
        for l in &s.not_doc {
            warnings.insert(format!("marker_not_doc: {}:{l}", f.path));
        }
        let comp = file_component(&f.path, &tool_names);
        for m in &s.markers {
            match (markers::parse_value(&m.raw), &comp) {
                (Err(e), _) => diags.push(
                    Diag::new(
                        diag::MARKER_GRAMMAR,
                        format!("{}:{}: {e}", f.path, m.marker_line),
                    )
                    .at(&f.path, Some(m.marker_line)),
                ),
                (Ok(_), None) => {
                    warnings.insert(format!("marker_unattached: {}:{}", f.path, m.marker_line));
                }
                (Ok(v), Some(c)) => marks.push(Mark {
                    component: c.clone(),
                    slug: v.slug,
                    note: v.note,
                    path: f.path.clone(),
                    line: m.line,
                    end_line: m.end_line,
                    item: m.item.clone(),
                    tokens: m.tokens,
                }),
            }
        }
        modules.push(module_value(&f.path, &s));
    }
    for f in &files {
        if f.symlink || !glob("src/**/*.rs", &f.path) {
            continue;
        }
        match file_component(&f.path, &tool_names) {
            Some(id) if id.starts_with("mod/") => {
                add_path(&mut comps, &id, "mod", &f.path, f.tokens.unwrap_or(0))
            }
            Some(id) if id.starts_with("profile/") => {
                add_path(&mut comps, &id, "profile", &f.path, f.tokens.unwrap_or(0))
            }
            _ => {}
        }
    }

    // Tools.
    for t in &tools {
        let name = t["name"].as_str().unwrap_or_default();
        let id = format!("tool/{name}");
        let schema_tokens = tok.count(&to_icj_string(t));
        let c = comps.entry(id).or_insert(Comp {
            kind: "tool",
            core: CORE_TOOLS.contains(&name),
            tokens: schema_tokens,
            paths: BTreeSet::new(),
        });
        let src = format!("src/tools/{name}.rs");
        if let Some(f) = by_path.get(src.as_str()) {
            c.tokens += f.tokens.unwrap_or(0);
            c.paths.insert(src);
        }
    }

    // Extensions.
    let mut extensions: Vec<Value> = Vec::new();
    let ext_names: BTreeSet<&str> = files
        .iter()
        .filter_map(|f| {
            let rest = f.path.strip_prefix("ext/")?.strip_suffix("/Cargo.toml")?;
            (!rest.contains('/')).then_some(rest)
        })
        .collect();
    for name in &ext_names {
        let id = format!("ext/{name}");
        let comp_path = format!("ext/{name}");
        comps.entry(id.clone()).or_insert(Comp {
            kind: "ext",
            core: false,
            tokens: 0,
            paths: BTreeSet::new(),
        });
        for f in files
            .iter()
            .filter(|f| f.path.starts_with(&format!("ext/{name}/")) && counted(f))
        {
            add_path(&mut comps, &id, "ext", &f.path, f.tokens.unwrap_or(0));
        }
        let krate = by_path
            .get(format!("ext/{name}/Cargo.toml").as_str())
            .and_then(|f| f.text.as_deref())
            .and_then(|t| t.parse::<toml::Table>().ok())
            .and_then(|d| d.get("package")?.get("name")?.as_str().map(str::to_string))
            .unwrap_or_else(|| name.to_string());
        let ext_err = |msg: String| {
            Diag::new(diag::EXT_CONTRACT, format!("{id}: {msg}")).at(&comp_path, None)
        };
        let Some(wasm) = cfg
            .components
            .as_deref()
            .and_then(|d| find_component(d, name, &krate))
        else {
            diags.push(ext_err("no built component".into()));
            continue;
        };
        let bytes = match std::fs::read(&wasm) {
            Ok(b) => b,
            Err(e) => {
                diags.push(ext_err(format!("cannot read {}: {e}", wasm.display())));
                continue;
            }
        };
        let inv = match wit_inventory(&bytes) {
            Ok(i) => i,
            Err(e) => {
                diags.push(ext_err(format!("does not decode as a component: {e:#}")));
                continue;
            }
        };
        let exported = |n: &str| {
            inv.exports
                .iter()
                .any(|i| matches!(i, WitItem::Func { name, .. } if name == n))
        };
        if !exported("describe") || !exported("call") {
            diags.push(ext_err(
                "the world-level exports `describe` and `call` are missing".into(),
            ));
            continue;
        }
        let described = fresh_dir(&cfg.scratch, "describe-cwd")
            .map_err(|e| format!("cannot create the describe cwd: {e}"))
            .and_then(|cwd| {
                let r = cfg.describe.run(&wasm, &cwd);
                let _ = std::fs::remove_dir_all(&cwd);
                r
            })
            .and_then(|(api, info)| crate::describe::validate(&info, name).map(|d| (api, d)));
        let (api, describe) = match described {
            Ok(x) => x,
            Err(e) => {
                diags.push(ext_err(e));
                continue;
            }
        };
        extensions.push(json!({
            "api": api,
            "crate": krate,
            "describe": describe,
            "name": name,
            "wit_exports": inv.exports.iter().map(WitItem::to_value).collect::<Vec<_>>(),
            "wit_imports": inv.imports.iter().map(WitItem::to_value).collect::<Vec<_>>(),
            "wit_sha256": inv.wit_sha256,
        }));
    }

    // Skills.
    let mut skills: Vec<Value> = Vec::new();
    let skill_dirs: BTreeSet<&str> = files
        .iter()
        .filter_map(|f| {
            let rest = f.path.strip_prefix("skills/")?.strip_suffix("/SKILL.md")?;
            (!rest.contains('/')).then_some(rest)
        })
        .collect();
    for dir in skill_dirs {
        let path = format!("skills/{dir}/SKILL.md");
        let text = by_path
            .get(path.as_str())
            .and_then(|f| f.text.as_deref())
            .unwrap_or("");
        let fm = front_matter(text);
        let name = fm
            .name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| dir.to_string());
        let id = format!("skill/{name}");
        let mut tokens = 0;
        for f in files
            .iter()
            .filter(|f| f.path.starts_with(&format!("skills/{dir}/")))
        {
            tokens += f.tokens.unwrap_or(0);
            add_path(&mut comps, &id, "skill", &f.path, f.tokens.unwrap_or(0));
        }
        if let Some((line, raw)) = &fm.compensates {
            match markers::parse_value(raw) {
                Ok(v) => marks.push(Mark {
                    component: id.clone(),
                    slug: v.slug,
                    note: v.note,
                    path: path.clone(),
                    line: *line,
                    end_line: text.lines().count().max(*line),
                    item: None,
                    tokens: by_path
                        .get(path.as_str())
                        .and_then(|f| f.tokens)
                        .unwrap_or(0),
                }),
                Err(e) => diags.push(
                    Diag::new(diag::MARKER_GRAMMAR, format!("{path}:{line}: {e}"))
                        .at(&path, Some(*line)),
                ),
            }
        }
        skills.push(json!({
            "description": fm.description.clone().unwrap_or_default(),
            "name": name,
            "path": path,
            "tokens": tokens,
        }));
    }

    // Profiles.
    let profiles: Vec<Value> = files
        .iter()
        .filter_map(|f| profile_family(&f.path).map(|fam| (fam, f)))
        .map(|(fam, f)| json!({"family": fam, "paths": [f.path], "tokens": f.tokens.unwrap_or(0)}))
        .collect();

    // Prompt blocks.
    for file in ["GENESIS.md", "SELF.md"] {
        let Some(text) = by_path.get(file).and_then(|f| f.text.as_deref()) else {
            continue;
        };
        let (blocks, errors) = prompt_blocks(text);
        for (line, e) in errors {
            diags.push(
                Diag::new(diag::MARKER_GRAMMAR, format!("{file}:{line}: {e}")).at(file, Some(line)),
            );
        }
        for b in blocks {
            match markers::parse_value(&b.raw) {
                Ok(v) => {
                    let id = format!("prompt/{file}/{}", v.slug);
                    let tokens = tok.count(&b.text);
                    add_path(&mut comps, &id, "prompt", file, tokens);
                    marks.push(Mark {
                        component: id,
                        slug: v.slug,
                        note: v.note,
                        path: file.to_string(),
                        line: b.line,
                        end_line: b.end_line,
                        item: None,
                        tokens,
                    });
                }
                Err(e) => diags.push(
                    Diag::new(diag::MARKER_GRAMMAR, format!("{file}:{}: {e}", b.line))
                        .at(file, Some(b.line)),
                ),
            }
        }
    }

    // One slug per component.
    marks.sort_by(|a, b| {
        (&a.path, a.line, a.end_line, &a.slug, &a.note, &a.item)
            .cmp(&(&b.path, b.line, b.end_line, &b.slug, &b.note, &b.item))
    });
    let mut tag: BTreeMap<&str, (&str, Option<&str>)> = BTreeMap::new();
    let mut slugs: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for m in &marks {
        tag.entry(&m.component)
            .or_insert((&m.slug, m.note.as_deref()));
        slugs.entry(&m.component).or_default().insert(&m.slug);
    }
    for (c, s) in &slugs {
        if s.len() > 1 {
            diags.push(Diag::new(
                diag::MARKER_CONFLICT,
                format!(
                    "{c} carries {} different slugs ({}); one weakness per component: split it",
                    s.len(),
                    s.iter().copied().collect::<Vec<_>>().join(", ")
                ),
            ));
        }
    }

    let components: Vec<Value> = comps
        .iter()
        .map(|(id, c)| {
            let (slug, note) = tag.get(id.as_str()).copied().unwrap_or(("none", None));
            json!({
                "compensates_for": slug,
                "compensates_note": note,
                "core": c.core,
                "id": id,
                "kind": c.kind,
                "paths": c.paths.iter().collect::<Vec<_>>(),
                "tokens": c.tokens,
            })
        })
        .collect();
    let markers_v: Vec<Value> = marks
        .iter()
        .map(|m| {
            json!({
                "component": m.component, "compensates_for": m.slug, "end_line": m.end_line, "item": m.item,
                "line": m.line, "note": m.note, "path": m.path, "tokens": m.tokens,
            })
        })
        .collect();

    let dependencies =
        match lock_dependencies(by_path.get("Cargo.lock").and_then(|f| f.text.as_deref())) {
            Ok(d) => d,
            Err(e) => {
                diags.push(Diag::new(diag::SELFMAP_ERROR, e).at("Cargo.lock", None));
                Vec::new()
            }
        };
    let files_v: Vec<Value> = files
        .iter()
        .map(|f| json!({"bytes": f.bytes, "counted": counted(f), "path": f.path, "sha256": f.sha256, "tokens": f.tokens}))
        .collect();
    let mut ext_sorted = extensions;
    ext_sorted.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let mut skills_sorted = skills;
    skills_sorted.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let mut modules_sorted = modules;
    modules_sorted.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));

    let value = json!({
        "body": cargo_package(&by_path),
        "body_tokens": b_tokens,
        "budget": {"bytes": b_bytes, "files": b_files, "tests_tokens": t_tokens, "tokens": b_tokens, "uncounted_tokens": u_tokens},
        "components": components,
        "dependencies": dependencies,
        "extensions": ext_sorted,
        "files": files_v,
        "generator": crate::generator_value(),
        "hooks": [],
        "manifest": Value::Object(manifest_obj),
        "markers": markers_v,
        "modules": modules_sorted,
        "profiles": profiles,
        "self_v": crate::SELF_V,
        "skills": skills_sorted,
        "source_digest": source_digest(&files),
        "tools": tools_object(&tools, &tok),
        "warnings": warnings.into_iter().collect::<Vec<_>>(),
    });
    let bytes = to_pretty(&value);
    Generated {
        value,
        bytes,
        diags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_set() {
        for p in [
            "src/main.rs",
            "src/a/b.rs",
            "build.rs",
            "Cargo.toml",
            ".cargo/config.toml",
            "GENESIS.md",
            "SELF.md",
            "skills/x/SKILL.md",
            "skills/x/assets/t.txt",
            "ext/Cargo.toml",
            "ext/pty/src/lib.rs",
            "ext/pty/Cargo.toml",
            "ext/pty/build.rs",
        ] {
            assert!(in_budget(p), "{p}");
        }
        for p in [
            "Cargo.lock",
            "ext/Cargo.lock",
            "tests/a.rs",
            "ext/pty/tests/a.rs",
            "wit/init-ext/v1/init-ext.wit",
            "README.md",
            "src/data.json",
            "ext/pty/README.md",
        ] {
            assert!(!in_budget(p), "{p}");
        }
        assert!(is_test("tests/a.rs") && is_test("ext/pty/tests/x.rs") && !is_test("src/tests.rs"));
    }

    #[test]
    fn component_ids() {
        let tools: BTreeSet<String> = ["bash".to_string()].into();
        assert_eq!(
            file_component("src/tools/bash.rs", &tools).as_deref(),
            Some("tool/bash")
        );
        assert_eq!(
            file_component("src/tools/mod.rs", &tools).as_deref(),
            Some("mod/src/tools/mod.rs")
        );
        assert_eq!(
            file_component("src/profiles/deepseek.rs", &tools).as_deref(),
            Some("profile/deepseek")
        );
        assert_eq!(
            file_component("src/profiles/mod.rs", &tools).as_deref(),
            Some("mod/src/profiles/mod.rs")
        );
        assert_eq!(
            file_component("ext/pty/src/lib.rs", &tools).as_deref(),
            Some("ext/pty")
        );
        assert_eq!(
            file_component("src/loop.rs", &tools).as_deref(),
            Some("mod/src/loop.rs")
        );
        assert_eq!(file_component("build.rs", &tools), None);
    }

    #[test]
    fn lock_deps_registry_only_sorted() {
        let lock = r#"
version = 4
[[package]]
name = "zeta"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aa"
[[package]]
name = "init-body"
version = "0.1.0"
[[package]]
name = "alpha"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
        let d = lock_dependencies(Some(lock)).unwrap();
        assert_eq!(
            to_icj_string(&Value::Array(d)),
            r#"[{"checksum":null,"name":"alpha","version":"2.0.0"},{"checksum":"aa","name":"zeta","version":"1.0.0"}]"#
        );
    }
}
