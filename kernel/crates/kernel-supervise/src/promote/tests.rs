//! Unit tests of the pipeline's control paths with fake seams (no processes): F16, the G-46
//! resubmission, `rollback`, and `promote`'s argument refusals.

use super::*;
use crate::instance::LocalSpawner;
use init_proto::initctl::{PromoteArgs, RollbackArgs};
use serde_json::json;

#[derive(Default)]
struct FakeSup {
    state: Mutex<Option<SupState>>,
    calls: Mutex<Vec<String>>,
    /// Owner-only events (`kind gate_id`), kept apart from `calls` (v0.5.2: `gate_deps`).
    events: Mutex<Vec<String>>,
}

impl Supervise for FakeSup {
    fn handoff(&self, job: &str, sha: &str, _: &str, _: Option<String>) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("handoff {job} {sha}"));
        Ok(())
    }
    fn g4_pass(&self, sha: &str, job: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("g4_pass {sha} {job}"));
        Ok(())
    }
    fn g4_reject(&self, sha: &str, job: &str, reason: &str) {
        self.calls.lock().unwrap().push(format!("g4_reject {sha} {job} {reason}"));
    }
    fn rollback(&self, reason: &str, immediate: bool) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("rollback {reason} {immediate}"));
        Ok(())
    }
    fn snapshot(&self) -> Option<SupState> {
        self.state.lock().unwrap().clone()
    }
    fn notice(&self, kind: &str, _: Value) {
        self.calls.lock().unwrap().push(format!("notice {kind}"));
    }
    fn owner_event(&self, kind: &str, data: Value) {
        self.events.lock().unwrap().push(format!("{kind} {}", data["gate_id"].as_str().unwrap_or("")));
    }
}

#[derive(Default)]
struct FakeLineage {
    down: AtomicBool,
    sent: Mutex<Vec<(String, Value)>>,
}

impl GateLineage for FakeLineage {
    fn append_wait(&self, kind: &str, data: Value) -> Result<u64, String> {
        if self.down.load(Ordering::SeqCst) {
            return Err("down".into());
        }
        let mut s = self.sent.lock().unwrap();
        s.push((kind.into(), data));
        Ok(100 + s.len() as u64)
    }
    fn local(&self, _: &str, _: Value) {}
}

fn env(name: &str, sup: Arc<FakeSup>, lin: Arc<FakeLineage>) -> Arc<Env> {
    let home = crate::testutil::test_dir(name);
    std::fs::create_dir_all(&home).unwrap();
    Arc::new(Env {
        home,
        cfg: crate::config::defaults(),
        gate: GateCfg::default(),
        spawner: Arc::new(LocalSpawner::new()),
        g0: Arc::new(kernel_gate::g0::DirectSpawner),
        proxy: Arc::new(NoGateProxy),
        lineage: lin,
        sup,
        jobs: Arc::new(MemJobs::default()),
        upload_pack: "git-upload-pack".into(),
        level: 0,
        kernel: KernelInfo::default(),
        model: "deepseek-flash".into(),
        pipeline_busy: AtomicBool::new(false),
        g4_running: Mutex::new(None),
        gates: AtomicU64::new(0),
        g4_attempts: Mutex::new(Default::default()),
        fault: None,
        slot_lock: Mutex::new(()),
        sealed: None,
        extend: None,
    })
}

fn st(cur: &str, lkg: &str, provisional: bool, busy: bool) -> Option<SupState> {
    Some(SupState { current_sha: cur.into(), lkg_sha: lkg.into(), provisional, busy })
}

/// F16: a provisional version older than `provisional_max_age_s` is rolled back; a younger one is not.
#[test]
fn f16_provisional_expiry_rolls_back() {
    let sup = Arc::new(FakeSup::default());
    let e = env("pw-f16", sup.clone(), Arc::new(FakeLineage::default()));
    *sup.state.lock().unwrap() = st(&"b".repeat(40), &"a".repeat(40), true, false);
    let out = provisional_watch(&e, 3600);
    assert_eq!(out["rolled_back"], false);
    assert!(sup.calls.lock().unwrap().is_empty());
    let out = provisional_watch(&e, 86_401);
    assert_eq!(out["rolled_back"], true);
    assert_eq!(sup.calls.lock().unwrap().as_slice(), [format!("g4_reject {} provisional_watch provisional_stale", "b".repeat(40))]);
    assert!(std::fs::read_to_string(e.home.join("state/kernel/alerts.jsonl")).unwrap().contains("provisional_stale"));
    crate::testutil::remove_test_dir(&e.home);
}

/// G-46 (control path): a pending accept record is resubmitted; the LKG commit follows the ack.
#[test]
fn g46_pending_record_is_resubmitted_then_lkg() {
    let sup = Arc::new(FakeSup::default());
    let lin = Arc::new(FakeLineage { down: AtomicBool::new(true), ..Default::default() });
    let e = env("pw-g46", sup.clone(), lin.clone());
    *sup.state.lock().unwrap() = st(&"b".repeat(40), &"a".repeat(40), true, false);
    let d = e.home.join("vault/gate/pending");
    std::fs::create_dir_all(&d).unwrap();
    let rec = json!({"kind": "gate", "v": 2, "gate_id": "j-900001", "gate_kind": "g4", "candidate": {"sha": "b".repeat(40)},
                     "kernel": {"build_sha256": "x"}, "stages": {"G4": {"status": "accept"}}, "verdict": "accept", "reason": "better",
                     "level": "L0", "scope": null, "escalated": false, "spent_musd": 0});
    crate::fsx::write_json(&d.join("j-900001.json"), &json!({"job": "j-900001", "sha": "b".repeat(40), "record": rec})).unwrap();
    let out = provisional_watch(&e, 10);
    assert_eq!(out["resubmitted"], json!([]), "still down");
    assert!(sup.calls.lock().unwrap().is_empty());
    // A stale provisional with a pending record is not rolled back: the verdict is in.
    assert_eq!(provisional_watch(&e, 100_000)["rolled_back"], false);
    lin.down.store(false, Ordering::SeqCst);
    let out = provisional_watch(&e, 10);
    assert_eq!(out["resubmitted"][0]["lkg"], true);
    assert_eq!(sup.calls.lock().unwrap().as_slice(), [format!("g4_pass {} j-900001", "b".repeat(40))]);
    let sent = lin.sent.lock().unwrap();
    assert_eq!(sent[0].0, "gate");
    assert!(sent[0].1.get("kernel").is_none(), "the Root gets the subset");
    assert!(!d.join("j-900001.json").exists());
    drop(sent);
    crate::testutil::remove_test_dir(&e.home);
}

/// `rollback`: `E_ALREADY_LKG`, `E_BUSY`, then from/to and supervise's rollback after the boundary.
#[test]
fn rollback_command() {
    let sup = Arc::new(FakeSup::default());
    let e = env("pw-rb", sup.clone(), Arc::new(FakeLineage::default()));
    let (a, b) = ("a".repeat(40), "b".repeat(40));
    *sup.state.lock().unwrap() = st(&a, &a, false, false);
    let j = MemJob::new("j-000001");
    assert_eq!(rollback(e.clone(), &RollbackArgs { reason: "x".into(), wait: None }, j.clone()).unwrap_err().code.as_str(), "E_ALREADY_LKG");
    *sup.state.lock().unwrap() = st(&b, &a, true, true);
    assert_eq!(rollback(e.clone(), &RollbackArgs { reason: "x".into(), wait: None }, j.clone()).unwrap_err().code.as_str(), "E_BUSY");
    *sup.state.lock().unwrap() = st(&b, &a, true, false);
    let r = rollback(e.clone(), &RollbackArgs { reason: "owner said so".into(), wait: None }, j.clone()).unwrap();
    assert_eq!((r["from"].as_str(), r["to"].as_str()), (Some(b.as_str()), Some(a.as_str())));
    let res = j.wait(std::time::Duration::from_secs(10)).unwrap().unwrap();
    assert_eq!(res["to"], a.as_str());
    assert!(j.boundary.load(Ordering::SeqCst));
    assert_eq!(sup.calls.lock().unwrap().as_slice(), ["rollback owner said so true".to_string()]);
    crate::testutil::remove_test_dir(&e.home);
}

/// `promote` refuses bad arguments before any work and leaves the slot free.
#[test]
fn promote_argument_refusals() {
    let sup = Arc::new(FakeSup::default());
    let e = env("pw-args", sup.clone(), Arc::new(FakeLineage::default()));
    let j = MemJob::new("j-000001");
    let code = |a: PromoteArgs| promote(e.clone(), &a, j.clone()).unwrap_err().code.as_str().to_string();
    assert_eq!(code(PromoteArgs { git_ref: None, sha: Some("a".repeat(40)), wait: None, check: None }), "E_LEVEL");
    assert_eq!(code(PromoteArgs { git_ref: Some("main".into()), sha: None, wait: None, check: None }), "E_ARGS");
    assert_eq!(code(PromoteArgs { git_ref: Some("evo/Bad Id".into()), sha: None, wait: None, check: None }), "E_ARGS");
    *sup.state.lock().unwrap() = st(&"a".repeat(40), &"a".repeat(40), false, true);
    assert_eq!(code(PromoteArgs { git_ref: Some("evo/ok".into()), sha: None, wait: None, check: None }), "E_BUSY");
    *sup.state.lock().unwrap() = st(&"a".repeat(40), &"a".repeat(40), false, false);
    // No lineage here: the fetch fails (`E_FETCH`) and the pipeline slot is released.
    assert_eq!(code(PromoteArgs { git_ref: Some("evo/ok".into()), sha: None, wait: None, check: None }), "E_FETCH");
    assert!(!e.pipeline_busy.load(Ordering::SeqCst));
    crate::testutil::remove_test_dir(&e.home);
}

/// The `Job` adapter contract: one terminal, `awaiting_boundary` before it.
#[test]
fn mem_job_terminal_once() {
    let j = MemJob::new("j-000002");
    assert!(j.awaiting_boundary());
    j.succeed(json!({"a": 1}));
    j.fail(init_proto::envelope::ErrorBody::new(init_proto::errors::ErrorCode::Internal, "late"));
    assert_eq!(j.wait(std::time::Duration::from_secs(1)).unwrap().unwrap()["a"], 1);
    let jobs = MemJobs::default();
    let a = jobs.create("g4", json!({}));
    assert_eq!(a.id(), "j-900001");
    assert_eq!(jobs.of("g4").len(), 1);
}

/// The `build` profile's section 5 (kernel/sandbox.md §3.4): reads under `SRC`, `OUT` or `KERNEL`,
/// writes under `OUT` only, nothing under `VAULT`.
fn build_profile_allows(params: &std::collections::BTreeMap<String, String>, p: &std::path::Path, write: bool) -> bool {
    let under = |k: &str| params.get(k).is_some_and(|v| p.starts_with(v));
    if under("VAULT") {
        return false;
    }
    if write {
        under("OUT")
    } else {
        under("SRC") || under("OUT") || under("KERNEL")
    }
}

/// S-REAL's live G1 failure (`gate.g1.manifest_error`: `init-body --manifest: cannot start`): the
/// selfmap child's `OUT` was `cache/gate/<job>/g1`, so the candidate binary and components in
/// `cache/gate/<job>/{bin,ext}` were outside its profile. Every path the child is given must lie
/// inside what the `build` profile allows, for the promote job and for G3's inner gate.
#[test]
fn g1_child_paths_lie_inside_the_build_profile() {
    let e = env("pw-g1out", Arc::new(FakeSup::default()), Arc::new(FakeLineage::default()));
    let src = crate::build::BuildSlot::new(&e.home).src();
    for dir in [stages::job_cache(&e, "j-000001"), stages::job_cache(&e, "j-000001").join("g3")] {
        let built = stages::Built { dir: dir.clone(), binary: dir.join("bin/init-body"), components: dir.join("ext"), record: Value::Null };
        let c = stages::g1_child(&e, &src, &built);
        let p = &c.req.params;
        assert_eq!(c.req.profile, "build");
        assert_eq!(p["OUT"], dir.to_string_lossy(), "OUT is the dir holding bin/ and ext/ (sandbox.md §3.4)");
        assert_eq!(p["SRC"], src.to_string_lossy());
        let cache = e.home.join("cache");
        for k in ["SRC", "OUT"] {
            assert!(std::path::Path::new(&p[k]).starts_with(&cache), "{k} under cache/ (the Root's class rule)");
        }
        for read in [&built.binary, &built.components.join("count-lines.wasm"), &src.join("Cargo.toml"), &e.home.join("kernel/bin/init-kernel")] {
            assert!(build_profile_allows(p, read, false), "{} readable", read.display());
        }
        let home = std::path::PathBuf::from(&c.req.env["HOME"]);
        let tmp = std::path::PathBuf::from(&c.req.env["TMPDIR"]);
        for write in [&c.out_file, &c.log, &home.join("x"), &tmp.join("x"), &c.req.cwd.join("x")] {
            assert!(build_profile_allows(p, write, true), "{} writable", write.display());
        }
        assert!(!build_profile_allows(p, &e.home.join("vault/gate/jobs/j-000001/self.json"), false));
        // The argv names exactly these paths.
        let a = &c.req.args;
        assert_eq!(a[a.iter().position(|x| x == "--binary").unwrap() + 1], built.binary.to_string_lossy());
        assert_eq!(a[a.iter().position(|x| x == "--components").unwrap() + 1], built.components.to_string_lossy());
        assert_eq!(a[a.iter().position(|x| x == "--out").unwrap() + 1], c.out_file.to_string_lossy());
    }
    crate::testutil::remove_test_dir(&e.home);
}

/// supervise.md §3.6.2 "Dependency cache", end to end with real cargo (LocalSpawner, DirectSpawner):
/// a tiny body with one vendored crate whose build script says `rerun-if-changed=build.rs`.
/// 1. Miss: the stub builds in the slot and is staged, the candidate then builds on its target
///    (cargo compiles only the body) and runs the candidate's code, never the stub's empty main.
/// 2. Published, a second commit whose export looks newer (vendor mtimes in the future, as a later
///    `git archive` stamps them) hits: the vendored crate is not compiled again.
/// 3. A corrupt entry fails verification, is deleted, and the build falls back to the stub (cold).
#[test]
fn deps_cache_stub_then_warm_hit_then_corrupt_falls_back() {
    use crate::build::{BuildSlot, DepsCache};
    let Some(tc) = std::env::var_os("CARGO").map(std::path::PathBuf::from).and_then(|c| Some(c.parent()?.parent()?.to_path_buf())).filter(|t| t.join("bin/rustc").is_file()) else {
        eprintln!("note: no toolchain found through $CARGO; skipped");
        return;
    };
    let fx = crate::git::testfx::Fx::new("deps-e2e");
    let sum = "5".repeat(64);
    fx.write("Cargo.toml", b"[package]\nname = \"init-body\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"init-body\"\npath = \"src/main.rs\"\n\n[dependencies]\ntinydep = \"0.1.0\"\n\n[workspace]\n");
    fx.write("Cargo.lock", format!("version = 4\n\n[[package]]\nname = \"init-body\"\nversion = \"0.1.0\"\ndependencies = [\n \"tinydep\",\n]\n\n[[package]]\nname = \"tinydep\"\nversion = \"0.1.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{sum}\"\n").as_bytes());
    fx.write("vendor/tinydep/Cargo.toml", b"[package]\nname = \"tinydep\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n");
    fx.write("vendor/tinydep/.cargo-checksum.json", format!("{{\"files\":{{}},\"package\":\"{sum}\"}}").as_bytes());
    fx.write("vendor/tinydep/build.rs", b"fn main() { println!(\"cargo:rerun-if-changed=build.rs\"); println!(\"cargo:rustc-env=TINY=dep\"); }\n");
    fx.write("vendor/tinydep/src/lib.rs", b"pub fn v() -> &'static str { env!(\"TINY\") }\n");
    fx.write("src/main.rs", b"fn main() { println!(\"one {}\", tinydep::v()); }\n");
    let c1 = fx.commit("one");
    fx.write("src/main.rs", b"fn main() { println!(\"two {}\", tinydep::v()); }\n");
    let c2 = fx.commit("two");
    fx.set_main(&c1);
    let mut cfg = crate::config::defaults();
    let sdk = std::process::Command::new("/usr/bin/xcrun").arg("--show-sdk-path").output().ok().filter(|o| o.status.success());
    cfg.sandbox = crate::spawnspec::SandboxPaths {
        toolchain: tc,
        devdir: if cfg!(target_os = "macos") { "/Library/Developer/CommandLineTools".into() } else { std::path::PathBuf::new() },
        home_real: "/nonexistent".into(),
        sdkroot: sdk.map(|o| std::path::PathBuf::from(String::from_utf8_lossy(&o.stdout).trim())),
    };
    cfg.supervise.git = crate::git::testfx::GIT_BIN.into();
    let sup = Arc::new(FakeSup::default());
    let e = Env {
        home: fx.home.clone(),
        cfg,
        gate: GateCfg::default(),
        spawner: Arc::new(LocalSpawner::new()),
        g0: Arc::new(kernel_gate::g0::DirectSpawner),
        proxy: Arc::new(NoGateProxy),
        lineage: Arc::new(FakeLineage::default()),
        sup: sup.clone(),
        jobs: Arc::new(MemJobs::default()),
        upload_pack: "git-upload-pack".into(),
        level: 0,
        kernel: KernelInfo::default(),
        model: "deepseek-flash".into(),
        pipeline_busy: AtomicBool::new(false),
        g4_running: Mutex::new(None),
        gates: AtomicU64::new(0),
        g4_attempts: Mutex::new(Default::default()),
        fault: None,
        slot_lock: Mutex::new(()),
        sealed: None,
        extend: None,
    };
    let git = e.git();
    let slot = BuildSlot::configured(&e.home, &e.cfg.supervise);
    let cache = DepsCache::new(&e.home);
    let log = e.home.join("state/kernel/gate/j-1/build.log");
    // Export `sha` (optionally making its vendor/ look newer), prepare the deps, build the candidate.
    let round = |sha: &str, newer_vendor: bool| {
        slot.reset().unwrap();
        slot.export(&git, sha).unwrap();
        if newer_vendor {
            let t = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
            for f in ["vendor/tinydep/build.rs", "vendor/tinydep/src/lib.rs", "vendor/tinydep/Cargo.toml"] {
                std::fs::File::options().write(true).open(slot.src().join(f)).unwrap().set_modified(t).unwrap();
            }
        }
        let prep = stages::prepare_deps(&e, &slot, &git, sha, "j-1", crate::build::SLOT_EPOCH_S).unwrap();
        let b = slot.run_build(e.spawner.as_ref(), &slot.build_request(crate::build::SLOT_EPOCH_S, &e.cfg.sandbox), prep.mode != "cold", &log);
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(b.ok, "{b:?}\n{text}\n{prep:?}");
        let out = std::process::Command::new(slot.built_binary()).env_clear().output().unwrap();
        (prep, text, String::from_utf8_lossy(&out.stdout).into_owned())
    };
    // 1. Miss: stub, staged; the candidate's own code runs.
    let (p1, log1, out1) = round(&c1, false);
    assert_eq!((p1.mode, p1.staged.is_some()), ("stub", true), "{p1:?}");
    let key = p1.key.clone().unwrap();
    assert_eq!(out1, "one dep\n");
    assert!(log1.contains("Compiling init-body") && !log1.contains("Compiling tinydep"), "deps came from the stub:\n{log1}");
    let stub_log = std::fs::read_to_string(e.home.join("state/kernel/gate/j-1/deps-stub.log")).unwrap();
    assert!(stub_log.contains("Compiling tinydep") && stub_log.contains("$ cargo check") && stub_log.contains("--no-run"), "{stub_log}");
    assert!(cache.lookup(&key).is_none(), "nothing is published before G0 passes");
    // v0.5.2 (R-TUI): the owner hears that the dependency build started (the gate track's `deps` step)
    assert_eq!(sup.events.lock().unwrap().as_slice(), ["gate_deps j-1"]);
    assert!(cache.publish(&key, p1.staged.as_deref().unwrap()).unwrap());
    let fps: Vec<String> = std::fs::read_dir(cache.lookup(&key).unwrap().join("debug/.fingerprint")).unwrap().flatten().map(|d| d.file_name().to_string_lossy().into_owned()).collect();
    assert!(fps.iter().any(|f| f.starts_with("tinydep-")) && !fps.iter().any(|f| f.starts_with("init-body-")), "{fps:?}");
    // 2. Hit: warm, the vendored crate is not compiled again although its export is newer.
    let (p2, log2, out2) = round(&c2, true);
    assert_eq!((p2.mode, p2.key.as_deref()), ("hit", Some(key.as_str())), "{p2:?}");
    assert_eq!(out2, "two dep\n");
    assert!(log2.contains("Compiling init-body") && !log2.contains("Compiling tinydep"), "{log2}");
    assert_eq!(sup.events.lock().unwrap().len(), 1, "a warm hit builds no dependencies, so no event");
    // 3. Corrupt entry: rejected, deleted, rebuilt from the stub.
    let rlib = std::fs::read_dir(cache.lookup(&key).unwrap().join("debug/deps")).unwrap().flatten().map(|d| d.path()).find(|p| p.extension().is_some_and(|x| x == "rlib")).unwrap();
    std::fs::write(&rlib, b"poisoned").unwrap();
    let (p3, _, out3) = round(&c2, false);
    assert_eq!(p3.mode, "stub", "{p3:?}");
    assert!(p3.note.contains("failed verification"), "{p3:?}");
    assert_eq!(out3, "two dep\n");
    assert!(cache.lookup(&key).is_none(), "the corrupt entry is gone");
    cache.discard(&key).unwrap();
}
