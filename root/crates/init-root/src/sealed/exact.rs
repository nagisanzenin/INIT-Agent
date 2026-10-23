//! G4 v2 statistics (lane ROOT-S1b; docs/m7-design.md §3.2.4–§3.2.7, §3.2.10, §3.2.13; kernel/gate.md §3.8.3
//! "G4 v2 superiority"). Pure: no I/O, no clock, no `/dev/urandom`; every result is a function of its
//! arguments. The exact sign-flip test, the margin and every decision boundary are integers or exact dyadic
//! rationals; only the bootstrap NI bound, the noise floor and the analytic MDE are f64, as gate.md §3.8.3 says.
//!
//! Units. A task's paired difference is `k_i = Σ candidate scores − Σ incumbent scores` over its R runs per arm,
//! in a per-run `unit`: 1 when scores are passes (0/1), 1000 when they are permille (partial credit). A full
//! task-equivalent is `repeats · unit`, so `d_i = k_i / (repeats · unit)` and `D = Σ k_i / (n · repeats · unit)`.
//! The Ladder margin `D ≥ 1/n` is then exactly `Σ k_i ≥ repeats · unit`.
#![allow(dead_code)] // the callers (g4_slice, calibrate, the milestone check) land with lane ROOT-S1a

use super::stats::{Rng, lower, noise, seed};
use serde_json::{Value, json};

/// m7 §4.3 constants (not charter keys), in permille so that every comparison is exact.
pub const ALPHA_GAIN_PERMILLE: u32 = 50;
pub const ALPHA_RATCHET_PERMILLE: u32 = 250;
pub const ALPHA_MILESTONE_PERMILLE: u32 = 50;
pub const RHO_PASS_PERMILLE: u64 = 1500;
pub const RHO_RATCHET_PERMILLE: u64 = 1200;
/// The largest number of discordant tasks the exact test takes (u128 counts with 10 bits of headroom). Beyond
/// it `p_sup`/`p_inf` return 1, which never claims anything; every design n is ≤ 40.
pub const MAX_N: usize = 100;
/// m7 §3.2.2 and §3.2.6: screen tasks per arm, and canary runs per G4 (3 Root canaries on the candidate,
/// `health` on both arms, the gate's own 2).
pub const SCREEN_K: u32 = 12;
pub const CANARY_RUNS: u32 = 7;
pub const SENTINEL_PERMILLE: u64 = 200;
/// Pseudo-A/A splits and δ splits (m7 §3.2.10), and the bootstrap size of the futility look (gate.md §3.8.4).
pub const CALIBRATE_B: usize = 2000;
const FUTILITY_B: usize = 10_000;
/// Normal quantiles for the analytic MDE: the exact test's **attained** size, about 3.5% at the design n (it
/// is discrete: 3.1–3.8% exactly under m7 §3.2.6's task model, see the tests), not the nominal 5%; and 80% power.
/// With the nominal z₀.₉₅ the MDE read 1–2 points optimistic against the exact power.
const Z_SIZE: f64 = 1.812;
const Z80: f64 = 0.841_621_233_572_914_3;

/// An exact p-value `num / 2^log2_den`, kept in lowest terms (so `==` compares values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PValue {
    pub num: u128,
    pub log2_den: u32,
}

impl PValue {
    pub const ONE: PValue = PValue { num: 1, log2_den: 0 };

    fn new(mut num: u128, mut log2_den: u32) -> PValue {
        if num == 0 {
            return PValue { num: 0, log2_den: 0 };
        }
        while num & 1 == 0 && log2_den > 0 {
            num >>= 1;
            log2_den -= 1;
        }
        PValue { num, log2_den }
    }
    /// Exact `p ≤ alpha_permille / 1000`.
    pub fn le_permille(self, alpha_permille: u32) -> bool {
        self.num * 1000 <= u128::from(alpha_permille) << self.log2_den
    }
    pub fn f64(self) -> f64 {
        self.num as f64 / (self.log2_den as f64).exp2()
    }
    /// `⌈1000 · p⌉`, for the owner's full detail (never for the agent: m7 §3.2.11).
    pub fn permille_ceil(self) -> u32 {
        let den = 1u128 << self.log2_den;
        (self.num * 1000).div_ceil(den) as u32
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn ceil_div(a: i64, b: i64) -> i64 {
    -((-a).div_euclid(b))
}

/// `P(Σ ε_i·|k_i| ≥ t)`, ε_i iid uniform on {−1, +1}: the exact DP over integer sums of m7 §3.2.4, as a
/// subset-sum count. Zeros and a common factor of the |k_i| change nothing, so both are taken out first.
fn tail(k: &[i64], t: i64) -> PValue {
    let a: Vec<u64> = k.iter().filter(|x| **x != 0).map(|x| x.unsigned_abs()).collect();
    if a.len() > MAX_N {
        return PValue::ONE;
    }
    let g = a.iter().fold(0, |g, x| gcd(g, *x));
    if g == 0 {
        return if t <= 0 { PValue::ONE } else { PValue::new(0, 0) };
    }
    let b: Vec<usize> = a.iter().map(|x| (x / g) as usize).collect();
    let total: usize = b.iter().sum();
    // Σ ε b = 2·S⁺ − total ≥ ⌈t/g⌉  ⇔  S⁺ ≥ ⌈(⌈t/g⌉ + total) / 2⌉, S⁺ the sum of the + terms
    let s0 = ceil_div(ceil_div(t, g as i64) + total as i64, 2);
    if s0 <= 0 {
        return PValue::ONE;
    }
    if s0 as usize > total {
        return PValue::new(0, 0);
    }
    let mut cnt = vec![0u128; total + 1];
    cnt[0] = 1;
    let mut top = 0;
    for &x in &b {
        top += x;
        for s in (x..=top).rev() {
            cnt[s] += cnt[s - x];
        }
    }
    PValue::new(cnt[s0 as usize..].iter().sum(), b.len() as u32)
}

/// One-sided superiority p-value `P(Σ ε_i·|k_i| ≥ Σ k_i)` (m7 §3.2.4). Exact, deterministic, order-free.
pub fn p_sup(k: &[i64]) -> PValue {
    tail(k, k.iter().sum())
}

/// The other direction, `P(Σ ε_i·|k_i| ≤ Σ k_i)` = `p_sup` of −k (the milestone's regression test, §3.2.13).
pub fn p_inf(k: &[i64]) -> PValue {
    tail(k, -k.iter().sum::<i64>())
}

/// Per-task `k_i` from run scores in permille (0..=1000), with **equal R** on both arms and every task (m7
/// §3.2.7: with unequal R the test is no longer exact). Returns `(k, R)`; use `unit = 1000`.
pub fn diffs(cand: &[Vec<i64>], inc: &[Vec<i64>]) -> Result<(Vec<i64>, u32), String> {
    if cand.len() != inc.len() {
        return Err(format!("{} candidate tasks against {} incumbent tasks", cand.len(), inc.len()));
    }
    let r = cand.first().map_or(0, Vec::len);
    if r == 0 || cand.iter().chain(inc).any(|v| v.len() != r) {
        return Err("every task needs the same R ≥ 1 on both arms".into());
    }
    if cand.iter().chain(inc).flatten().any(|s| !(0..=1000).contains(s)) {
        return Err("scores are permille, 0..=1000".into());
    }
    Ok((cand.iter().zip(inc).map(|(c, p)| c.iter().sum::<i64>() - p.iter().sum::<i64>()).collect(), r as u32))
}

/// `d_i = k_i / (repeats · unit)` as f64, the input of `stats::lower` (the bootstrap NI, gate.md §3.8.3).
pub fn d_ratios(k: &[i64], repeats: u32, unit: i64) -> Vec<f64> {
    let full = full(repeats, unit) as f64;
    k.iter().map(|x| *x as f64 / full).collect()
}

/// gate.md §3.8.3: `m = max(2·δ, 0.5/n)`.
pub fn margin(delta: f64, n: usize) -> f64 {
    (2.0 * delta).max(0.5 / n.max(1) as f64)
}

fn full(repeats: u32, unit: i64) -> i64 {
    (i64::from(repeats) * unit).max(1)
}

/// `round(num / den)`, half away from zero.
fn div_round(num: i128, den: i128) -> i64 {
    let (n, d) = (num.abs(), den.abs());
    let q = ((2 * n + d) / (2 * d)) as i64;
    if (num < 0) != (den < 0) { -q } else { q }
}

fn pm(x: f64) -> i64 {
    (x * 1000.0).round() as i64
}

/// `x` rounded to the nearest multiple of `step`, half away from zero. The agent-visible slice fields use it
/// (m7 §3.2.11): d to 50‰, m to 10‰, cpp ratio to 50‰.
pub fn round_to(x: i64, step: i64) -> i64 {
    div_round(i128::from(x), i128::from(step.max(1))) * step.max(1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Superior,
    Inferior,
    Underpowered,
    Tie,
    TieNegative,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Superior => "superior",
            Status::Inferior => "inferior",
            Status::Underpowered => "underpowered",
            Status::Tie => "tie",
            Status::TieNegative => "tie_negative",
        }
    }
}

/// A slice's statistics (m7 §3.2.4). `d_permille`, `m_permille`, `l_permille` are unrounded (round with
/// `round_to` for the agent-visible record); `p_sup` is for the owner's detail only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decision {
    pub status: Status,
    pub p_sup: PValue,
    /// `p_sup ≤ ALPHA_RATCHET ∧ D ≥ 1/n` (m7 §3.2.13 stage 1); the kernel's accept rule adds the rest.
    pub ratchet_signal: bool,
    pub n: usize,
    /// `Σ k_i`, in `unit`s.
    pub total: i64,
    pub d: f64,
    pub d_permille: i64,
    pub m_permille: i64,
    pub l_permille: i64,
}

/// m7 §3.2.4's status, in order: `superior` (p_sup ≤ 0.05 ∧ D ≥ 1/n); else `inferior` (L ≤ −m); else
/// `underpowered` (m > margin_max); else `tie` (D ≥ 0); else `tie_negative`. `l_ni` is `stats::lower` on
/// `d_ratios(k)`, `margin` is `margin(δ_S, n)`.
pub fn slice_status(k: &[i64], repeats: u32, unit: i64, l_ni: f64, margin: f64, margin_max: f64) -> Decision {
    let (n, total, full) = (k.len(), k.iter().sum::<i64>(), full(repeats, unit));
    let p = p_sup(k);
    let gain = n > 0 && total >= full;
    let status = if gain && p.le_permille(ALPHA_GAIN_PERMILLE) {
        Status::Superior
    } else if l_ni <= -margin {
        Status::Inferior
    } else if margin > margin_max {
        Status::Underpowered
    } else if total >= 0 {
        Status::Tie
    } else {
        Status::TieNegative
    };
    let den = (n.max(1) as i64 * full) as i128;
    Decision {
        status,
        p_sup: p,
        ratchet_signal: gain && p.le_permille(ALPHA_RATCHET_PERMILLE),
        n,
        total,
        d: total as f64 / den as f64,
        d_permille: div_round(1000 * i128::from(total), den),
        m_permille: pm(margin),
        l_permille: pm(l_ni),
    }
}

/// gate.md §3.8.4's futility look over the first ⌈n/2⌉ tasks: with δ from history only (none → no look),
/// `m = max(2·δ_hist, 0.5/n_look)` and `U99` the bootstrap's 99% upper bound (seed purpose `futility`), stop
/// (`inferior_early`) iff `U99 < −m`.
pub fn futility(k_look: &[i64], repeats: u32, unit: i64, delta_hist: Option<f64>, gate_id: &str) -> bool {
    let Some(dh) = delta_hist else { return false };
    if k_look.is_empty() {
        return false;
    }
    let d = d_ratios(k_look, repeats, unit);
    let mut rng = Rng(seed(gate_id, "futility"));
    let mut v: Vec<f64> =
        (0..FUTILITY_B).map(|_| (0..d.len()).map(|_| d[rng.idx(d.len())]).sum::<f64>() / d.len() as f64).collect();
    v.sort_by(f64::total_cmp);
    v[(0.99 * FUTILITY_B as f64).ceil() as usize - 1] < -margin(dh, k_look.len())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Confirmed,
    NotConfirmed,
    Regression,
    /// Set by the driver when a check cannot finish (m7 §5); `milestone` never returns it.
    Partial,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Confirmed => "confirmed",
            Outcome::NotConfirmed => "not_confirmed",
            Outcome::Regression => "regression",
            Outcome::Partial => "partial",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Milestone {
    pub outcome: Outcome,
    pub p_sup: PValue,
    pub p_inf: PValue,
    pub n: usize,
    pub total: i64,
    pub d_permille: i64,
}

/// m7 §3.2.13 stage 2, current against `confirmed` on the reserve: `confirmed` ⇔ p_sup ≤ 0.05 ∧ D ≥ 1/n;
/// else `regression` ⇔ p_inf ≤ 0.05, or the bootstrap NI fails (L ≤ −m) with m ≤ margin_max; else
/// `not_confirmed`. The table applies no 1/n margin on the regression side, and neither does this.
pub fn milestone(k: &[i64], repeats: u32, unit: i64, l_ni: f64, margin: f64, margin_max: f64) -> Milestone {
    let (n, total) = (k.len(), k.iter().sum::<i64>());
    let (ps, pi) = (p_sup(k), p_inf(k));
    let outcome = if n > 0 && total >= full(repeats, unit) && ps.le_permille(ALPHA_MILESTONE_PERMILLE) {
        Outcome::Confirmed
    } else if n > 0 && (pi.le_permille(ALPHA_MILESTONE_PERMILLE) || (margin <= margin_max && l_ni <= -margin)) {
        Outcome::Regression
    } else {
        Outcome::NotConfirmed
    };
    let den = (n.max(1) as i64 * full(repeats, unit)) as i128;
    Milestone { outcome, p_sup: ps, p_inf: pi, n, total, d_permille: div_round(1000 * i128::from(total), den) }
}

/// `ratchet_k` for the next span (m7 §3.2.13): reset to the charter value on `confirmed`, halved (at least 1)
/// on `not_confirmed` and on `regression` (the design leaves regression open; it is at least as bad), and
/// unchanged on `partial`.
pub fn ratchet_k_next(outcome: Outcome, k: u32, charter_k: u32) -> u32 {
    match outcome {
        Outcome::Confirmed => charter_k,
        Outcome::NotConfirmed | Outcome::Regression => (k / 2).max(1),
        Outcome::Partial => k,
    }
}

/// Cost per pass ratio cpp(c)/cpp(p) in permille, cpp(a) = Σ cost_ref / max(1, Σ passes) (m7 §3.2.4).
/// `better` needs ≤ RHO_PASS_PERMILLE, `lineage_gain` ≤ RHO_RATCHET_PERMILLE. A free incumbent gives 1000 against
/// a free candidate and `u64::MAX` otherwise.
pub fn cpp_ratio_permille(c_cost_musd: u64, c_passes: u64, p_cost_musd: u64, p_passes: u64) -> u64 {
    if p_cost_musd == 0 {
        return if c_cost_musd == 0 { 1000 } else { u64::MAX };
    }
    let num = 1000 * u128::from(c_cost_musd) * u128::from(p_passes.max(1));
    let den = u128::from(c_passes.max(1)) * u128::from(p_cost_musd);
    u64::try_from((2 * num + den) / (2 * den)).unwrap_or(u64::MAX)
}

/// m7 §3.2.7's sentinel: the cache is invalid when the fresh R = 1 pass count differs from Σ p̂ (the cached
/// per-task pass rates of the sentinel tasks, permille) by more than 2.5·max(1, √Σ p̂(1 − p̂)). Exact integers.
pub fn sentinel_drift(observed_passes: u64, cached_rates_permille: &[u32]) -> bool {
    let e: i128 = cached_rates_permille.iter().map(|r| i128::from(*r)).sum();
    let v: i128 = cached_rates_permille.iter().map(|r| i128::from(*r) * (1000 - i128::from((*r).min(1000)))).sum();
    let diff = i128::from(observed_passes) * 1000 - e;
    4 * diff * diff > 25 * v.max(1_000_000)
}

/// m7 §3.1.6: a task is in band when the incumbent neither always passes nor always fails it (R = 6 at
/// calibration, R = 3 for replacements, R = 2 for template probes).
pub fn in_band(passes: u32, runs: u32) -> bool {
    passes > 0 && passes < runs
}

/// m7 §3.1.6's template knob: −1 (one step easier) below a 20% pass rate, +1 (harder) above 80%, else 0.
pub fn knob_step(rate_permille: u32) -> i8 {
    if rate_permille < 200 { -1 } else if rate_permille > 800 { 1 } else { 0 }
}

/// A G4 v2 tier (m7 §3.2.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tier {
    pub name: &'static str,
    pub n: usize,
    pub repeats: u32,
}

pub const LEAN: Tier = Tier { name: "lean", n: 24, repeats: 2 };
pub const STANDARD: Tier = Tier { name: "standard", n: 32, repeats: 3 };
pub const STRONG: Tier = Tier { name: "strong", n: 32, repeats: 5 };
pub const TIERS: [Tier; 3] = [LEAN, STANDARD, STRONG];

pub fn tier(name: &str) -> Option<Tier> {
    TIERS.into_iter().find(|t| t.name == name)
}

impl Tier {
    /// Runs per G4 including the screen and the 7 canary runs: fresh, both arms everywhere; with reuse, the
    /// candidate arm plus an R = 1 incumbent sentinel on ⌈20%·n⌉ tasks and a cached screen incumbent.
    pub fn runs(self, reuse: bool) -> u32 {
        let arm = self.n as u32 * self.repeats;
        let sentinel = (self.n as u64 * SENTINEL_PERMILLE).div_ceil(1000) as u32;
        if reuse { arm + sentinel + SCREEN_K + CANARY_RUNS } else { 2 * arm + 2 * SCREEN_K + CANARY_RUNS }
    }
    pub fn cost_musd(self, reuse: bool, per_run_musd: u64) -> u64 {
        u64::from(self.runs(reuse)) * per_run_musd
    }
    /// The analytic MDE for 80% power of the superiority test on a uniform gain (m7 §3.2.10 step 4):
    /// `(z_size + z₀.₈₀)·√(2·v_w / (R·n))`, never below the margin 1/n; permille, rounded up. Within about a
    /// point of the exact 80%-power gain (tests).
    pub fn mde_permille(self, var_within_permille: u32) -> u32 {
        let se = (2.0 * f64::from(var_within_permille) / 1000.0 / (f64::from(self.repeats) * self.n as f64)).sqrt();
        ((Z_SIZE + Z80) * se * 1000.0).ceil().max((1000.0 / self.n as f64).ceil()) as u32
    }
}

/// Expected false `better` accepts per epoch, in thousandths: looks × P(`better` | null) (m7 §3.1.7: 20 looks
/// at 3.6% → 720, about 0.7).
pub fn expected_false_accepts_milli(looks: u32, fa_permille: u32) -> u32 {
    looks * fa_permille
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalibVerdict {
    Pass,
    /// A pass criterion failed (FA > 50‰ or its upper bound > 70‰, FR > 100‰, or m > 250‰ at standard).
    Fail,
    /// FA above 70‰: the statistics or the harness are broken; stop all spend (m7 §3.2.10).
    StopSpend,
}

/// The A/A calibration's statistics (m7 §3.2.10 steps 3–5). `delta_r*` are f64 for `sealed/noise.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct Calib {
    pub n_kept: usize,
    pub splits: usize,
    pub fa_permille: u32,
    pub fa_lo_permille: u32,
    pub fa_hi_permille: u32,
    pub fr_permille: u32,
    pub fr_lo_permille: u32,
    pub fr_hi_permille: u32,
    pub delta_r2: f64,
    pub delta_r3: f64,
    pub delta_r5: f64,
    pub var_between_permille: u32,
    pub var_within_permille: u32,
    pub mde_lean_permille: u32,
    pub mde_standard_permille: u32,
    pub mde_strong_permille: u32,
    /// m = max(2·δ_r3, 0.5/n_kept), the standard tier's margin.
    pub m_standard_permille: u32,
    pub verdict: CalibVerdict,
}

impl Calib {
    /// The statistics part of the `sealed_calibrate` record (m7 §4.2); the driver adds `s_version`,
    /// `selected` and `spent_musd`.
    pub fn json(&self) -> Value {
        let p = |x: f64| (x * 1000.0).round() as i64;
        json!({"n_kept": self.n_kept, "fa_permille": self.fa_permille, "fr_permille": self.fr_permille,
            "delta_permille": {"r2": p(self.delta_r2), "r3": p(self.delta_r3), "r5": p(self.delta_r5)},
            "mde_permille": {"lean": self.mde_lean_permille, "standard": self.mde_standard_permille,
                "strong": self.mde_strong_permille},
            "var_between_permille": self.var_between_permille, "var_within_permille": self.var_within_permille})
    }
}

/// The A/A calibration on the kept tasks' incumbent runs (`runs[i]`: task i's scores in permille, the same
/// R ≥ 6 for every task; m7 §3.2.10):
/// - pseudo-A/A: `CALIBRATE_B` seeded splits of each task's runs into 3 + 3, each judged by `slice_status`
///   exactly, δ from the split's own "incumbent" half (`stats::noise`, unsmoothed), L from `stats::lower`;
///   FA = share `superior`, FR = share `inferior`, with Clopper–Pearson 95% intervals;
/// - δ_S for R = 2 and 3 from seeded 2 + 2 and 3 + 3 splits (one-sided 95% quantile of D), R = 5 as δ₃·√(3/5);
/// - variance components: mean within-task variance (R/(R − 1) corrected) and the between-task variance of
///   the task means less within/R; the per-tier analytic MDE; the pass criteria.
pub fn calibrate(runs: &[Vec<i64>], seed_id: &str, margin_max: f64) -> Result<Calib, String> {
    calibrate_b(runs, seed_id, margin_max, CALIBRATE_B)
}

/// `calibrate` with `b` splits (tests use fewer; the Root always uses `CALIBRATE_B`).
pub fn calibrate_b(runs: &[Vec<i64>], seed_id: &str, margin_max: f64, b: usize) -> Result<Calib, String> {
    let r = runs.first().map_or(0, Vec::len);
    if r < 6 || runs.iter().any(|v| v.len() != r) {
        return Err("calibrate needs the same R ≥ 6 on every kept task".into());
    }
    if runs.iter().flatten().any(|s| !(0..=1000).contains(s)) {
        return Err("scores are permille, 0..=1000".into());
    }
    let (n, b) = (runs.len(), b.max(1));
    let mut rng = Rng(seed(seed_id, "calibrate:aa"));
    let (mut fa, mut fr) = (0u64, 0u64);
    for j in 0..b {
        let (k, inc) = aa_split(&mut rng, runs);
        let id = format!("{seed_id}/aa{j}");
        let m = margin(noise(&inc, &id).unwrap_or(0.0), n);
        let s = slice_status(&k, 3, 1000, lower(&d_ratios(&k, 3, 1000), &id, "ni"), m, margin_max).status;
        fa += u64::from(s == Status::Superior);
        fr += u64::from(s == Status::Inferior);
    }
    let delta = |rr: usize| {
        let mut rng = Rng(seed(seed_id, &format!("calibrate:delta:r{rr}")));
        let mut v: Vec<f64> = (0..CALIBRATE_B)
            .map(|_| {
                let s: i64 = runs
                    .iter()
                    .map(|t| {
                        let mut p: Vec<usize> = (0..r).collect();
                        rng.shuffle(&mut p);
                        p[..rr].iter().map(|&x| t[x]).sum::<i64>() - p[rr..2 * rr].iter().map(|&x| t[x]).sum::<i64>()
                    })
                    .sum();
                s as f64 / (1000 * rr * n) as f64
            })
            .collect();
        v.sort_by(f64::total_cmp);
        v[(0.95 * CALIBRATE_B as f64).ceil() as usize - 1]
    };
    let (d2, d3) = (delta(2), delta(3));
    let means: Vec<f64> = runs.iter().map(|t| t.iter().sum::<i64>() as f64 / (1000 * r) as f64).collect();
    let within = runs
        .iter()
        .zip(&means)
        .map(|(t, m)| t.iter().map(|x| (*x as f64 / 1000.0 - m).powi(2)).sum::<f64>() / (r - 1) as f64)
        .sum::<f64>()
        / n as f64;
    let grand = means.iter().sum::<f64>() / n as f64;
    let between_raw = if n > 1 { means.iter().map(|m| (m - grand).powi(2)).sum::<f64>() / (n - 1) as f64 } else { 0.0 };
    let vw = (within * 1000.0).round() as u32;
    let rate = |x: u64| div_round(1000 * i128::from(x), b as i128) as u32;
    let ((fa_lo, fa_hi), (fr_lo, fr_hi)) = (clopper_pearson_permille(fa, b as u64), clopper_pearson_permille(fr, b as u64));
    let m_std = pm(margin(d3, n)) as u32;
    let (fa_pm, fr_pm) = (rate(fa), rate(fr));
    let verdict = if fa_pm > 70 {
        CalibVerdict::StopSpend
    } else if fa_pm > 50 || fa_hi > 70 || fr_pm > 100 || m_std > 250 {
        CalibVerdict::Fail
    } else {
        CalibVerdict::Pass
    };
    Ok(Calib {
        n_kept: n,
        splits: b,
        fa_permille: fa_pm,
        fa_lo_permille: fa_lo,
        fa_hi_permille: fa_hi,
        fr_permille: fr_pm,
        fr_lo_permille: fr_lo,
        fr_hi_permille: fr_hi,
        delta_r2: d2,
        delta_r3: d3,
        delta_r5: d3 * (3.0f64 / 5.0).sqrt(),
        var_between_permille: ((between_raw - within / r as f64).max(0.0) * 1000.0).round() as u32,
        var_within_permille: vw,
        mde_lean_permille: LEAN.mde_permille(vw),
        mde_standard_permille: STANDARD.mde_permille(vw),
        mde_strong_permille: STRONG.mde_permille(vw),
        m_standard_permille: m_std,
        verdict,
    })
}

/// One pseudo-A/A split: each task's runs shuffled, the first 3 the "candidate", the next 3 the "incumbent".
/// Returns the split's `k` (unit 1000, R = 3) and the incumbent halves as fractions for `stats::noise`.
fn aa_split(rng: &mut Rng, runs: &[Vec<i64>]) -> (Vec<i64>, Vec<Vec<f64>>) {
    runs.iter()
        .map(|t| {
            let mut p: Vec<usize> = (0..t.len()).collect();
            rng.shuffle(&mut p);
            let k = p[..3].iter().map(|&x| t[x]).sum::<i64>() - p[3..6].iter().map(|&x| t[x]).sum::<i64>();
            (k, p[3..6].iter().map(|&x| t[x] as f64 / 1000.0).collect())
        })
        .unzip()
}

/// `P(X ≤ x)` for X ~ Bin(n, p), summed in log space (no underflow at n = 2,000).
fn binom_cdf(x: u64, n: u64, p: f64) -> f64 {
    if p <= 0.0 {
        return 1.0;
    }
    if p >= 1.0 {
        return if x >= n { 1.0 } else { 0.0 };
    }
    let (lp, lq) = (p.ln(), (1.0 - p).ln());
    let mut l = n as f64 * lq;
    let mut s = l.exp();
    for j in 0..x.min(n) {
        l += ((n - j) as f64).ln() - ((j + 1) as f64).ln() + lp - lq;
        s += l.exp();
    }
    s.min(1.0)
}

/// The exact (Clopper–Pearson) two-sided 95% interval for x successes in n, in permille: lower bound rounded
/// down, upper bound rounded up.
pub fn clopper_pearson_permille(x: u64, n: u64) -> (u32, u32) {
    if n == 0 {
        return (0, 1000);
    }
    let solve = |f: &dyn Fn(f64) -> bool| {
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        for _ in 0..100 {
            let mid = 0.5 * (lo + hi);
            if f(mid) { hi = mid } else { lo = mid }
        }
        hi
    };
    // lower: P(X ≥ x | p) = 0.025, increasing in p; upper: P(X ≤ x | p) = 0.025, decreasing in p
    let lo = if x == 0 { 0.0 } else { solve(&|p| 1.0 - binom_cdf(x - 1, n, p) >= 0.025) };
    let hi = if x >= n { 1.0 } else { solve(&|p| binom_cdf(x, n, p) <= 0.025) };
    ((lo * 1000.0).floor() as u32, (hi * 1000.0).ceil().min(1000.0) as u32)
}

#[cfg(test)]
mod tests;
