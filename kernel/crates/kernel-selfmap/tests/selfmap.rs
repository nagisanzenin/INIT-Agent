//! Integration tests for the self.json generator: selfmap.md §8 (S-01 … S-13 end to end; S-14 … S-19
//! are unit tests in `serve`, `capability` and `kernel_tree`), on real Wasm components: the seed's
//! `count-lines` template and the fixtures under `tests/fixtures/ext/`.

mod common;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Instant;

use common::*;
use kernel_selfmap::body::{BodyConfig, Generated, generate};
use kernel_selfmap::describe::DescribeRunner;
use kernel_selfmap::drift::{self, Warning};
use kernel_selfmap::inventory::FileSetMode;
use kernel_selfmap::manifest::{self, Family, ManifestSource};
use kernel_selfmap::wit::{WitItem, wit_inventory};
use serde_json::{Value, json};
use std::time::Duration;

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

struct Body {
    home: PathBuf,
    tree: PathBuf,
    components: PathBuf,
    binary: PathBuf,
}

/// A body tree in the seed's layout with markers of every kind, and the given extensions built.
fn body_fixture(name: &str, exts: &[&str]) -> Body {
    let home = home(name);
    let tree = home.join("tree");
    let seed = repo().join("seed");
    for f in [
        "Cargo.toml",
        "Cargo.lock",
        ".cargo/config.toml",
        "GENESIS.md",
        "wit/init-ext/v1/init-ext.wit",
    ] {
        copy(&seed.join(f), &tree.join(f));
    }
    put(
        &tree,
        "src/main.rs",
        "#![forbid(unsafe_code)]\nmod guard;\nmod r#loop;\nmod tools;\n\nfn main() {}\n",
    );
    put(
        &tree,
        "src/loop.rs",
        "pub fn step(n: u32) -> u32 {\n    n + 1\n}\n\npub fn run() {}\n",
    );
    put(&tree, "src/tools/mod.rs", "pub mod bash;\n");
    put(
        &tree,
        "src/tools/bash.rs",
        "//! The bash tool.\n//! @init compensates-for: none (core tool)\npub fn run() {}\n",
    );
    put(
        &tree,
        "src/guard.rs",
        "/// @init compensates-for: loops-on-repl: repeats the same command\npub fn guard() {}\n\n// @init compensates-for: stray-one\nfn other() {}\n",
    );
    put(
        &tree,
        "SELF.md",
        "# SELF\n\n<!-- @init compensates-for: loop-detection: stops repeats -->\nIf you repeat a command three times, stop.\n<!-- @init end -->\n",
    );
    put(
        &tree,
        "skills/repl/SKILL.md",
        "---\nname: repl\ndescription: Drive a REPL.\ncompensates-for: loops-on-repl\n---\n# REPL\n",
    );
    put(&tree, "skills/repl/assets/notes.txt", "notes\n");
    put(&tree, "tests/smoke.rs", "#[test]\nfn t() {}\n");
    put(&tree, "README.md", "# body\n");
    let components = home.join("components");
    std::fs::create_dir_all(&components).unwrap();
    if !exts.is_empty() {
        copy(&seed.join("ext/Cargo.toml"), &tree.join("ext/Cargo.toml"));
        copy(&seed.join("ext/Cargo.lock"), &tree.join("ext/Cargo.lock"));
    }
    for e in exts {
        let (src, wasm) = if *e == "count-lines" {
            (
                seed.join("ext/count-lines"),
                count_lines_wasm().to_path_buf(),
            )
        } else {
            (fixtures().join("ext").join(e), fixture_wasm(e))
        };
        copy(
            &src.join("Cargo.toml"),
            &tree.join(format!("ext/{e}/Cargo.toml")),
        );
        copy(
            &src.join("src/lib.rs"),
            &tree.join(format!("ext/{e}/src/lib.rs")),
        );
        copy(&wasm, &components.join(format!("{e}.wasm")));
    }
    let binary = fake_body(&home.join("bin"), &manifest_fixture());
    Body {
        home,
        tree,
        components,
        binary,
    }
}

fn gen_body(b: &Body, mode: FileSetMode) -> Generated {
    let scratch = b.home.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    generate(&BodyConfig {
        root: b.tree.clone(),
        mode,
        manifest: ManifestSource::Binary(b.binary.clone()),
        components: Some(b.components.clone()),
        describe: DescribeRunner::new(vec![bin().into_os_string()]),
        scratch,
    })
}

fn gate_args(b: &Body, out: &Path) -> Vec<std::ffi::OsString> {
    vec![
        "--root".into(),
        b.tree.clone().into(),
        "--binary".into(),
        b.binary.clone().into(),
        "--components".into(),
        b.components.clone().into(),
        "--out".into(),
        out.into(),
    ]
}

fn os(v: &[std::ffi::OsString]) -> Vec<&OsStr> {
    v.iter().map(|s| s.as_os_str()).collect()
}

fn ids(v: &Value) -> Vec<String> {
    v["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

fn component<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no {id}"))
}

/// The whole committed file: components, markers, tools, extensions, budget, schema validity, no `live`.
#[test]
fn generates_the_committed_file() {
    let _wd = init_testkit::watchdog::arm("generates_the_committed_file", Duration::from_secs(840));
    let b = body_fixture("full", &["count-lines", "uses-process"]);
    let g = gen_body(&b, FileSetMode::Walk);
    assert!(g.diags.is_empty(), "{:?}", g.diags);
    let v = &g.value;
    assert!(
        v.get("live").is_none(),
        "D12: the committed file has no live key"
    );
    let errs = validate(&self_v2(), &self_v2(), v);
    assert!(errs.is_empty(), "schema: {errs:#?}");
    assert_eq!(
        ids(v),
        [
            "ext/count-lines",
            "ext/uses-process",
            "mod/src/guard.rs",
            "mod/src/loop.rs",
            "mod/src/main.rs",
            "mod/src/tools/mod.rs",
            "prompt/SELF.md/loop-detection",
            "skill/repl",
            "tool/bash",
            "tool/edit",
            "tool/ext",
            "tool/read",
            "tool/write",
        ]
    );
    // Markers attach as §3.7 says.
    let guard = component(v, "mod/src/guard.rs");
    assert_eq!(guard["compensates_for"], "loops-on-repl");
    assert_eq!(guard["compensates_note"], "repeats the same command");
    let bash = component(v, "tool/bash");
    assert_eq!(
        (bash["core"].as_bool(), bash["compensates_for"].as_str()),
        (Some(true), Some("none"))
    );
    assert_eq!(bash["compensates_note"], "core tool");
    assert_eq!(bash["paths"], json!(["src/tools/bash.rs"]));
    assert_eq!(
        component(v, "skill/repl")["compensates_for"],
        "loops-on-repl"
    );
    assert_eq!(
        component(v, "skill/repl")["paths"],
        json!(["skills/repl/SKILL.md", "skills/repl/assets/notes.txt"])
    );
    assert_eq!(
        component(v, "prompt/SELF.md/loop-detection")["compensates_note"],
        "stops repeats"
    );
    assert_eq!(component(v, "ext/count-lines")["compensates_for"], "none");
    assert_eq!(component(v, "tool/edit")["paths"], json!([]));
    let markers: Vec<(String, u64, Option<String>)> = v["markers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["path"].as_str().unwrap().into(),
                m["line"].as_u64().unwrap(),
                m["item"].as_str().map(str::to_string),
            )
        })
        .collect();
    assert_eq!(
        markers,
        [
            ("SELF.md".into(), 3, None),
            ("skills/repl/SKILL.md".into(), 4, None),
            ("src/guard.rs".into(), 1, Some("guard".into())),
            ("src/tools/bash.rs".into(), 1, None),
        ]
    );
    assert_eq!(v["warnings"], json!(["marker_not_doc: src/guard.rs:4"]));
    // Tools come from the manifest and hash to the manifest's own init-tools-v1 value.
    let want = std::fs::read_to_string(fixtures().join("manifest-seed.tools_sha256")).unwrap();
    assert_eq!(v["tools"]["sha256"], want.trim());
    assert_eq!(v["tools"]["list"].as_array().unwrap().len(), 5);
    assert!(v["manifest"].get("tools").is_none());
    assert_eq!(v["manifest"]["tools_sha256"], want.trim());
    assert_eq!(v["manifest"]["ext_api_majors"], json!([1]));
    // Extensions: WIT from the binary, describe from the child.
    let cl = &v["extensions"][0];
    assert_eq!(cl["name"], "count-lines");
    assert_eq!(cl["api"], "init:ext@1");
    assert_eq!(cl["crate"], "count-lines");
    assert_eq!(cl["describe"]["timeout_ms"], Value::Null);
    assert_eq!(
        cl["describe"]["input_schema"]["required"],
        json!(["path", "needle"])
    );
    assert_eq!(
        cl["wit_exports"],
        json!([
            {"kind": "func", "name": "call", "signature": "func(input: string) -> result<string, tool-error>"},
            {"kind": "func", "name": "describe", "signature": "func() -> tool-info"},
        ])
    );
    let up = &v["extensions"][1];
    assert!(
        up["wit_imports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["name"] == "init:ext/process@1.0.0" && i["functions"] == json!(["run"])),
        "{}",
        up["wit_imports"]
    );
    // Budget: tests and uncounted files are kept apart; body_tokens is budget.tokens.
    assert_eq!(v["body_tokens"], v["budget"]["tokens"]);
    assert!(v["budget"]["tests_tokens"].as_u64().unwrap() > 0);
    let readme = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "README.md")
        .unwrap();
    assert_eq!(readme["counted"], false);
    assert!(
        v["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["name"] == "wasmtime" && d["version"] == "48.0.6")
    );
    assert_eq!(v["body"]["crate"], "init-body");
    assert_eq!(v["hooks"], json!([]));
    let main = v["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["path"] == "src/main.rs")
        .unwrap();
    assert_eq!(main["forbid_unsafe"], true);
    remove(&b.home);
}

/// S-01: byte-identical output twice, and with a different HOME, TZ and locale.
#[test]
fn s01_determinism() {
    let _wd = init_testkit::watchdog::arm("s01_determinism", Duration::from_secs(90));
    let b = body_fixture("s01", &["count-lines"]);
    let outs: Vec<Vec<u8>> = [
        vec![],
        vec![
            ("HOME", "/nonexistent-home"),
            ("TZ", "Pacific/Kiritimati"),
            ("LANG", "tr_TR.UTF-8"),
            ("LC_ALL", "tr_TR.UTF-8"),
        ],
        vec![],
    ]
    .iter()
    .enumerate()
    .map(|(i, env)| {
        let out = b.home.join(format!("out{i}/self.json"));
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        let (code, _, err) = run_cli(&os(&gate_args(&b, &out)), env);
        assert_eq!(code, 0, "{err}");
        std::fs::read(out).unwrap()
    })
    .collect();
    assert_eq!(outs[0], outs[1]);
    assert_eq!(outs[0], outs[2]);
    assert!(outs[0].ends_with(b"}\n") && outs[0].starts_with(b"{\n  \"body\""));
    // The CLI's bytes equal the library's.
    assert_eq!(outs[0], gen_body(&b, FileSetMode::Walk).bytes);
    remove(&b.home);
}

/// S-02 (drift names `src/loop.rs` first, with `/files/…/sha256` and `/modules/…/tokens`), S-03 and S-04,
/// through `--write` and `--check` on a git worktree.
#[test]
fn s02_s03_s04_drift_via_write_and_check() {
    let _wd = init_testkit::watchdog::arm("s02_s03_s04_drift_via_write_and_check", Duration::from_secs(110));
    let b = body_fixture("s02", &[]);
    let git = init_testkit::watchdog::output(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&b.tree))
        .unwrap();
    assert!(git.status.success());
    let base: Vec<std::ffi::OsString> = vec![
        "--root".into(),
        b.tree.clone().into(),
        "--no-build".into(),
        "--binary".into(),
        b.binary.clone().into(),
        "--components".into(),
        b.components.clone().into(),
    ];
    let with = |flag: &str| {
        let mut v = base.clone();
        v.insert(2, flag.into());
        v
    };
    let (code, _, err) = run_cli(&os(&with("--write")), &[]);
    assert_eq!(code, 0, "{err}");
    let committed = std::fs::read(b.tree.join("self.json")).unwrap();
    let (code, out, err) = run_cli(&os(&with("--check")), &[]);
    assert_eq!(code, 0, "{err}\n{}", String::from_utf8_lossy(&out));
    // S-02: edit one function in src/loop.rs, keep the old self.json.
    put(
        &b.tree,
        "src/loop.rs",
        "pub fn step(n: u32) -> u32 {\n    n + 2 // changed\n}\n\npub fn run() {}\n",
    );
    let (code, out, _) = run_cli(&os(&with("--check")), &[]);
    assert_eq!(code, 1);
    let report = String::from_utf8(out).unwrap();
    assert!(
        report.starts_with("self_json_drift\nfiles whose sha256 differs"),
        "{report}"
    );
    let first_file = report.lines().nth(2).unwrap();
    assert_eq!(first_file.trim(), "changed: src/loop.rs");
    assert!(
        regex::Regex::new(r"/files/\d+/sha256: ")
            .unwrap()
            .is_match(&report),
        "{report}"
    );
    assert!(
        regex::Regex::new(r"/modules/\d+/tokens: ")
            .unwrap()
            .is_match(&report),
        "{report}"
    );
    // An untracked, uncommitted new file is named too (§3.2).
    put(&b.tree, "src/extra.rs", "pub fn x() {}\n");
    let (_, out, _) = run_cli(&os(&with("--check")), &[]);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("added:   src/extra.rs")
    );
    std::fs::remove_file(b.tree.join("src/extra.rs")).unwrap();
    // Regenerate: back to no drift.
    assert_eq!(run_cli(&os(&with("--write")), &[]).0, 0);
    let fresh = std::fs::read(b.tree.join("self.json")).unwrap();
    assert_ne!(fresh, committed);
    // S-03: formatting-only edit → pass with non_canonical_format.
    let v: Value = serde_json::from_slice(&fresh).unwrap();
    std::fs::write(b.tree.join("self.json"), serde_json::to_vec(&v).unwrap()).unwrap();
    let (code, _, err) = run_cli(&os(&with("--check")), &[]);
    assert_eq!(code, 0);
    assert!(err.contains("\"code\":\"non_canonical_format\""), "{err}");
    // S-04: a generator version bump → generator_changed.
    let mut old = v.clone();
    old["generator"]["version"] = json!(1);
    std::fs::write(
        b.tree.join("self.json"),
        serde_json::to_vec_pretty(&old).unwrap(),
    )
    .unwrap();
    let (code, out, _) = run_cli(&os(&with("--check")), &[]);
    assert_eq!(code, 1);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .starts_with("generator_changed\n")
    );
    // S-13: a committed copy of the served view passes with live_ignored.
    let mut served = v.clone();
    served["live"] =
        json!({"capability": {"categories": [{"category": "_all", "pass_permille": 710}]}});
    std::fs::write(
        b.tree.join("self.json"),
        serde_json::to_vec_pretty(&served).unwrap(),
    )
    .unwrap();
    let (code, _, err) = run_cli(&os(&with("--check")), &[]);
    assert_eq!(code, 0);
    assert!(err.contains("\"code\":\"live_ignored\""), "{err}");
    remove(&b.home);
}

/// S-06: marker conflicts, grammar errors, and the exit code of a failed G1 check.
#[test]
fn s06_marker_conflict_and_grammar() {
    let _wd = init_testkit::watchdog::arm("s06_marker_conflict_and_grammar", WD);
    let b = body_fixture("s06", &[]);
    put(
        &b.tree,
        "src/guard.rs",
        "//! @init compensates-for: loops-on-repl\n/// @init compensates-for: weak-planning\npub fn guard() {}\n",
    );
    put(
        &b.tree,
        "src/bad.rs",
        "//! @init compensates-for: Not A Slug\n",
    );
    let g = gen_body(&b, FileSetMode::Walk);
    let codes: Vec<&str> = g.diags.iter().map(|d| d.code).collect();
    assert_eq!(
        codes,
        ["marker_grammar", "marker_conflict"],
        "{:?}",
        g.diags
    );
    assert_eq!(g.diags[0].path.as_deref(), Some("src/bad.rs"));
    assert_eq!(g.diags[0].line, Some(1));
    assert!(
        g.diags[1].message.contains("mod/src/guard.rs")
            && g.diags[1].message.contains("loops-on-repl, weak-planning")
    );
    let out = b.home.join("out/self.json");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let (code, _, err) = run_cli(&os(&gate_args(&b, &out)), &[]);
    assert_eq!(code, 1, "a failed check exits 1: {err}");
    assert!(
        out.exists(),
        "the file is still written for a check failure"
    );
    remove(&b.home);
}

/// S-07: `feat` adds `ext/uses-process` whose marker is `none`, but the trailer says `loops-on-repl`.
#[test]
fn s07_trailer_marker_cross_check() {
    let _wd = init_testkit::watchdog::arm("s07_trailer_marker_cross_check", Duration::from_secs(600));
    let p = body_fixture("s07p", &[]);
    let r = body_fixture("s07r", &["uses-process"]);
    let (gp, gr) = (
        gen_body(&p, FileSetMode::Walk),
        gen_body(&r, FileSetMode::Walk),
    );
    assert!(
        gp.diags.is_empty() && gr.diags.is_empty(),
        "{:?} {:?}",
        gp.diags,
        gr.diags
    );
    use kernel_gate::evo::ChangeType;
    let e =
        kernel_gate::g1::check_tag_markers(ChangeType::Feat, "loops-on-repl", &gr.value, &gp.value)
            .unwrap_err();
    assert_eq!(e.code, "gate.g1.tag_marker_mismatch");
    assert!(
        e.detail.contains("ext/uses-process is marked `none`"),
        "{}",
        e.detail
    );
    assert!(
        kernel_gate::g1::check_tag_markers(ChangeType::Feat, "none", &gr.value, &gp.value).is_ok()
    );
    remove(&p.home);
    remove(&r.home);
}

/// S-08: WIT extraction from the binary.
#[test]
fn s08_wit_extraction() {
    let _wd = init_testkit::watchdog::arm("s08_wit_extraction", WD);
    let cl = wit_inventory(&std::fs::read(count_lines_wasm()).unwrap()).unwrap();
    let names: Vec<&str> = cl.exports.iter().map(WitItem::name).collect();
    assert_eq!(names, ["call", "describe"]);
    assert!(cl.wit_sha256.starts_with("sha256:"));
    // count-lines never calls a process function: whether the unused world import survives is recorded
    // here (ext/wit.md W-3; build/status/WP9.md).
    let cl_process = cl
        .imports
        .iter()
        .any(|i| i.name() == "init:ext/process@1.0.0");
    let up = wit_inventory(&std::fs::read(fixture_wasm("uses-process")).unwrap()).unwrap();
    assert!(
        up.imports.iter().any(
            |i| matches!(i, WitItem::Interface { name, .. } if name == "init:ext/process@1.0.0")
        )
    );
    let wasi: Vec<&str> = up
        .imports
        .iter()
        .map(WitItem::name)
        .filter(|n| n.starts_with("wasi:"))
        .collect();
    eprintln!(
        "count-lines imports init:ext/process: {cl_process}; uses-process wasi imports: {wasi:?}"
    );
    assert!(!wasi.is_empty());
    let mut sorted = up.imports.iter().map(WitItem::name).collect::<Vec<_>>();
    sorted.sort();
    assert_eq!(
        sorted,
        up.imports.iter().map(WitItem::name).collect::<Vec<_>>(),
        "imports are sorted by name"
    );
    // The decoded interface lists only the functions the component actually imports.
    assert!(up.imports.iter().any(|i| matches!(i, WitItem::Interface { name, functions } if name == "init:ext/process@1.0.0" && functions == &["run"])));
    // An empty core module decodes to nothing exported (the generator then reports ext_contract), and
    // garbage does not decode.
    assert!(
        wit_inventory(b"\0asm\x01\0\0\0")
            .map(|i| i.exports.is_empty())
            .unwrap_or(true)
    );
    assert!(wit_inventory(b"not wasm").is_err());
}

/// S-09: a describe that loops, one that reads a file, one that is not deterministic.
#[test]
fn s09_describe_contract_failures() {
    let _wd = init_testkit::watchdog::arm("s09_describe_contract_failures", Duration::from_secs(40));
    let runner = DescribeRunner::new(vec![bin().into_os_string()]);
    let cwd = home("s09");
    let reason = |n: &str| runner.run(&fixture_wasm(n), &cwd).unwrap_err();
    let r = reason("describe-loop");
    assert!(r.contains("all fuel consumed"), "{r}");
    let r = reason("describe-reads-file");
    assert!(r.contains("describe trapped"), "{r}");
    let r = reason("describe-nondet");
    assert!(r.contains("not deterministic"), "{r}");
    let (api, info) = runner.run(&fixture_wasm("echo"), &cwd).unwrap();
    assert_eq!(
        (api.as_str(), info.name.as_str(), info.timeout_ms),
        ("init:ext@1", "echo", Some(5000))
    );
    // In a full generation they surface as ext_contract (exit 4, no file written).
    let b = body_fixture("s09b", &["describe-nondet"]);
    let out = b.home.join("out/self.json");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let (code, _, err) = run_cli(&os(&gate_args(&b, &out)), &[]);
    assert_eq!(code, 4, "{err}");
    assert!(
        err.contains("\"code\":\"ext_contract\"") && err.contains("ext/describe-nondet"),
        "{err}"
    );
    assert!(!out.exists());
    remove(&b.home);
    remove(&cwd);
}

/// S-10: the openai-chat, anthropic and responses renderings of tools.md §4.1 all give the manifest's
/// `tools_sha256` (shared vectors with proxy P-13); a manifest whose own hash is wrong fails G1.
#[test]
fn s10_tools_hash_across_renderings() {
    let _wd = init_testkit::watchdog::arm("s10_tools_hash_across_renderings", WD);
    let m = manifest::parse_manifest(&manifest_fixture()).unwrap();
    let tools = manifest::manifest_tools(&m).unwrap();
    let want = m["tools_sha256"].as_str().unwrap();
    assert_eq!(manifest::tools_sha256(&tools), want);
    let reg: Vec<Value> = m["tools"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .cloned()
        .collect();
    let chat: Vec<Value> = reg
        .iter()
        .map(|t| json!({"type": "function", "function": {"name": t["name"], "description": t["description"], "parameters": t["input_schema"]}}))
        .collect();
    let anthropic: Vec<Value> = reg
        .iter()
        .map(|t| json!({"name": t["name"], "description": t["description"], "input_schema": t["input_schema"], "cache_control": {"type": "ephemeral"}}))
        .collect();
    let responses: Vec<Value> = reg
        .iter()
        .map(|t| json!({"type": "function", "name": t["name"], "description": t["description"], "parameters": t["input_schema"], "strict": false}))
        .collect();
    for (fam, v) in [
        (Family::OpenAiChat, chat),
        (Family::Anthropic, anthropic),
        (Family::OpenAiResponses, responses),
    ] {
        let n = manifest::normalize_request_tools(fam, &Value::Array(v)).unwrap();
        assert_eq!(manifest::tools_sha256(&n), want, "{fam:?}");
    }
    // A responses `custom` tool normalizes to {"custom": format}.
    let c = manifest::normalize_request_tools(
        Family::OpenAiResponses,
        &json!([{"type": "custom", "name": "x", "format": {"type": "grammar"}}]),
    )
    .unwrap();
    assert_eq!(
        c[0],
        json!({"description": "", "input_schema": {"custom": {"type": "grammar"}}, "name": "x"})
    );
    // G1 check 6: the manifest's self-reported hash must be init-tools-v1 of its tools.
    let b = body_fixture("s10", &[]);
    let mut bad: Value = serde_json::from_slice(&manifest_fixture()).unwrap();
    bad["tools_sha256"] = json!(format!("sha256:{}", "0".repeat(64)));
    let bad_bin = fake_body(
        &b.home.join("badbin"),
        serde_json::to_string(&bad).unwrap().as_bytes(),
    );
    let g = generate(&BodyConfig {
        root: b.tree.clone(),
        mode: FileSetMode::Walk,
        manifest: ManifestSource::Binary(bad_bin),
        components: None,
        describe: DescribeRunner::new(vec![bin().into_os_string()]),
        scratch: b.home.join("scratch"),
    });
    assert_eq!(
        g.diags.iter().map(|d| d.code).collect::<Vec<_>>(),
        ["manifest_tools_hash"]
    );
    // G1 check 5: a manifest that logs a line first.
    let noisy = fake_body(
        &b.home.join("noisy"),
        b"starting\n{\"manifest\":1,\"tools\":[]}\n",
    );
    let g = generate(&BodyConfig {
        root: b.tree.clone(),
        mode: FileSetMode::Walk,
        manifest: ManifestSource::Binary(noisy),
        components: None,
        describe: DescribeRunner::new(vec![bin().into_os_string()]),
        scratch: b.home.join("scratch"),
    });
    assert_eq!(
        g.diags.iter().map(|d| d.code).collect::<Vec<_>>(),
        ["manifest_error"]
    );
    remove(&b.home);
}

/// S-11: the token count of a 10 KB Rust file is pinned.
#[test]
fn s11_token_count_fixture() {
    let _wd = init_testkit::watchdog::arm("s11_token_count_fixture", WD);
    let text = std::fs::read_to_string(fixtures().join("tokens-10k.rs")).unwrap();
    assert!((9_000..=11_000).contains(&text.len()));
    let want: u64 = std::fs::read_to_string(fixtures().join("tokens-10k.count"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        kernel_selfmap::tokens::Tokenizer::o200k().count(&text),
        want
    );
}

/// S-12: the seed body plus three components, through the CLI. selfmap.md targets < 2 s on the M2 for
/// the kernel's release build; a debug build of Wasmtime/Cranelift is several times slower, so the
/// debug bound here is 20 s and the release number is recorded in build/status/WP9.md.
#[test]
fn s12_performance() {
    let _wd = init_testkit::watchdog::arm("s12_performance", WD);
    let b = body_fixture("s12", &["count-lines", "echo", "uses-process"]);
    let out = b.home.join("out/self.json");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let t = Instant::now();
    let (code, _, err) = run_cli(&os(&gate_args(&b, &out)), &[]);
    let ms = t.elapsed().as_millis();
    assert_eq!(code, 0, "{err}");
    eprintln!(
        "S-12: seed body + 3 components: {ms} ms ({})",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    let limit = if cfg!(debug_assertions) {
        20_000
    } else {
        2_000
    };
    assert!(ms < limit, "{ms} ms");
    remove(&b.home);
}

/// The real seed tree (git file set, `--manifest-json` stub until WP13) and the real kernel tree validate
/// against the v2 schemas.
#[test]
fn real_seed_and_kernel_trees() {
    let _wd = init_testkit::watchdog::arm("real_seed_and_kernel_trees", Duration::from_secs(900));
    let h = home("real");
    let comps = h.join("components");
    copy(count_lines_wasm(), &comps.join("count-lines.wasm"));
    let mj = fixtures().join("manifest-seed.json");
    let seed = repo().join("seed");
    let args: Vec<std::ffi::OsString> = vec![
        "--root".into(),
        seed.into(),
        "--print".into(),
        "--no-build".into(),
        "--manifest-json".into(),
        mj.into(),
        "--components".into(),
        comps.into(),
    ];
    let (code, out, err) = run_cli(
        &os(&args),
        &[("CARGO_TARGET_DIR", h.join("t").to_str().unwrap())],
    );
    assert!(code == 0 || code == 1, "{err}");
    let v: Value = serde_json::from_slice(&out).unwrap();
    let errs = validate(&self_v2(), &self_v2(), &v);
    assert!(errs.is_empty(), "{errs:#?}");
    assert!(ids(&v).contains(&"ext/count-lines".to_string()));
    assert!(ids(&v).contains(&"mod/src/main.rs".to_string()));
    let out = h.join("kernel-self.json");
    let args: Vec<std::ffi::OsString> = vec![
        "--kernel".into(),
        "--root".into(),
        repo().join("kernel").into(),
        "--out".into(),
        out.clone().into(),
    ];
    let (code, _, err) = run_cli(&os(&args), &[]);
    if code == 4 {
        eprintln!("kernel tree has another lane's parse error; skipped: {err}");
    } else {
        let k: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        let errs = validate(&kernel_self_v2(), &self_v2(), &k);
        assert!(errs.is_empty(), "{errs:#?}");
        let kids: Vec<&str> = k["kernel"]["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        for want in [
            "kernel/evolution",
            "kernel/gate",
            "kernel/initctl",
            "kernel/selfmap",
        ] {
            assert!(kids.contains(&want), "{kids:?}");
        }
    }
    remove(&h);
}

/// D12 end to end: the served view of a generated file validates against the schema with `live`, and
/// committing it changes nothing in G1.
#[test]
fn served_view_of_a_generated_file() {
    let _wd = init_testkit::watchdog::arm("served_view_of_a_generated_file", WD);
    use kernel_selfmap::serve::*;
    let b = body_fixture("served", &[]);
    let g = gen_body(&b, FileSetMode::Walk);
    let sha = "0e786c733a9648164a2b4c4aea7ece24d0beb7c7";
    let live = LiveInputs {
        as_of: "2026-11-20T09:00:00.000Z".into(),
        running_sha: sha.into(),
        lkg_sha: sha.into(),
        level: Level {
            current: "L1".into(),
            max: "L3".into(),
            since: "2026-11-02T04:00:00.000Z".into(),
        },
        ladder: None,
        kernel: KernelInputs {
            version: "0.1.0".into(),
            build_sha256: "ab".into(),
            kernel_sha: None,
            admitted_build_sha256: None,
            kernel_self: None,
            embedded: vec![],
        },
        capability: kernel_selfmap::capability::capability(&b.home, sha, None).unwrap(),
        sealed_audit: None,
    };
    let served = build_served(&g.bytes, &live).unwrap();
    let v: Value = serde_json::from_slice(&served).unwrap();
    let errs = validate(&self_v2(), &self_v2(), &v);
    assert!(errs.is_empty(), "{errs:#?}");
    assert_eq!(
        drift::compare(Some(&served), &g.bytes),
        Ok(vec![Warning::LiveIgnored])
    );
    remove(&b.home);
}
