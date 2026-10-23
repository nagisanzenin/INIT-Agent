//! The evolution engine's initctl commands (lane EVO-LITE; evolution-v2 §4.1, PROOF-LEAN D-L5):
//! kernel-serve's [`Backend::evo`](kernel_serve::Backend::evo) and `evo_screen` over kernel-evo,
//! plus the background miner.
//!
//! - `evo frictions`: mines (incremental, $0) and returns the digest or the full index.
//! - `evo propose`: mines, then validates and stores the draft (`state/kernel/evo/proposals/`).
//! - `evo variant`: a kernel-built commit on `refs/init/evo/<p>/v<k>` from the agent's patch.
//! - `evo screen` (job): per variant E0 (build + G0 in the build slot) then E3 (GATE-V2b's
//!   candidate-only mini-run on the pool cache, under the gate budget); then the go/no-go
//!   (est ≥ 80‰) and the chosen variant. Reject-only: the chosen variant still goes through the
//!   ordinary `initctl promote` (G0–G3, handoff, provisional, G4 v2).
//! - `evo status`, `evo abandon`.
//!
//! Every proposal transition and screen result is a kernel-local record in `vault/lineage.jsonl`
//! (kinds `proposal`, `screen`, `friction_index`; evolution-v2 §4.7).

use init_proto::envelope::{ErrorBody, ErrorDetail};
use init_proto::errors::ErrorCode;
use init_proto::initctl::{Command, EvoScreenArgs};
use init_proto::serde_json::{json, Value};
use kernel_evo::mine::{self, MineCfg};
use kernel_evo::proposal::{self, Problem, Proposal, Variant};
use kernel_evo::screen::{self as escreen, Outcome, Screener};
use kernel_gate::evo::trailers::GitTrailerParser;
use kernel_serve::{Begun, LaneCtx};
use kernel_supervise::promote::g4run::ArmSpec;
use kernel_supervise::promote::screen::{screen_e3, E3Request};
use kernel_supervise::promote::{stages, Env, GateLineage, Job};
use kernel_supervise::supervisor::Services;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// `budget.screen_usd` ($0.15 per proposal, evolution-v2 §3.9.1), in µUSD.
pub const SCREEN_BUDGET_MUSD: u64 = 150_000;
/// `evolution.engine.mine_interval_s`.
pub const MINE_EVERY: Duration = Duration::from_secs(3600);

fn problems(probs: Vec<Problem>) -> ErrorBody {
    let first = probs.first().map(|p| p.code.clone()).unwrap_or_else(|| "evo.io".into());
    let code = if first == "evo.io" { ErrorCode::Internal } else { ErrorCode::Other(first.clone()) };
    let mut e = ErrorBody::new(code, format!("{} problem(s); first: {first}", probs.len()));
    e.details = probs.into_iter().map(|p| ErrorDetail { code: p.code, detail: p.detail }).collect();
    e
}

fn io(e: std::io::Error) -> ErrorBody {
    ErrorBody::new(ErrorCode::Internal, format!("evo: {e}"))
}

fn parser(sv: &Services) -> GitTrailerParser {
    let empty = sv.home.join("cache/evo-empty");
    let _ = std::fs::create_dir_all(&empty);
    GitTrailerParser::new(&sv.cfg.supervise.git, empty)
}

/// The Root's `extend.mode` as the supervisor last saw it (ADR-011; `hello` and `ceilings`).
fn mode(sv: &Services) -> kernel_gate::evo::ExtendMode {
    if sv.extend_free.load(std::sync::atomic::Ordering::SeqCst) { kernel_gate::evo::ExtendMode::Free } else { kernel_gate::evo::ExtendMode::Gated }
}

/// Mines and records the index size (`friction_index`, counts only).
pub fn mine_now(home: &Path, lineage: &dyn GateLineage) -> Result<mine::MineOut, ErrorBody> {
    let out = mine::mine(home, &MineCfg::for_home(home)).map_err(io)?;
    if out.parsed_sources > 0 {
        let eligible = out.clusters.iter().filter(|c| c.eligible).count();
        lineage.local("friction_index", json!({"clusters": out.clusters.len(), "eligible": eligible, "instances": out.instances, "failing_runs": out.failing_runs, "ms": out.ms}));
    }
    Ok(out)
}

/// The immediate commands.
pub fn command(sv: &Services, lane: &LaneCtx, cmd: &Command, text: Option<&str>) -> Result<Value, ErrorBody> {
    let home = &sv.home;
    let lineage: &dyn GateLineage = &*sv.outbox;
    match cmd {
        Command::EvoFrictions(a) => {
            let out = mine_now(home, lineage)?;
            if a.all == Some(true) {
                return Ok(json!({"frictions": out.clusters}));
            }
            let digest = mine::digest(&out.clusters);
            Ok(json!({"digest": digest.lines().collect::<Vec<_>>(), "clusters": out.clusters.len(), "eligible": out.clusters.iter().filter(|c| c.eligible).count()}))
        }
        Command::EvoPropose(_) => {
            mine_now(home, lineage)?;
            let filer = proposal::Filer { lane: lane.key(), session: None, model: Some(format!("{}/{}", sv.cfg.model.provider, sv.cfg.model.id)) };
            let (p, rec) = proposal::propose_in(home, text.unwrap_or("").as_bytes(), &filer, &parser(sv), mode(sv)).map_err(problems)?;
            lineage.local("proposal", rec);
            Ok(json!({"id": p.id, "state": p.state, "s_measurable": p.s_measurable, "targets": p.targets, "k": p.k}))
        }
        Command::EvoVariant(a) => {
            let git = kernel_evo::variant::Git::new(&sv.cfg.supervise.git, home);
            let b = kernel_evo::variant::build_in(home, &git, &a.proposal, a.k, text.unwrap_or(""), &parser(sv), mode(sv)).map_err(problems)?;
            lineage.local("proposal", b.record);
            Ok(json!({"ref": b.variant.git_ref, "sha": b.variant.sha, "diff_stat": b.variant.diff_stat, "s_measurable": b.variant.s_measurable}))
        }
        Command::EvoStatus(a) => proposal::status(home, a.proposal.as_deref()).map_err(|p| problems(vec![p])),
        Command::EvoAbandon(a) => {
            let (p, rec) = proposal::abandon(home, &a.proposal, &a.reason).map_err(|p| problems(vec![p]))?;
            lineage.local("proposal", rec);
            Ok(json!({"id": p.id, "state": p.state}))
        }
        other => Err(kernel_serve::not_implemented(other.name())),
    }
}

/// E0 (build + G0 of the variant) then E3 (GATE-V2b) on the kernel's probes.
struct KernelScreener<'a> {
    env: &'a Env,
    job: String,
    main: String,
    lkg: String,
}

impl Screener for KernelScreener<'_> {
    fn screen(&self, p: &Proposal, v: &Variant) -> Result<Outcome, String> {
        let env = self.env;
        let vjob = format!("{}-v{}", self.job, v.k);
        let cache = stages::job_cache(env, &vjob);
        let built = {
            let _slot = env.slot_lock.lock().unwrap_or_else(|e| e.into_inner());
            stages::build_and_g0(env, &vjob, &v.sha, &self.main, &cache, false)
        };
        let built = match built {
            Ok(b) => b,
            Err(f) => {
                let _ = std::fs::remove_dir_all(&cache);
                return Ok(Outcome { pass: false, reason: Some(format!("screen.e0:{}", f.code)), targeted_fixed: 0, targeted_total: 0, controls_lost: 0, cost_musd: 0, stages: vec![json!({"stage": "E0", "pass": false, "code": f.code})] });
            }
        };
        let s_c = escreen::s_c_permille(&env.home, p);
        let _ = env.proxy.open_gate(&vjob, "screen", SCREEN_BUDGET_MUSD);
        let r = screen_e3(
            env,
            &E3Request {
                job: vjob.clone(),
                candidate: ArmSpec { sha: v.sha.clone(), binary: built.binary.clone() },
                incumbent: ArmSpec { sha: self.lkg.clone(), binary: env.home.join("bin/init-body.lkg") },
                targeted: p.targets.clone(),
                controls_n: 2,
                s_c_permille: s_c,
            },
        );
        let spent = env.proxy.close_gate(&vjob);
        let _ = std::fs::remove_dir_all(&cache);
        let vd = &r.verdict;
        Ok(Outcome {
            pass: vd.pass,
            reason: vd.reason.clone(),
            targeted_fixed: vd.targeted_fixed,
            targeted_total: vd.targeted_total,
            controls_lost: vd.controls_lost,
            cost_musd: spent,
            stages: vec![json!({"stage": "E0", "pass": true}), json!({"stage": "E3", "verdict": vd, "est_permille": r.est_permille, "reused": r.reused, "runs": r.runs.len()})],
        })
    }
}

/// `evo screen` as a job thread.
pub fn screen(env: Arc<Env>, args: &EvoScreenArgs, job: Arc<dyn Job>) -> Result<Begun, ErrorBody> {
    if let Some(repeats) = args.pool_repeats {
        return calibrate(env, repeats, args.pool_limit.unwrap_or(0), job);
    }
    let p = proposal::load(&env.home, &args.proposal).map_err(io)?.ok_or_else(|| problems(vec![Problem::new("evo.state", format!("{}: no such proposal", args.proposal))]))?;
    if p.state != "variants" {
        return Err(problems(vec![Problem::new("evo.state", format!("{} is {}; screen it after `evo variant`", p.id, p.state))]));
    }
    let git = env.git();
    let main = git.rev_parse_commit("refs/heads/main").ok_or_else(|| ErrorBody::new(ErrorCode::Internal, "lineage has no refs/heads/main"))?;
    let lkg = git.rev_parse_commit("refs/heads/lkg").unwrap_or_else(|| main.clone());
    let id = p.id.clone();
    std::thread::Builder::new()
        .name(format!("evo-screen-{id}"))
        .spawn(move || {
            job.progress(init_proto::initctl::JobState::Running, None, 5, &format!("screening {id}"));
            let sc = KernelScreener { env: &env, job: job.id(), main, lkg };
            match escreen::run(&env.home, &id, &sc, SCREEN_BUDGET_MUSD) {
                Ok(s) => {
                    for (k, r) in s.records {
                        env.lineage.local(&k, r);
                    }
                    let p = s.proposal;
                    job.succeed(json!({"id": p.id, "state": p.state, "est": p.est, "go": p.go, "chosen": p.chosen,
                        "variants": p.variants.iter().map(|v| json!({"k": v.k, "sha": v.sha, "s_measurable": v.s_measurable, "screen": v.screen})).collect::<Vec<_>>()}));
                }
                Err(e) => job.fail(problems(vec![e])),
            }
        })
        .map_err(|e| ErrorBody::new(ErrorCode::Internal, format!("job thread: {e}")))?;
    Ok(Begun::started().with("proposal", args.proposal.clone()))
}

/// `evo calibrate` as a job thread (REAL-M7, M7 line C): the LKG body on the visible pool, `repeats` runs per
/// task, under `budget.gate_usd`; it seeds the pool cache (E3's incumbent arm) and the miner's failing runs.
fn calibrate(env: Arc<Env>, repeats: u32, limit: u32, job: Arc<dyn Job>) -> Result<Begun, ErrorBody> {
    let git = env.git();
    let lkg = git
        .rev_parse_commit("refs/heads/lkg")
        .or_else(|| git.rev_parse_commit("refs/heads/main"))
        .ok_or_else(|| ErrorBody::new(ErrorCode::Internal, "lineage has no refs/heads/main"))?;
    std::thread::Builder::new()
        .name("evo-calibrate".into())
        .spawn(move || {
            let jid = job.id();
            job.progress(init_proto::initctl::JobState::Running, None, 5, &format!("pool calibration, R = {repeats}"));
            let _ = env.proxy.open_gate(&jid, "screen", env.gate.gate_musd);
            let inc = ArmSpec { sha: lkg.clone(), binary: env.home.join("bin/init-body.lkg") };
            let runs = kernel_supervise::promote::screen::pool_calibrate(&env, &jid, &inc, repeats, limit as usize);
            let spent = env.proxy.close_gate(&jid);
            let failed = runs.iter().filter(|r| r.score_permille < 1000).count();
            let requests: u64 = runs.iter().map(|r| r.requests).sum();
            let tasks: std::collections::BTreeSet<&str> = runs.iter().map(|r| r.task.as_str()).collect();
            if let Err(e) = mine_now(&env.home, env.lineage.as_ref()) {
                eprintln!("{}", json!({"c": "evo", "msg": format!("mine after calibrate: {}", e.message)}));
            }
            job.succeed(json!({"job": jid, "lkg": lkg, "tasks": tasks.len(), "runs": runs.len(), "failed": failed,
                "requests": requests, "spent_musd": spent}));
        })
        .map_err(|e| ErrorBody::new(ErrorCode::Internal, format!("job thread: {e}")))?;
    Ok(Begun::started().with("calibrate", json!(repeats)))
}

/// After a promote started on `sha`: the screened proposal whose chosen variant it is becomes
/// `promoted` (the pipeline itself is unchanged).
pub fn promoted(home: &Path, lineage: &dyn GateLineage, job: &str, sha: &str) {
    for p in proposal::all(home).into_iter().filter(|p| p.state == "screened") {
        if let Some(rec) = escreen::mark_promoted(home, &p.id, job, sha) {
            lineage.local("proposal", rec);
        }
    }
}

/// ADR-011 X0b: the promote job, wrapped so that its terminal failure returns the proposal whose
/// chosen variant it gated to `screened` (a G0–G3 failure, a refused `change.open`, a failed
/// handoff: no verdict was reached), so the same chosen sha can be gated again (STRANDED-PROPOSAL).
pub struct EvoJob {
    pub inner: Arc<dyn Job>,
    pub home: std::path::PathBuf,
    pub lineage: Arc<dyn GateLineage>,
}

impl Job for EvoJob {
    fn id(&self) -> String {
        self.inner.id()
    }
    fn cancelled(&self) -> bool {
        self.inner.cancelled()
    }
    fn progress(&self, state: init_proto::initctl::JobState, stage: Option<init_proto::initctl::Stage>, pct: u8, msg: &str) {
        self.inner.progress(state, stage, pct, msg)
    }
    fn stage_done(&self, r: init_proto::initctl::StageResult) {
        self.inner.stage_done(r)
    }
    fn awaiting_boundary(&self) -> bool {
        self.inner.awaiting_boundary()
    }
    fn succeed(&self, result: Value) {
        self.inner.succeed(result)
    }
    fn fail(&self, err: ErrorBody) {
        let why = err.data.as_ref().and_then(|d| d["reason"].as_str()).unwrap_or(err.code.as_str()).to_string();
        let ev = proposal::ChangeEv::NoVerdict { reason: format!("promote_failed:{why}") };
        for rec in proposal::on_job(&self.home, &self.inner.id(), &ev) {
            self.lineage.local("proposal", rec);
        }
        self.inner.fail(err)
    }
}

/// The background miner: once a minute after start, then every [`MINE_EVERY`].
pub fn spawn_miner(home: std::path::PathBuf, lineage: Arc<dyn GateLineage>) {
    let _ = std::thread::Builder::new().name("evo-mine".into()).spawn(move || {
        std::thread::sleep(Duration::from_secs(60));
        loop {
            if let Err(e) = mine_now(&home, lineage.as_ref()) {
                eprintln!("{}", json!({"c": "evo", "msg": format!("mine: {}", e.message)}));
            }
            std::thread::sleep(MINE_EVERY);
        }
    });
}
