//! The v0.3 accept rule (kernel/gate.md §3.9), `cost_guard`, `audit_required` (§3.9.2) and the
//! mechanical resolution of inconclusive verdicts (§3.12; v03-decisions D4: no owner step anywhere,
//! `needs_owner` removed).
//!
//! [`accept`] is pure: the same [`G4Data`] and [`Deltas`] give the same [`G4Eval`], bit for bit (every
//! resample is seeded from the gate id). [`judge`] adds the noise-floor computation in front of it.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::Serialize;

use crate::evo::ChangeType;
use crate::stats::boot::{
    CapFamily, CapResult, FutilityLook, bootstrap_ni, capability_test, delta_min, floored, margin,
    n_needed,
};
use crate::stats::data::{Arm, Metric, TaskRuns, TaskSet, arm_stat, effect};
use crate::stats::noise::{Deltas, HistoryCtx, NoiseEntry, compute_deltas};

/// Accept reasons (§3.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptReason {
    PerfGain,
    Better,
    CapabilityGain,
    Cheaper,
    Smaller,
    ReproFixed,
    /// G4 v2 (M7): the lineage ratchet (m7-design.md §3.2.13); carries `milestone_required`.
    LineageGain,
}

/// Reject reasons (§3.9, §3.8.4, §3.12). `needs_owner` no longer exists (D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    Canary,
    ScoreInferior,
    ScoreInferiorEarly,
    PoolInferior,
    HoldoutInferior,
    PoolHoldoutDivergence,
    PerfNoGain,
    Costlier,
    CostlierThanBudget,
    NoGain,
    InconclusiveFinal,
    /// G4 v2 (M7; m7-design.md §3.2.5): the ratchet span holds `ratchet_k` unconfirmed accepts.
    RatchetFull,
    /// A candidate run flagged `tamper` (screen or slice).
    Tamper,
    /// The Root's `leak` canary or GUID scan.
    Leak,
    /// The candidate failed the Root's `health` canary (the incumbent passed it).
    Health,
    /// The screen: net candidate − incumbent passes ≤ −`screen_reject_net`.
    ScreenInferior,
    /// The screen: ≥ `screen_reject_errors` candidate run errors.
    ScreenErrors,
    /// `better` with cost per pass above RHO_PASS × the incumbent's.
    CostlierPerPass,
}

/// Inconclusive reasons (§3.7.2, §3.7.4, §3.8, §3.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InconclusiveReason {
    Insufficient,
    Underpowered,
    /// A score-gain accept (`better`, `capability_gain`, `repro_fixed`) on a holdout of fewer than
    /// [`GAIN_MIN_HOLDOUT`] tasks (D14, FIX2): the holdout cannot show that the gain generalizes.
    #[serde(rename = "underpowered:holdout")]
    UnderpoweredHoldout,
    NoNoiseEstimate,
    BudgetInsufficient,
    Budget,
    Timeout,
    Disk,
    /// G4 v2 (M7): the Root refused the slice (`root.busy`, `root.mode`) or the link was lost.
    SealedUnavailable,
    /// G4 v2 (M7): looks exhausted and the rotation failed (`root.sealed_quota`).
    SealedQuota,
}

/// Outcomes where the candidate is not judged: infrastructure, a cancel, runs that never ran
/// (`runs_failed:<cause>`: too many runs, or every run of one arm, made no model request; K-GATE),
/// or a frozen task whose cost cap is below one request's reservation (`task_cost_cap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorReason {
    Infra,
    Cancelled,
    #[serde(rename = "runs_failed:cost_capped")]
    RunsFailedCostCapped,
    #[serde(rename = "runs_failed:no_ready")]
    RunsFailedNoReady,
    #[serde(rename = "runs_failed:crash")]
    RunsFailedCrash,
    #[serde(rename = "runs_failed:timeout")]
    RunsFailedTimeout,
    #[serde(rename = "runs_failed:no_request")]
    RunsFailedNoRequest,
    TaskCostCap,
    /// G4 v2 (M7): the incumbent failed the Root's `health` canary (`error:harness_health`).
    HarnessHealth,
    /// G4 v2 (M7): the check directory changed during a sealed run (`error:checker_tamper`).
    CheckerTamper,
    /// G4 v2 (M7): any other `error:<cause>` of the slice, or a malformed answer (the cause is in
    /// the record's `sealed.status`).
    Sealed,
}

impl ErrorReason {
    /// `runs_failed:<cause>` for a run outcome that made no model request.
    pub fn runs_failed(outcome: &str) -> ErrorReason {
        match outcome {
            "cost_capped" => ErrorReason::RunsFailedCostCapped,
            "no_ready" => ErrorReason::RunsFailedNoReady,
            "crash" => ErrorReason::RunsFailedCrash,
            "timeout" => ErrorReason::RunsFailedTimeout,
            _ => ErrorReason::RunsFailedNoRequest,
        }
    }

    /// Only `infra` is transient: a G4 that ended in any other error is not retried (F12), because a
    /// re-run would fail the same way. M7: `harness_health` and `checker_tamper` are retried like
    /// `infra` (m7-design.md §3.1.4, §5).
    pub fn retryable(&self) -> bool {
        matches!(self, ErrorReason::Infra | ErrorReason::HarnessHealth | ErrorReason::CheckerTamper)
    }
}

/// The verdict vocabulary of the gate contract (§4.1a): `accept | reject | inconclusive | error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "verdict", content = "reason", rename_all = "snake_case")]
pub enum Verdict {
    Accept(AcceptReason),
    Reject(RejectReason),
    Inconclusive(InconclusiveReason),
    Error(ErrorReason),
}

/// The accept reasons that add capability (§3.9 `ADDS_CAPABILITY`).
pub const ADDS_CAPABILITY: [AcceptReason; 3] = [
    AcceptReason::Better,
    AcceptReason::CapabilityGain,
    AcceptReason::ReproFixed,
];

/// D14 (FIX2): an accept that rests on a score gain (a reason in [`ADDS_CAPABILITY`]) needs a holdout of
/// at least this many tasks; below it the verdict is `inconclusive: underpowered:holdout`. A smaller
/// holdout cannot show that a gain generalizes: with the divergence guard's half-task floor, a 1-task
/// holdout never trips it, so a pool-only gain (the MS-5 answer-key signature) would pass as `better`.
/// The `cheaper`, `smaller` and `perf_gain` accepts are not score gains and are unaffected. A constant,
/// not a config key, so it cannot be tuned away (config.md: a gate parameter can weaken the gate).
pub const GAIN_MIN_HOLDOUT: usize = 4;

macro_rules! reason_str {
    ($t:ty { $($v:ident => $s:literal),+ $(,)? }) => {
        impl $t {
            pub fn as_str(&self) -> &'static str {
                match self { $(Self::$v => $s),+ }
            }
        }
    };
}

reason_str!(AcceptReason {
    PerfGain => "perf_gain", Better => "better", CapabilityGain => "capability_gain",
    Cheaper => "cheaper", Smaller => "smaller", ReproFixed => "repro_fixed", LineageGain => "lineage_gain",
});
reason_str!(RejectReason {
    Canary => "canary", ScoreInferior => "score_inferior", ScoreInferiorEarly => "score_inferior_early",
    PoolInferior => "pool_inferior", HoldoutInferior => "holdout_inferior",
    PoolHoldoutDivergence => "pool_holdout_divergence", PerfNoGain => "perf_no_gain", Costlier => "costlier",
    CostlierThanBudget => "costlier_than_budget", NoGain => "no_gain", InconclusiveFinal => "inconclusive_final",
    RatchetFull => "ratchet_full", Tamper => "tamper", Leak => "leak", Health => "health",
    ScreenInferior => "screen_inferior", ScreenErrors => "screen_errors", CostlierPerPass => "costlier_per_pass",
});
reason_str!(InconclusiveReason {
    Insufficient => "insufficient", Underpowered => "underpowered",
    UnderpoweredHoldout => "underpowered:holdout", NoNoiseEstimate => "no_noise_estimate",
    BudgetInsufficient => "budget_insufficient", Budget => "budget", Timeout => "timeout", Disk => "disk",
    SealedUnavailable => "sealed_unavailable", SealedQuota => "sealed_quota",
});
reason_str!(ErrorReason {
    Infra => "infra", Cancelled => "cancelled",
    RunsFailedCostCapped => "runs_failed:cost_capped", RunsFailedNoReady => "runs_failed:no_ready",
    RunsFailedCrash => "runs_failed:crash", RunsFailedTimeout => "runs_failed:timeout",
    RunsFailedNoRequest => "runs_failed:no_request", TaskCostCap => "task_cost_cap",
    HarnessHealth => "harness_health", CheckerTamper => "checker_tamper", Sealed => "sealed",
});

impl Verdict {
    pub fn kind(&self) -> &'static str {
        match self {
            Verdict::Accept(_) => "accept",
            Verdict::Reject(_) => "reject",
            Verdict::Inconclusive(_) => "inconclusive",
            Verdict::Error(_) => "error",
        }
    }

    /// The short reason carried by events, notes and reports (e.g. `pool_holdout_divergence`).
    pub fn reason(&self) -> &'static str {
        match self {
            Verdict::Accept(r) => r.as_str(),
            Verdict::Reject(r) => r.as_str(),
            Verdict::Inconclusive(r) => r.as_str(),
            Verdict::Error(r) => r.as_str(),
        }
    }

    /// The job error code `gate.g4.<reason>` (§4.1); `None` for an accept.
    pub fn code(&self) -> Option<String> {
        match self {
            Verdict::Accept(_) => None,
            _ => Some(format!("gate.g4.{}", self.reason())),
        }
    }

    /// `true` for an accept whose reason is in [`ADDS_CAPABILITY`].
    pub fn adds_capability(&self) -> bool {
        matches!(self, Verdict::Accept(r) if ADDS_CAPABILITY.contains(r))
    }
}

/// The `[gate.g4]` keys the rule reads (kernel/config.md §4.6; defaults from gate.md §3.8.5).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct G4Config {
    pub min_pool: usize,
    pub min_holdout: usize,
    pub min_canaries: usize,
    pub alpha: f64,
    pub margin_factor: f64,
    pub margin_max: f64,
    pub bootstrap_b: usize,
    pub noise_b: usize,
    pub rho_max: f64,
    /// `gate.g4.ct_include_wall`: CT is `cost_x_wall` when true, `cost_usd` when false (test configs).
    pub ct_include_wall: bool,
    pub capability_min_tasks: usize,
}

impl Default for G4Config {
    fn default() -> Self {
        G4Config {
            min_pool: 8,
            min_holdout: 4,
            min_canaries: 1,
            alpha: 0.05,
            margin_factor: 2.0,
            margin_max: 0.15,
            bootstrap_b: 10_000,
            noise_b: 2_000,
            rho_max: 1.5,
            ct_include_wall: true,
            capability_min_tasks: 4,
        }
    }
}

/// Inputs of `cost_guard` (§3.9): the task part of the soft day budget and the observed task rate.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CostBudget {
    /// The effective soft day budget (kernel/proxy.md §3.8), µUSD.
    pub soft_day_musd: u64,
    /// `budget.evolution_share` in permille (600).
    pub evolution_share_permille: u64,
    /// Mean distinct `x-init-task` ids per Root day over the last 30 days (at least 1).
    pub tasks_per_day: f64,
}

/// One `Repro:` result (§3.9.1). Scores are per repeat, permille.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum ReproEvidence {
    /// A frozen owner-accepted or maintainer-imported task.
    Frozen {
        reference: String,
        c: Vec<u32>,
        p: Vec<u32>,
    },
    /// `Repro: task:<family>` on an agent family: only its **sealed** instances count, all must flip, and
    /// only if the family is eligible (§3.7.5).
    AgentFamily {
        reference: String,
        eligible: bool,
        sealed: Vec<(Vec<u32>, Vec<u32>)>,
    },
}

fn all_pass(v: &[u32]) -> bool {
    !v.is_empty() && v.iter().all(|&s| s == 1000)
}

fn all_fail(v: &[u32]) -> bool {
    !v.is_empty() && v.iter().all(|&s| s < 1000)
}

impl ReproEvidence {
    pub fn is_agent(&self) -> bool {
        matches!(self, ReproEvidence::AgentFamily { .. })
    }

    /// Passed on all R repeats by the candidate and failed on all R by LKG.
    pub fn fixed(&self) -> bool {
        match self {
            ReproEvidence::Frozen { c, p, .. } => all_pass(c) && all_fail(p),
            ReproEvidence::AgentFamily {
                eligible, sealed, ..
            } => {
                *eligible
                    && !sealed.is_empty()
                    && sealed.iter().all(|(c, p)| all_pass(c) && all_fail(p))
            }
        }
    }
}

/// Everything the rule needs about one G4 (§3.9 "Inputs").
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct G4Data {
    pub gate_id: String,
    pub change_type: ChangeType,
    /// `Validated.capability_labels`: a non-empty list is a pre-registered capability claim.
    pub capability_labels: Vec<String>,
    /// Pool (including the agent stratum and the repro tasks) ∪ holdout.
    pub tasks: Vec<TaskRuns>,
    /// Canary scores, candidate arm, R = 1.
    pub canaries: Vec<u32>,
    /// The capability set A (sealed instances of eligible families with a claimed label), per family.
    pub capability: Vec<CapFamily>,
    pub repro: Vec<ReproEvidence>,
    pub body_tokens_c: u64,
    pub body_tokens_p: u64,
    pub budget: CostBudget,
    /// The one top-up is still available: `repeats < repeats_max` and the remaining gate budget covers it.
    pub top_up_allowed: bool,
    pub config: G4Config,
}

impl G4Data {
    /// The same runs with every agent-authored task removed: the stratum, the capability set A and the
    /// agent-family repros (§3.9.2).
    pub fn without_agent_tasks(&self) -> G4Data {
        let mut g = self.clone();
        g.tasks.retain(|t| !t.is_agent());
        g.capability.clear();
        g.repro.retain(|r| !r.is_agent());
        g
    }

    fn count(&self, set: TaskSet) -> usize {
        self.tasks.iter().filter(|t| t.set == set).count()
    }

    fn ct_metric(&self) -> Metric {
        if self.config.ct_include_wall {
            Metric::CostXWall
        } else {
            Metric::CostUsd
        }
    }

    pub fn repro_fixed(&self) -> bool {
        self.repro.iter().any(ReproEvidence::fixed)
    }
}

/// The non-terminal top-up request, or a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Verdict(Verdict),
    /// m > `margin_max` and a top-up is possible: run one more repeat for both arms on every task, then
    /// judge again (§3.8.3).
    TopUp {
        margin: f64,
        n_needed: usize,
    },
}

impl Decision {
    pub fn verdict(&self) -> Option<Verdict> {
        match self {
            Decision::Verdict(v) => Some(*v),
            Decision::TopUp { .. } => None,
        }
    }
}

/// Cost × time class of the tie-breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CtClass {
    Cheaper,
    Costlier,
    Same,
}

/// Per-split score statistics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SplitStats {
    pub d: f64,
    /// δ_used of this split as measured (§3.8.2): what the split guard uses.
    pub delta: f64,
    /// δ_min = 0.5 / n of this split (D14).
    pub delta_min: f64,
    /// `max(delta, delta_min)`: what the divergence guard uses (D14).
    pub delta_floored: f64,
}

impl SplitStats {
    fn new(d: f64, delta: f64, n: usize) -> Self {
        SplitStats {
            d,
            delta,
            delta_min: delta_min(n),
            delta_floored: floored(delta, n),
        }
    }
}

/// Score statistics (`G4.score` of the record, as fractions; the record rounds to permille).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ScoreStats {
    pub d: f64,
    pub l: f64,
    pub u: f64,
    pub margin: f64,
    /// δ_used(pass_rate) over pool ∪ holdout as measured; `margin` is `max(2·delta, delta_min)`.
    pub delta: f64,
    /// δ_min = 0.5 / n over pool ∪ holdout (D14): the margin's floor.
    pub delta_min: f64,
    pub c: f64,
    pub p: f64,
    pub pool: SplitStats,
    pub holdout: SplitStats,
}

/// Cost × time statistics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CtStats {
    pub metric: Metric,
    pub effect: f64,
    pub delta: f64,
    pub class: CtClass,
}

/// Counts per set (`G4.n`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Counts {
    pub pool: usize,
    pub holdout: usize,
    pub agent: usize,
    pub canaries: usize,
    pub capability: usize,
    pub repro: usize,
}

/// What the rule computed, for the gate record and `evo::predict`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct G4Stats {
    pub n: Counts,
    pub canary_max_permille: u32,
    pub score: Option<ScoreStats>,
    pub ct: Option<CtStats>,
    pub capability: Option<CapResult>,
    pub repro_fixed: bool,
}

/// The rule's output: the decision, the `audit_required` flag (§3.9.2) and the statistics.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct G4Eval {
    pub decision: Decision,
    pub audit_required: bool,
    pub stats: G4Stats,
}

/// The checks that need no δ: a canary at 1000 stops G4 (`reject: canary`); below the minimums the
/// result is `inconclusive: insufficient` (§3.7.2).
fn precheck(g: &G4Data, n: &Counts) -> Option<Verdict> {
    let cfg = &g.config;
    if g.canaries.contains(&1000) {
        return Some(Verdict::Reject(RejectReason::Canary));
    }
    if n.pool < cfg.min_pool || n.holdout < cfg.min_holdout || n.canaries < cfg.min_canaries {
        return Some(Verdict::Inconclusive(InconclusiveReason::Insufficient));
    }
    None
}

fn counts(g: &G4Data) -> Counts {
    Counts {
        pool: g.count(TaskSet::Pool),
        holdout: g.count(TaskSet::Holdout),
        agent: g.tasks.iter().filter(|t| t.is_agent()).count(),
        canaries: g.canaries.len(),
        capability: g.capability.iter().map(|f| f.n_instances).sum(),
        repro: g.repro.len(),
    }
}

fn base_stats(g: &G4Data) -> G4Stats {
    G4Stats {
        n: counts(g),
        canary_max_permille: g.canaries.iter().copied().max().unwrap_or(0),
        score: None,
        ct: None,
        capability: None,
        repro_fixed: g.repro_fixed(),
    }
}

fn mean(v: impl Iterator<Item = f64>) -> f64 {
    let (mut s, mut n) = (0.0, 0usize);
    for x in v {
        s += x;
        n += 1;
    }
    if n == 0 { 0.0 } else { s / n as f64 }
}

/// `accept(g)` of §3.9 with δ given, then D14's holdout rule: a score-gain accept on a holdout of fewer
/// than [`GAIN_MIN_HOLDOUT`] tasks is `inconclusive: underpowered:holdout`. `fixed_margin` is used by the
/// §3.9.2 recompute ("the same δ, margin and seeds"); it also disables the top-up and `underpowered`
/// branch, which the main run already passed.
fn accept_inner(g: &G4Data, deltas: &Deltas, fixed_margin: Option<f64>) -> (Decision, G4Stats) {
    let (decision, st) = accept_rule(g, deltas, fixed_margin);
    match decision {
        Decision::Verdict(v) if v.adds_capability() && st.n.holdout < GAIN_MIN_HOLDOUT => (
            Decision::Verdict(Verdict::Inconclusive(
                InconclusiveReason::UnderpoweredHoldout,
            )),
            st,
        ),
        other => (other, st),
    }
}

/// §3.9's rule proper (with D14's divergence floor), before the holdout rule of [`accept_inner`].
fn accept_rule(g: &G4Data, deltas: &Deltas, fixed_margin: Option<f64>) -> (Decision, G4Stats) {
    let cfg = &g.config;
    let mut st = base_stats(g);
    let done = |v: Verdict, st: G4Stats| (Decision::Verdict(v), st);

    if let Some(v) = precheck(g, &st.n) {
        return done(v, st);
    }
    let n = g.tasks.len();
    let m = match fixed_margin {
        Some(m) => m,
        None => {
            let m = margin(deltas.pass_all, n, cfg.margin_factor);
            if m > cfg.margin_max {
                if g.top_up_allowed {
                    return (
                        Decision::TopUp {
                            margin: m,
                            n_needed: n_needed(n, m, cfg.margin_max),
                        },
                        st,
                    );
                }
                return done(Verdict::Inconclusive(InconclusiveReason::Underpowered), st);
            }
            m
        }
    };

    let refs: Vec<&TaskRuns> = g.tasks.iter().collect();
    let d: Vec<f64> = g.tasks.iter().map(TaskRuns::d).collect();
    let iv = bootstrap_ni(&g.gate_id, &d, cfg.bootstrap_b, cfg.alpha);
    let d_pool = mean(
        g.tasks
            .iter()
            .filter(|t| t.set == TaskSet::Pool)
            .map(TaskRuns::d),
    );
    let d_holdout = mean(
        g.tasks
            .iter()
            .filter(|t| t.set == TaskSet::Holdout)
            .map(TaskRuns::d),
    );
    // D14: the divergence guard uses each split's δ floored at half a task of that split. The split
    // guards and the margin's δ stay as measured (the margin keeps its own 0.5 / n floor).
    let pool = SplitStats::new(d_pool, deltas.pass_pool, st.n.pool);
    let holdout = SplitStats::new(d_holdout, deltas.pass_holdout, st.n.holdout);
    st.score = Some(ScoreStats {
        d: iv.mean,
        l: iv.lower,
        u: iv.upper,
        margin: m,
        delta: deltas.pass_all,
        delta_min: delta_min(n),
        c: arm_stat(Metric::PassRate, &refs, Arm::C),
        p: arm_stat(Metric::PassRate, &refs, Arm::P),
        pool,
        holdout,
    });

    if iv.lower <= -m {
        return done(Verdict::Reject(RejectReason::ScoreInferior), st);
    }
    if d_pool < -2.0 * deltas.pass_pool {
        return done(Verdict::Reject(RejectReason::PoolInferior), st);
    }
    if d_holdout < -2.0 * deltas.pass_holdout {
        return done(Verdict::Reject(RejectReason::HoldoutInferior), st);
    }
    let (dp, dh) = (pool.delta_floored, holdout.delta_floored);
    if d_pool - d_holdout > 2.0 * (dp.powi(2) + dh.powi(2)).sqrt() {
        return done(Verdict::Reject(RejectReason::PoolHoldoutDivergence), st);
    }

    let ctm = g.ct_metric();
    let ct = effect(ctm, &refs);
    let dct = deltas.get(ctm);
    let class = if ct < -dct {
        CtClass::Cheaper
    } else if ct > dct {
        CtClass::Costlier
    } else {
        CtClass::Same
    };
    st.ct = Some(CtStats {
        metric: ctm,
        effect: ct,
        delta: dct,
        class,
    });
    let size = g.body_tokens_c.cmp(&g.body_tokens_p);

    let cost_guard = |reason: AcceptReason| -> Verdict {
        if ct.exp() <= cfg.rho_max {
            return Verdict::Accept(reason);
        }
        // Dearer by more than ρ_max: accept only if the day still fits at the new cost (evolution.md §3.2).
        let projected = g.budget.tasks_per_day.max(1.0) * arm_stat(Metric::CostUsd, &refs, Arm::C);
        let task_part = g.budget.soft_day_musd as f64
            * (1000.0 - g.budget.evolution_share_permille as f64)
            / 1000.0;
        if projected <= task_part {
            Verdict::Accept(reason)
        } else {
            Verdict::Reject(RejectReason::CostlierThanBudget)
        }
    };

    if g.change_type == ChangeType::Perf {
        let gain = effect(Metric::CostUsd, &refs) < -deltas.get(Metric::CostUsd)
            || effect(Metric::WallS, &refs) < -deltas.get(Metric::WallS);
        let v = if gain {
            Verdict::Accept(AcceptReason::PerfGain)
        } else {
            Verdict::Reject(RejectReason::PerfNoGain)
        };
        return done(v, st);
    }
    if iv.lower > 0.0 {
        let v = cost_guard(AcceptReason::Better);
        return done(v, st);
    }
    if !g.capability_labels.is_empty() {
        let cap = capability_test(
            &g.gate_id,
            &g.capability,
            cfg.capability_min_tasks,
            cfg.bootstrap_b,
            cfg.alpha,
        );
        let gain = cap.gain();
        st.capability = Some(cap);
        if gain {
            let v = cost_guard(AcceptReason::CapabilityGain);
            return done(v, st);
        }
    }
    // Score tie (non-inferior, not superior, no capability evidence): spec §6.3 tie-breaks.
    let v = match class {
        CtClass::Cheaper => Verdict::Accept(AcceptReason::Cheaper),
        CtClass::Costlier => {
            if st.repro_fixed && ct.exp() <= cfg.rho_max {
                Verdict::Accept(AcceptReason::ReproFixed)
            } else {
                Verdict::Reject(RejectReason::Costlier)
            }
        }
        CtClass::Same => match size {
            Ordering::Less => Verdict::Accept(AcceptReason::Smaller),
            _ => {
                if st.repro_fixed {
                    Verdict::Accept(AcceptReason::ReproFixed)
                } else {
                    Verdict::Reject(RejectReason::NoGain)
                }
            }
        },
    };
    done(v, st)
}

/// The accept rule with δ given, plus `audit_required` (§3.9.2):
/// `audit_required = v ∈ ADDS_CAPABILITY ∧ ¬(accept(g.without_agent_tasks()) ∈ ADDS_CAPABILITY)`,
/// recomputed with the same δ, margin and seeds.
pub fn accept(g: &G4Data, deltas: &Deltas) -> G4Eval {
    let (decision, stats) = accept_inner(g, deltas, None);
    let audit_required = match (decision, stats.score) {
        (Decision::Verdict(v), Some(score)) if v.adds_capability() => {
            let (d2, _) = accept_inner(&g.without_agent_tasks(), deltas, Some(score.margin));
            !matches!(d2, Decision::Verdict(v2) if v2.adds_capability())
        }
        _ => false,
    };
    G4Eval {
        decision,
        audit_required,
        stats,
    }
}

/// The full G4 judgement: canary and minimums first, then δ(m) from the LKG repeats smoothed with
/// `noise.json` (`inconclusive: no_noise_estimate` when neither exists), then [`accept`]. Returns the
/// `noise.json` entries to store back (empty when δ was not computed).
pub fn judge(
    g: &G4Data,
    hist: Option<&HistoryCtx<'_>>,
    now_rfc3339: &str,
) -> (G4Eval, Option<Deltas>, BTreeMap<String, NoiseEntry>) {
    let stats = base_stats(g);
    if let Some(v) = precheck(g, &stats.n) {
        return (
            G4Eval {
                decision: Decision::Verdict(v),
                audit_required: false,
                stats,
            },
            None,
            BTreeMap::new(),
        );
    }
    let refs: Vec<&TaskRuns> = g.tasks.iter().collect();
    match compute_deltas(&g.gate_id, &refs, hist, g.config.noise_b, now_rfc3339) {
        Ok((deltas, store)) => (accept(g, &deltas), Some(deltas), store),
        Err(_) => {
            let v = Verdict::Inconclusive(InconclusiveReason::NoNoiseEstimate);
            (
                G4Eval {
                    decision: Decision::Verdict(v),
                    audit_required: false,
                    stats,
                },
                None,
                BTreeMap::new(),
            )
        }
    }
}

/// The futility look's verdict (§3.8.4): a stop is `reject: score_inferior_early`.
pub fn futility_verdict(look: Option<FutilityLook>) -> Option<Verdict> {
    match look {
        Some(l) if l.stop => Some(Verdict::Reject(RejectReason::ScoreInferiorEarly)),
        _ => None,
    }
}

/// How a G4 outcome is carried forward (§3.12).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// The verdict stands.
    Final(Verdict),
    /// Exactly one escalated re-run: a new G4 job with R = `evolution.g4_escalate_repeats` (4) and every
    /// pool and holdout task the gate budget allows, record `escalated: true`.
    Escalate { repeats: u32 },
}

/// Mechanical resolution of inconclusive verdicts (§3.12, D4): the first inconclusive G4 escalates once;
/// an escalated re-run that is still inconclusive becomes `reject: inconclusive_final` (the incumbent
/// wins). Nothing waits for an owner.
pub fn resolve(v: Verdict, escalated: bool, escalate_repeats: u32) -> Resolution {
    match v {
        Verdict::Inconclusive(_) if !escalated => Resolution::Escalate {
            repeats: escalate_repeats,
        },
        Verdict::Inconclusive(_) => {
            Resolution::Final(Verdict::Reject(RejectReason::InconclusiveFinal))
        }
        other => Resolution::Final(other),
    }
}

/// F16: a version still provisional after `evolution.provisional_max_age_s` (86,400 s) is rolled back.
pub fn provisional_expired(age_s: u64, max_age_s: u64) -> bool {
    age_s > max_age_s
}

#[cfg(test)]
mod tests;
