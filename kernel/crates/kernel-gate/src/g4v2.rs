//! G4 v2, the kernel half, lean (M7; m7-design.md §3.2; kernel/gate.md §3.7.1a, §3.8.5, §3.9 M7 notes;
//! build/specs/PROOF-LEAN.md D-L3).
//!
//! What lives here is pure (no process, no link):
//! - [`Tier`] and [`preflight`]: the tier choice against the remaining gate budget (m7 §3.2.6);
//! - the `sealed.g4_slice` wire types ([`SliceArgs`], [`SliceAnswer`], [`SliceRefusal`]) as ROOT-S1a
//!   pinned them (build/status/ROOT-S1a.md, 455b246; m7 §4.1);
//! - [`ScreenSummary`], the screen's result (the screen runner is lane GATE-V2b's), and
//!   [`screen_stats`], the paired statistics of the screen's runs;
//! - [`accept_v2`], the accept rule v2 (m7 §3.2.5) on the S path;
//! - [`V2Record`], the G4 record v3 keys (m7 §4.4) and the agent view (m7 §3.2.11).
//!
//! Cut by D-L3: judge mode, `gate simulate --rule v2`, the gog v2 enumeration and the v0.3 fallback
//! with v2 rules. Without S (`root.sealed_none`, test homes) G4 runs the unchanged v0.3 kernel path.
//! The capability path (L1-V2) is cut by D-L6.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accept::{
    AcceptReason, CtClass, CtStats, ErrorReason, InconclusiveReason, RejectReason, Verdict,
};
use crate::evo::ChangeType;
use crate::g4::{RunRecord, run_error};
use crate::stats::boot::delta_min;
use crate::stats::data::{Arm, Metric, RunValues, Source, TaskRuns, TaskSet, arm_stat, effect};
use crate::stats::noise::NoiseHistory;

/// `better` and `capability_gain` need cpp(c) ≤ RHO_PASS · cpp(p) (m7 §3.2.4). A constant.
pub const RHO_PASS_PERMILLE: i64 = 1500;
/// The ratchet's cost-per-pass bound (m7 §3.2.13). A constant.
pub const RHO_RATCHET_PERMILLE: i64 = 1200;
/// Pre-flight: a tier fits when its estimate × 1.2 fits the remaining budget (m7 §3.2.6).
pub const PREFLIGHT_SLACK_PERMILLE: u64 = 1200;
/// The per-run estimate when nothing better is known: $0.005 (m7 §3.2.6), µUSD.
pub const DEFAULT_RUN_MUSD: u64 = 5_000;
/// The screen's size (`gate.g4.screen_k`, m7 §4.3).
pub const SCREEN_K: u64 = 12;
/// The Root's canary runs per slice: `impossible`, `tamper`, `leak` on the candidate and `health`
/// on both arms (m7 §3.1.4).
pub const ROOT_CANARY_RUNS: u64 = 5;

// ---------------------------------------------------------------------------------------------
// Tiers and pre-flight (m7 §3.2.6)
// ---------------------------------------------------------------------------------------------

/// A G4 v2 tier: the S slice's n × R.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Lean,
    Standard,
    Strong,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Lean, Tier::Standard, Tier::Strong];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Lean => "lean",
            Tier::Standard => "standard",
            Tier::Strong => "strong",
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.as_str() == s)
    }

    /// Slice tasks.
    pub fn n(self) -> u32 {
        match self {
            Tier::Lean => 24,
            Tier::Standard | Tier::Strong => 32,
        }
    }

    /// Runs per arm per task.
    pub fn repeats(self) -> u32 {
        match self {
            Tier::Lean => 2,
            Tier::Standard => 3,
            Tier::Strong => 5,
        }
    }

    /// The next tier up (escalation, m7 §3.2.5); strong stays strong.
    pub fn up(self) -> Tier {
        match self {
            Tier::Lean => Tier::Standard,
            Tier::Standard | Tier::Strong => Tier::Strong,
        }
    }

    /// Runs of one G4 with both arms fresh (m7 §3.2.6's "fresh" column): the Root's canary runs, the
    /// gate's own canaries, the screen (`SCREEN_K` per arm) and 2·n·R slice runs. The kernel does not
    /// know the Root's incumbent cache, so the pre-flight always assumes fresh runs: it can pick a
    /// weaker tier than the budget allows, never a dearer one.
    pub fn runs_fresh(self, gate_canaries: u64) -> u64 {
        ROOT_CANARY_RUNS + gate_canaries + 2 * SCREEN_K + 2 * u64::from(self.n()) * u64::from(self.repeats())
    }

    /// The slice's own runs, both arms fresh, plus the Root's canaries: what the Root spends.
    pub fn slice_runs_fresh(self) -> u64 {
        ROOT_CANARY_RUNS + 2 * u64::from(self.n()) * u64::from(self.repeats())
    }
}

/// `gate.g4.tier`: `auto` (default) or a pinned tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TierCfg {
    #[default]
    Auto,
    Fixed(Tier),
}

impl TierCfg {
    pub fn parse(s: &str) -> Option<TierCfg> {
        if s == "auto" {
            return Some(TierCfg::Auto);
        }
        Tier::parse(s).map(TierCfg::Fixed)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TierCfg::Auto => "auto",
            TierCfg::Fixed(t) => t.as_str(),
        }
    }
}

/// The pre-flight's choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Preflight {
    pub tier: Tier,
    /// Runs assumed (fresh).
    pub runs: u64,
    /// `runs × per_run_musd`, µUSD.
    pub estimate_musd: u64,
}

/// m7 §3.2.6 "Tier choice": `auto` takes the strongest tier whose estimate × 1.2 fits `remaining_musd`;
/// a pinned tier is used only if it fits. `at_least` is the escalated re-run's floor (the next tier up,
/// m7 §3.2.5). Nothing that fits → `inconclusive: budget_insufficient`.
pub fn preflight(
    cfg: TierCfg,
    at_least: Option<Tier>,
    remaining_musd: u64,
    per_run_musd: u64,
    gate_canaries: u64,
) -> Result<Preflight, InconclusiveReason> {
    let per_run = if per_run_musd == 0 { DEFAULT_RUN_MUSD } else { per_run_musd };
    let fits = |t: Tier| -> Option<Preflight> {
        let runs = t.runs_fresh(gate_canaries);
        let estimate_musd = runs.saturating_mul(per_run);
        let need = estimate_musd.saturating_mul(PREFLIGHT_SLACK_PERMILLE) / 1000;
        (need <= remaining_musd).then_some(Preflight { tier: t, runs, estimate_musd })
    };
    let floor = at_least.unwrap_or(Tier::Lean);
    let pick = match cfg {
        TierCfg::Auto => Tier::ALL.into_iter().rev().filter(|t| *t >= floor).find_map(fits),
        TierCfg::Fixed(t) => fits(t.max(floor)),
    };
    pick.ok_or(InconclusiveReason::BudgetInsufficient)
}

// ---------------------------------------------------------------------------------------------
// `sealed.g4_slice` (m7 §4.1; pinned by ROOT-S1a in build/status/ROOT-S1a.md, 455b246)
// ---------------------------------------------------------------------------------------------

/// `{name, sha256}` of an ext component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SliceComponent {
    pub name: String,
    pub sha256: String,
}

/// `candidate.ext` (the lean Root accepts and ignores it).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SliceExt {
    pub set: Option<String>,
    pub components: Vec<SliceComponent>,
}

/// `candidate`: sha256s already in the Root store. `t0` is always null in the lean build (D-L2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SliceCandidate {
    pub body: String,
    pub ext: SliceExt,
    pub t0: Option<String>,
}

/// `tier`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SliceTier {
    pub name: String,
    pub n: u32,
    pub repeats: u32,
}

impl From<Tier> for SliceTier {
    fn from(t: Tier) -> SliceTier {
        SliceTier { name: t.as_str().into(), n: t.n(), repeats: t.repeats() }
    }
}

/// `sealed.g4_slice` args.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SliceArgs {
    pub gate_id: String,
    pub candidate: SliceCandidate,
    /// The Root's `lkg` tuple id (`root.args` otherwise).
    pub incumbent: String,
    pub tier: SliceTier,
    /// The lane cap for the whole slice: the gate's remaining soft budget.
    pub budget_musd: u64,
    /// 60–14,400.
    pub wall_s: u64,
}

/// `sealed.g4_slice` result (agent-visible values only: the Root rounds them, m7 §3.2.11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SliceAnswer {
    pub seq: u64,
    pub status: String,
    #[serde(default)]
    pub ratchet_signal: Option<bool>,
    #[serde(default)]
    pub unconfirmed: Option<u32>,
    #[serde(default)]
    pub ratchet_k: Option<u32>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub n: u32,
    #[serde(default)]
    pub repeats: u32,
    #[serde(default)]
    pub d_permille: Option<i64>,
    #[serde(default)]
    pub m_permille: Option<i64>,
    #[serde(default)]
    pub cpp_ratio_permille: Option<i64>,
    /// Always null in the lean build (D-L2).
    #[serde(default)]
    pub looks_left: Option<u32>,
    #[serde(default)]
    pub s_version: Option<String>,
    #[serde(default)]
    pub dup: bool,
    #[serde(default)]
    pub spent_musd: u64,
}

/// The slice status (m7 §3.2.3 item 8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SliceStatus {
    Superior,
    Tie,
    TieNegative,
    Inferior,
    InferiorEarly,
    Underpowered,
    Canary,
    Tamper,
    Leak,
    Health,
    /// `error:<cause>`.
    Error(String),
}

impl SliceStatus {
    pub fn parse(s: &str) -> Option<SliceStatus> {
        Some(match s {
            "superior" => SliceStatus::Superior,
            "tie" => SliceStatus::Tie,
            "tie_negative" => SliceStatus::TieNegative,
            "inferior" => SliceStatus::Inferior,
            "inferior_early" => SliceStatus::InferiorEarly,
            "underpowered" => SliceStatus::Underpowered,
            "canary" => SliceStatus::Canary,
            "tamper" => SliceStatus::Tamper,
            "leak" => SliceStatus::Leak,
            "health" => SliceStatus::Health,
            _ => SliceStatus::Error(s.strip_prefix("error:").filter(|c| !c.is_empty())?.to_string()),
        })
    }

    /// A valid test (a look): the statistics exist.
    pub fn is_test(&self) -> bool {
        matches!(
            self,
            SliceStatus::Superior
                | SliceStatus::Tie
                | SliceStatus::TieNegative
                | SliceStatus::Inferior
                | SliceStatus::InferiorEarly
                | SliceStatus::Underpowered
        )
    }
}

/// Round half away from zero to a multiple of `step`.
pub fn round_to(x: i64, step: i64) -> i64 {
    let h = step / 2;
    if x >= 0 { (x + h) / step * step } else { -((-x + h) / step * step) }
}

impl SliceAnswer {
    /// The parsed status, or `None` when the answer is malformed (an unknown status, or a valid
    /// test without `d`, `m` or `cpp`): the kernel then ends G4 with `error: sealed`.
    pub fn parsed(&self) -> Option<SliceStatus> {
        let s = SliceStatus::parse(&self.status)?;
        if s.is_test() && (self.d_permille.is_none() || self.m_permille.is_none() || self.cpp_ratio_permille.is_none() || self.n == 0) {
            return None;
        }
        Some(s)
    }

    /// The agent view (m7 §3.2.11): the kernel re-applies the Root's rounding, so an unrounded
    /// value can never reach a report, a note or `initctl`.
    pub fn agent_view(&self) -> Value {
        json!({
            "seq": self.seq, "status": self.status, "n": self.n, "repeats": self.repeats,
            "tier": self.tier,
            "d_permille": self.d_permille.map(|d| round_to(d, 50)),
            "m_permille": self.m_permille.map(|m| round_to(m, 10)),
            "cpp_ratio_permille": self.cpp_ratio_permille.map(|c| round_to(c, 50)),
            "looks_left": self.looks_left, "s_version": self.s_version, "dup": self.dup,
            "spent_musd": self.spent_musd,
        })
    }
}

/// Why the slice did not run (a Root refusal or a link failure; nothing ran, no record).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SliceRefusal {
    /// `root.sealed_none` (or a Root that does not know the command): no S; the v0.3 kernel path.
    SealedNone,
    /// `root.busy`, `root.mode`, a lost link or no answer in time.
    Unavailable(String),
    /// `root.sealed_quota`.
    Quota,
    /// `root.budget`: `budget_musd` below the Root's estimate.
    Budget,
    /// `root.store`: the body blob is missing (a failed `store.put`; transient).
    Store(String),
    /// `root.args` or anything else: a kernel/Root contract bug.
    Other(String),
}

impl SliceRefusal {
    /// The `root.*` refusal code to a [`SliceRefusal`] (`unknown_cmd`: an older Root, no S).
    pub fn from_code(code: &str, message: &str) -> SliceRefusal {
        match code {
            "root.sealed_none" | "root.unknown_cmd" => SliceRefusal::SealedNone,
            "root.busy" | "root.mode" => SliceRefusal::Unavailable(format!("{code}: {message}")),
            "root.sealed_quota" => SliceRefusal::Quota,
            "root.budget" => SliceRefusal::Budget,
            "root.store" => SliceRefusal::Store(message.to_string()),
            _ => SliceRefusal::Other(format!("{code}: {message}")),
        }
    }

    /// The verdict of a refused slice; `None` for [`SliceRefusal::SealedNone`] (run v0.3 instead).
    pub fn verdict(&self) -> Option<Verdict> {
        Some(match self {
            SliceRefusal::SealedNone => return None,
            SliceRefusal::Unavailable(_) => Verdict::Inconclusive(InconclusiveReason::SealedUnavailable),
            SliceRefusal::Quota => Verdict::Inconclusive(InconclusiveReason::SealedQuota),
            SliceRefusal::Budget => Verdict::Inconclusive(InconclusiveReason::BudgetInsufficient),
            SliceRefusal::Store(_) => Verdict::Error(ErrorReason::Infra),
            SliceRefusal::Other(_) => Verdict::Error(ErrorReason::Sealed),
        })
    }
}

/// `error:<cause>` → the verdict (m7 §3.2.5: never a candidate verdict; m7 §5: a budget or
/// window stop mid-slice is inconclusive).
pub fn slice_error_verdict(cause: &str) -> Verdict {
    match cause {
        "budget" => Verdict::Inconclusive(InconclusiveReason::Budget),
        "wall" => Verdict::Inconclusive(InconclusiveReason::Timeout),
        "harness_health" => Verdict::Error(ErrorReason::HarnessHealth),
        "checker_tamper" => Verdict::Error(ErrorReason::CheckerTamper),
        "cancelled" => Verdict::Error(ErrorReason::Cancelled),
        "infra" => Verdict::Error(ErrorReason::Infra),
        "task_cost_cap" => Verdict::Error(ErrorReason::TaskCostCap),
        c => match c.strip_prefix("runs_failed:") {
            Some(o) => Verdict::Error(ErrorReason::runs_failed(o)),
            None => Verdict::Error(ErrorReason::Sealed),
        },
    }
}

// ---------------------------------------------------------------------------------------------
// The screen's result (the runner is lane GATE-V2b's; m7 §3.2.2)
// ---------------------------------------------------------------------------------------------

/// A screen reject, in the order m7 §3.2.2 checks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenReject {
    Canary,
    Tamper,
    ScreenErrors,
    ScreenInferior,
}

impl ScreenReject {
    pub fn reason(self) -> RejectReason {
        match self {
            ScreenReject::Canary => RejectReason::Canary,
            ScreenReject::Tamper => RejectReason::Tamper,
            ScreenReject::ScreenErrors => RejectReason::ScreenErrors,
            ScreenReject::ScreenInferior => RejectReason::ScreenInferior,
        }
    }
}

/// The screen (gate canaries first, then `screen_k` pool tasks, R = 1 per arm). It can reject,
/// never accept; its data feed only the divergence guard, the tie-breaks' cost class and
/// `Predicts:` scoring. `n = 0`: no screen ran (no pool, or the runner is not wired).
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct ScreenSummary {
    /// Paired pool tasks judged.
    pub n: u32,
    pub c_pass: u32,
    pub p_pass: u32,
    /// Σ (candidate passes − incumbent passes).
    pub net: i64,
    /// Candidate run errors (c712701 rules).
    pub c_errors: u32,
    /// Any candidate run flagged tamper.
    pub tamper: bool,
    /// A gate canary scored 1000.
    pub canary_trip: bool,
    pub reject: Option<ScreenReject>,
    /// D on the screen, permille (mean per-task d).
    pub d_permille: i64,
    /// δ_pool from `noise.json` history (pass_rate@pool), permille, if known.
    pub delta_permille: Option<i64>,
    /// The cost × time class of the paired screen runs, when its δ is known.
    pub ct: Option<CtStats>,
    /// `perf` changes: a cost or wall gain beyond δ on the screen.
    pub perf_gain: Option<bool>,
    /// The candidate's median cost per screen task, µUSD (`cost_guard`'s projection).
    pub c_cost_musd: Option<u64>,
}

impl ScreenSummary {
    /// The `screen` object of the G4 record v3 (m7 §4.4) and the agent view.
    pub fn record(&self) -> Value {
        let verdict = match self.reject {
            Some(r) => r.reason().as_str().to_string(),
            None => "continue".to_string(),
        };
        json!({"n": self.n, "c_pass": self.c_pass, "p_pass": self.p_pass, "net": self.net, "verdict": verdict})
    }
}

/// δ values the screen statistics use, from `noise.json` history (the screen's R = 1 has no
/// within-gate repeats).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScreenNoise {
    pub pass_pool: Option<f64>,
    pub cost_x_wall: Option<f64>,
    pub cost_usd: Option<f64>,
    pub wall_s: Option<f64>,
}

impl ScreenNoise {
    pub fn from_history(h: &NoiseHistory, model: &str, now_unix: i64, max_age_days: u32) -> ScreenNoise {
        let d = |k: &str| h.recent(model, k, now_unix, max_age_days).and_then(|e| e.delta());
        ScreenNoise {
            pass_pool: d("pass_rate@pool").or_else(|| d("pass_rate")),
            cost_x_wall: d(Metric::CostXWall.name()),
            cost_usd: d(Metric::CostUsd.name()),
            wall_s: d(Metric::WallS.name()),
        }
    }
}

/// What [`screen_stats`] computes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScreenStats {
    pub n: u32,
    pub d_permille: i64,
    pub delta_permille: Option<i64>,
    pub ct: Option<CtStats>,
    pub perf_gain: Option<bool>,
    pub c_cost_musd: Option<u64>,
}

/// The paired statistics of the screen's runs (`set` `pool`, both arms, any repeats; canary runs
/// are ignored). c712701 rules as in the v0.3 path: a task with an incumbent run error is left
/// out; a candidate run error scores 0 at the incumbent's values. The CT class and the perf gain
/// need a δ from history; without one they are `None` (the tie-breaks then see `same`).
pub fn screen_stats(runs: &[RunRecord], noise: &ScreenNoise, ct_include_wall: bool) -> ScreenStats {
    let mut ids: Vec<&str> = runs.iter().filter(|r| r.set != "canary").map(|r| r.task.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut tasks = Vec::new();
    for id in ids {
        let mut reps: Vec<u32> = runs.iter().filter(|r| r.task == id && r.set != "canary").map(|r| r.repeat).collect();
        reps.sort_unstable();
        reps.dedup();
        let one = |a: &str, k: u32| runs.iter().find(|r| r.task == id && r.arm == a && r.repeat == k && r.set != "canary");
        if reps.iter().any(|&k| one("p", k).is_some_and(|r| run_error(r).is_some())) {
            continue;
        }
        let (mut c, mut p) = (Vec::new(), Vec::new());
        for k in reps {
            let (Some(rc), Some(rp)) = (one("c", k), one("p", k)) else { continue };
            let vp = rp.values();
            let vc = if run_error(rc).is_some() { RunValues { score_permille: 0, ..vp } } else { rc.values() };
            c.push(vc);
            p.push(vp);
        }
        if !c.is_empty() {
            tasks.push(TaskRuns { id: id.to_string(), set: TaskSet::Pool, source: Source::Maintainer, family: None, c, p });
        }
    }
    let n = tasks.len();
    if n == 0 {
        return ScreenStats::default();
    }
    let refs: Vec<&TaskRuns> = tasks.iter().collect();
    let d = tasks.iter().map(TaskRuns::d).sum::<f64>() / n as f64;
    let ctm = if ct_include_wall { Metric::CostXWall } else { Metric::CostUsd };
    let dct = if ct_include_wall { noise.cost_x_wall } else { noise.cost_usd };
    let ct = dct.map(|delta| {
        let e = effect(ctm, &refs);
        let class = if e < -delta {
            CtClass::Cheaper
        } else if e > delta {
            CtClass::Costlier
        } else {
            CtClass::Same
        };
        CtStats { metric: ctm, effect: e, delta, class }
    });
    let perf_gain = match (noise.cost_usd, noise.wall_s) {
        (Some(dc), Some(dw)) => Some(effect(Metric::CostUsd, &refs) < -dc || effect(Metric::WallS, &refs) < -dw),
        _ => None,
    };
    ScreenStats {
        n: n as u32,
        d_permille: crate::record::permille(d),
        delta_permille: noise.pass_pool.map(crate::record::permille),
        ct,
        perf_gain,
        c_cost_musd: Some(arm_stat(Metric::CostUsd, &refs, Arm::C).round() as u64),
    }
}

// ---------------------------------------------------------------------------------------------
// Accept rule v2 (m7 §3.2.5), S path
// ---------------------------------------------------------------------------------------------

/// Inputs of [`accept_v2`] besides the screen and the slice.
#[derive(Debug, Clone, PartialEq)]
pub struct V2Inputs {
    pub change_type: ChangeType,
    /// A pre-registered improvement on S (`Predicts: pass_rate@sealed >= parent+N%`, N > 0).
    pub predicts_sealed_gain: bool,
    pub body_tokens_c: u64,
    pub body_tokens_p: u64,
    /// `gate.g4.rho_max` (`cost_guard`).
    pub rho_max: f64,
    pub budget: crate::accept::CostBudget,
}

/// Which accept class (orchestrator 2026-10-20: the proof sprint counts only `better` and
/// `lineage_gain` as "sticks", so a tie-break accept is tagged apart).
pub fn accept_class(v: &Verdict) -> Option<String> {
    match v {
        Verdict::Accept(AcceptReason::Better) => Some("better".into()),
        Verdict::Accept(AcceptReason::LineageGain) => Some("lineage_gain".into()),
        Verdict::Accept(AcceptReason::CapabilityGain) => Some("capability_gain".into()),
        Verdict::Accept(r) => Some(format!("tie_break:{}", r.as_str())),
        _ => None,
    }
}

/// [`accept_v2`]'s result.
#[derive(Debug, Clone, PartialEq)]
pub struct V2Decision {
    pub verdict: Verdict,
    /// A `lineage_gain` accept (m7 §3.2.13).
    pub milestone_required: bool,
    /// The divergence guard's inputs `(d_screen − d_S, bound)`, when it was evaluated.
    pub divergence: Option<(f64, f64)>,
}

fn class_of(screen: &ScreenSummary) -> CtClass {
    screen.ct.map_or(CtClass::Same, |c| c.class)
}

/// gate.md §3.9's tie-breaks on a `tie` (spec §6.3). `repro_fixed` needs repro runs, which the lean
/// S path does not run, so it never fires here.
fn tie_breaks(screen: &ScreenSummary, x: &V2Inputs) -> Verdict {
    match class_of(screen) {
        CtClass::Cheaper => Verdict::Accept(AcceptReason::Cheaper),
        CtClass::Costlier => Verdict::Reject(RejectReason::Costlier),
        CtClass::Same if x.body_tokens_c < x.body_tokens_p => Verdict::Accept(AcceptReason::Smaller),
        CtClass::Same => Verdict::Reject(RejectReason::NoGain),
    }
}

/// gate.md §3.9's `cost_guard` with the screen's cost × time effect: dearer by more than ρ_max is
/// accepted only if the day still fits at the candidate's cost. Without a screen CT the cost check
/// rests on the slice's cost per pass alone (≤ RHO_PASS, checked before).
fn cost_guard(screen: &ScreenSummary, x: &V2Inputs, reason: AcceptReason) -> Verdict {
    let Some(ct) = screen.ct else { return Verdict::Accept(reason) };
    if ct.effect.exp() <= x.rho_max {
        return Verdict::Accept(reason);
    }
    let projected = x.budget.tasks_per_day.max(1.0) * screen.c_cost_musd.unwrap_or(0) as f64;
    let task_part = x.budget.soft_day_musd as f64 * (1000.0 - x.budget.evolution_share_permille as f64) / 1000.0;
    if projected <= task_part { Verdict::Accept(reason) } else { Verdict::Reject(RejectReason::CostlierThanBudget) }
}

/// The screen's own verdict, before any slice (m7 §3.2.1: a screen reject spends no S look): a gate
/// canary at 1000, then the screen's reject.
pub fn screen_verdict(screen: &ScreenSummary) -> Option<Verdict> {
    if screen.canary_trip {
        return Some(Verdict::Reject(RejectReason::Canary));
    }
    screen.reject.map(|r| Verdict::Reject(r.reason()))
}

/// The accept rule v2 on the S path (m7 §3.2.5; kernel/gate.md §3.9 M7 notes), lean: no capability
/// path (D-L6). The ratchet fires only with the Root's `ratchet_signal`, `unconfirmed` and
/// `ratchet_k` (ROOT-S1a sends all three).
pub fn accept_v2(screen: &ScreenSummary, s: &SliceAnswer, x: &V2Inputs) -> V2Decision {
    let done = |v: Verdict| V2Decision { verdict: v, milestone_required: false, divergence: None };
    if let Some(v) = screen_verdict(screen) {
        return done(v);
    }
    let Some(status) = s.parsed() else {
        return done(Verdict::Error(ErrorReason::Sealed));
    };
    match &status {
        SliceStatus::Canary => return done(Verdict::Reject(RejectReason::Canary)),
        SliceStatus::Tamper => return done(Verdict::Reject(RejectReason::Tamper)),
        SliceStatus::Leak => return done(Verdict::Reject(RejectReason::Leak)),
        SliceStatus::Health => return done(Verdict::Reject(RejectReason::Health)),
        SliceStatus::Error(c) => return done(slice_error_verdict(c)),
        SliceStatus::Inferior => return done(Verdict::Reject(RejectReason::ScoreInferior)),
        SliceStatus::InferiorEarly => return done(Verdict::Reject(RejectReason::ScoreInferiorEarly)),
        _ => {}
    }
    let (d_s, m_s, cpp) = (
        s.d_permille.unwrap_or(0) as f64 / 1000.0,
        s.m_permille.unwrap_or(0) as f64 / 1000.0,
        s.cpp_ratio_permille.unwrap_or(i64::MAX),
    );
    // The divergence guard (gate.md §3.9, D14 floors): the screen as the pool split, the slice as the
    // holdout split. δ_S comes back from m = max(2·δ_S, 0.5/n), so δ_S ≥ m/2 is conservative only
    // when m is at its floor; the floor 0.5/n then applies anyway.
    let mut divergence = None;
    if screen.n > 0 {
        let d_screen = screen.d_permille as f64 / 1000.0;
        let ds = screen.delta_permille.map_or(0.0, |d| d as f64 / 1000.0).max(delta_min(screen.n as usize));
        let dh = (m_s / 2.0).max(delta_min(s.n as usize));
        let bound = 2.0 * (ds * ds + dh * dh).sqrt();
        divergence = Some((d_screen - d_s, bound));
        if d_screen - d_s > bound {
            return V2Decision { verdict: Verdict::Reject(RejectReason::PoolHoldoutDivergence), milestone_required: false, divergence };
        }
    }
    let out = |v: Verdict, milestone_required: bool| V2Decision { verdict: v, milestone_required, divergence };
    if x.change_type == ChangeType::Perf {
        let ok = matches!(status, SliceStatus::Tie | SliceStatus::Superior) && screen.perf_gain == Some(true);
        return out(if ok { Verdict::Accept(AcceptReason::PerfGain) } else { Verdict::Reject(RejectReason::PerfNoGain) }, false);
    }
    if status == SliceStatus::Superior {
        if cpp > RHO_PASS_PERMILLE {
            return out(Verdict::Reject(RejectReason::CostlierPerPass), false);
        }
        return out(cost_guard(screen, x, AcceptReason::Better), false);
    }
    if status == SliceStatus::Tie
        && s.ratchet_signal == Some(true)
        && x.predicts_sealed_gain
        && class_of(screen) != CtClass::Costlier
        && cpp <= RHO_RATCHET_PERMILLE
        && let (Some(unconfirmed), Some(k)) = (s.unconfirmed, s.ratchet_k)
    {
        return if unconfirmed < k {
            out(Verdict::Accept(AcceptReason::LineageGain), true)
        } else {
            out(Verdict::Reject(RejectReason::RatchetFull), false)
        };
    }
    let v = match status {
        SliceStatus::TieNegative => Verdict::Reject(RejectReason::NoGain),
        SliceStatus::Underpowered => match tie_breaks(screen, x) {
            Verdict::Accept(_) => Verdict::Inconclusive(InconclusiveReason::Underpowered),
            _ => Verdict::Reject(RejectReason::NoGain),
        },
        _ => tie_breaks(screen, x),
    };
    out(v, false)
}

// ---------------------------------------------------------------------------------------------
// G4 record v3 (m7 §4.4)
// ---------------------------------------------------------------------------------------------

/// The G4 v2 part of a [`crate::g4::G4Outcome`]: what the record v3 adds.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct V2Record {
    pub tier: Option<Tier>,
    pub preflight: Option<Preflight>,
    pub screen: Option<ScreenSummary>,
    /// The Root's answer; `None` when the slice did not run.
    pub sealed: Option<SliceAnswer>,
    /// Why the slice did not run (a refusal), agent-safe.
    pub sealed_refusal: Option<String>,
    pub milestone_required: bool,
    pub accept_class: Option<String>,
    pub divergence: Option<(f64, f64)>,
}

impl V2Record {
    /// Add the v3 keys to a `stages.G4` object: `tier`, `screen`, `sealed` (agent view; null when
    /// no slice ran), `cpp`, `milestone_required`, `accept_class`. The slice's spend joins the
    /// stage's `spent_musd` (the Root charged it to the agent pool).
    pub fn extend(&self, v: &mut Value) {
        let pm = crate::record::permille;
        v["tier"] = self.tier.map_or(Value::Null, |t| json!(t.as_str()));
        if let Some(p) = &self.preflight {
            v["preflight"] = json!({"runs": p.runs, "estimate_musd": p.estimate_musd});
        }
        v["screen"] = self.screen.as_ref().map_or(Value::Null, ScreenSummary::record);
        v["sealed"] = self.sealed.as_ref().map_or(Value::Null, SliceAnswer::agent_view);
        if let Some(r) = &self.sealed_refusal {
            v["sealed_refusal"] = json!(r);
        }
        v["cpp"] = json!({"ratio_permille": self.sealed.as_ref().and_then(|s| s.cpp_ratio_permille).map(|c| round_to(c, 50))});
        v["milestone_required"] = json!(self.milestone_required);
        v["accept_class"] = self.accept_class.as_ref().map_or(Value::Null, |c| json!(c));
        if let Some((gap, bound)) = self.divergence {
            v["divergence"] = json!({"gap_permille": pm(gap), "bound_permille": pm(bound)});
        }
        if let Some(s) = &self.sealed {
            let spent = v["spent_musd"].as_u64().unwrap_or(0) + s.spent_musd;
            v["spent_musd"] = json!(spent);
        }
    }

    /// The agent-visible aggregate for `initctl status` (`evolution.g4_last`): no ids, no
    /// per-task values (m7 §3.2.11).
    pub fn status_view(&self, gate_id: &str, verdict: &Verdict) -> Value {
        json!({
            "gate_id": gate_id, "verdict": verdict.kind(), "reason": verdict.reason(),
            "accept_class": self.accept_class, "tier": self.tier.map(Tier::as_str),
            "sealed": self.sealed.as_ref().map(SliceAnswer::agent_view),
            "screen": self.screen.as_ref().map(ScreenSummary::record),
            "milestone_required": self.milestone_required,
        })
    }
}

#[cfg(test)]
mod tests;
