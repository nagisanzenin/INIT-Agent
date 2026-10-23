//! WP8b ↔ WP7: the pipeline's `promote::Job` seam over kernel-serve's real `JobCtl`, as the
//! `Backend` implementation in `init-kernel` will wire it (`ServeJob` is that adapter verbatim).

use init_proto::envelope::ErrorBody;
use init_proto::initctl::{JobState, PromoteArgs, RollbackArgs, Stage, StageResult};
use kernel_serve::{Begun, JobConfig, JobCtl, JobStore, LaneCtx};
use kernel_supervise::promote::{self, Env, GateCfg, GateLineage, Job, KernelInfo, MemJobs, NoGateProxy, SupState, Supervise};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

/// The adapter `init-kernel`'s `Backend` uses: one line per method.
struct ServeJob(JobCtl);

impl Job for ServeJob {
    fn id(&self) -> String {
        self.0.id().to_string()
    }
    fn cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
    fn progress(&self, state: JobState, stage: Option<Stage>, pct: u8, msg: &str) {
        self.0.progress(state, stage, pct, None, msg);
    }
    fn stage_done(&self, r: StageResult) {
        self.0.stage_done(r);
    }
    fn awaiting_boundary(&self) -> bool {
        self.0.awaiting_boundary().is_ok()
    }
    fn succeed(&self, result: Value) {
        self.0.succeed_value(result);
    }
    fn fail(&self, err: ErrorBody) {
        self.0.fail(err);
    }
}

/// `Backend::promote` and `Backend::rollback` as `init-kernel` implements them.
fn backend_promote(env: &Arc<Env>, a: &PromoteArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
    let r = promote::promote(env.clone(), a, Arc::new(ServeJob(job)))?;
    Ok(Begun::started().with("sha", r["sha"].clone()))
}

fn backend_rollback(env: &Arc<Env>, a: &RollbackArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
    let r = promote::rollback(env.clone(), a, Arc::new(ServeJob(job)))?;
    Ok(Begun::started().with("from", r["from"].clone()).with("to", r["to"].clone()))
}

struct Sup(Mutex<Vec<String>>);
impl Supervise for Sup {
    fn handoff(&self, _: &str, _: &str, _: &str, _: Option<String>) -> Result<(), String> {
        Ok(())
    }
    fn g4_pass(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn g4_reject(&self, _: &str, _: &str, _: &str) {}
    fn rollback(&self, reason: &str, immediate: bool) -> Result<(), String> {
        self.0.lock().unwrap().push(format!("{reason} {immediate}"));
        Ok(())
    }
    fn snapshot(&self) -> Option<SupState> {
        Some(SupState { current_sha: "b".repeat(40), lkg_sha: "a".repeat(40), provisional: true, busy: false })
    }
    fn notice(&self, _: &str, _: Value) {}
}

struct NoLineage;
impl GateLineage for NoLineage {
    fn append_wait(&self, _: &str, _: Value) -> Result<u64, String> {
        Ok(1)
    }
    fn local(&self, _: &str, _: Value) {}
}

fn home() -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/w8b/seam");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("state/kernel/jobs")).unwrap();
    std::fs::canonicalize(d).unwrap()
}

#[test]
fn promote_and_rollback_through_kernel_serve_jobs() {
    let _wd = init_testkit::watchdog::arm("promote_and_rollback_through_kernel_serve_jobs", WD);
    let h = home();
    let sup = Arc::new(Sup(Mutex::new(vec![])));
    let env = Arc::new(Env {
        home: h.clone(),
        cfg: kernel_supervise::config::defaults(),
        gate: GateCfg::default(),
        spawner: Arc::new(kernel_supervise::instance::LocalSpawner::new()),
        g0: Arc::new(kernel_gate::g0::DirectSpawner),
        proxy: Arc::new(NoGateProxy),
        lineage: Arc::new(NoLineage),
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
    });
    let (store, _) = JobStore::open(&h.join("state/kernel/jobs"), JobConfig { flush_timeout: Duration::from_millis(100), ..JobConfig::default() }).unwrap();
    let lane = LaneCtx::live();
    // The synchronous part's refusal becomes the response (serve then discards the job).
    let j = store.create(&lane, "promote", json!({"ref": "main"})).unwrap();
    let e = backend_promote(&env, &PromoteArgs { git_ref: Some("main".into()), sha: None, wait: None }, j).unwrap_err();
    assert_eq!(e.code.as_str(), "E_ARGS");
    // rollback: `{from, to}`, then the boundary milestone, then exactly one terminal.
    let j = store.create(&lane, "rollback", json!({"reason": "t"})).unwrap();
    let id = j.id().to_string();
    let b = backend_rollback(&env, &RollbackArgs { reason: "t".into(), wait: None }, j).unwrap();
    assert_eq!(b, Begun::started().with("from", "b".repeat(40)).with("to", "a".repeat(40)));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let r = store.record(&id).unwrap();
        if r.state.is_terminal() {
            assert_eq!(r.state, JobState::Succeeded, "{r:?}");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "rollback job never finished: {r:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(sup.0.lock().unwrap().as_slice(), ["t true".to_string()]);
    let _ = std::fs::remove_dir_all(&h);
}
