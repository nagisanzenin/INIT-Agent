//! ext/host.md §8 (H0–H24), ext/wit.md §8 (W-*) and ext/authoring.md AU-1/AU-2, against real
//! components:
//! - `count-lines` (seed/ext), built exactly as `CARGO_TARGET_DIR=seed/target cargo build
//!   --offline --target wasm32-wasip2` inside `seed/ext`;
//! - fixture guests (`fixtures/*.rs`), written into a throwaway ext workspace under
//!   `target/test-homes/wp14/` and built the same way, into the same target dir.
//!
//! Every test home is `target/test-homes/wp14/homes/<test>-<pid>/`, removed by exact path.
//! Every process a test checks or kills is one the host spawned and reported (pid files).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use super::registry::{RawInfo, check_schema_text, valid_name, validate};
use super::state::{CallState, Caps, wasi_ctx};
use super::*;
use crate::tools::bash::group_alive;

// ---------------------------------------------------------------------------------------------
// Support: paths, builds, homes
// ---------------------------------------------------------------------------------------------

fn seed_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The seed's target dir, derived from this test binary (`<target>/debug/deps/init_body-…`), so it
/// follows `CARGO_TARGET_DIR` overrides.
fn target_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    exe.ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| seed_dir().join("target"))
}

fn wp14_dir() -> PathBuf {
    target_dir().join("test-homes/wp14")
}

fn wasm_dir() -> PathBuf {
    target_dir().join("wasm32-wasip2/debug")
}

/// The vendored crates: `<seed>/vendor` in a body checkout (the install packaging cuts one and
/// names it `directory = "vendor"`), `<repo>/vendor` in the dev repo (`seed/.cargo/config.toml`
/// names `../vendor`).
fn vendor_dir() -> PathBuf {
    let own = seed_dir().join("vendor");
    match seed_dir().parent() {
        Some(up) if !own.is_dir() => up.join("vendor"),
        _ => own,
    }
}

/// The two `--config` flags the gate passes (gate.md §3.3), with the vendor dir absolute: a fixture
/// workspace under the target dir lies outside the seed tree, so no `.cargo/config.toml` above it
/// names the vendored sources (G0 builds into `cache/build/slot/out/target`).
fn vendor_config_args() -> [String; 4] {
    let dir = vendor_dir()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    [
        "--config".into(),
        "source.crates-io.replace-with=\"vendored-sources\"".into(),
        "--config".into(),
        format!("source.vendored-sources.directory=\"{dir}\""),
    ]
}

fn cargo_wasm_build(cwd: &Path) -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .arg("build")
        .args(vendor_config_args())
        .args(["--offline", "--target", "wasm32-wasip2"])
        .current_dir(cwd)
        .env("CARGO_TARGET_DIR", target_dir())
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_BUILD_TARGET")
        .output()
        .map_err(|e| format!("cargo: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "cargo build in {} failed:\n{}",
            cwd.display(),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

fn write_if_changed(p: &Path, content: &str) {
    if fs::read_to_string(p).ok().as_deref() == Some(content) {
        return;
    }
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

/// The real `count-lines` component (AU-1): built in `seed/ext` into the seed target dir.
fn count_lines_wasm() -> PathBuf {
    static R: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    R.get_or_init(|| {
        cargo_wasm_build(&seed_dir().join("ext"))?;
        let p = wasm_dir().join("count_lines.wasm");
        if p.is_file() {
            Ok(p)
        } else {
            Err(format!("{} missing after build", p.display()))
        }
    })
    .clone()
    .unwrap_or_else(|e| panic!("{e}"))
}

struct Variant {
    krate: &'static str,
    name: &'static str,
    schema: &'static str,
    timeout: &'static str,
    describe: u8,
    mark: &'static str,
    source: &'static str,
}

const ANY_SCHEMA: &str = r#"{"type":"object","properties":{"op":{"type":"string","description":"what to do"}},"required":["op"]}"#;

const VARIANTS: &[Variant] = &[
    Variant {
        krate: "fx",
        name: "fx",
        schema: ANY_SCHEMA,
        timeout: "None",
        describe: 0,
        mark: "main",
        source: "common",
    },
    Variant {
        krate: "fx-t200",
        name: "fx-t200",
        schema: ANY_SCHEMA,
        timeout: "Some(200)",
        describe: 0,
        mark: "t200",
        source: "common",
    },
    Variant {
        krate: "fx-t500",
        name: "fx-t500",
        schema: ANY_SCHEMA,
        timeout: "Some(500)",
        describe: 0,
        mark: "t500",
        source: "common",
    },
    Variant {
        krate: "fx-clamp",
        name: "fx-clamp",
        schema: ANY_SCHEMA,
        timeout: "Some(5)",
        describe: 0,
        mark: "clamp",
        source: "common",
    },
    Variant {
        krate: "fx-badschema",
        name: "fx-badschema",
        schema: r##"{"type":"object","properties":{"a":{"$ref":"#/x"}}}"##,
        timeout: "None",
        describe: 0,
        mark: "",
        source: "common",
    },
    Variant {
        krate: "fx-wrongname",
        name: "other-name",
        schema: ANY_SCHEMA,
        timeout: "None",
        describe: 0,
        mark: "",
        source: "common",
    },
    Variant {
        krate: "fx-nondet",
        name: "fx-nondet",
        schema: ANY_SCHEMA,
        timeout: "None",
        describe: 1,
        mark: "",
        source: "common",
    },
    Variant {
        krate: "fx-descproc",
        name: "fx-descproc",
        schema: ANY_SCHEMA,
        timeout: "None",
        describe: 2,
        mark: "",
        source: "common",
    },
    Variant {
        krate: "fx-descfs",
        name: "fx-descfs",
        schema: ANY_SCHEMA,
        timeout: "None",
        describe: 3,
        mark: "",
        source: "common",
    },
    Variant {
        krate: "fx-badsig",
        name: "fx-badsig",
        schema: "",
        timeout: "",
        describe: 0,
        mark: "",
        source: "badsig",
    },
    Variant {
        krate: "fx-badimport",
        name: "fx-badimport",
        schema: "",
        timeout: "",
        describe: 0,
        mark: "",
        source: "badimport",
    },
];

fn crate_toml(name: &str, version: &str) -> String {
    format!(
        "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition.workspace = true\npublish = false\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nwit-bindgen.workspace = true\nserde.workspace = true\nserde_json.workspace = true\n"
    )
}

fn common_lib(name: &str, schema: &str, timeout: &str, describe: u8, mark: &str) -> String {
    format!(
        "#![cfg_attr(not(target_arch = \"wasm32\"), allow(dead_code))]\nconst NAME: &str = {name:?};\nconst SCHEMA: &str = {schema:?};\nconst TIMEOUT_MS: Option<u32> = {timeout};\nconst DESCRIBE: u8 = {describe};\nconst MARK: &str = {mark:?};\n\n{}",
        include_str!("fixtures/fx_common.rs")
    )
}

/// An ext workspace (`<root>/body/{ext,wit}`) that mirrors the real body layout.
fn write_workspace(root: &Path, crates: &[(String, String, String)]) -> PathBuf {
    let body = root.join("body");
    write_if_changed(
        &body.join("wit/init-ext/v1/init-ext.wit"),
        &fs::read_to_string(seed_dir().join("wit/init-ext/v1/init-ext.wit")).unwrap(),
    );
    let ext = body.join("ext");
    write_if_changed(
        &ext.join("Cargo.toml"),
        &fs::read_to_string(seed_dir().join("ext/Cargo.toml")).unwrap(),
    );
    if !ext.join("Cargo.lock").is_file() {
        fs::copy(seed_dir().join("ext/Cargo.lock"), ext.join("Cargo.lock")).unwrap();
    }
    for (krate, toml, lib) in crates {
        write_if_changed(&ext.join(krate).join("Cargo.toml"), toml);
        write_if_changed(&ext.join(krate).join("src/lib.rs"), lib);
    }
    ext
}

/// Builds every fixture once per test process; returns the artifact directory.
fn fixtures() -> PathBuf {
    static R: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    R.get_or_init(|| {
        let crates: Vec<(String, String, String)> = VARIANTS
            .iter()
            .map(|v| {
                let lib = match v.source {
                    "badsig" => include_str!("fixtures/fx_badsig.rs").to_string(),
                    "badimport" => include_str!("fixtures/fx_badimport.rs").to_string(),
                    _ => common_lib(v.name, v.schema, v.timeout, v.describe, v.mark),
                };
                (v.krate.to_string(), crate_toml(v.krate, "0.1.0"), lib)
            })
            .collect();
        let ext = write_workspace(&wp14_dir().join("fx"), &crates);
        cargo_wasm_build(&ext)?;
        Ok(wasm_dir())
    })
    .clone()
    .unwrap_or_else(|e| panic!("{e}"))
}

fn fx(krate: &str) -> PathBuf {
    fixtures().join(super::loader::file_name(krate))
}

/// `swapper` v1 (mark "1", version 0.1.0) and v2 (mark "2", 0.2.0), each a real build of an
/// edited `lib.rs`; copies kept in the wp14 dir.
fn swap_builds() -> (PathBuf, PathBuf) {
    static R: OnceLock<Result<(PathBuf, PathBuf), String>> = OnceLock::new();
    R.get_or_init(|| {
        let root = wp14_dir().join("swap");
        let mut out = Vec::new();
        for (ver, mark) in [("0.1.0", "1"), ("0.2.0", "2")] {
            let lib = common_lib("swapper", ANY_SCHEMA, "None", 0, mark);
            let ext = write_workspace(
                &root,
                &[("swapper".into(), crate_toml("swapper", ver), lib)],
            );
            cargo_wasm_build(&ext)?;
            let dst = root.join(format!("swapper-{ver}.wasm"));
            fs::copy(wasm_dir().join("swapper.wasm"), &dst).map_err(|e| e.to_string())?;
            out.push(dst);
        }
        Ok((out[0].clone(), out[1].clone()))
    })
    .clone()
    .unwrap_or_else(|e| panic!("{e}"))
}

struct Home {
    root: PathBuf,
    paths: ExtPaths,
    work: PathBuf,
}

impl Home {
    fn new(test: &str) -> Home {
        let root = wp14_dir()
            .join("homes")
            .join(format!("{test}-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        for d in ["bin/ext", "body/ext", "state/ext", "work/task"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let paths = ExtPaths::from_home(&root, None);
        let work = root.join("work/task");
        Home { root, paths, work }
    }

    fn keep(&self, src: &Path, name: &str) {
        fs::copy(src, self.paths.kept.join(super::loader::file_name(name))).unwrap();
    }

    /// A trial artifact for `name` (and a source dir so Request reloads see the crate).
    fn trial(&self, src: &Path, name: &str, mtime: SystemTime) {
        self.src_dir(name);
        let dir = self.paths.target.join("wasm32-wasip2/debug");
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join(super::loader::file_name(name));
        fs::copy(src, &p).unwrap();
        set_mtime(&p, mtime);
    }

    fn src_dir(&self, name: &str) {
        let d = self.paths.src.join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("Cargo.toml"), crate_toml(name, "0.1.0")).unwrap();
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn set_mtime(p: &Path, t: SystemTime) {
    fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

/// Host + Boot load of the kept set.
fn boot(home: &Home, kept: &[&str]) -> (Arc<ExtHost>, ReloadReport) {
    for k in kept {
        home.keep(&fx(k), k);
    }
    let h = ExtHost::new(home.paths.clone()).unwrap();
    let r = h
        .commit_reload(Duration::from_secs(120))
        .expect("boot report");
    (h, r)
}

fn call(h: &ExtHost, name: &str, input: Value, ws: &Path) -> ExtOutcome {
    let abort = AtomicBool::new(false);
    h.call(
        &h.snapshot(),
        name,
        &input,
        &CallCtx {
            call_id: "t1",
            workspace: ws,
            abort: &abort,
        },
    )
}

fn json_of(o: &ExtOutcome) -> Value {
    assert_eq!(o.kind, OutcomeKind::Ok, "{}", o.text);
    serde_json::from_str(&o.text).unwrap_or_else(|e| panic!("{e}: {}", o.text))
}

fn wait_until(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    f()
}

fn percentile(v: &mut [u128], p: f64) -> u128 {
    v.sort_unstable();
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

// ---------------------------------------------------------------------------------------------
// H0, H1, H2, AU-1, AU-2: engine, bindings, the real count-lines component
// ---------------------------------------------------------------------------------------------

#[test]
fn h0_h1_engine_starts_with_forbid_and_bindgen() {
    // H0 is the fact that this crate (`#![forbid(unsafe_code)]`) compiled `bindgen!`.
    // H1: `Engine::new` accepts the full §3.3 config, including `concurrency_support(false)`.
    let home = Home::new("h1");
    let h = ExtHost::build(home.paths.clone()).expect("engine + linker");
    assert!(
        h.engine.pooling_allocator_metrics().is_some(),
        "pooling allocator in use"
    );
    assert_eq!(SUPPORTED_MAJORS, &[1]);
    assert!(!h.reload_pending());
    assert!(h.commit_reload(Duration::ZERO).is_none());
}

#[test]
fn h2_au1_au2_count_lines_loads_and_counts() {
    let wasm = count_lines_wasm();
    let home = Home::new("h2");
    home.keep(&wasm, "count-lines");
    let h = ExtHost::new(home.paths.clone()).unwrap();
    let r = h.commit_reload(Duration::from_secs(120)).unwrap();
    assert!(r.ok, "{}", r.text);
    assert_eq!(r.components, vec!["count-lines".to_string()]);
    assert!(
        r.text
            .starts_with("[init:reload] ext generation 1\n+ count-lines 0.1.0 (kept, compiled in "),
        "{}",
        r.text
    );
    assert!(
        r.text
            .contains("  input_schema: {\"additionalProperties\":false,\"properties\":{\"needle\""),
        "{}",
        r.text
    );
    let v = h.snapshot().exts["count-lines"].clone();
    eprintln!(
        "H2 count-lines: {} bytes, compile_ms {}",
        v.fp.len, v.compile_ms
    );

    // Pool usage while one instance is alive (Q-H2).
    let log = wasmtime_wasi::p2::pipe::MemoryOutputPipe::new(LOG_CAP);
    let st = CallState::new(
        wasi_ctx(None, &log).unwrap(),
        Arc::from("count-lines"),
        Instant::now() + Duration::from_secs(5),
        Arc::new(AtomicBool::new(false)),
        Caps::None,
        None,
        h.procs.clone(),
    );
    let mut store = wasmtime::Store::new(&h.engine, st);
    store.set_epoch_deadline(u64::MAX / 2);
    let super::registry::Pre::V1(pre) = &v.pre;
    pre.instantiate(&mut store).unwrap();
    let m = h.engine.pooling_allocator_metrics().unwrap();
    eprintln!(
        "H2 pool while 1 instance alive: component_instances={} core_instances={} memories={} tables={}",
        m.component_instances(),
        m.core_instances(),
        m.memories(),
        m.tables()
    );
    assert_eq!(m.component_instances(), 1);
    assert!(m.core_instances() <= 16 && m.memories() <= 2);
    drop(store);

    // Q-H3: instantiate time alone (Store::new + limiter + instantiate from the pool + drop).
    let mut us = Vec::with_capacity(300);
    for _ in 0..300 {
        let log = wasmtime_wasi::p2::pipe::MemoryOutputPipe::new(LOG_CAP);
        let st = CallState::new(
            wasi_ctx(None, &log).unwrap(),
            Arc::from("count-lines"),
            Instant::now() + Duration::from_secs(5),
            Arc::new(AtomicBool::new(false)),
            Caps::None,
            None,
            h.procs.clone(),
        );
        let t = Instant::now();
        let mut store = wasmtime::Store::new(&h.engine, st);
        store.limiter(|s| &mut s.limits);
        store.set_epoch_deadline(u64::MAX / 2);
        pre.instantiate(&mut store).unwrap();
        drop(store);
        us.push(t.elapsed().as_micros());
    }
    let (p50, p99) = (percentile(&mut us, 0.5), percentile(&mut us, 0.99));
    eprintln!(
        "H2/Q-H3 count-lines instantiate (Store + limiter + pool instantiate + drop): p50 {p50} us, p99 {p99} us"
    );

    // AU-2: a real call on a fixture workspace.
    fs::write(
        home.work.join("README.md"),
        "INIT Agent\nnothing\nINIT again\n",
    )
    .unwrap();
    let o = call(
        &h,
        "count-lines",
        json!({"path": "README.md", "needle": "INIT"}),
        &home.work,
    );
    assert_eq!(json_of(&o), json!({"count": 2, "lines": [1, 3]}));
    // Guest-side errors keep the "ext <name> <version>: " prefix.
    let o = call(
        &h,
        "count-lines",
        json!({"path": "missing.md", "needle": "x"}),
        &home.work,
    );
    assert_eq!(o.kind, OutcomeKind::Failed);
    assert!(
        o.text
            .starts_with("ext count-lines 0.1.0: failed: /work/missing.md: "),
        "{}",
        o.text
    );
    let o = call(
        &h,
        "count-lines",
        json!({"path": "a", "needle": "b", "extra": 1}),
        &home.work,
    );
    assert_eq!(o.kind, OutcomeKind::InvalidInput);
    assert!(
        o.text
            .starts_with("ext count-lines 0.1.0: invalid input: unknown field `extra`"),
        "{}",
        o.text
    );
}

// ---------------------------------------------------------------------------------------------
// H3, H15, W-14: round trip, overhead, canonical input
// ---------------------------------------------------------------------------------------------

#[test]
fn h3_h15_w14_echo_round_trip_overhead_and_sorted_input() {
    let home = Home::new("h3");
    let (h, r) = boot(&home, &["fx"]);
    assert!(r.ok, "{}", r.text);
    // W-14: keys sorted at every depth, compact.
    let o = call(
        &h,
        "fx",
        json!({"op": "echo", "b": 1, "a": {"z": 1, "y": [2, 1]}}),
        &home.work,
    );
    assert_eq!(o.text, r#"{"a":{"y":[2,1],"z":1},"b":1,"op":"echo"}"#);
    assert!(!o.is_error);
    assert_eq!(o.version.as_deref(), Some("0.1.0"));

    // H15: 1,000 sequential calls; host overhead = whole call minus nothing (echo does no work).
    let input = json!({"op": "echo", "s": "x"});
    let snap = h.snapshot();
    let abort = AtomicBool::new(false);
    let cx = CallCtx {
        call_id: "h15",
        workspace: &home.work,
        abort: &abort,
    };
    for _ in 0..50 {
        h.call(&snap, "fx", &input, &cx);
    }
    let mut us = Vec::with_capacity(1000);
    for _ in 0..1000 {
        let t = Instant::now();
        let o = h.call(&snap, "fx", &input, &cx);
        us.push(t.elapsed().as_micros());
        assert!(!o.is_error);
    }
    let (p50, p99) = (percentile(&mut us, 0.5), percentile(&mut us, 0.99));
    eprintln!(
        "H15 echo call (thread + WASI ctx + Store + instantiate + call + log append): p50 {p50} us, p99 {p99} us"
    );
    assert!(p50 < 5_000, "p50 {p50} us");
}

// ---------------------------------------------------------------------------------------------
// H4–H8, exit, guest errors, output truncation, log file
// ---------------------------------------------------------------------------------------------

#[test]
fn h4_loop_times_out_then_next_call_succeeds() {
    let home = Home::new("h4");
    let (h, _) = boot(&home, &["fx-t200"]);
    let t = Instant::now();
    let o = call(&h, "fx-t200", json!({"op": "loop"}), &home.work);
    let ms = t.elapsed().as_millis();
    eprintln!("H4 loop timeout after {ms} ms");
    assert_eq!(o.kind, OutcomeKind::Timeout, "{}", o.text);
    assert_eq!(o.text, "ext fx-t200 0.1.0: timed out after 200 ms");
    assert!((200..400).contains(&ms), "{ms} ms");
    let o = call(&h, "fx-t200", json!({"op": "mark"}), &home.work);
    assert_eq!(o.text, "t200");
}

#[test]
fn h5_sleep_is_abandoned_busy_then_released() {
    let home = Home::new("h5");
    let (h, _) = boot(&home, &["fx-t500", "fx"]);
    let t = Instant::now();
    let o = call(
        &h,
        "fx-t500",
        json!({"op": "sleep", "ms": 4000}),
        &home.work,
    );
    let ms = t.elapsed().as_millis();
    eprintln!("H5 abandoned after {ms} ms");
    assert_eq!(o.kind, OutcomeKind::Timeout);
    assert!(
        o.text.ends_with(
            "timed out after 500 ms; the call was blocked in a host call and is still running"
        ),
        "{}",
        o.text
    );
    assert!(ms <= 2_600, "{ms} ms");
    assert_eq!(h.stuck.count("fx-t500"), 1);
    let o = call(&h, "fx-t500", json!({"op": "mark"}), &home.work);
    assert_eq!(o.kind, OutcomeKind::Busy);
    assert_eq!(
        o.text,
        "ext fx-t500 0.1.0: busy: a previous call is still blocked in a host call"
    );
    assert_eq!(
        call(&h, "fx", json!({"op": "mark"}), &home.work).text,
        "main"
    );
    // The reload report and the catalog list stuck names.
    h.begin_reload(ReloadKind::Boot);
    let r = h.commit_reload(Duration::from_secs(60)).unwrap();
    assert!(
        r.text
            .contains("· stuck: fx-t500 (1 call still blocked in a host call"),
        "{}",
        r.text
    );
    // When the sleep ends, the worker traps at its next epoch check and releases its slot.
    assert!(wait_until(Duration::from_secs(6), || h.stuck.total() == 0));
    assert!(wait_until(Duration::from_secs(1), || h.ticker.active() == 0));
    assert_eq!(
        call(&h, "fx-t500", json!({"op": "mark"}), &home.work).text,
        "t500"
    );
}

#[test]
fn h6_h7_h8_oom_stack_overflow_panic_exit_and_guest_errors() {
    let home = Home::new("h6");
    let (h, _) = boot(&home, &["fx"]);
    let w = &home.work;

    let o = call(&h, "fx", json!({"op": "alloc", "mb": 512}), w);
    assert_eq!(o.kind, OutcomeKind::Oom, "{}", o.text);
    assert!(
        o.text
            .starts_with("ext fx 0.1.0: out of memory: linear memory limit is 256 MiB"),
        "{}",
        o.text
    );
    // 128 MiB fits.
    assert_eq!(
        call(&h, "fx", json!({"op": "alloc", "mb": 128}), w).text,
        (128u64 << 20).to_string()
    );

    let o = call(&h, "fx", json!({"op": "recurse"}), w);
    assert_eq!(o.kind, OutcomeKind::StackOverflow, "{}", o.text);
    assert!(
        o.text
            .starts_with("ext fx 0.1.0: trap: stack overflow (512 KiB wasm stack)")
    );
    // Finding: recursion with locals overflows Rust's shadow stack (in linear memory) first,
    // which Wasmtime reports as an out-of-bounds access, with the backtrace frames.
    let o = call(&h, "fx", json!({"op": "recurse_big"}), w);
    assert_eq!(o.kind, OutcomeKind::Trap, "{}", o.text);
    assert!(
        o.text
            .starts_with("ext fx 0.1.0: trap: wasm trap: out of bounds memory access\n"),
        "{}",
        o.text
    );
    assert!(o.text.contains("recurse_big"), "{}", o.text);

    let o = call(&h, "fx", json!({"op": "panic"}), w);
    assert_eq!(o.kind, OutcomeKind::Panic, "{}", o.text);
    assert!(
        o.text
            .starts_with("ext fx 0.1.0: panicked (see log below)\n--- log (last 4096 bytes) ---\n"),
        "{}",
        o.text
    );
    assert!(o.text.contains("boom"), "{}", o.text);

    let o = call(&h, "fx", json!({"op": "exit"}), w);
    assert_eq!(o.kind, OutcomeKind::Exit, "{}", o.text);
    // Finding: WASI p2's `exit` takes a result, not a code, so `std::process::exit(3)` arrives
    // as exit(1); only 0 and 1 are distinguishable.
    assert!(
        o.text.starts_with("ext fx 0.1.0: called exit(1)"),
        "{}",
        o.text
    );

    let o = call(&h, "fx", json!({"op": "fail"}), w);
    assert_eq!(
        (o.kind, o.text.as_str()),
        (OutcomeKind::Failed, "ext fx 0.1.0: failed: nope")
    );
    let o = call(&h, "fx", json!({"op": "invalid"}), w);
    assert_eq!(
        (o.kind, o.text.as_str()),
        (
            OutcomeKind::InvalidInput,
            "ext fx 0.1.0: invalid input: bad input"
        )
    );

    // Output over 1 MiB is cut and flagged.
    let o = call(&h, "fx", json!({"op": "bigout", "n": (1 << 20) + 10}), w);
    assert!(o.truncated && o.text.len() == 1 << 20 && !o.is_error);

    // Log: stdout and stderr are not shown on success, but always appended to state/ext/<n>/log.
    let o = call(&h, "fx", json!({"op": "print", "n": 5}), w);
    assert_eq!(o.text, "printed");
    let log = fs::read_to_string(home.paths.state.join("fx/log")).unwrap();
    assert!(log.contains(" call=t1 fx 0.1.0 kept result=ok "), "{log}");
    assert!(
        log.contains("result=panic") && log.contains("xxxxx\nto-stderr\n"),
        "{log}"
    );

    // A trap never disables the extension.
    assert_eq!(call(&h, "fx", json!({"op": "mark"}), w).text, "main");
    assert_eq!(h.ticker.active(), 0);
}

#[test]
fn unknown_name_lists_loaded_failed_and_not_built() {
    let home = Home::new("unknown");
    home.src_dir("draft");
    let (h, _) = boot(&home, &["fx", "fx-badschema"]);
    h.begin_reload(ReloadKind::Request);
    h.commit_reload(Duration::from_secs(60)).unwrap();
    let o = call(&h, "nope", json!({}), &home.work);
    assert_eq!(o.kind, OutcomeKind::Unknown);
    // Request: only crates in body/ext count; the kept fx has no source dir there, so it unloads.
    assert!(
        o.text
            .starts_with("unknown extension \"nope\". Loaded: none. Not built: draft."),
        "{}",
        o.text
    );
    assert!(o.text.ends_with("(input schemas: state/ext/catalog.json)"));
    h.begin_reload(ReloadKind::Boot);
    h.commit_reload(Duration::from_secs(60)).unwrap();
    let o = call(&h, "nope", json!({}), &home.work);
    assert!(
        o.text.starts_with(
            "unknown extension \"nope\". Loaded: fx — WP14 test fixture.. Failed: fx-badschema ("
        ),
        "{}",
        o.text
    );
}

// ---------------------------------------------------------------------------------------------
// H9: hot swap and drain
// ---------------------------------------------------------------------------------------------

#[test]
fn h9_hot_swap_new_batch_sees_v2_while_v1_call_drains() {
    let (v1, v2) = swap_builds();
    let home = Home::new("h9");
    let h = ExtHost::new(home.paths.clone()).unwrap();
    let r = h.commit_reload(Duration::from_secs(60)).unwrap();
    assert_eq!(r.text, "[init:reload] ext generation 1\n(no changes)");

    home.trial(&v1, "swapper", SystemTime::now());
    h.begin_reload(ReloadKind::Request);
    assert!(h.reload_pending());
    let r = h.commit_reload(RELOAD_WAIT_REQUESTED).unwrap();
    assert!(r.ok && r.components == ["swapper"], "{}", r.text);
    assert!(
        r.text.contains("+ swapper 0.1.0 (trial, compiled in "),
        "{}",
        r.text
    );
    let gen1 = h.snapshot();
    assert_eq!(
        call(&h, "swapper", json!({"op": "mark"}), &home.work).text,
        "1"
    );

    // A v1 call that is still running at the commit (process.run sleep 1).
    let h2 = h.clone();
    let ws = home.work.clone();
    let long = std::thread::spawn(move || {
        let abort = AtomicBool::new(false);
        let cx = CallCtx {
            call_id: "long",
            workspace: &ws,
            abort: &abort,
        };
        h2.call(
            &gen1,
            "swapper",
            &json!({"op": "slowmark", "argv": ["sleep", "1"]}),
            &cx,
        )
    });
    std::thread::sleep(Duration::from_millis(200));

    home.trial(&v2, "swapper", SystemTime::now());
    h.begin_reload(ReloadKind::Request);
    let r = h.commit_reload(RELOAD_WAIT_REQUESTED).unwrap();
    assert!(
        r.text
            .contains("~ swapper 0.1.0 → 0.2.0 (trial, compiled in "),
        "{}",
        r.text
    );
    assert_eq!(r.generation, 3);
    assert_eq!(
        call(&h, "swapper", json!({"op": "mark"}), &home.work).text,
        "2"
    );
    let o = long.join().unwrap();
    assert_eq!(
        (o.text.as_str(), o.version.as_deref()),
        ("1", Some("0.1.0"))
    );

    // Nothing pending: commit is free and returns None.
    assert!(!h.reload_pending());
    assert!(h.commit_reload(Duration::ZERO).is_none());
}

// ---------------------------------------------------------------------------------------------
// H10, H22, W-4, W-5, W-15: load errors
// ---------------------------------------------------------------------------------------------

#[test]
fn h10_h22_load_errors_are_listed_and_previous_version_keeps_serving() {
    let home = Home::new("h10");
    let (h, r) = boot(
        &home,
        &[
            "fx-badschema",
            "fx-wrongname",
            "fx-nondet",
            "fx-badsig",
            "fx-badimport",
            "fx-descproc",
            "fx-descfs",
            "fx-clamp",
        ],
    );
    eprintln!("{}", r.text);
    assert!(!r.ok);
    let t = &r.text;
    assert!(t.contains("! fx-badschema: tool-info.input-schema: keyword \"$ref\" at /properties/a is not allowed"), "{t}");
    assert!(t.contains("! fx-wrongname: tool-info.name \"other-name\" does not match directory \"fx-wrongname\" (no previous version)"), "{t}");
    assert!(
        t.contains("! fx-nondet: describe is not deterministic"),
        "{t}"
    );
    assert!(
        t.contains("! fx-badsig: does not match init:ext@1: "),
        "{t}"
    );
    assert!(
        t.contains("! fx-badimport: does not match init:ext@1: ")
            && t.contains("fx:badimport/outgoing-handler@1.0.0"),
        "{t}"
    );
    // H22 / W-5: describe has no capabilities; a describe that depends on them fails to load.
    assert!(t.contains("! fx-descproc: describe trapped: wasm trap: wasm `unreachable` instruction executed; log: ") && t.contains("describe has no capabilities"), "{t}");
    assert!(t.contains("! fx-descfs: describe trapped: "), "{t}");
    // timeout_ms is clamped and noted.
    assert!(
        t.contains("+ fx-clamp 0.1.0 (kept, compiled in ")
            && t.contains("  note: timeout_ms 5 clamped to 100"),
        "{t}"
    );
    assert_eq!(
        h.snapshot().exts.keys().collect::<Vec<_>>(),
        vec!["fx-clamp"]
    );

    // A broken rebuild leaves the previous version serving.
    home.trial(&fx("fx"), "fx", SystemTime::now());
    h.begin_reload(ReloadKind::Request);
    let r = h.commit_reload(RELOAD_WAIT_REQUESTED).unwrap();
    assert!(r.ok && r.text.contains("+ fx 0.1.0 (trial"), "{}", r.text);
    std::thread::sleep(Duration::from_millis(5));
    home.trial(&fx("fx-badschema"), "fx", SystemTime::now());
    h.begin_reload(ReloadKind::Request);
    let r = h.commit_reload(RELOAD_WAIT_REQUESTED).unwrap();
    assert!(!r.ok);
    assert!(r.text.contains("! fx: tool-info.name \"fx-badschema\" does not match directory \"fx\" (version 0.1.0 still serving)"), "{}", r.text);
    assert_eq!(
        call(&h, "fx", json!({"op": "mark"}), &home.work).text,
        "main"
    );
    assert!(h.snapshot().failed.contains_key("fx"));
}

#[test]
fn w4_tool_info_validation_rows() {
    let ok = RawInfo {
        name: "semver-sort".into(),
        description: "Sort versions.".into(),
        input_schema: r#"{"type":"object","properties":{"v":{"type":"array","items":{"type":"string"},"maxItems":10}},"required":["v"],"additionalProperties":false}"#.into(),
        version: "0.1.0".into(),
        timeout_ms: None,
    };
    let (v, notes) = validate(Some("semver-sort"), &ok).unwrap();
    assert_eq!(v.timeout_ms, 30_000);
    assert!(notes.is_empty());
    assert_eq!(
        v.schema_text(),
        r#"{"additionalProperties":false,"properties":{"v":{"items":{"type":"string"},"maxItems":10,"type":"array"}},"required":["v"],"type":"object"}"#
    );
    let bad = |f: &dyn Fn(&mut RawInfo)| {
        let mut r = ok.clone();
        f(&mut r);
        validate(Some("semver-sort"), &r).unwrap_err()
    };
    assert!(bad(&|r| r.name = "Semver".into()).contains("tool-info.name"));
    assert!(bad(&|r| r.name = "a--b".into()).contains("tool-info.name"));
    assert!(bad(&|r| r.name = "ab-".into()).contains("tool-info.name"));
    assert!(bad(&|r| r.name = "other".into()).contains("does not match directory"));
    assert!(
        validate(
            Some("bash"),
            &RawInfo {
                name: "bash".into(),
                ..ok.clone()
            }
        )
        .unwrap_err()
        .contains("reserved")
    );
    assert!(bad(&|r| r.description = String::new()).contains("description"));
    assert!(bad(&|r| r.description = "x".repeat(1025)).contains("description"));
    assert!(bad(&|r| r.input_schema = "x".repeat(16_385)).contains("16384"));
    assert!(bad(&|r| r.input_schema = "{".into()).contains("not JSON"));
    assert!(
        bad(&|r| r.input_schema = r#"{"type":"array"}"#.into())
            .contains("root must be an object schema")
    );
    assert!(
        bad(&|r| r.input_schema = r#"{"type":"object","oneOf":[]}"#.into()).contains("\"oneOf\"")
    );
    assert!(
        bad(&|r| r.input_schema =
            r#"{"type":"object","properties":{"x":{"type":"number","maximum":1.5}}}"#.into())
        .contains("not an integer")
    );
    assert!(bad(&|r| r.version = "1.0".into()).contains("version"));
    assert!(bad(&|r| r.version = "v1.0.0".into()).contains("version"));
    let (v, notes) = validate(
        Some("semver-sort"),
        &RawInfo {
            timeout_ms: Some(700_000),
            ..ok.clone()
        },
    )
    .unwrap();
    assert_eq!((v.timeout_ms, notes.len()), (600_000, 1));
    // `enum`/`const`/`default` values are data, not schemas.
    assert!(
        check_schema_text(
            r#"{"type":"object","properties":{"m":{"enum":[{"$ref":1}],"default":{"oneOf":2}}}}"#
        )
        .is_ok()
    );
    assert!(
        valid_name("a")
            && valid_name("count-lines")
            && !valid_name("1a")
            && !valid_name(&"a".repeat(33))
    );
}

// ---------------------------------------------------------------------------------------------
// H11, H12, W-8, W-9: filesystem and network from inside the guest
// ---------------------------------------------------------------------------------------------

#[test]
fn h11_h12_filesystem_confined_network_denied_data_persists() {
    let home = Home::new("h11");
    let (h, _) = boot(&home, &["fx"]);
    let w = &home.work;
    fs::write(w.join("ok.txt"), "hello").unwrap();
    fs::write(home.root.join("work/secret.txt"), "outside").unwrap();
    std::os::unix::fs::symlink(home.root.join("work/secret.txt"), w.join("link")).unwrap();

    let read = |p: &str| json_of(&call(&h, "fx", json!({"op": "read", "path": p}), w));
    assert_eq!(read("/work/ok.txt")["ok"], "hello");
    // Finding: wasi-libc does not resolve relative paths against initial-cwd ("/work"); guests
    // must use absolute paths (the skill says so). Asserted so a toolchain change is noticed.
    assert!(
        read("ok.txt").get("err").is_some(),
        "relative paths now resolve: update the skill"
    );
    for p in [
        "/etc/passwd",
        "/work/../secret.txt",
        "/work/link",
        "/data/../../x",
    ] {
        let v = read(p);
        assert!(v.get("err").is_some(), "{p}: {v}");
    }
    // /data persists across calls (fresh instance each time).
    let wr = json_of(&call(
        &h,
        "fx",
        json!({"op": "write", "path": "/data/state", "data": "42"}),
        w,
    ));
    assert_eq!(wr["ok"], true);
    assert_eq!(read("/data/state")["ok"], "42");
    assert_eq!(
        fs::read_to_string(home.paths.data_dir("fx").join("state")).unwrap(),
        "42"
    );

    // W-8 / H12: a connect is an error returned to the guest, not a trap.
    let v = json_of(&call(
        &h,
        "fx",
        json!({"op": "connect", "addr": "1.1.1.1:80"}),
        w,
    ));
    assert!(v.get("err").is_some(), "{v}");
    let v = json_of(&call(
        &h,
        "fx",
        json!({"op": "connect", "addr": "127.0.0.1:9"}),
        w,
    ));
    assert!(v.get("err").is_some(), "{v}");
}

// ---------------------------------------------------------------------------------------------
// H13, W-10, W-11: process.run
// ---------------------------------------------------------------------------------------------

#[test]
fn h13_process_run_echo_timeout_kill_and_cwd_escape() {
    let home = Home::new("h13");
    let (h, _) = boot(&home, &["fx"]);
    let w = &home.work;
    fs::create_dir_all(w.join("sub")).unwrap();

    let v = json_of(&call(
        &h,
        "fx",
        json!({"op": "run", "argv": ["sh", "-c", "echo hi; echo err >&2"]}),
        w,
    ));
    assert_eq!(
        v,
        json!({"exit_code": 0, "stdout": "hi\n", "stderr": "err\n", "timed_out": false, "truncated": false})
    );
    let v = json_of(&call(
        &h,
        "fx",
        json!({"op": "run", "argv": ["pwd"], "cwd": "/work/sub"}),
        w,
    ));
    assert_eq!(
        v["stdout"].as_str().unwrap().trim_end(),
        w.join("sub").canonicalize().unwrap().to_str().unwrap()
    );
    let v = json_of(&call(
        &h,
        "fx",
        json!({"op": "run", "argv": ["cat"], "stdin": "piped"}),
        w,
    ));
    assert_eq!(v["stdout"], "piped");
    let v = json_of(&call(
        &h,
        "fx",
        json!({"op": "run", "argv": ["sh", "-c", "exit 7"]}),
        w,
    ));
    assert_eq!(v["exit_code"], 7);

    // Timeout kill of the whole group at 100 ms.
    let t = Instant::now();
    let v = json_of(&call(
        &h,
        "fx",
        json!({"op": "run", "argv": ["sh", "-c", "echo $$ > pid; exec sleep 10"], "timeout_ms": 100}),
        w,
    ));
    let ms = t.elapsed().as_millis();
    eprintln!("H13 run timeout returned after {ms} ms");
    assert_eq!(
        (v["timed_out"].clone(), v["exit_code"].clone()),
        (json!(true), Value::Null)
    );
    assert!(ms < 400, "{ms} ms");
    let pgid: i32 = fs::read_to_string(w.join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(pgid > 1);
    assert!(
        wait_until(Duration::from_millis(200), || !group_alive(pgid)),
        "group {pgid} survived"
    );

    // Denied: argv empty, cwd outside, `..`, symlink escape; not-found.
    std::os::unix::fs::symlink(home.root.join("bin"), w.join("out")).unwrap();
    for (argv, cwd) in [
        (json!([]), "/work"),
        (json!(["true"]), "/etc"),
        (json!(["true"]), "/work/../.."),
        (json!(["true"]), "/work/out"),
        (json!(["true"]), "/workx"),
        (json!(["true"]), "/work/nope"),
    ] {
        let v = json_of(&call(
            &h,
            "fx",
            json!({"op": "run", "argv": argv, "cwd": cwd}),
            w,
        ));
        assert_eq!(v["err"], "denied", "{argv} {cwd}: {v}");
    }
    let v = json_of(&call(
        &h,
        "fx",
        json!({"op": "run", "argv": ["/nonexistent/prog"]}),
        w,
    ));
    assert_eq!(v["err"], "not-found", "{v}");
    let v = json_of(&call(
        &h,
        "fx",
        json!({"op": "run", "argv": ["pwd"], "cwd": "/data"}),
        w,
    ));
    assert_eq!(v["exit_code"], 0);
}

// ---------------------------------------------------------------------------------------------
// H14, W-12, W-13: sessions
// ---------------------------------------------------------------------------------------------

#[test]
fn h14_sessions_spawn_write_read_limits_reap_shutdown() {
    let home = Home::new("h14");
    let (h, _) = boot(&home, &["fx", "fx-t500"]);
    let w = &home.work;
    let c = |name: &str, v: Value| json_of(&call(&h, name, v, w));

    let id = c("fx", json!({"op": "spawn", "argv": ["cat"]}))["id"]
        .as_u64()
        .unwrap();
    assert_eq!(id, 1);
    assert_eq!(
        c("fx", json!({"op": "pwrite", "id": id, "data": "x\n"}))["ok"],
        true
    );
    let r = c(
        "fx",
        json!({"op": "pread", "id": id, "wait_ms": 2000, "until": "x"}),
    );
    assert_eq!(
        (r["data"].clone(), r["exited"].clone()),
        (json!("x\n"), json!(false))
    );
    // Quiet-period read (no `until`): returns once output stops for 50 ms.
    c("fx", json!({"op": "pwrite", "id": id, "data": "a\nb\n"}));
    let r = c("fx", json!({"op": "pread", "id": id, "wait_ms": 2000}));
    assert_eq!(r["data"], "a\nb\n");
    // wait_ms elapses with nothing to read.
    let t = Instant::now();
    let r = c("fx", json!({"op": "pread", "id": id, "wait_ms": 100}));
    assert_eq!(r["data"], "");
    assert!(t.elapsed() >= Duration::from_millis(100));

    // Sessions survive calls and belong to the name; limits: 4 per extension.
    for _ in 0..3 {
        assert!(c("fx", json!({"op": "spawn", "argv": ["cat"]}))["id"].is_u64());
    }
    let v = c("fx", json!({"op": "spawn", "argv": ["cat"]}));
    assert_eq!(v["err"], "denied", "{v}");
    assert!(v["msg"].as_str().unwrap().contains("4 per extension"));
    assert_eq!(c("fx", json!({"op": "sessions"})), json!([1, 2, 3, 4]));
    assert_eq!(c("fx-t500", json!({"op": "sessions"})), json!([]));
    let v = c(
        "fx-t500",
        json!({"op": "spawn", "argv": ["cat"], "pty": true}),
    );
    assert_eq!(
        (v["err"].clone(), v["msg"].clone()),
        (json!("denied"), json!("pty is not supported by this body"))
    );
    let v = c("fx", json!({"op": "pread", "id": 99, "wait_ms": 10}));
    assert_eq!(v["err"], "denied");

    // Exit is reported once all output is read.
    let e = c(
        "fx-t500",
        json!({"op": "spawn", "argv": ["sh", "-c", "echo done; exit 4"]}),
    )["id"]
        .as_u64()
        .unwrap();
    let r = c(
        "fx-t500",
        json!({"op": "pread", "id": e, "wait_ms": 400, "until": "done"}),
    );
    assert_eq!(r["data"], "done\n");
    assert!(wait_until(Duration::from_secs(2), || {
        let r = c("fx-t500", json!({"op": "pread", "id": e, "wait_ms": 50}));
        r["exited"] == json!(true) && r["exit_code"] == json!(4)
    }));
    assert_eq!(c("fx-t500", json!({"op": "close", "id": e}))["ok"], true);
    assert_eq!(
        c("fx-t500", json!({"op": "close", "id": e}))["err"],
        "denied"
    );

    // close kills the group.
    let pg = h.procs.session_pgids();
    assert_eq!(pg.len(), 4);
    assert_eq!(c("fx", json!({"op": "close", "id": 4}))["ok"], true);
    assert_eq!(c("fx", json!({"op": "sessions"})), json!([1, 2, 3]));

    // Idle reap with a 1 s test override (at the next call).
    h.procs.set_idle_timeout(Duration::from_secs(1));
    let p1 = h.procs.session_pgids();
    std::thread::sleep(Duration::from_millis(1100));
    assert_eq!(c("fx-t500", json!({"op": "sessions"})), json!([]));
    assert_eq!(h.procs.count(), 0);
    for g in &p1 {
        assert!(
            wait_until(Duration::from_millis(500), || !group_alive(*g)),
            "group {g} survived idle reap"
        );
    }
    h.procs.set_idle_timeout(process::SESSION_IDLE);

    // shutdown kills all.
    c("fx", json!({"op": "spawn", "argv": ["cat"]}));
    c("fx-t500", json!({"op": "spawn", "argv": ["sleep", "30"]}));
    let p2 = h.procs.session_pgids();
    assert_eq!(p2.len(), 2);
    h.shutdown();
    assert_eq!(h.procs.count(), 0);
    for g in &p2 {
        assert!(
            wait_until(Duration::from_millis(500), || !group_alive(*g)),
            "group {g} survived shutdown"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// H16, H17: memory over many calls, parallel and pool exhaustion
// ---------------------------------------------------------------------------------------------

fn rss_kib() -> u64 {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// Run alone (`cargo test --offline h16 -- --ignored`): RSS is per process, and other tests in the
/// same process (40 parallel instances, compiles) move it by tens of MiB.
#[test]
#[ignore]
fn h16_rss_flat_over_10k_calls() {
    let home = Home::new("h16");
    let (h, _) = boot(&home, &["fx"]);
    let snap = h.snapshot();
    let abort = AtomicBool::new(false);
    let cx = CallCtx {
        call_id: "h16",
        workspace: &home.work,
        abort: &abort,
    };
    let input = json!({"op": "echo"});
    for _ in 0..100 {
        h.call(&snap, "fx", &input, &cx);
    }
    let before = rss_kib();
    let t = Instant::now();
    for _ in 0..10_000 {
        assert!(!h.call(&snap, "fx", &input, &cx).is_error);
    }
    let after = rss_kib();
    eprintln!(
        "H16 10,000 calls in {} ms; RSS {} KiB → {} KiB (growth {} KiB; shared test process)",
        t.elapsed().as_millis(),
        before,
        after,
        after as i64 - before as i64
    );
    // Other tests run in this process at the same time; the bound is the doc's 5 MiB.
    assert!(
        after <= before + 5 * 1024,
        "RSS grew {} KiB",
        after - before
    );
}

#[test]
fn h17_parallel_calls_and_pool_exhaustion() {
    let home = Home::new("h17");
    let (h, _) = boot(&home, &["fx", "fx-t200"]);
    let ok = std::thread::scope(|s| {
        let hs: Vec<_> = (0..8)
            .map(|i| {
                let h = &h;
                let w = &home.work;
                s.spawn(move || call(h, "fx", json!({"op": "echo", "i": i}), w))
            })
            .collect();
        hs.into_iter()
            .map(|j| j.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(ok.iter().all(|o| !o.is_error));

    let outs = std::thread::scope(|s| {
        let hs: Vec<_> = (0..40)
            .map(|_| {
                let h = &h;
                let w = &home.work;
                s.spawn(move || call(h, "fx-t200", json!({"op": "sleep", "ms": 1500}), w))
            })
            .collect();
        hs.into_iter()
            .map(|j| j.join().unwrap())
            .collect::<Vec<_>>()
    });
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for o in &outs {
        *kinds.entry(format!("{:?}", o.kind)).or_default() += 1;
        assert!(
            matches!(
                o.kind,
                OutcomeKind::Timeout | OutcomeKind::Host | OutcomeKind::Busy
            ),
            "{}",
            o.text
        );
        if o.kind == OutcomeKind::Host {
            assert!(
                o.text
                    .contains("instance pool exhausted (32 concurrent calls)"),
                "{}",
                o.text
            );
        }
    }
    eprintln!("H17 40 parallel blocked calls: {kinds:?}");
    assert!(kinds.get("Host").copied().unwrap_or(0) >= 8, "{kinds:?}");
    assert!(wait_until(Duration::from_secs(5), || h.stuck.total() == 0
        && h.ticker.active() == 0));
    assert_eq!(
        call(&h, "fx-t200", json!({"op": "mark"}), &home.work).text,
        "t200"
    );
}

// ---------------------------------------------------------------------------------------------
// H18, H19, H24: trial/kept resolution, catalog, probe roots
// ---------------------------------------------------------------------------------------------

#[test]
fn h18_trial_kept_resolution() {
    let home = Home::new("h18");
    home.keep(&fx("fx"), "fx");
    let h = ExtHost::new(home.paths.clone()).unwrap();
    h.commit_reload(Duration::from_secs(60)).unwrap();
    let origin = |h: &ExtHost| h.snapshot().exts.get("fx").map(|v| v.origin);
    assert_eq!(origin(&h), Some(Origin::Kept));

    // A trial built before this session is ignored.
    let old = SystemTime::now() - Duration::from_secs(60);
    home.trial(&fx("fx"), "fx", old);
    h.begin_reload(ReloadKind::Request);
    let r = h.commit_reload(Duration::from_secs(60)).unwrap();
    assert_eq!(origin(&h), Some(Origin::Kept), "{}", r.text);

    // A trial built in this session wins.
    home.trial(&fx("fx"), "fx", SystemTime::now());
    h.begin_reload(ReloadKind::Request);
    let r = h.commit_reload(Duration::from_secs(60)).unwrap();
    assert_eq!(origin(&h), Some(Origin::Trial));
    assert!(r.text.contains("~ fx 0.1.0 (trial)"), "{}", r.text); // same bytes: no recompile

    // reset_trials: a new session loads only kept components.
    std::thread::sleep(Duration::from_millis(5));
    h.reset_trials();
    h.commit_reload(Duration::from_secs(60)).unwrap();
    assert_eq!(origin(&h), Some(Origin::Kept));

    // A trial built after that wins again, until the kernel publishes `fx` (Kept([fx])).
    std::thread::sleep(Duration::from_millis(5));
    home.trial(&fx("fx"), "fx", SystemTime::now());
    h.begin_reload(ReloadKind::Request);
    h.commit_reload(Duration::from_secs(60)).unwrap();
    assert_eq!(origin(&h), Some(Origin::Trial));
    std::thread::sleep(Duration::from_millis(5));
    h.begin_reload(ReloadKind::Kept(vec!["fx".into()]));
    let r = h.commit_reload(Duration::from_secs(60)).unwrap();
    assert_eq!(origin(&h), Some(Origin::Kept), "{}", r.text);

    // Request with the source dir deleted: unloaded.
    fs::remove_dir_all(home.paths.src.join("fx")).unwrap();
    h.begin_reload(ReloadKind::Request);
    let r = h.commit_reload(Duration::from_secs(60)).unwrap();
    assert!(
        r.text.contains("- fx (removed)") && r.components == ["fx"],
        "{}",
        r.text
    );
}

#[test]
fn h19_catalog_is_sorted_pretty_and_atomic() {
    let home = Home::new("h19");
    home.src_dir("draft");
    let (h, _) = boot(&home, &["fx", "fx-badschema"]);
    let path = home.paths.state.join("catalog.json");
    let bytes = fs::read_to_string(&path).unwrap();
    let v: Value = serde_json::from_str(&bytes).unwrap();
    // Canonical: re-serializing the parsed value (BTreeMap order) gives the same bytes.
    assert_eq!(
        bytes,
        format!("{}\n", serde_json::to_string_pretty(&v).unwrap())
    );
    assert_eq!(v["generation"], 1);
    let e = &v["extensions"]["fx"];
    assert_eq!(e["api"], "init:ext@1");
    assert_eq!(e["artifact"], "bin/ext/fx.wasm");
    assert_eq!(e["origin"], "kept");
    assert_eq!(e["timeout_ms"], 30_000);
    assert_eq!(e["input_schema"]["type"], "object");
    assert!(
        v["failed"]["fx-badschema"]
            .as_str()
            .unwrap()
            .contains("$ref")
    );
    assert_eq!(v["stuck"], json!({}));
    assert_eq!(v["not_built"], json!([]));
    h.begin_reload(ReloadKind::Request);
    h.commit_reload(Duration::from_secs(60)).unwrap();
    let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        (v["generation"].clone(), v["not_built"].clone()),
        (json!(2), json!(["draft"]))
    );
    // No temp file left behind.
    let names: Vec<String> = fs::read_dir(&home.paths.state)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    assert!(names.iter().all(|n| !n.contains(".tmp")), "{names:?}");
}

#[test]
fn h24_probe_root_boot_loads_exactly_the_candidate_set() {
    let home = Home::new("h24");
    let probe = home.root.join("probe/p1");
    let set = probe.join("bin/ext.d/set-a");
    fs::create_dir_all(&set).unwrap();
    fs::copy(fx("fx"), set.join("fx.wasm")).unwrap();
    fs::copy(fx("fx-t200"), set.join("fx_t200.wasm")).unwrap();
    std::os::unix::fs::symlink("ext.d/set-a", probe.join("bin/ext")).unwrap();
    let h = ExtHost::new(ExtPaths::from_home(&probe, None)).unwrap();
    let r = h.commit_reload(Duration::from_secs(60)).unwrap();
    assert_eq!(
        r.components,
        vec!["fx".to_string(), "fx-t200".to_string()],
        "{}",
        r.text
    );
    assert_eq!(h.snapshot().exts.len(), 2);
}

// ---------------------------------------------------------------------------------------------
// H20, W-2, W-3: init-body ext describe|call|inspect
// ---------------------------------------------------------------------------------------------

#[test]
fn h20_cli_describe_call_inspect() {
    let home = Home::new("h20");
    let wasm = fx("fx").to_string_lossy().into_owned();
    let (code, v) = cli::run(&["describe".into(), wasm.clone()]);
    let v = v.unwrap();
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["api"], "init:ext@1");
    assert_eq!(v["info"]["name"], "fx");
    assert!(v["compile_ms"].is_u64());

    let data = home.root.join("cli-data");
    let (code, v) = cli::run(&[
        "call".into(),
        wasm.clone(),
        "--input".into(),
        r#"{"op":"print","n":3}"#.into(),
        "--workspace".into(),
        home.work.to_string_lossy().into_owned(),
        "--data".into(),
        data.to_string_lossy().into_owned(),
    ]);
    let v = v.unwrap();
    assert_eq!(code, 0);
    assert_eq!(
        v,
        json!({"elapsed_ms": v["elapsed_ms"], "is_error": false, "kind": "ok", "log": "xxx\nto-stderr\n", "text": "printed"})
    );
    assert!(data.is_dir());

    let (code, v) = cli::run(&["inspect".into(), wasm]);
    let v = v.unwrap();
    assert_eq!(code, 0);
    assert_eq!(v["exports"], json!(["call", "describe"]));
    let imports: Vec<String> = serde_json::from_value(v["imports"].clone()).unwrap();
    assert!(
        imports.contains(&"init:ext/process@1.0.0".to_string()),
        "{imports:?}"
    );
    assert!(
        imports.iter().any(|i| i.starts_with("wasi:cli/")),
        "{imports:?}"
    );
    assert!(!imports.iter().any(|i| i.starts_with("wasi:http")));

    // W-3: does a guest that never calls `process` import it? (recorded, not asserted)
    let (_, v) = cli::run(&[
        "inspect".into(),
        count_lines_wasm().to_string_lossy().into_owned(),
    ]);
    let v = v.unwrap();
    eprintln!("W-3 count-lines imports: {}", v["imports"]);

    // Load errors exit 1; usage errors exit 2.
    let bad = fx("fx-badsig").to_string_lossy().into_owned();
    let (code, v) = cli::run(&["describe".into(), bad]);
    assert_eq!(code, 1);
    assert!(
        v.unwrap()["error"]
            .as_str()
            .unwrap()
            .starts_with("does not match init:ext@1")
    );
    assert_eq!(cli::run(&["frobnicate".into()]).0, 2);
    assert_eq!(cli::run(&["call".into(), "x.wasm".into()]).0, 2);
}

// ---------------------------------------------------------------------------------------------
// H21: Cranelift incremental cache, cold vs warm (M-2 host side)
// ---------------------------------------------------------------------------------------------

#[test]
fn h21_incremental_cache_warm_vs_cold_recompile() {
    let (v1, v2) = swap_builds();
    let home = Home::new("h21");
    let a = ExtHost::build(home.paths.clone()).unwrap();
    let ms = |h: &ExtHost, p: &Path| {
        h.load_one(None, p, Origin::Trial, None)
            .unwrap()
            .0
            .compile_ms
    };
    let cold_v1 = ms(&a, &v1);
    let after_v1 = a.incr.stats();
    let warm_v2 = ms(&a, &v2);
    let after_v2 = a.incr.stats();
    let b = ExtHost::build(home.paths.clone()).unwrap();
    let cold_v2 = ms(&b, &v2);
    let cold = b.incr.stats();
    let cl = count_lines_wasm();
    let cold_cl = ms(&b, &cl);
    // The cache is keyed by each function's compile input, and an entry is inserted only on a
    // miss. So the entries a compile adds count the functions it compiled; the rest came from
    // the cache. A cold engine compiles every function of v2; the warm engine (which compiled
    // v1) recompiles only what the edit changed. Counting entries is deterministic; wall-clock
    // milliseconds are not (K-FLAKE: warm 310 ms vs cold 148 ms once under load), so the
    // timings are a measurement print only.
    let warm_compiled = after_v2.0 - after_v1.0;
    let warm_hits = cold.0.saturating_sub(warm_compiled);
    eprintln!(
        "H21 measurement (not asserted): compile_ms swapper v1 cold {cold_v1}; v2 (edited) warm {warm_v2} vs cold {cold_v2}; count-lines (after swapper on the same engine) {cold_cl}"
    );
    eprintln!(
        "H21 cache entries: v1 cold {} ({} bytes); v2 warm compiled {warm_compiled}, reused {warm_hits}; v2 cold compiled {} ({} bytes)",
        after_v1.0, after_v1.1, cold.0, cold.1
    );
    assert!(after_v1.0 > 0, "the incremental cache was used");
    assert!(
        after_v2.0 >= after_v1.0 && after_v2.1 >= after_v1.1 && cold.1 <= INCR_CACHE_BYTES,
        "no eviction during the test, so entry counts are miss counts"
    );
    assert!(cold.0 > 0, "a cold compile fills the cache");
    assert!(
        warm_compiled * 2 < cold.0,
        "the warm recompile of the edited component took most of its functions from the cache: compiled {warm_compiled} of {} (cold)",
        cold.0
    );
}

// ---------------------------------------------------------------------------------------------
// H23: owner abort
// ---------------------------------------------------------------------------------------------

/// Sets `abort` after `after` and, with `ready`, not before that file holds a whole line (the
/// guest's process has started: a fixed delay alone loses that race on a loaded machine, and the
/// abort would then land before the spawn). Returns (outcome, ms from abort to return).
fn abort_after(
    h: &ExtHost,
    name: &str,
    input: Value,
    ws: &Path,
    after: Duration,
    ready: Option<&Path>,
) -> (ExtOutcome, u128) {
    let abort = AtomicBool::new(false);
    let set_at: Mutex<Option<Instant>> = Mutex::new(None);
    let o = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(after);
            if let Some(p) = ready {
                let t = Instant::now();
                while !fs::read_to_string(p).is_ok_and(|s| s.ends_with('\n'))
                    && t.elapsed() < Duration::from_secs(30)
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            *set_at.lock().unwrap() = Some(Instant::now());
            abort.store(true, SeqCst);
        });
        h.call(
            &h.snapshot(),
            name,
            &input,
            &CallCtx {
                call_id: "abort",
                workspace: ws,
                abort: &abort,
            },
        )
    });
    let ms = set_at
        .lock()
        .unwrap()
        .map(|t| t.elapsed().as_millis())
        .unwrap_or(0);
    (o, ms)
}

#[test]
fn h23_owner_abort_loop_run_and_read() {
    let home = Home::new("h23");
    let (h, _) = boot(&home, &["fx"]);
    let w = &home.work;

    // (a) guest loop
    let (o, ms) = abort_after(
        &h,
        "fx",
        json!({"op": "loop"}),
        w,
        Duration::from_millis(100),
        None,
    );
    eprintln!("H23a loop aborted {ms} ms after the flag");
    assert_eq!(o.kind, OutcomeKind::Aborted, "{}", o.text);
    assert_eq!(o.text, "ext fx 0.1.0: aborted by owner interrupt");
    assert!(ms <= 100, "{ms} ms");
    assert_eq!(call(&h, "fx", json!({"op": "mark"}), w).text, "main");

    // (b) process.run(sleep 10): the group dies, the call ends as aborted.
    let (o, ms) = abort_after(
        &h,
        "fx",
        json!({"op": "run", "argv": ["sh", "-c", "echo $$ > pid; exec sleep 10"], "timeout_ms": 20000}),
        w,
        Duration::from_millis(200),
        Some(&w.join("pid")),
    );
    eprintln!("H23b run aborted {ms} ms after the flag");
    assert_eq!(o.kind, OutcomeKind::Aborted, "{}", o.text);
    assert!(ms <= 250, "{ms} ms");
    let pgid: i32 = fs::read_to_string(w.join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        wait_until(Duration::from_millis(200), || !group_alive(pgid)),
        "group {pgid} survived the abort"
    );

    // (c) process.read(wait 10 s)
    let id = json_of(&call(&h, "fx", json!({"op": "spawn", "argv": ["cat"]}), w))["id"].clone();
    let (o, ms) = abort_after(
        &h,
        "fx",
        json!({"op": "pread", "id": id, "wait_ms": 10000}),
        w,
        Duration::from_millis(100),
        None,
    );
    eprintln!("H23c read aborted {ms} ms after the flag");
    assert_eq!(o.kind, OutcomeKind::Aborted, "{}", o.text);
    assert!(ms <= 150, "{ms} ms");
    assert_eq!(call(&h, "fx", json!({"op": "mark"}), w).text, "main");
    assert_eq!(h.stuck.total(), 0);
    h.shutdown();
}

// ---------------------------------------------------------------------------------------------
// The seam to the ext tool (WP11's ExtHostApi)
// ---------------------------------------------------------------------------------------------

#[test]
fn api_seam_through_dyn_ext_host_api() {
    use crate::tools::ext::ExtHostApi;
    let home = Home::new("api");
    home.keep(&fx("fx"), "fx");
    let host: Arc<dyn ExtHostApi> = ExtHost::new(home.paths.clone()).unwrap();
    assert!(host.reload_pending());
    let r = host.commit_reload(RELOAD_WAIT_BEFORE_EXT).unwrap();
    assert_eq!(r.components, vec!["fx".to_string()]);
    let snap = host.snapshot();
    let abort = AtomicBool::new(false);
    let o = host.call(
        &snap,
        "fx",
        &json!({"op": "mark"}),
        &CallCtx {
            call_id: "api",
            workspace: &home.work,
            abort: &abort,
        },
    );
    assert_eq!((o.text.as_str(), o.kind), ("main", OutcomeKind::Ok));
    // A foreign snapshot falls back to the host's own registry.
    let foreign: crate::tools::ext::Snapshot = Arc::new(());
    let o = host.call(
        &foreign,
        "fx",
        &json!({"op": "mark"}),
        &CallCtx {
            call_id: "api",
            workspace: &home.work,
            abort: &abort,
        },
    );
    assert_eq!(o.text, "main");
    host.shutdown();
}
