//! The screen: a gate kind that can reject and never accept (m7-design.md §3.2.2; evolution-v2.md
//! §3.5.2–§3.5.3; lean scope build/specs/PROOF-LEAN.md D-L5).
//!
//! Two uses share one runner ([`run_cached_pairs`]): the candidate always runs fresh, R = 1; the
//! incumbent comes from the pool cache when valid (crate::pool_cache), with a ⌈20% · n⌉ sentinel,
//! else it runs fresh, back to back with the candidate. Fresh and sentinel incumbent runs enter the
//! cache.
//! - **G4 v2's screen** ([`select_screen`], [`screen_decide`]): 12 pool tasks, seeded by
//!   `sha256(gate_id + "/screen")`, stratified by label, `smoke` excluded; rejects in order `canary`,
//!   `tamper`, `screen_errors` (≥ 3 candidate run errors), `screen_inferior` (net ≤ −4). Otherwise G4
//!   continues to the slice.
//! - **Stage E3 of the evolution screen** ([`e3_decide`], [`est_permille`], [`go`]): the candidate on
//!   ≤ 4 targeted pool tasks plus 2 seeded controls the incumbent passed; rejects `screen.tamper`,
//!   `screen.mini_inferior`, `screen.costlier`. A pass only lets the go/no-go estimate run: go iff
//!   `est ≥ 80‰` (the ratchet threshold; `aim` names the path).
//!
//! The run grid is asserted before any statistic ([`assert_grid`], `screen.grid`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

use crate::caps::RunCaps;
use crate::g4::{RunExecutor, RunRecord, RunSet, TaskDef, run_error, run_id};
use crate::pool_cache::{self, CacheQuery, PoolCache};
use crate::stats::data::Arm;
use crate::stats::rng::SplitMix64;

/// `gate.g4.screen_k`.
pub const SCREEN_K: usize = 12;
/// `gate.g4.screen_reject_errors`: this many candidate run errors reject.
pub const SCREEN_REJECT_ERRORS: u32 = 3;
/// `gate.g4.screen_reject_net`: net ≤ −this rejects.
pub const SCREEN_REJECT_NET: i64 = 4;
/// E3: at most this many targeted tasks.
pub const E3_TARGETED_MAX: usize = 4;
/// E3: seeded controls the incumbent passed.
pub const E3_CONTROLS: usize = 2;
/// `go_gain_permille`: aim for `better`.
pub const GO_GAIN_PERMILLE: u32 = 150;
/// `ratchet_gain_permille`: the go/no-go floor (lean: go iff est ≥ 80‰).
pub const RATCHET_GAIN_PERMILLE: u32 = 80;
/// `q = 1 − p̄_S` until the Root publishes p̄_S (S v1 targets 20–80% incumbent pass rates).
pub const DEFAULT_Q_PERMILLE: u32 = 500;
/// `transfer_permille` (pool → S discount) until refitted.
pub const DEFAULT_TRANSFER_PERMILLE: u32 = 800;

fn passed(r: &RunRecord) -> bool {
    r.score_permille >= 1000
}

/// A candidate run that is an error: no model request (K-GATE, c712701), or `infra` after its re-run.
pub fn c_error(r: &RunRecord) -> bool {
    run_error(r).is_some() || r.outcome == "infra"
}

/// An incumbent run that measured nothing: the task is left out of the comparison.
pub fn p_error(r: &RunRecord) -> bool {
    run_error(r).is_some() || r.outcome == "infra"
}

// ---------------------------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------------------------

/// The screen's pool sample: up to `k` frozen pool tasks, `smoke` excluded, filled round-robin over
/// labels (sorted) with a seeded shuffle inside each label, seed `sha256(gate_id + "/screen")`.
/// Agent-authored tasks (L1) are left out at L0 (lean: no agent stratum).
pub fn select_screen(gate_id: &str, pool: &[TaskDef], k: usize) -> Vec<String> {
    let mut rng = SplitMix64::for_gate(gate_id, "screen");
    let mut by_label: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in pool.iter().filter(|t| t.set == RunSet::Pool && !t.labels.iter().any(|l| l == "smoke")) {
        if t.labels.is_empty() {
            by_label.entry(String::new()).or_default().push(t.id.clone());
        }
        for l in &t.labels {
            by_label.entry(l.clone()).or_default().push(t.id.clone());
        }
    }
    let mut queues: Vec<Vec<String>> = by_label
        .into_values()
        .map(|mut v| {
            v.sort();
            v.dedup();
            rng.shuffle(&mut v);
            v.reverse();
            v
        })
        .collect();
    let mut chosen = Vec::new();
    let mut seen = BTreeSet::new();
    while chosen.len() < k && queues.iter().any(|q| !q.is_empty()) {
        for q in queues.iter_mut() {
            if chosen.len() >= k {
                break;
            }
            while let Some(id) = q.pop() {
                if seen.insert(id.clone()) {
                    chosen.push(id);
                    break;
                }
            }
        }
    }
    chosen
}

/// E3's controls: up to `n` seeded pool tasks (not targeted, not `smoke`) that the incumbent passed in
/// a valid cached run; when fewer are known, the rest are filled from other pool tasks, seeded.
pub fn select_controls(gate_id: &str, pool: &[TaskDef], targeted: &[String], incumbent_passed: &BTreeSet<String>, n: usize) -> Vec<String> {
    let mut rng = SplitMix64::for_gate(gate_id, "controls");
    let eligible: Vec<&TaskDef> = pool.iter().filter(|t| t.set == RunSet::Pool && !targeted.contains(&t.id) && !t.labels.iter().any(|l| l == "smoke")).collect();
    let mut known: Vec<String> = eligible.iter().filter(|t| incumbent_passed.contains(&t.id)).map(|t| t.id.clone()).collect();
    let mut other: Vec<String> = eligible.iter().filter(|t| !incumbent_passed.contains(&t.id)).map(|t| t.id.clone()).collect();
    known.sort();
    other.sort();
    rng.shuffle(&mut known);
    rng.shuffle(&mut other);
    known.into_iter().chain(other).take(n).collect()
}

// ---------------------------------------------------------------------------------------------
// The runner
// ---------------------------------------------------------------------------------------------

/// Where the incumbent's runs may come from.
pub struct CacheUse<'a> {
    pub cache: &'a PoolCache,
    pub query: &'a CacheQuery,
    pub now_unix: i64,
    pub max_age_s: u64,
    pub sentinel_permille: u64,
}

/// What the sentinel saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sentinel {
    pub tasks: usize,
    pub passes: u64,
    /// Σ p̂ of the cached runs on the same tasks, permille.
    pub expected_permille: u64,
    pub drift: bool,
}

/// The paired runs of one screen, in task order.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedPairs {
    /// `(candidate, incumbent)` per task.
    pub pairs: Vec<(RunRecord, RunRecord)>,
    /// Incumbent runs taken from the cache (after the sentinel).
    pub reused: usize,
    pub sentinel: Option<Sentinel>,
    /// Alert codes raised (`incumbent_drift`).
    pub alerts: Vec<String>,
}

fn with_cache(mut r: RunRecord, how: &str) -> RunRecord {
    r.cache = Some(how.into());
    r
}

/// One run with the infra re-run (gate.md §4.4: re-run once).
fn one(ex: &dyn RunExecutor, arm: Arm, t: &TaskDef) -> RunRecord {
    let id = run_id(arm, &t.id, 1);
    let r = ex.run(arm, t, 1, &id);
    if r.outcome == "infra" { ex.run(arm, t, 1, &id) } else { r }
}

fn key_for(cu: &CacheUse<'_>, t: &TaskDef, served: &str) -> Option<String> {
    let digest = pool_cache::task_digest(&t.dir).ok()?;
    Some(pool_cache::entry_key(&digest, &cu.query.arm, &cu.query.model(served), &cu.query.harness(RunCaps::for_task(&t.task.limits))))
}

fn put(cu: &CacheUse<'_>, t: &TaskDef, r: &RunRecord) {
    let Some(served) = r.model_served.as_deref() else { return };
    if let Some(key) = key_for(cu, t, served) {
        let _ = cu.cache.put(&cu.query.arm, &key, &t.id, cu.now_unix, true, r);
    }
}

fn parallel_map<T: Send>(n: usize, parallel: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<T>>> = Mutex::new((0..n).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..parallel.clamp(1, n.max(1)) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= n {
                        break;
                    }
                    let v = f(i);
                    out.lock().unwrap()[i] = Some(v);
                }
            });
        }
    });
    out.into_inner().unwrap().into_iter().map(|v| v.expect("every index ran")).collect()
}

/// Run the candidate fresh (R = 1) on every task and pair it with an incumbent run: from the cache
/// when a valid entry exists under the key with the served model the candidate's run observed, else
/// fresh, back to back. Then the sentinel re-runs ⌈`sentinel_permille` · n⌉ seeded reused tasks
/// fresh; on drift the tuple's cache is invalidated (alert `incumbent_drift`) and every other reused
/// task runs fresh. Fresh and sentinel incumbent runs enter the cache. `gate_id` seeds the subsets.
pub fn run_cached_pairs(gate_id: &str, ex: &dyn RunExecutor, tasks: &[TaskDef], cache: Option<&CacheUse<'_>>, parallel: usize) -> CachedPairs {
    // Phase 1: candidate, then the incumbent from the cache or fresh, back to back.
    let first: Vec<(RunRecord, Option<RunRecord>, Option<u64>)> = parallel_map(tasks.len(), parallel, |i| {
        let t = &tasks[i];
        let c = with_cache(one(ex, Arm::C, t), "fresh");
        if let Some(cu) = cache
            && let Some(served) = c.model_served.as_deref()
            && let Some(key) = key_for(cu, t, served)
            && let Some(hit) = cu.cache.lookup(&cu.query.arm, &key, cu.now_unix, cu.max_age_s, 1, (gate_id, &t.id))
        {
            let p_hat = pool_cache::p_hat_permille(&cu.cache.valid(&cu.query.arm, &key, cu.now_unix, cu.max_age_s));
            return (c, Some(with_cache(hit[0].run.clone(), "reused")), Some(p_hat));
        }
        let p = with_cache(one(ex, Arm::P, t), "fresh");
        if let Some(cu) = cache {
            put(cu, t, &p);
        }
        (c, Some(p), None)
    });
    let mut pairs: Vec<(RunRecord, RunRecord)> = Vec::with_capacity(tasks.len());
    let mut p_hat: Vec<Option<u64>> = Vec::with_capacity(tasks.len());
    for (c, p, ph) in first {
        pairs.push((c, p.expect("an incumbent run")));
        p_hat.push(ph);
    }
    let mut out = CachedPairs { pairs, reused: 0, sentinel: None, alerts: Vec::new() };
    let reused: Vec<usize> = (0..tasks.len()).filter(|i| p_hat[*i].is_some()).collect();
    let Some(cu) = cache else { return out };
    if reused.is_empty() {
        return out;
    }
    // Phase 2: the sentinel on ⌈permille · n⌉ seeded reused tasks.
    let mut order = reused.clone();
    SplitMix64::for_gate(gate_id, "sentinel").shuffle(&mut order);
    order.truncate(pool_cache::sentinel_count(tasks.len(), cu.sentinel_permille).min(reused.len()));
    order.sort_unstable();
    let fresh: Vec<RunRecord> = parallel_map(order.len(), parallel, |j| with_cache(one(ex, Arm::P, &tasks[order[j]]), "sentinel"));
    let usable: Vec<(usize, RunRecord)> = order.iter().copied().zip(fresh).collect();
    let measured: Vec<&(usize, RunRecord)> = usable.iter().filter(|(_, r)| !p_error(r)).collect();
    let passes = measured.iter().filter(|(_, r)| passed(r)).count() as u64;
    let hats: Vec<u64> = measured.iter().map(|(i, _)| p_hat[*i].unwrap_or(0)).collect();
    let drift = !measured.is_empty() && pool_cache::sentinel_drift(passes, &hats);
    out.sentinel = Some(Sentinel { tasks: usable.len(), passes, expected_permille: hats.iter().sum(), drift });
    if drift {
        let _ = cu.cache.invalidate(&cu.query.arm);
        out.alerts.push("incumbent_drift".into());
    }
    for (i, r) in usable {
        put(cu, &tasks[i], &r);
        out.pairs[i].1 = r;
    }
    let rest: Vec<usize> = reused.iter().copied().filter(|i| !order.contains(i)).collect();
    if drift {
        let rerun: Vec<RunRecord> = parallel_map(rest.len(), parallel, |j| with_cache(one(ex, Arm::P, &tasks[rest[j]]), "fresh"));
        for (i, r) in rest.iter().copied().zip(rerun) {
            put(cu, &tasks[i], &r);
            out.pairs[i].1 = r;
        }
    } else {
        out.reused = rest.len();
    }
    out
}

/// Pool calibration (M7 line C, REAL-M7; m7-design.md §3.2.10's pool half): the incumbent alone on every
/// task, `repeats` fresh runs each (with the infra re-run), every run put into the pool cache, so the screen
/// and E3 find the incumbent cached and the miner finds the failing pool runs. Run ids carry the repeat.
pub fn run_pool_calibration(gate_id: &str, ex: &dyn RunExecutor, tasks: &[TaskDef], repeats: u32, cache: Option<&CacheUse<'_>>, parallel: usize) -> Vec<RunRecord> {
    let _ = gate_id;
    let grid: Vec<(usize, u32)> = (0..tasks.len()).flat_map(|i| (1..=repeats.max(1)).map(move |r| (i, r))).collect();
    parallel_map(grid.len(), parallel, |j| {
        let (i, r) = grid[j];
        let t = &tasks[i];
        let id = run_id(Arm::P, &t.id, r);
        let first = ex.run(Arm::P, t, r, &id);
        let run = with_cache(if first.outcome == "infra" { ex.run(Arm::P, t, r, &id) } else { first }, "fresh");
        if let Some(cu) = cache {
            put(cu, t, &run);
        }
        run
    })
}

/// The run grid (Harness² `ledger.assert_identity`): every expected `(task, arm)` recorded exactly
/// once, nothing else. Duplicates are checked first. `Err` is the `screen.grid` detail.
pub fn assert_grid(expected: &[(String, &str)], runs: &[RunRecord]) -> Result<(), String> {
    let mut seen: BTreeMap<(String, String), usize> = BTreeMap::new();
    for r in runs {
        *seen.entry((r.task.clone(), r.arm.clone())).or_default() += 1;
    }
    if let Some(((t, a), n)) = seen.iter().find(|(_, n)| **n > 1) {
        return Err(format!("duplicate run {a}/{t} ×{n}"));
    }
    let want: BTreeSet<(String, String)> = expected.iter().map(|(t, a)| (t.clone(), a.to_string())).collect();
    let got: BTreeSet<(String, String)> = seen.into_keys().collect();
    if let Some((t, a)) = want.difference(&got).next() {
        return Err(format!("missing run {a}/{t}"));
    }
    if let Some((t, a)) = got.difference(&want).next() {
        return Err(format!("unexpected run {a}/{t}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// G4 v2's screen decision (m7 §3.2.2)
// ---------------------------------------------------------------------------------------------

/// A screen reject, in the order they are checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenRejectCode {
    Canary,
    Tamper,
    ScreenErrors,
    ScreenInferior,
}

impl ScreenRejectCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ScreenRejectCode::Canary => "canary",
            ScreenRejectCode::Tamper => "tamper",
            ScreenRejectCode::ScreenErrors => "screen_errors",
            ScreenRejectCode::ScreenInferior => "screen_inferior",
        }
    }
}

/// G4 v2's screen result (the counts of the record's `screen` object).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenOutcome {
    /// Tasks judged: the incumbent measured something.
    pub n: u32,
    pub c_pass: u32,
    pub p_pass: u32,
    /// Σ(candidate passes − incumbent passes).
    pub net: i64,
    /// Candidate run errors (every screen task, judged or not).
    pub c_errors: u32,
    pub tamper: bool,
    pub canary_trip: bool,
    pub reject: Option<ScreenRejectCode>,
    /// round(1000 · net / n), 0 when n = 0.
    pub d_permille: i64,
}

/// The screen's decision from the gate canaries' runs (candidate) and the paired pool runs.
pub fn screen_decide(canaries: &[RunRecord], pairs: &[(RunRecord, RunRecord)], reject_errors: u32, reject_net: i64) -> ScreenOutcome {
    let canary_trip = canaries.iter().any(|r| r.score_permille >= 1000);
    let tamper = canaries.iter().chain(pairs.iter().map(|p| &p.0)).any(|r| r.tamper);
    let c_errors = pairs.iter().filter(|(c, _)| c_error(c)).count() as u32;
    let judged: Vec<&(RunRecord, RunRecord)> = pairs.iter().filter(|(_, p)| !p_error(p)).collect();
    let n = judged.len() as u32;
    let c_pass = judged.iter().filter(|(c, _)| !c_error(c) && passed(c)).count() as u32;
    let p_pass = judged.iter().filter(|(_, p)| passed(p)).count() as u32;
    let net = c_pass as i64 - p_pass as i64;
    let reject = if canary_trip {
        Some(ScreenRejectCode::Canary)
    } else if tamper {
        Some(ScreenRejectCode::Tamper)
    } else if c_errors >= reject_errors {
        Some(ScreenRejectCode::ScreenErrors)
    } else if net <= -reject_net {
        Some(ScreenRejectCode::ScreenInferior)
    } else {
        None
    };
    let d_permille = if n == 0 { 0 } else { (net * 2000 + n as i64 * net.signum()) / (2 * n as i64) };
    ScreenOutcome { n, c_pass, p_pass, net, c_errors, tamper, canary_trip, reject, d_permille }
}

/// G4 v2's screen on the process side: the gate canaries first (candidate, R = 1), then the paired
/// pool runs. Returns the decision, every run (canaries first) and the cache's report.
pub fn run_g4_screen(gate_id: &str, ex: &dyn RunExecutor, screen_tasks: &[TaskDef], canaries: &[TaskDef], cache: Option<&CacheUse<'_>>, parallel: usize) -> (ScreenOutcome, Vec<RunRecord>, CachedPairs) {
    let canary_runs: Vec<RunRecord> = canaries.iter().map(|t| with_cache(one(ex, Arm::C, t), "fresh")).collect();
    if canary_runs.iter().any(|r| r.score_permille >= 1000) {
        let o = screen_decide(&canary_runs, &[], SCREEN_REJECT_ERRORS, SCREEN_REJECT_NET);
        let empty = CachedPairs { pairs: Vec::new(), reused: 0, sentinel: None, alerts: Vec::new() };
        return (o, canary_runs, empty);
    }
    let cp = run_cached_pairs(gate_id, ex, screen_tasks, cache, parallel);
    let o = screen_decide(&canary_runs, &cp.pairs, SCREEN_REJECT_ERRORS, SCREEN_REJECT_NET);
    let mut runs = canary_runs;
    for (c, p) in &cp.pairs {
        runs.push(c.clone());
        runs.push(p.clone());
    }
    (o, runs, cp)
}

// ---------------------------------------------------------------------------------------------
// E3 (evolution-v2 §3.5.2) and the go/no-go estimate (§3.5.3)
// ---------------------------------------------------------------------------------------------

/// E3's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct E3Verdict {
    pub pass: bool,
    /// `screen.tamper`, `screen.mini_inferior`, `screen.costlier`, `screen.grid`.
    pub reason: Option<String>,
    pub targeted_fixed: u32,
    pub targeted_total: u32,
    pub controls_lost: u32,
    pub controls_total: u32,
    /// Σ cost_ref over the judged controls, per arm.
    pub cand_cost_ref_musd: u64,
    pub inc_cost_ref_musd: u64,
}

/// One E3 task: whether it is targeted (a control otherwise) and its pair.
#[derive(Debug, Clone, PartialEq)]
pub struct E3Task {
    pub targeted: bool,
    pub c: RunRecord,
    pub p: RunRecord,
}

/// E3's decision. Rejects, in order: any tamper or a canary trip (`screen.tamper`); 0 targeted tasks
/// fixed and ≥ 1 control lost, or ≥ 2 controls lost (`screen.mini_inferior`); the candidate's mean
/// cost per run on the controls above 2× the incumbent's (`screen.costlier`). A task whose incumbent
/// run measured nothing is left out; a candidate run error counts as a fail.
pub fn e3_decide(tasks: &[E3Task], canary_trip: bool) -> E3Verdict {
    let judged: Vec<&E3Task> = tasks.iter().filter(|t| !p_error(&t.p)).collect();
    let cpass = |t: &E3Task| !c_error(&t.c) && passed(&t.c);
    let targeted: Vec<&&E3Task> = judged.iter().filter(|t| t.targeted).collect();
    let controls: Vec<&&E3Task> = judged.iter().filter(|t| !t.targeted).collect();
    let fixed = targeted.iter().filter(|t| cpass(t) && !passed(&t.p)).count() as u32;
    let lost = controls.iter().filter(|t| passed(&t.p) && !cpass(t)).count() as u32;
    let cc: u64 = controls.iter().map(|t| t.c.cost_ref_musd).sum();
    let pc: u64 = controls.iter().map(|t| t.p.cost_ref_musd).sum();
    let tamper = canary_trip || tasks.iter().any(|t| t.c.tamper);
    let reason = if tamper {
        Some("screen.tamper")
    } else if (fixed == 0 && lost >= 1) || lost >= 2 {
        Some("screen.mini_inferior")
    } else if pc > 0 && cc > 2 * pc {
        Some("screen.costlier")
    } else {
        None
    };
    E3Verdict {
        pass: reason.is_none(),
        reason: reason.map(str::to_string),
        targeted_fixed: fixed,
        targeted_total: targeted.len() as u32,
        controls_lost: lost,
        controls_total: controls.len() as u32,
        cand_cost_ref_musd: cc,
        inc_cost_ref_musd: pc,
    }
}

/// `est = round(1000 · q · s_c · fix · T)` in permille, with `fix = (fixed + 1) / (targeted + 2)`
/// (Laplace-shrunk) and `q`, `s_c`, `T` in permille (evolution-v2 §3.5.3).
pub fn est_permille(s_c_permille: u32, targeted_fixed: u32, targeted_total: u32, q_permille: u32, transfer_permille: u32) -> u32 {
    let num = q_permille as u128 * s_c_permille as u128 * transfer_permille as u128 * (targeted_fixed as u128 + 1);
    let den = (targeted_total as u128 + 2) * 1_000_000;
    ((2 * num + den) / (2 * den)) as u32
}

/// The lean go/no-go rule: spend G4 only when `est ≥ 80‰`.
pub fn go(est_permille: u32) -> bool {
    est_permille >= RATCHET_GAIN_PERMILLE
}

/// The path a go aims for: `better` (est ≥ 150‰), `ratchet` (80–149‰) or `below_resolution`.
pub fn aim(est_permille: u32) -> &'static str {
    if est_permille >= GO_GAIN_PERMILLE {
        "better"
    } else if go(est_permille) {
        "ratchet"
    } else {
        "below_resolution"
    }
}

/// E3 on the process side: canaries (candidate, R = 1, first), then the targeted tasks and the
/// controls through [`run_cached_pairs`]; the grid is asserted before the decision.
pub fn run_e3(gate_id: &str, ex: &dyn RunExecutor, targeted: &[TaskDef], controls: &[TaskDef], canaries: &[TaskDef], cache: Option<&CacheUse<'_>>, parallel: usize) -> (E3Verdict, Vec<RunRecord>, CachedPairs) {
    let canary_runs: Vec<RunRecord> = canaries.iter().map(|t| with_cache(one(ex, Arm::C, t), "fresh")).collect();
    let canary_trip = canary_runs.iter().any(|r| r.score_permille >= 1000);
    let tasks: Vec<TaskDef> = targeted.iter().chain(controls).cloned().collect();
    let cp = if canary_trip { CachedPairs { pairs: Vec::new(), reused: 0, sentinel: None, alerts: Vec::new() } } else { run_cached_pairs(gate_id, ex, &tasks, cache, parallel) };
    let mut runs = canary_runs.clone();
    for (c, p) in &cp.pairs {
        runs.push(c.clone());
        runs.push(p.clone());
    }
    let n_t = targeted.len();
    let e3: Vec<E3Task> = cp.pairs.iter().enumerate().map(|(i, (c, p))| E3Task { targeted: i < n_t, c: c.clone(), p: p.clone() }).collect();
    let mut v = e3_decide(&e3, canary_trip || canary_runs.iter().any(|r| r.tamper));
    if !canary_trip {
        let mut expected: Vec<(String, &str)> = Vec::new();
        for t in &tasks {
            expected.push((t.id.clone(), "c"));
            expected.push((t.id.clone(), "p"));
        }
        let pool_runs: Vec<RunRecord> = runs.iter().filter(|r| r.set != "canary").cloned().collect();
        if let Err(e) = assert_grid(&expected, &pool_runs) {
            v.pass = false;
            v.reason = Some(format!("screen.grid: {e}"));
        }
    }
    (v, runs, cp)
}

#[cfg(test)]
mod tests;
