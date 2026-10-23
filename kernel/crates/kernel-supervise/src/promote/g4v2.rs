//! G4 v2's process side (M7, lane GATE-V2a; m7-design.md §3.2.1; build/specs/PROOF-LEAN.md D-L3):
//! the slice client and the driver the pipeline calls for every G4 (sync and async).
//!
//! ```text
//! g4_v2:  S path eligible? (env.sealed set; candidate differs from LKG in the body binary only)
//!         · pre-flight: tier and estimate (kernel_gate::g4v2::preflight)
//!         · run_screen: the gate's own canaries, then the screen (GATE-V2b)   → reject? done
//!         · store.put the candidate body · tuple.get (lkg id) · sealed.g4_slice
//!         · accept_v2 → G4Outcome with the record v3 keys (`v2`)
//! ```
//!
//! `root.sealed_none` (no S drawn; a Root that does not know the command) sends G4 down the unchanged
//! v0.3 kernel path; the caller does that ([`V2Path::Fallback`]). The pure rules are
//! `kernel_gate::g4v2`.

use super::g4run::{ArmSpec, ProbeRuns};
use super::Env;
use crate::link::LinkError;
use init_proto::root_link::{KernelRequest, StoreKind, StorePutArgs};
use kernel_gate::accept::{CostBudget, Verdict};
use kernel_gate::g4::{G4Outcome, RunRecord};
use kernel_gate::g4v2::{
    self as v2, Preflight, ScreenSummary, SliceAnswer, SliceArgs, SliceCandidate, SliceExt, SliceRefusal, Tier, V2Inputs, V2Record,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The Root side of G4 v2, as the driver uses it (a seam: tests pass an in-process fake).
pub trait SealedLink: Send + Sync {
    /// `store.put kind:"init-body"` of the candidate binary; its sha256.
    fn store_body(&self, path: &Path) -> Result<String, SliceRefusal>;
    /// `tuple.get` → the `lkg` tuple id (the slice's `incumbent`).
    fn lkg_tuple(&self) -> Result<String, SliceRefusal>;
    /// `sealed.g4_slice`; the answer comes when the slice ends (≤ `wall_s` + about 60 s).
    fn g4_slice(&self, args: &SliceArgs, timeout: Duration) -> Result<SliceAnswer, SliceRefusal>;
}

fn refusal(e: LinkError) -> SliceRefusal {
    match e {
        LinkError::Root(b) => SliceRefusal::from_code(b.code.as_str(), &b.message),
        LinkError::Timeout => SliceRefusal::Unavailable("control link: no answer in time".into()),
        LinkError::Lost => SliceRefusal::Unavailable("control link lost".into()),
        LinkError::Other(m) => SliceRefusal::Unavailable(format!("control link: {m}")),
    }
}

/// The production slice client: the outbox's control link.
impl SealedLink for crate::outbox::Outbox {
    fn store_body(&self, path: &Path) -> Result<String, SliceRefusal> {
        use std::os::fd::AsFd;
        let link = self.root_link().ok_or_else(|| SliceRefusal::Unavailable("control link not attached".into()))?;
        let sha256 = crate::fsx::sha256_file(path).map_err(|e| SliceRefusal::Store(format!("{}: {e}", path.display())))?;
        let f = std::fs::File::open(path).map_err(|e| SliceRefusal::Store(format!("{}: {e}", path.display())))?;
        let bytes = f.metadata().map(|m| m.len()).unwrap_or(0);
        let put = KernelRequest::StorePut(StorePutArgs { kind: StoreKind::InitBody, name: None, sha256: sha256.clone(), bytes, job: None });
        link.call_with_fd(&put, f.as_fd(), crate::link::STORE_TIMEOUT).map_err(refusal)?;
        Ok(sha256)
    }

    fn lkg_tuple(&self) -> Result<String, SliceRefusal> {
        let link = self.root_link().ok_or_else(|| SliceRefusal::Unavailable("control link not attached".into()))?;
        let v = link.call(&KernelRequest::TupleGet, link.request_timeout.max(Duration::from_secs(5))).map_err(refusal)?;
        tuple_id(&v["lkg"]).ok_or_else(|| SliceRefusal::Other("tuple.get: no lkg id".into()))
    }

    fn g4_slice(&self, args: &SliceArgs, timeout: Duration) -> Result<SliceAnswer, SliceRefusal> {
        let link = self.root_link().ok_or_else(|| SliceRefusal::Unavailable("control link not attached".into()))?;
        let a = serde_json::to_value(args).map_err(|e| SliceRefusal::Other(e.to_string()))?;
        let v = link.call(&KernelRequest::SealedG4Slice(a), timeout).map_err(refusal)?;
        serde_json::from_value(v).map_err(|e| SliceRefusal::Other(format!("sealed.g4_slice answer: {e}")))
    }
}

/// A tuple reference in `tuple.get`: an id string or an object with `id`.
pub fn tuple_id(v: &Value) -> Option<String> {
    v.as_str().or_else(|| v["id"].as_str()).filter(|s| !s.is_empty()).map(str::to_string)
}

/// The driver's result.
pub enum V2Path {
    /// G4 v2 decided (any verdict, the record v3 keys in `v2`).
    Ran(Box<G4Outcome>),
    /// No S path for this candidate: run the v0.3 kernel path; the reason goes into the record.
    Fallback(String),
}

/// Tiers of the first G4 per candidate sha, so the escalated re-run goes one tier up (m7 §3.2.5).
/// Process-local: after a kernel restart the re-run's floor is `standard`.
static FIRST_TIER: Mutex<BTreeMap<String, Tier>> = Mutex::new(BTreeMap::new());

/// When this kernel last heard `root.sealed_none` (per home): for an hour every G4 takes the v0.3
/// path directly, so a home without S does not pay for a screen before each fallback.
static SEALED_NONE_AT: Mutex<BTreeMap<std::path::PathBuf, Instant>> = Mutex::new(BTreeMap::new());
const SEALED_NONE_TTL: Duration = Duration::from_secs(3600);

/// D-L2: the lean Root runs the candidate's **body binary only** (it ignores `candidate.ext` and has
/// no t0 blob), so an S slice measures nothing of an ext or T0 change. The S path is therefore
/// taken only when the candidate differs from the LKG in body paths alone (scopes t2 and dep, with
/// no `skills/`, `SELF.md`, `GENESIS.md` or `ext/` path anywhere in LKG..candidate, which also
/// covers a provisional chain). t1 and t0_escalated changes take the v0.3 kernel path
/// (orchestrator decision 2026-10-20, build/status/GATE-V2a.md).
pub fn s_path_paths_ok(paths: &[String]) -> Result<(), String> {
    match paths.iter().find(|p| kernel_gate::scope::is_t0(p) || p.starts_with("ext/")) {
        Some(p) => Err(format!("the change touches {p}; the lean S slice measures the body binary only (D-L2)")),
        None if paths.iter().all(|p| p == "self.json") => Err("no body change against LKG".into()),
        None => Ok(()),
    }
}

fn s_path_ok(env: &Env, lkg: &str, sha: &str) -> Result<(), String> {
    if lkg.is_empty() {
        return Err("no LKG sha".into());
    }
    let out = env.git().run(&["diff", "--name-only", "--no-renames", lkg, sha], None, Duration::from_secs(60)).map_err(|e| format!("git diff: {e}"))?;
    let paths: Vec<String> = out.text().lines().map(str::to_string).filter(|p| !p.is_empty()).collect();
    s_path_paths_ok(&paths)
}

/// The gate's own canaries, then the screen of m7 §3.2.2 on the pool cache (lane GATE-V2b:
/// [`super::screen::g4_screen`]): 12 seeded pool tasks, the candidate fresh, the incumbent from
/// `vault/gate/cache/` (with the 20% sentinel) or fresh. The paired statistics (D, δ, CT, perf) come
/// from [`v2::screen_stats`] with the `noise.json` history.
pub fn run_screen(env: &Env, gate_id: &str, ex: &ProbeRuns<'_>) -> (ScreenSummary, Vec<RunRecord>) {
    let (o, runs, _cache) = super::screen::g4_screen(env, gate_id, ex);
    let history = std::fs::read_to_string(env.home.join("vault/gate/noise.json")).ok().and_then(|t| kernel_gate::stats::NoiseHistory::parse(&t).ok());
    let now_unix = crate::clock::now_ms() / 1000;
    let noise = history.as_ref().map(|h| v2::ScreenNoise::from_history(h, &env.model, now_unix, env.gate.noise_history_days)).unwrap_or_default();
    let st = v2::screen_stats(&runs, &noise, env.gate.g4.ct_include_wall);
    let reject = o.reject.map(|r| match r {
        kernel_gate::screen::ScreenRejectCode::Canary => v2::ScreenReject::Canary,
        kernel_gate::screen::ScreenRejectCode::Tamper => v2::ScreenReject::Tamper,
        kernel_gate::screen::ScreenRejectCode::ScreenErrors => v2::ScreenReject::ScreenErrors,
        kernel_gate::screen::ScreenRejectCode::ScreenInferior => v2::ScreenReject::ScreenInferior,
    });
    let s = ScreenSummary {
        n: o.n,
        c_pass: o.c_pass,
        p_pass: o.p_pass,
        net: o.net,
        c_errors: o.c_errors,
        tamper: o.tamper,
        canary_trip: o.canary_trip,
        reject,
        d_permille: if st.n > 0 { st.d_permille } else { o.d_permille },
        delta_permille: st.delta_permille,
        ct: st.ct,
        perf_gain: st.perf_gain,
        c_cost_musd: st.c_cost_musd,
    };
    (s, runs)
}

/// What the driver needs besides the env.
pub struct V2Job<'a> {
    pub gate_id: &'a str,
    pub v: &'a kernel_gate::evo::Validated,
    pub c: ArmSpec,
    pub p: ArmSpec,
    pub body_tokens_c: u64,
    pub body_tokens_p: u64,
    pub lkg: &'a str,
    pub escalated: bool,
}

fn outcome(verdict: Verdict, runs: Vec<RunRecord>, repeats: u32, t0: Instant, note: String, rec: V2Record) -> G4Outcome {
    G4Outcome {
        verdict,
        eval: None,
        deltas: None,
        noise_store: BTreeMap::new(),
        runs,
        flaky: vec![],
        top_up: false,
        repeats,
        futility: None,
        wall_ms: t0.elapsed().as_millis() as u64,
        note,
        v2: Some(rec),
    }
}

/// G4 v2 for one candidate (m7 §3.2.1). `None` sealed link → [`V2Path::Fallback`].
pub fn g4_v2(env: &Env, j: &V2Job<'_>) -> V2Path {
    let Some(sealed) = env.sealed.as_ref() else {
        return V2Path::Fallback("no sealed link (test home)".into());
    };
    if SEALED_NONE_AT.lock().unwrap_or_else(|e| e.into_inner()).get(&env.home).is_some_and(|t| t.elapsed() < SEALED_NONE_TTL) {
        return V2Path::Fallback("root.sealed_none (cached for 1 h)".into());
    }
    if let Err(why) = s_path_ok(env, j.lkg, &j.c.sha) {
        return V2Path::Fallback(why);
    }
    let t0 = Instant::now();
    let mut rec = V2Record::default();
    // Pre-flight (m7 §3.2.6). The escalated re-run goes one tier up and never re-tests at a lower one.
    let at_least = j.escalated.then(|| FIRST_TIER.lock().unwrap_or_else(|e| e.into_inner()).get(&j.c.sha).copied().unwrap_or(Tier::Standard).up());
    let gate_canaries = env.gate.canaries as u64;
    let pre: Preflight = match v2::preflight(env.gate.g4_tier, at_least, env.gate.gate_musd, env.gate.g4_run_estimate_musd, gate_canaries) {
        Ok(p) => p,
        Err(r) => {
            let note = format!(
                "G4 did not start: no tier fits the gate budget ({} µUSD) at {} µUSD per run with a 1.2 margin (tier {}). The incumbent stays.",
                env.gate.gate_musd, env.gate.g4_run_estimate_musd, env.gate.g4_tier.as_str()
            );
            return V2Path::Ran(Box::new(outcome(Verdict::Inconclusive(r), vec![], 0, t0, note, rec)));
        }
    };
    rec.tier = Some(pre.tier);
    rec.preflight = Some(pre);
    if !j.escalated {
        FIRST_TIER.lock().unwrap_or_else(|e| e.into_inner()).insert(j.c.sha.clone(), pre.tier);
    }
    // The gate's own canaries and the screen (kernel side; it can reject, never accept).
    let ex = ProbeRuns { env, job: j.gate_id.into(), c: j.c.clone(), p: j.p.clone() };
    let (screen, runs) = run_screen(env, j.gate_id, &ex);
    let repeats = pre.tier.repeats();
    if let Some(verdict) = v2::screen_verdict(&screen) {
        rec.screen = Some(screen);
        return V2Path::Ran(Box::new(outcome(verdict, runs, repeats, t0, String::new(), rec)));
    }
    rec.screen = Some(screen.clone());
    let spent: u64 = runs.iter().map(|r| r.spent_musd).sum();
    let refused = |rec: V2Record, r: SliceRefusal, runs: Vec<RunRecord>| -> V2Path {
        let why = match &r {
            SliceRefusal::Unavailable(m) | SliceRefusal::Store(m) | SliceRefusal::Other(m) => m.clone(),
            SliceRefusal::Quota => "root.sealed_quota".into(),
            SliceRefusal::Budget => "root.budget".into(),
            SliceRefusal::SealedNone => "root.sealed_none".into(),
        };
        match r.verdict() {
            None => {
                SEALED_NONE_AT.lock().unwrap_or_else(|e| e.into_inner()).insert(env.home.clone(), Instant::now());
                V2Path::Fallback(why)
            }
            Some(v) => {
                let mut rec = rec;
                rec.sealed_refusal = Some(kernel_gate::record::cap(&why, 300));
                let note = format!("the Root did not run the sealed slice: {}", kernel_gate::record::cap(&why, 300));
                V2Path::Ran(Box::new(outcome(v, runs, repeats, t0, note, rec)))
            }
        }
    };
    let body = match sealed.store_body(&j.c.binary) {
        Ok(s) => s,
        Err(r) => return refused(rec, r, runs),
    };
    let incumbent = match sealed.lkg_tuple() {
        Ok(t) => t,
        Err(r) => return refused(rec, r, runs),
    };
    let wall_s = env.gate.wall_s.clamp(60, 14_400);
    let args = SliceArgs {
        gate_id: j.gate_id.to_string(),
        // `ext` and `t0` are ignored by the lean Root (D-L2); the S path is body-only (s_path_ok).
        candidate: SliceCandidate { body, ext: SliceExt::default(), t0: None },
        incumbent,
        tier: pre.tier.into(),
        budget_musd: env.gate.gate_musd.saturating_sub(spent),
        wall_s,
    };
    env.point("gate.g4.slice");
    let ans = match sealed.g4_slice(&args, Duration::from_secs(wall_s + 120)) {
        Ok(a) => a,
        Err(r) => return refused(rec, r, runs),
    };
    let x = V2Inputs {
        change_type: j.v.change_type,
        predicts_sealed_gain: kernel_gate::evo::predicts_sealed_gain(&j.v.trailers.predicts),
        body_tokens_c: j.body_tokens_c,
        body_tokens_p: j.body_tokens_p,
        rho_max: env.gate.g4.rho_max,
        budget: CostBudget { soft_day_musd: env.gate.soft_day_musd, evolution_share_permille: env.gate.evolution_share_permille, tasks_per_day: 10.0 },
    };
    let d = v2::accept_v2(&screen, &ans, &x);
    let note = match &d.verdict {
        Verdict::Error(_) | Verdict::Inconclusive(_) => format!("the Root's sealed slice ended `{}`", kernel_gate::record::cap(&ans.status, 80)),
        _ => String::new(),
    };
    rec.milestone_required = d.milestone_required;
    rec.accept_class = v2::accept_class(&d.verdict);
    rec.divergence = d.divergence;
    rec.sealed = Some(ans);
    V2Path::Ran(Box::new(outcome(d.verdict, runs, repeats, t0, note, rec)))
}

/// Whether the gate record of `job` (`vault/gate/jobs/<job>/result.json`, written before the LKG
/// commit) is a `lineage_gain` accept with `milestone_required` (the `promotion` record's flag).
pub fn milestone_required(home: &Path, job: &str) -> bool {
    let ok = !job.is_empty() && job.len() <= 64 && job.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) && !job.starts_with('.');
    ok && std::fs::read(home.join("vault/gate/jobs").join(job).join("result.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .is_some_and(|r| r["stages"]["G4"]["milestone_required"] == Value::Bool(true) && r["verdict"] == "accept")
}

/// `state/kernel/gate/g4_last.json`: the last G4's agent-visible aggregate for `initctl status`
/// (`evolution.g4_last`; m7 §3.2.11: no ids, no per-task values).
pub fn write_last(env: &Env, gate_id: &str, o: &G4Outcome) {
    let view = match &o.v2 {
        Some(r) => r.status_view(gate_id, &o.verdict),
        None => serde_json::json!({"gate_id": gate_id, "verdict": o.verdict.kind(), "reason": o.verdict.reason(),
                                   "accept_class": v2::accept_class(&o.verdict), "sealed": Value::Null}),
    };
    let dir = env.home.join("state/kernel/gate");
    let _ = std::fs::create_dir_all(&dir);
    let _ = crate::fsx::write_json(&dir.join("g4_last.json"), &view);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::LocalSpawner;
    use crate::promote::{GateCfg, GateLineage, KernelInfo, MemJobs, NoGateProxy, SupState, Supervise};
    use kernel_gate::accept::{AcceptReason, ErrorReason, InconclusiveReason, RejectReason};
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::Arc;

    struct NoSup;
    impl Supervise for NoSup {
        fn handoff(&self, _: &str, _: &str, _: &str, _: Option<String>) -> Result<(), String> {
            Ok(())
        }
        fn g4_pass(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn g4_reject(&self, _: &str, _: &str, _: &str) {}
        fn rollback(&self, _: &str, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn snapshot(&self) -> Option<SupState> {
            None
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

    /// A scripted Root: answers in order; records every slice request.
    #[derive(Default)]
    struct FakeSealed {
        answers: Mutex<Vec<Result<SliceAnswer, SliceRefusal>>>,
        slices: Mutex<Vec<SliceArgs>>,
        stored: Mutex<Vec<String>>,
    }
    impl SealedLink for FakeSealed {
        fn store_body(&self, path: &Path) -> Result<String, SliceRefusal> {
            let s = crate::fsx::sha256_file(path).map_err(|e| SliceRefusal::Store(e.to_string()))?;
            self.stored.lock().unwrap().push(s.clone());
            Ok(s)
        }
        fn lkg_tuple(&self) -> Result<String, SliceRefusal> {
            Ok("t9a07c2e91d04".into())
        }
        fn g4_slice(&self, args: &SliceArgs, _: Duration) -> Result<SliceAnswer, SliceRefusal> {
            self.slices.lock().unwrap().push(args.clone());
            let mut a = self.answers.lock().unwrap();
            if a.is_empty() { Err(SliceRefusal::SealedNone) } else { a.remove(0) }
        }
    }

    fn answer(status: &str, d: i64, cpp: i64) -> SliceAnswer {
        serde_json::from_value(serde_json::json!({"seq": 77, "status": status, "ratchet_signal": false, "unconfirmed": 0, "ratchet_k": 4,
            "tier": "strong", "n": 32, "repeats": 5, "d_permille": d, "m_permille": 200, "cpp_ratio_permille": cpp,
            "looks_left": null, "s_version": "s1.0", "dup": false, "spent_musd": 500000})).unwrap()
    }

    /// `lineage/body.git` with two commits: LKG (`src/main.rs`) and a candidate that changes `files`.
    /// Plain git with a cleared environment and a scratch HOME (never the owner's git config).
    fn lineage(home: &Path, files: &[(&str, &str)]) -> (String, String) {
        let w = home.join("w");
        std::fs::create_dir_all(&w).unwrap();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("/usr/bin/git").args(["-c", "user.name=t", "-c", "user.email=t@init.local", "-c", "commit.gpgsign=false"]).args(args)
                .current_dir(&w).env_clear().env("PATH", "/usr/bin:/bin").env("HOME", &w).env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        git(&["init", "-q"]);
        std::fs::create_dir_all(w.join("src")).unwrap();
        std::fs::write(w.join("src/main.rs"), "fn main() {}\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "lkg"]);
        let lkg = git(&["rev-parse", "HEAD"]);
        for (p, text) in files {
            let f = w.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, text).unwrap();
        }
        git(&["add", "-A"]);
        git(&["commit", "-qm", "candidate"]);
        let cand = git(&["rev-parse", "HEAD"]);
        std::fs::create_dir_all(home.join("lineage")).unwrap();
        git(&["clone", "-q", "--bare", w.to_str().unwrap(), home.join("lineage/body.git").to_str().unwrap()]);
        (lkg, cand)
    }

    fn env(name: &str, sealed: Option<Arc<FakeSealed>>) -> Arc<Env> {
        let home = crate::testutil::test_dir(name);
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::write(home.join("bin/init-body"), b"candidate body").unwrap();
        Arc::new(Env {
            home,
            cfg: crate::config::defaults(),
            gate: GateCfg::default(),
            spawner: Arc::new(LocalSpawner::new()),
            g0: Arc::new(kernel_gate::g0::DirectSpawner),
            proxy: Arc::new(NoGateProxy),
            lineage: Arc::new(NoLineage),
            sup: Arc::new(NoSup),
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
            sealed: sealed.map(|s| s as Arc<dyn SealedLink>),
            extend: None,
        })
    }

    fn validated(sha: &str, predicts: &str) -> kernel_gate::evo::Validated {
        kernel_gate::evo::Validated {
            sha: sha.into(),
            parent: String::new(),
            change_type: kernel_gate::evo::ChangeType::Feat,
            scope: None,
            summary: String::new(),
            trailers: kernel_gate::evo::Trailers { predicts: kernel_gate::evo::predicts::parse_predicts(predicts).clauses, ..Default::default() },
            changed_paths: vec![],
            capability_labels: vec![],
        }
    }

    fn run(e: &Env, lkg: &str, cand: &str, v: &kernel_gate::evo::Validated, escalated: bool) -> V2Path {
        let j = V2Job {
            gate_id: "j-900051",
            v,
            c: ArmSpec { sha: cand.into(), binary: e.home.join("bin/init-body") },
            p: ArmSpec { sha: lkg.into(), binary: e.home.join("bin/init-body.lkg") },
            body_tokens_c: 100,
            body_tokens_p: 100,
            lkg,
            escalated,
        };
        g4_v2(e, &j)
    }

    fn ran(p: V2Path) -> G4Outcome {
        match p {
            V2Path::Ran(o) => *o,
            V2Path::Fallback(why) => panic!("fell back: {why}"),
        }
    }

    /// M7-G02 at the driver: a body-only change → pre-flight (strong fits $3 at $0.005 per run),
    /// store.put, tuple.get, the slice with the pinned args, `accept: better`, the record v3 keys.
    #[test]
    fn s_path_better_with_pinned_args() {
        let fake = Arc::new(FakeSealed::default());
        fake.answers.lock().unwrap().push(Ok(answer("superior", 200, 950)));
        let e = env("v2-better", Some(fake.clone()));
        let (lkg, cand) = lineage(&e.home, &[("src/loop.rs", "// verify on finish\n")]);
        let o = ran(run(&e, &lkg, &cand, &validated(&cand, "pass_rate@sealed >= parent+15%"), false));
        assert_eq!(o.verdict, Verdict::Accept(AcceptReason::Better));
        let s = fake.slices.lock().unwrap()[0].clone();
        assert_eq!((s.tier.name.as_str(), s.tier.n, s.tier.repeats), ("strong", 32, 5));
        assert_eq!((s.incumbent.as_str(), s.candidate.t0.clone(), s.budget_musd, s.wall_s), ("t9a07c2e91d04", None, 3_000_000, 5400));
        assert_eq!(fake.stored.lock().unwrap()[0], s.candidate.body);
        let rec = kernel_gate::g4::stage_record(&o, false);
        assert_eq!((rec["tier"].as_str(), rec["accept_class"].as_str(), rec["sealed"]["seq"].as_u64()), (Some("strong"), Some("better"), Some(77)));
        assert_eq!(rec["sealed"]["d_permille"], 200);
        assert_eq!(rec["spent_musd"], 500_000);
        write_last(&e, "j-900051", &o);
        let last: Value = serde_json::from_slice(&std::fs::read(e.home.join("state/kernel/gate/g4_last.json")).unwrap()).unwrap();
        assert_eq!((last["accept_class"].as_str(), last["sealed"]["status"].as_str()), (Some("better"), Some("superior")));
        crate::testutil::remove_test_dir(&e.home);
    }

    /// D-L2 (orchestrator decision): ext or T0 paths anywhere in LKG..candidate → the v0.3 path, no slice.
    #[test]
    fn d_l2_ext_and_t0_changes_fall_back() {
        let fake = Arc::new(FakeSealed::default());
        let e = env("v2-dl2", Some(fake.clone()));
        let (lkg, cand) = lineage(&e.home, &[("src/a.rs", "x"), ("skills/s/SKILL.md", "y")]);
        assert!(matches!(run(&e, &lkg, &cand, &validated(&cand, ""), false), V2Path::Fallback(w) if w.contains("skills/s/SKILL.md")));
        assert!(fake.slices.lock().unwrap().is_empty());
        let none = env("v2-none", None);
        assert!(matches!(run(&none, &lkg, &cand, &validated(&cand, ""), false), V2Path::Fallback(_)));
        crate::testutil::remove_test_dir(&e.home);
        crate::testutil::remove_test_dir(&none.home);
    }

    /// Refusals and tripwires at the driver; `root.sealed_none` falls back and is remembered per home.
    #[test]
    fn refusals_tripwires_and_sealed_none() {
        let fake = Arc::new(FakeSealed::default());
        *fake.answers.lock().unwrap() = vec![
            Err(SliceRefusal::Unavailable("root.busy: a slice is running".into())),
            Err(SliceRefusal::Budget),
            Ok(answer("health", 0, 0)),
            Ok(answer("error:harness_health", 0, 0)),
            Ok(answer("tamper", 0, 0)),
            Err(SliceRefusal::SealedNone),
        ];
        let e = env("v2-refusals", Some(fake.clone()));
        let (lkg, cand) = lineage(&e.home, &[("src/a.rs", "x")]);
        let v = validated(&cand, "");
        let o = ran(run(&e, &lkg, &cand, &v, false));
        assert_eq!(o.verdict, Verdict::Inconclusive(InconclusiveReason::SealedUnavailable));
        assert!(kernel_gate::g4::stage_record(&o, false)["sealed_refusal"].as_str().unwrap().contains("root.busy"));
        assert_eq!(ran(run(&e, &lkg, &cand, &v, false)).verdict, Verdict::Inconclusive(InconclusiveReason::BudgetInsufficient));
        assert_eq!(ran(run(&e, &lkg, &cand, &v, false)).verdict, Verdict::Reject(RejectReason::Health));
        let o = ran(run(&e, &lkg, &cand, &v, false));
        assert_eq!(o.verdict, Verdict::Error(ErrorReason::HarnessHealth));
        assert!(o.note.contains("error:harness_health"), "{}", o.note);
        assert_eq!(ran(run(&e, &lkg, &cand, &v, false)).verdict, Verdict::Reject(RejectReason::Tamper));
        assert!(matches!(run(&e, &lkg, &cand, &v, false), V2Path::Fallback(w) if w.contains("sealed_none")));
        let n = fake.slices.lock().unwrap().len();
        assert!(matches!(run(&e, &lkg, &cand, &v, false), V2Path::Fallback(w) if w.contains("cached")));
        assert_eq!(fake.slices.lock().unwrap().len(), n, "no slice request while sealed_none is remembered");
        crate::testutil::remove_test_dir(&e.home);
    }

    /// Pre-flight: nothing fits → `inconclusive: budget_insufficient` before any run; the escalated
    /// re-run goes one tier up from the first.
    #[test]
    fn preflight_and_escalation() {
        let fake = Arc::new(FakeSealed::default());
        *fake.answers.lock().unwrap() = vec![Ok(answer("underpowered", 0, 900)), Ok(answer("tie", 0, 900))];
        let mut e = env("v2-tiers", Some(fake.clone()));
        Arc::get_mut(&mut e).unwrap().gate.gate_musd = 1_200_000; // lean fits at $0.005 per run (762k with the margin), standard does not (1.34M)
        let (lkg, cand) = lineage(&e.home, &[("src/a.rs", "x")]);
        let v = validated(&cand, "");
        let o = ran(run(&e, &lkg, &cand, &v, false));
        assert_eq!(o.verdict, Verdict::Reject(RejectReason::NoGain), "an underpowered tie with no tie-break win");
        let o = ran(run(&e, &lkg, &cand, &v, true));
        assert_eq!(o.verdict, Verdict::Inconclusive(InconclusiveReason::BudgetInsufficient), "the re-run needs standard, which does not fit");
        let tiers: Vec<String> = fake.slices.lock().unwrap().iter().map(|s| s.tier.name.clone()).collect();
        assert_eq!(tiers, ["lean"]);
        Arc::get_mut(&mut e).unwrap().gate.gate_musd = 3_000_000;
        assert_eq!(ran(run(&e, &lkg, &cand, &v, true)).verdict, Verdict::Reject(RejectReason::NoGain));
        assert_eq!(fake.slices.lock().unwrap().last().unwrap().tier.name, "strong");
        Arc::get_mut(&mut e).unwrap().gate.gate_musd = 100_000;
        let o = ran(run(&e, &lkg, &cand, &v, false));
        assert_eq!(o.verdict, Verdict::Inconclusive(InconclusiveReason::BudgetInsufficient));
        assert!(o.runs.is_empty() && o.note.contains("no tier fits"));
        crate::testutil::remove_test_dir(&e.home);
    }

    /// M7-G15 at the driver: a ratchet accept carries `milestone_required` into the record, and the
    /// supervisor's `promotion` reads it back from the job's gate record.
    #[test]
    fn lineage_gain_sets_milestone_required() {
        let fake = Arc::new(FakeSealed::default());
        let mut a = answer("tie", 50, 1000);
        a.ratchet_signal = Some(true);
        a.unconfirmed = Some(0);
        fake.answers.lock().unwrap().push(Ok(a));
        let e = env("v2-ratchet", Some(fake.clone()));
        let (lkg, cand) = lineage(&e.home, &[("src/a.rs", "x")]);
        let o = ran(run(&e, &lkg, &cand, &validated(&cand, "pass_rate@sealed >= parent+15%"), false));
        assert_eq!(o.verdict, Verdict::Accept(AcceptReason::LineageGain));
        let st = kernel_gate::g4::stage_record(&o, false);
        assert_eq!((st["milestone_required"].as_bool(), st["accept_class"].as_str()), (Some(true), Some("lineage_gain")));
        let dir = e.home.join("vault/gate/jobs/j-900051");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("result.json"), serde_json::to_vec(&serde_json::json!({"verdict": "accept", "stages": {"G4": st}})).unwrap()).unwrap();
        assert!(milestone_required(&e.home, "j-900051"));
        assert!(!milestone_required(&e.home, "j-900052"));
        assert!(!milestone_required(&e.home, "../x"));
        crate::testutil::remove_test_dir(&e.home);
    }

    #[test]
    fn d_l2_s_path_is_body_only() {
        let p = |v: &[&str]| s_path_paths_ok(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(p(&["src/loop/mod.rs", "self.json"]).is_ok());
        assert!(p(&["Cargo.lock", "vendor/x/lib.rs"]).is_ok());
        assert!(p(&["src/loop/mod.rs", "skills/a/SKILL.md"]).is_err());
        assert!(p(&["ext/echo/src/lib.rs"]).is_err());
        assert!(p(&["SELF.md"]).is_err());
        assert!(p(&["self.json"]).is_err());
        assert!(p(&[]).is_err());
    }

    #[test]
    fn tuple_ids() {
        assert_eq!(tuple_id(&serde_json::json!("t9a07c2e91d04")).as_deref(), Some("t9a07c2e91d04"));
        assert_eq!(tuple_id(&serde_json::json!({"id": "t000000000000"})).as_deref(), Some("t000000000000"));
        assert_eq!(tuple_id(&Value::Null), None);
    }
}
