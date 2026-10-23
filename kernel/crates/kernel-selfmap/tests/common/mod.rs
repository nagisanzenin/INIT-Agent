//! Shared helpers for the selfmap integration tests: test homes, fixture builds and a small JSON Schema
//! validator for the keywords of selfmap.md §4.2/§4.2a.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

pub fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The repository root.
pub fn repo() -> PathBuf {
    manifest_dir().join("../../..").canonicalize().unwrap()
}

pub fn fixtures() -> PathBuf {
    manifest_dir().join("tests/fixtures")
}

/// The `kernel-selfmap` binary under test.
pub fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kernel-selfmap"))
}

/// A fresh directory under `kernel/target/test-homes/WP9/`.
pub fn home(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = repo()
        .join("kernel/target/test-homes/WP9")
        .join(format!("it-{name}-{}-{n}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Removes a test home by its exact path.
pub fn remove(dir: &Path) {
    assert!(
        dir.to_string_lossy()
            .contains("/kernel/target/test-homes/WP9/")
    );
    let _ = std::fs::remove_dir_all(dir);
}

pub fn put(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

pub fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    std::fs::copy(from, to)
        .unwrap_or_else(|e| panic!("copy {} → {}: {e}", from.display(), to.display()));
}

fn cargo_build_wasm(dir: &Path, target: &Path) {
    let o = init_testkit::watchdog::output(Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(dir)
        .args(["build", "--offline", "--target", "wasm32-wasip2"])
        .env("CARGO_TARGET_DIR", target))
        .expect("run cargo");
    assert!(
        o.status.success(),
        "ext build in {} failed:\n{}",
        dir.display(),
        String::from_utf8_lossy(&o.stderr)
    );
}

/// The test fixtures' components (`echo`, `uses-process`, `describe-loop`, `describe-reads-file`,
/// `describe-nondet`), built once per test binary into `CARGO_TARGET_TMPDIR`. Returns the output dir.
pub fn fixture_components() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("selfmap-ext");
        cargo_build_wasm(&fixtures().join("ext"), &target);
        target.join("wasm32-wasip2/debug")
    })
}

/// The seed's `count-lines` template (WP0), built as the brief says:
/// `CARGO_TARGET_DIR=seed/target cargo build --offline --target wasm32-wasip2` inside `seed/ext`.
pub fn count_lines_wasm() -> &'static Path {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| {
        let target = repo().join("seed/target");
        cargo_build_wasm(&repo().join("seed/ext"), &target);
        target.join("wasm32-wasip2/debug/count_lines.wasm")
    })
}

pub fn fixture_wasm(name: &str) -> PathBuf {
    fixture_components().join(format!("{}.wasm", name.replace('-', "_")))
}

/// Writes a fake body binary that prints `manifest` for `--manifest` (stub until WP13).
pub fn fake_body(dir: &Path, manifest: &[u8]) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    let p = dir.join("init-body");
    std::fs::write(&p, "#!/bin/sh\n[ \"$1\" = \"--manifest\" ] || exit 64\nexec /bin/cat \"${0%/*}/manifest.json\"\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

pub fn manifest_fixture() -> Vec<u8> {
    std::fs::read(fixtures().join("manifest-seed.json")).unwrap()
}

/// Runs the CLI with an extra environment; returns (exit code, stdout, stderr).
pub fn run_cli(args: &[&std::ffi::OsStr], env: &[(&str, &str)]) -> (i32, Vec<u8>, String) {
    let mut c = Command::new(bin());
    c.args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    let o = init_testkit::watchdog::output(&mut c).unwrap();
    (
        o.status.code().unwrap_or(-1),
        o.stdout,
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

// ---- JSON Schema (draft 2020-12 subset: the keywords selfmap.md §4.2/§4.2a use) ----

pub fn self_v2() -> Value {
    serde_json::from_str(kernel_selfmap::schema::SELF_V2).unwrap()
}

pub fn kernel_self_v2() -> Value {
    serde_json::from_str(kernel_selfmap::schema::KERNEL_SELF_V2).unwrap()
}

/// Errors of `v` against the root schema `schema` (`self-v2.json#…` references resolve to `self_v2`).
pub fn validate(schema: &Value, self_v2: &Value, v: &Value) -> Vec<String> {
    let mut errs = Vec::new();
    node(schema, schema, self_v2, v, "", &mut errs);
    errs
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => panic!("type {t}"),
    }
}

fn node(s: &Value, root: &Value, sv2: &Value, v: &Value, ptr: &str, errs: &mut Vec<String>) {
    let Some(o) = s.as_object() else { return };
    if let Some(r) = o.get("$ref").and_then(Value::as_str) {
        let (doc, frag) = match r.split_once('#') {
            Some(("", f)) => (root, f),
            Some(("self-v2.json", f)) => (sv2, f),
            _ => panic!("unsupported $ref {r}"),
        };
        let target = doc
            .pointer(frag)
            .unwrap_or_else(|| panic!("dangling $ref {r}"));
        node(target, doc, sv2, v, ptr, errs);
    }
    if let Some(t) = o.get("type") {
        let ok = match t {
            Value::String(t) => type_ok(t, v),
            Value::Array(ts) => ts.iter().any(|t| type_ok(t.as_str().unwrap(), v)),
            _ => true,
        };
        if !ok {
            errs.push(format!("{ptr}: type {t} expected, got {v}"));
            return;
        }
    }
    if let Some(c) = o.get("const")
        && c != v
    {
        errs.push(format!("{ptr}: const {c} expected, got {v}"));
    }
    if let Some(e) = o.get("enum").and_then(Value::as_array)
        && !e.contains(v)
    {
        errs.push(format!("{ptr}: {v} not in enum"));
    }
    if let Some(alts) = o.get("oneOf").and_then(Value::as_array) {
        let n = alts
            .iter()
            .filter(|a| {
                let mut e = Vec::new();
                node(a, root, sv2, v, ptr, &mut e);
                e.is_empty()
            })
            .count();
        if n != 1 {
            errs.push(format!("{ptr}: oneOf matched {n}"));
        }
    }
    if let (Some(p), Some(s)) = (o.get("pattern").and_then(Value::as_str), v.as_str())
        && !regex::Regex::new(p).unwrap().is_match(s)
    {
        errs.push(format!("{ptr}: {s:?} does not match {p}"));
    }
    if let (Some(m), Some(n)) = (o.get("minimum").and_then(Value::as_i64), v.as_i64())
        && n < m
    {
        errs.push(format!("{ptr}: {n} < minimum {m}"));
    }
    if let (Some(m), Some(n)) = (o.get("maximum").and_then(Value::as_i64), v.as_i64())
        && n > m
    {
        errs.push(format!("{ptr}: {n} > maximum {m}"));
    }
    if let (Some(m), Some(s)) = (o.get("maxLength").and_then(Value::as_u64), v.as_str())
        && s.chars().count() as u64 > m
    {
        errs.push(format!("{ptr}: longer than {m}"));
    }
    if let Value::Object(obj) = v {
        if let Some(req) = o.get("required").and_then(Value::as_array) {
            for k in req {
                if !obj.contains_key(k.as_str().unwrap()) {
                    errs.push(format!("{ptr}: missing {k}"));
                }
            }
        }
        let props = o.get("properties").and_then(Value::as_object);
        for (k, x) in obj {
            match props.and_then(|p| p.get(k)) {
                Some(ps) => node(ps, root, sv2, x, &format!("{ptr}/{k}"), errs),
                None if o.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    errs.push(format!("{ptr}: additional property {k}"))
                }
                None => {}
            }
        }
    }
    if let Value::Array(a) = v {
        if let Some(m) = o.get("maxItems").and_then(Value::as_u64)
            && a.len() as u64 > m
        {
            errs.push(format!("{ptr}: more than {m} items"));
        }
        if let Some(it) = o.get("items") {
            for (i, x) in a.iter().enumerate() {
                node(it, root, sv2, x, &format!("{ptr}/{i}"), errs);
            }
        }
    }
}
