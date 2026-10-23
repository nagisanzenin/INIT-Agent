//! The screen on probes (lane GATE-V2b; m7-design.md §3.2.2, §3.2.7; evolution-v2.md §3.5.2 E3,
//! §3.5.3): G4 v2's screen and the evolution screen's stage E3, both on [`ProbeRuns`] with the pool
//! cache (`vault/gate/cache/`). The decisions are `kernel_gate::screen`; this module supplies the
//! frozen tasks, the incumbent's cache key and the job's `runs.jsonl`. Both can reject, never accept.

use super::g4run::{self, ArmSpec, ProbeRuns};
use super::Env;
use kernel_gate::g4::{RunRecord, RunSet, TaskDef};
use kernel_gate::pool_cache::{self, ArmKey, CacheQuery, PoolCache};
use kernel_gate::screen::{self, CacheUse, CachedPairs, E3Verdict, ScreenOutcome};
use serde::Serialize;

/// The incumbent arm's cache identity: the binary's sha256, no ext set (gate runs use none), and the
/// lineage commit standing in for the T0 digest (a finer key: it only costs reuse).
pub fn arm_key(arm: &ArmSpec) -> ArmKey {
    ArmKey { body_sha256: crate::fsx::sha256_file(&arm.binary).unwrap_or_default(), ext_set: String::new(), t0_sha256: arm.sha.clone() }
}

/// The lookup context for an incumbent arm under this kernel's routing and harness.
pub fn cache_query(env: &Env, incumbent: &ArmSpec) -> CacheQuery {
    CacheQuery {
        arm: arm_key(incumbent),
        routed: env.model.clone(),
        routing: env.level_str(),
        runner: format!("{}+{}", pool_cache::HARNESS_V, env.kernel.version),
        effort: "default".into(),
        reserve_output_tokens: env.proxy.request_reservation_musd().unwrap_or(0),
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The frozen pool (`vault/gate/pool/`).
pub fn pool(env: &Env) -> Vec<TaskDef> {
    g4run::load_set(env, "vault/gate/pool", RunSet::Pool)
}

/// The gate's canaries for `gate_id` (`gate.g4.canaries`, seeded).
pub fn canaries(env: &Env, gate_id: &str) -> Vec<TaskDef> {
    let all = g4run::load_set(env, "vault/canaries", RunSet::Canary);
    let ids: Vec<String> = all.iter().map(|t| t.id.clone()).collect();
    let pick = kernel_gate::tasks::select_canaries(gate_id, &ids, env.gate.canaries);
    all.into_iter().filter(|t| pick.contains(&t.id)).collect()
}

fn append_runs(env: &Env, job: &str, runs: &[RunRecord]) {
    let path = env.home.join("vault/gate/jobs").join(job).join("runs.jsonl");
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    for r in runs {
        if let Ok(mut line) = serde_json::to_vec(r) {
            line.push(b'\n');
            let _ = crate::fsx::append_line(&path, &line);
        }
    }
}

fn alert(env: &Env, job: &str, codes: &[String]) {
    for c in codes {
        crate::klog::log("gate", &format!("{job}: alert {c}: the pool cache of the incumbent drifted (sentinel); it was invalidated and the incumbent ran fresh"));
    }
    let _ = env;
}

/// G4 v2's screen (m7 §3.2.2): the gate's canaries first, then 12 seeded pool tasks (`smoke`
/// excluded), the candidate fresh and the incumbent from the pool cache or fresh. Appends every run to
/// the job's `runs.jsonl`.
pub fn g4_screen(env: &Env, gate_id: &str, ex: &ProbeRuns<'_>) -> (ScreenOutcome, Vec<RunRecord>, CachedPairs) {
    let pool = pool(env);
    let pick = screen::select_screen(gate_id, &pool, screen::SCREEN_K);
    let tasks: Vec<TaskDef> = pool.into_iter().filter(|t| pick.contains(&t.id)).collect();
    let cache = PoolCache::open(&env.home);
    let q = cache_query(env, &ex.p);
    let cu = CacheUse { cache: &cache, query: &q, now_unix: now_unix(), max_age_s: pool_cache::DEFAULT_MAX_AGE_S, sentinel_permille: pool_cache::DEFAULT_SENTINEL_PERMILLE };
    let (o, runs, cp) = screen::run_g4_screen(gate_id, ex, &tasks, &canaries(env, gate_id), Some(&cu), env.gate.parallel.max(1));
    append_runs(env, gate_id, &runs);
    alert(env, gate_id, &cp.alerts);
    (o, runs, cp)
}

/// Pool calibration (REAL-M7, M7 line C): the incumbent (LKG) alone on the screen-eligible pool tasks
/// (`smoke` excluded; a seeded, label-stratified subset of `limit` when `limit` > 0), `repeats` fresh runs
/// each, into the pool cache and the job's `runs.jsonl`. The probe runner retains failing pool runs' session
/// logs, so the miner (EV-MINE) and E3 get their evidence and incumbent runs before any proposal exists.
pub fn pool_calibrate(env: &Env, job: &str, incumbent: &ArmSpec, repeats: u32, limit: usize) -> Vec<RunRecord> {
    let pool = pool(env);
    let k = if limit == 0 { pool.len() } else { limit };
    let pick = screen::select_screen(job, &pool, k);
    let tasks: Vec<TaskDef> = pool.into_iter().filter(|t| pick.contains(&t.id)).collect();
    let cache = PoolCache::open(&env.home);
    let q = cache_query(env, incumbent);
    let ex = ProbeRuns { env, job: job.to_string(), c: incumbent.clone(), p: incumbent.clone() };
    let cu = CacheUse { cache: &cache, query: &q, now_unix: now_unix(), max_age_s: pool_cache::DEFAULT_MAX_AGE_S, sentinel_permille: pool_cache::DEFAULT_SENTINEL_PERMILLE };
    let runs = screen::run_pool_calibration(job, &ex, &tasks, repeats, Some(&cu), env.gate.parallel.max(1));
    append_runs(env, job, &runs);
    runs
}

/// One E3 request (evolution-v2 §3.5.2): the candidate (already built, E0) against the incumbent on
/// the targeted pool tasks plus `controls_n` seeded controls the incumbent passed.
#[derive(Clone, Debug)]
pub struct E3Request {
    /// The screen's job id (`j-…`): the lane ids, the seeds and `vault/gate/jobs/<job>/`.
    pub job: String,
    pub candidate: ArmSpec,
    pub incumbent: ArmSpec,
    /// Pool task ids where the proposal's cluster was fatal (at most 4 are used).
    pub targeted: Vec<String>,
    pub controls_n: usize,
    /// `s_c` from the miner, permille (evolution-v2 §3.5.3).
    pub s_c_permille: u32,
}

/// E3's answer.
#[derive(Clone, Debug, Serialize)]
pub struct E3Result {
    pub verdict: E3Verdict,
    /// `est` (‰), computed only when E3 passed (else 0).
    pub est_permille: u32,
    /// `est ≥ 80‰` and E3 passed.
    pub go: bool,
    /// `better`, `ratchet` or `below_resolution` (`rejected` when E3 failed).
    pub aim: String,
    pub reused: usize,
    pub runs: Vec<RunRecord>,
}

/// Stage E3 on probes. The candidate's lanes are opened under `req.job` (the caller opens and closes
/// the gate budget around it, `budget.screen_usd`).
pub fn screen_e3(env: &Env, req: &E3Request) -> E3Result {
    let pool = pool(env);
    let targeted_ids: Vec<String> = req.targeted.iter().take(screen::E3_TARGETED_MAX).cloned().collect();
    let targeted: Vec<TaskDef> = targeted_ids.iter().filter_map(|id| pool.iter().find(|t| &t.id == id).cloned()).collect();
    let cache = PoolCache::open(&env.home);
    let q = cache_query(env, &req.incumbent);
    let now = now_unix();
    let passed = cache.passed_tasks(&q.arm, now, pool_cache::DEFAULT_MAX_AGE_S);
    let ctl = screen::select_controls(&req.job, &pool, &targeted_ids, &passed, req.controls_n);
    let controls: Vec<TaskDef> = ctl.iter().filter_map(|id| pool.iter().find(|t| &t.id == id).cloned()).collect();
    let ex = ProbeRuns { env, job: req.job.clone(), c: req.candidate.clone(), p: req.incumbent.clone() };
    let cu = CacheUse { cache: &cache, query: &q, now_unix: now, max_age_s: pool_cache::DEFAULT_MAX_AGE_S, sentinel_permille: pool_cache::DEFAULT_SENTINEL_PERMILLE };
    let (verdict, runs, cp) = screen::run_e3(&req.job, &ex, &targeted, &controls, &canaries(env, &req.job), Some(&cu), env.gate.parallel.max(1));
    append_runs(env, &req.job, &runs);
    alert(env, &req.job, &cp.alerts);
    let est = if verdict.pass {
        screen::est_permille(req.s_c_permille, verdict.targeted_fixed, verdict.targeted_total, screen::DEFAULT_Q_PERMILLE, screen::DEFAULT_TRANSFER_PERMILLE)
    } else {
        0
    };
    let go = verdict.pass && screen::go(est);
    let aim = if verdict.pass { screen::aim(est).to_string() } else { "rejected".to_string() };
    E3Result { verdict, est_permille: est, go, aim, reused: cp.reused, runs }
}
