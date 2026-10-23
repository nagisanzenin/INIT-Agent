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
    assert_eq!(code(PromoteArgs { git_ref: None, sha: Some("a".repeat(40)), wait: None }), "E_LEVEL");
    assert_eq!(code(PromoteArgs { git_ref: Some("main".into()), sha: None, wait: None }), "E_ARGS");
    assert_eq!(code(PromoteArgs { git_ref: Some("evo/Bad Id".into()), sha: None, wait: None }), "E_ARGS");
    *sup.state.lock().unwrap() = st(&"a".repeat(40), &"a".repeat(40), false, true);
    assert_eq!(code(PromoteArgs { git_ref: Some("evo/ok".into()), sha: None, wait: None }), "E_BUSY");
    *sup.state.lock().unwrap() = st(&"a".repeat(40), &"a".repeat(40), false, false);
    // No lineage here: the fetch fails (`E_FETCH`) and the pipeline slot is released.
    assert_eq!(code(PromoteArgs { git_ref: Some("evo/ok".into()), sha: None, wait: None }), "E_FETCH");
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
