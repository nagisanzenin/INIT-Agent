//! Lane M #4 (G-21, v0.5.3): a betting lower confidence bound for the mean of G4's bounded paired task differences
//! d[i] ∈ [−1, 1], measured beside the percentile bootstrap G4 uses (kernel/gate.md §3.8.3): their one-sided widths
//! and coverage at n = 16–40 ([`super::sim::widths`]). Measurement only: G4's decision does not use it (an e-process
//! gate is v0.6 work, PLAN-052 Scope).
//!
//! Construction (testing by betting; Waudby-Smith & Ramdas, "Estimating means of bounded random variables by
//! betting", JRSS B 86(1), 2024, arXiv 2010.09686). Rescale x = (d + 1)/2 ∈ [0, 1]. For a candidate mean m the
//! bettor's wealth is K_t(m) = Π_{i≤t} (1 + λ_i(m)·(x_i − m)), with a predictable bet λ_i(m) = min(λ̂_i, c/m), c = 1/2,
//! so that every factor is at least 1 − c > 0 (x_i ≥ 0). Under H0: E[x] ≤ m every factor has conditional mean
//! 1 + λ_i(m)·(E[x] − m) ≤ 1, so K(m) is a non-negative supermartingale and Ville's inequality gives
//! P(∃t: K_t(m) ≥ 1/α) ≤ α, whatever the bets, as long as each is fixed before its task is seen. Each factor falls in m,
//! so the rejected m form an interval [0, L), and the lower bound L = sup{m : max_{t≤n} K_t(m) ≥ 1/α} is valid at every
//! n at once (anytime-valid, the running intersection). The bet is the paper's predictable plug-in, tuned for the
//! planned n instead of the time-uniform t·ln(1 + t): λ̂_t = √(2·ln(1/α) / (n·σ̂²_{t−1})), with the regularized running
//! estimates μ̂_t = (1/2 + Σ_{i≤t} x_i)/(t + 1) and σ̂²_t = (1/4 + Σ_{i≤t}(x_i − μ̂_i)²)/(t + 1) (σ̂²_0 = 1/4, the largest
//! variance on [0, 1]).
//!
//! Tasks must enter in a random order (G4's schedule order is seeded): the caller shuffles.

/// The bet's cap: λ ≤ c/m keeps every factor ≥ 1 − c (the paper's truncation, c = 1/2).
const C: f64 = 0.5;
/// Bisection steps on m ∈ [0, 1]: 2⁻⁶⁰ is far below any width reported (permille).
const STEPS: usize = 60;

/// max_{t≤n} ln K_t(m) for the data `x` (in [0, 1]) and the predictable bets `lam` (before the cap c/m).
fn max_log_wealth(x: &[f64], lam: &[f64], m: f64) -> f64 {
    let (mut lk, mut best) = (0.0f64, 0.0f64);
    for (xi, li) in x.iter().zip(lam) {
        let l = if m > 0.0 { li.min(C / m) } else { *li };
        lk += (1.0 + l * (xi - m)).ln();
        best = best.max(lk);
    }
    best
}

/// The predictable plug-in bets λ̂_1..λ̂_n for data in [0, 1], tuned for `n_plan` tasks at level α.
fn bets(x: &[f64], alpha: f64, n_plan: usize) -> Vec<f64> {
    let (mut sum, mut ss) = (0.0f64, 0.0f64);
    let mut out = Vec::with_capacity(x.len());
    for (t, xi) in x.iter().enumerate() {
        let var = (0.25 + ss) / (t as f64 + 1.0); // σ̂²_{t−1}, from tasks before this one only
        out.push((2.0 * (1.0 / alpha).ln() / (n_plan.max(1) as f64 * var)).sqrt());
        sum += xi;
        let mu = (0.5 + sum) / (t as f64 + 2.0); // μ̂_t
        ss += (xi - mu).powi(2);
    }
    out
}

/// The one-sided (1 − α) betting lower bound of mean(d), d[i] ∈ [−1, 1], in schedule order, tuned for `n_plan`.
pub fn lower_bound(d: &[f64], alpha: f64, n_plan: usize) -> f64 {
    if d.is_empty() {
        return -1.0;
    }
    let x: Vec<f64> = d.iter().map(|v| ((v + 1.0) / 2.0).clamp(0.0, 1.0)).collect();
    let lam = bets(&x, alpha, n_plan);
    let need = (1.0 / alpha).ln();
    if max_log_wealth(&x, &lam, 0.0) < need {
        return -1.0; // not even m = 0 (d = −1) is rejected
    }
    let (mut lo, mut hi) = (0.0f64, 1.0f64); // invariant: lo rejected, hi not (K(1) has every factor ≤ 1)
    for _ in 0..STEPS {
        let mid = 0.5 * (lo + hi);
        if max_log_wealth(&x, &lam, mid) >= need { lo = mid } else { hi = mid }
    }
    2.0 * lo - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::rng::SplitMix64;

    /// The bound is below the sample mean, rises with the data, and holds its level: on 4,000 null samples of 32
    /// bounded differences (mean 0) it exceeds the true mean at most α of the time, plus Monte Carlo slack.
    #[test]
    fn betting_bound_is_valid_and_ordered() {
        let d = [0.0; 32];
        let l0 = lower_bound(&d, 0.05, 32);
        assert!(l0 < 0.0 && l0 > -0.5, "{l0}");
        assert!(lower_bound(&[0.5; 32], 0.05, 32) > l0 + 0.4);
        assert_eq!(lower_bound(&[], 0.05, 32), -1.0);
        let mut r = SplitMix64::new(0x6b65_7462_6574_0001);
        let mut miss = 0;
        for _ in 0..4_000 {
            // a quarter of the tasks flip (±1 with equal chance), the rest tie: mean 0, G-21's shape
            let d: Vec<f64> = (0..32).map(|i| if i % 4 == 0 { if r.next() & 1 == 0 { 1.0 } else { -1.0 } } else { 0.0 }).collect();
            if lower_bound(&d, 0.05, 32) > 0.0 {
                miss += 1;
            }
        }
        assert!(miss <= 4_000 * 5 / 100 + 60, "{miss} of 4000 null bounds above the true mean");
    }
}
