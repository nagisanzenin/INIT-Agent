//! Offline calibration of the non-inferiority test (kernel/gate.md §3.8.3 test G-21; the
//! `init-kernel gate simulate` command of §4.1). No model, no files: synthetic paired gates.
//!
//! Design (the §3.8.5 worked example): a quarter of the tasks are flaky (LKG pass probability 0.5), the
//! rest deterministic (LKG passes always). The candidate's pass probability on every task is the LKG's
//! plus `effect` (clamped to [0, 1]), so the true mean effect is exactly `effect` for −0.5 ≤ effect ≤ 0.
//! Each synthetic gate measures its own δ from the LKG arm's repeats (no history), takes
//! m = margin_factor·δ (with the half-task floor; `margin_max` is not applied, because G-21 calibrates
//! the test itself, not the power policy), and counts as accepted when the bootstrap lower bound L > −m.

use serde::Serialize;

use super::boot::{bootstrap_ni, margin};
use super::data::{Metric, RunValues, Source, TaskRuns, TaskSet};
use super::noise::{NoiseEntry, noise, smooth};

fn entry0() -> NoiseEntry {
    NoiseEntry {
        delta_log_ppm: None,
        delta_permille: None,
        n: 0,
        repeats: 0,
        at: String::new(),
    }
}
use super::rng::SplitMix64;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimParams {
    pub n: usize,
    pub repeats: usize,
    /// True mean effect on the pass rate (e.g. −0.1).
    pub effect: f64,
    pub gates: usize,
    pub bootstrap_b: usize,
    pub noise_b: usize,
    pub alpha: f64,
    pub margin_factor: f64,
    /// Distinguishes independent simulation runs (part of every synthetic gate id).
    pub label: String,
    /// How δ_used is formed (§3.8.2 smoothing).
    pub history: SimHistory,
}

/// The noise history the synthetic gates see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SimHistory {
    /// δ_used = δ_gate (a first gate on a new model: no `noise.json` entry).
    None,
    /// Steady state: each gate smooths with the previous gate's δ_used (w = 0.5), as `noise.json` does.
    Carried,
    /// δ_used = the design's analytic δ (isolates the bootstrap test from the δ estimate).
    Oracle,
}

impl SimParams {
    pub fn new(n: usize, repeats: usize, effect: f64, gates: usize) -> Self {
        SimParams {
            n,
            repeats,
            effect,
            gates,
            bootstrap_b: 10_000,
            noise_b: 2_000,
            alpha: 0.05,
            margin_factor: 2.0,
            label: "sim".into(),
            history: SimHistory::Carried,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimResult {
    pub params: SimParams,
    pub accepted: usize,
    pub accept_rate: f64,
    /// Mean of the per-gate δ estimates.
    pub mean_delta: f64,
    /// The design's analytic δ: `1.645 · sqrt(Σ 2·p_i(1−p_i) / R) / n`.
    pub analytic_delta: f64,
}

/// LKG pass probabilities of the design: ⌈n/4⌉ flaky tasks at 0.5, the rest at 1.0.
pub fn design(n: usize) -> Vec<f64> {
    let flaky = n.div_ceil(4);
    (0..n).map(|i| if i < flaky { 0.5 } else { 1.0 }).collect()
}

/// The analytic one-sided 95% half-width of D under no change for the design.
pub fn analytic_delta(n: usize, repeats: usize) -> f64 {
    let s: f64 = design(n)
        .iter()
        .map(|p| 2.0 * p * (1.0 - p) / repeats as f64)
        .sum();
    1.645 * s.sqrt() / n as f64
}

fn bernoulli(rng: &mut SplitMix64, p: f64) -> u32 {
    let u = (rng.next() >> 11) as f64 / (1u64 << 53) as f64;
    if u < p { 1000 } else { 0 }
}

/// Runs `params.gates` synthetic gates and reports the accept (non-inferior) rate.
pub fn simulate(params: &SimParams) -> SimResult {
    let ps = design(params.n);
    let mut accepted = 0;
    let mut delta_sum = 0.0;
    let mut prev: Option<f64> = None;
    for g in 0..params.gates {
        let gate_id = format!(
            "{}-n{}-r{}-e{:+.4}-{g}",
            params.label, params.n, params.repeats, params.effect
        );
        let mut rng = SplitMix64::for_gate(&gate_id, "sim-scores");
        let tasks: Vec<TaskRuns> = ps
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                let q = (p + params.effect).clamp(0.0, 1.0);
                let c = (0..params.repeats)
                    .map(|_| RunValues::score(bernoulli(&mut rng, q)))
                    .collect();
                let lkg = (0..params.repeats)
                    .map(|_| RunValues::score(bernoulli(&mut rng, p)))
                    .collect();
                TaskRuns {
                    id: format!("t{i}"),
                    set: TaskSet::Pool,
                    source: Source::Maintainer,
                    family: None,
                    c,
                    p: lkg,
                }
            })
            .collect();
        let refs: Vec<&TaskRuns> = tasks.iter().collect();
        let gate_delta = noise(
            &gate_id,
            "pass_rate",
            Metric::PassRate,
            &refs,
            params.noise_b,
        );
        let delta = match (params.history, prev) {
            (SimHistory::Carried, Some(h)) => {
                // noise.json stores δ in integer permille; n is unchanged between gates.
                let e = NoiseEntry {
                    delta_permille: Some((h * 1000.0).round() as i64),
                    n: params.n as u64,
                    ..entry0()
                };
                smooth(gate_delta, Some(&e), params.n)
            }
            (SimHistory::Oracle, _) => Some(analytic_delta(params.n, params.repeats)),
            _ => gate_delta,
        }
        .unwrap_or(0.0);
        prev = Some(delta);
        delta_sum += delta;
        let m = margin(delta, params.n, params.margin_factor);
        let d: Vec<f64> = tasks.iter().map(TaskRuns::d).collect();
        let iv = bootstrap_ni(&gate_id, &d, params.bootstrap_b, params.alpha);
        if iv.lower > -m {
            accepted += 1;
        }
    }
    SimResult {
        params: params.clone(),
        accepted,
        accept_rate: accepted as f64 / params.gates.max(1) as f64,
        mean_delta: delta_sum / params.gates.max(1) as f64,
        analytic_delta: analytic_delta(params.n, params.repeats),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(n: usize, r: usize, k: f64, gates: usize) -> SimResult {
        let mut p = SimParams::new(n, r, -k * analytic_delta(n, r), gates);
        p.bootstrap_b = 2_000;
        p.noise_b = 400;
        simulate(&p)
    }

    #[test]
    fn analytic_delta_matches_worked_example() {
        // §3.8.5: n = 16, 4 flaky: δ = 0.103 at R = 2, 0.084 at R = 3.
        assert!((analytic_delta(16, 2) - 0.1028).abs() < 0.001);
        assert!((analytic_delta(16, 3) - 0.0840).abs() < 0.001);
    }

    /// G-21 (reduced size for the unit suite; the full 2,000-gate grid is `g21_full`, run with
    /// `--ignored`). Steady-state history (`Carried`), as in production after the first gates.
    ///
    /// - n = 30, R = 3 meets gate.md's targets: accept rate at effect 0 ≥ 90%, at −2δ ≤ 7%.
    /// - n = 16, R = 2 does **not** (full grid: 83.8% at 0, 8.2% at −2δ; 73.9% / 11.6% for a first
    ///   gate without history). That is a known gap reported to the gate.md owner in
    ///   build/status/WP8.md; these bounds pin the measured baseline so a regression shows.
    #[test]
    fn g21_calibration_reduced() {
        let a0 = cell(30, 3, 0.0, 400);
        let a1 = cell(30, 3, 1.0, 100);
        let a2 = cell(30, 3, 2.0, 400);
        let a3 = cell(30, 3, 3.0, 100);
        assert!(
            a0.accept_rate >= 0.90,
            "n=30 R=3 effect 0: {}",
            a0.accept_rate
        );
        assert!(
            a2.accept_rate <= 0.07,
            "n=30 R=3 effect -2δ: {}",
            a2.accept_rate
        );
        assert!(
            a0.accept_rate >= a1.accept_rate
                && a1.accept_rate >= a2.accept_rate
                && a2.accept_rate >= a3.accept_rate
        );
        let b0 = cell(16, 2, 0.0, 400);
        let b2 = cell(16, 2, 2.0, 400);
        assert!(
            b0.accept_rate >= 0.78,
            "n=16 R=2 effect 0: {}",
            b0.accept_rate
        );
        assert!(
            b2.accept_rate <= 0.12,
            "n=16 R=2 effect -2δ: {}",
            b2.accept_rate
        );
    }

    #[test]
    #[ignore = "full G-21 grid: 2,000 gates per cell, B = 10,000; run in release with --ignored --nocapture"]
    fn g21_full() {
        for hist in [SimHistory::None, SimHistory::Carried, SimHistory::Oracle] {
            for n in [16, 30] {
                for r in [2, 3] {
                    for k in [0.0, 1.0, 2.0, 3.0] {
                        let mut p = SimParams::new(n, r, -k * analytic_delta(n, r), 2_000);
                        p.history = hist;
                        let res = simulate(&p);
                        eprintln!(
                            "G21 {hist:?} n={n} R={r} effect=-{k}δ accept={:.4} mean_δ={:.4} analytic_δ={:.4}",
                            res.accept_rate, res.mean_delta, res.analytic_delta
                        );
                    }
                }
            }
        }
    }
}
