//! Tests of `sealed::exact` (lane ROOT-S1b). Test-only, outside the Root's size budget (root.md §4.8).
//! M7-R03 (the DP against brute force), the statistics parts of M7-R04, R09, R13, R14 and R17, and the design
//! check of m7-design.md §3.2.6 and §3.2.13's tables, computed exactly (no simulation) by `design::*` below.

use super::super::stats::{Rng, lower, seed};
use super::*;

/// Brute force over all 2^n sign vectors: (P(S ≥ Σk), P(S ≤ Σk)).
fn brute(k: &[i64]) -> (PValue, PValue) {
    let t: i64 = k.iter().sum();
    let (mut up, mut dn) = (0u128, 0u128);
    for mask in 0u32..(1u32 << k.len()) {
        let s: i64 = k.iter().enumerate().map(|(i, x)| if mask >> i & 1 == 1 { x.abs() } else { -x.abs() }).sum();
        up += u128::from(s >= t);
        dn += u128::from(s <= t);
    }
    (PValue::new(up, k.len() as u32), PValue::new(dn, k.len() as u32))
}

#[test]
fn m7_r03_dp_equals_brute_force() {
    let mut rng = Rng(seed("m7-r03", "vectors"));
    let mut checked = 0;
    for n in 0..=14usize {
        for r in [1i64, 2, 3, 5] {
            let count = if n <= 10 { 120 } else { 25 };
            for v in 0..count {
                // three shapes: symmetric in [−R, R], skewed non-negative (small p), and partial credit in permille sums
                let k: Vec<i64> = (0..n)
                    .map(|_| match v % 3 {
                        0 => rng.idx(2 * r as usize + 1) as i64 - r,
                        1 => rng.idx(r as usize + 1) as i64 - i64::from(rng.idx(4) == 0),
                        _ => rng.idx(2000 * r as usize + 1) as i64 - 1000 * r,
                    })
                    .collect();
                let (up, dn) = brute(&k);
                assert_eq!(p_sup(&k), up, "p_sup {k:?}");
                assert_eq!(p_inf(&k), dn, "p_inf {k:?}");
                checked += 1;
            }
        }
    }
    assert!(checked > 5000, "{checked}");
}

#[test]
fn edges_ties_n1_and_max_n() {
    // all ties: p = 1 both ways; a tie, never a signal
    let ties = [0i64; 32];
    assert_eq!((p_sup(&ties), p_inf(&ties)), (PValue::ONE, PValue::ONE));
    let d = slice_status(&ties, 3, 1, -0.05, 0.2, 0.25);
    assert_eq!((d.status, d.ratchet_signal, d.d_permille), (Status::Tie, false, 0));
    assert_eq!(p_sup(&[]), PValue::ONE);
    // n = 1
    assert_eq!(p_sup(&[3]), PValue { num: 1, log2_den: 1 });
    assert_eq!(p_sup(&[-3]), PValue::ONE);
    assert_eq!(p_inf(&[-3]), PValue { num: 1, log2_den: 1 });
    assert_eq!(slice_status(&[3], 3, 1, 0.0, 0.5, 0.6).status, Status::Tie);
    // the strength floor: 5 discordant flips reach 0.05 (2⁻⁵), 4 do not, but 4 reach the ratchet's 0.25
    let five = slice_status(&[1, 1, 1, 1, 1, 0, 0, 0], 1, 1, 0.0, 0.1, 0.25);
    assert_eq!((five.status, five.p_sup), (Status::Superior, PValue { num: 1, log2_den: 5 }));
    let four = slice_status(&[1, 1, 1, 1, 0, 0, 0, 0], 1, 1, 0.0, 0.1, 0.25);
    assert_eq!((four.status, four.ratchet_signal), (Status::Tie, true));
    // the maximum n: 2⁻¹⁰⁰ exactly, no overflow in the exact comparison; one more discordant task → p = 1
    let max = vec![1i64; MAX_N];
    assert_eq!(p_sup(&max), PValue { num: 1, log2_den: 100 });
    assert!(p_sup(&max).le_permille(ALPHA_GAIN_PERMILLE) && p_sup(&max).f64() > 0.0);
    assert_eq!(p_sup(&vec![1i64; MAX_N + 1]), PValue::ONE);
    // the maximum n with partial credit at R = 5: 100 tasks of permille sums up to ±5,000
    let mut rng = Rng(seed("edges", "big"));
    let big: Vec<i64> = (0..MAX_N).map(|_| rng.idx(10_001) as i64 - 5000).collect();
    let (s, i) = (p_sup(&big), p_inf(&big));
    assert!(s.f64() + i.f64() >= 1.0 && s.f64() <= 1.0 && i.f64() <= 1.0);
    assert!(s.permille_ceil() <= 1000);
}

#[test]
fn exact_test_invariances() {
    let mut rng = Rng(seed("inv", "k"));
    for _ in 0..200 {
        let n = 1 + rng.idx(32);
        let mut k: Vec<i64> = (0..n).map(|_| rng.idx(7) as i64 - 3).collect();
        let p = p_sup(&k);
        let neg: Vec<i64> = k.iter().map(|x| -x).collect();
        assert_eq!(p_inf(&neg), p, "p_inf(−k) = p_sup(k)");
        let scaled: Vec<i64> = k.iter().map(|x| x * 1000).collect();
        assert_eq!(p_sup(&scaled), p, "a common factor changes nothing");
        rng.shuffle(&mut k);
        assert_eq!(p_sup(&k), p, "order-free");
        k.push(0);
        assert_eq!(p_sup(&k), p, "zeros change nothing");
    }
    // the margin binds in permille units: five 1‰ gains are p = 1/32 but far below one task-equivalent
    let tiny = slice_status(&[1, 1, 1, 1, 1], 1, 1000, 0.0, 0.1, 0.25);
    assert_eq!((tiny.status, tiny.ratchet_signal), (Status::Tie, false));
    // PValue arithmetic
    let p = PValue::new(3, 6); // 3/64 = 0.046875
    assert!(p.le_permille(47) && !p.le_permille(46));
    assert_eq!(p.permille_ceil(), 47);
    assert_eq!(PValue::new(8, 3), PValue::ONE);
    assert_eq!(PValue::new(0, 9), PValue::new(0, 0));
}

#[test]
fn deterministic() {
    let mut rng = Rng(seed("det", "k"));
    let k: Vec<i64> = (0..32).map(|_| rng.idx(7) as i64 - 2).collect();
    let l = lower(&d_ratios(&k, 3, 1), "j-000051", "ni");
    let a = slice_status(&k, 3, 1, l, margin(0.11, 32), 0.25);
    let b = slice_status(&k, 3, 1, lower(&d_ratios(&k, 3, 1), "j-000051", "ni"), margin(0.11, 32), 0.25);
    assert_eq!(a, b);
    assert_eq!(futility(&k[..16], 3, 1, Some(0.1), "j-1"), futility(&k[..16], 3, 1, Some(0.1), "j-1"));
    let runs = synthetic_r6(16, "det");
    assert_eq!(calibrate_b(&runs, "cal", 0.25, 6), calibrate_b(&runs, "cal", 0.25, 6));
}

/// m7 §3.2.4's status order, one case per branch (the statistics part of M7-R04).
#[test]
fn m7_r04_status_branches() {
    let mut sup = vec![3i64; 8];
    sup.extend([0; 24]);
    // superior is checked first, even when the NI bound would say inferior
    assert_eq!(slice_status(&sup, 3, 1, -0.9, 0.2, 0.25).status, Status::Superior);
    let down: Vec<i64> = sup.iter().map(|x| -x).collect();
    assert_eq!(slice_status(&down, 3, 1, -0.3, 0.2, 0.25).status, Status::Inferior);
    let flat = [1i64, -1, 2, -1, 0, 0, 1, -1];
    assert_eq!(slice_status(&flat, 3, 1, -0.05, 0.3, 0.25).status, Status::Underpowered);
    assert_eq!(slice_status(&flat, 3, 1, -0.05, 0.2, 0.25).status, Status::Tie);
    let neg = [1i64, -1, -2, -1, 0, 0, 1, -1];
    assert_eq!(slice_status(&neg, 3, 1, -0.05, 0.2, 0.25).status, Status::TieNegative);
    // end to end through diffs and the Root's bootstrap: a candidate that passes 2 more of 3 on 12 of 32 tasks
    let cand: Vec<Vec<i64>> = (0..32).map(|i| if i < 12 { vec![1000, 1000, 0] } else { vec![1000, 0, 0] }).collect();
    let inc: Vec<Vec<i64>> = (0..32).map(|i| if i < 12 { vec![0, 0, 0] } else { vec![0, 1000, 0] }).collect();
    let (k, r) = diffs(&cand, &inc).unwrap();
    assert_eq!((r, k[0], k[31]), (3, 2000, 0));
    let d = slice_status(&k, r, 1000, lower(&d_ratios(&k, r, 1000), "g", "ni"), margin(0.1, 32), 0.25);
    assert_eq!((d.status, d.ratchet_signal, d.d_permille), (Status::Superior, true, 250));
    assert_eq!(d.p_sup, PValue { num: 1, log2_den: 12 });
    assert!(diffs(&cand, &inc[..31]).is_err());
    assert!(diffs(&[vec![1000, 0]], &[vec![0]]).is_err(), "unequal R");
    assert!(diffs(&[vec![1001]], &[vec![0]]).is_err());
    // the futility look: a clearly broken half stops; a flat one does not; no history, no look
    assert!(futility(&[-3; 16], 3, 1, Some(0.1), "g"));
    assert!(!futility(&[0; 16], 3, 1, Some(0.1), "g"));
    assert!(!futility(&[-3; 16], 3, 1, None, "g"));
}

/// The statistics part of M7-R17: the milestone's outcomes and the span allowance.
#[test]
fn m7_r17_milestone_outcomes() {
    let mut gain = vec![2i64; 9];
    gain.extend([0; 15]);
    let c = milestone(&gain, 3, 1, 0.1, 0.2, 0.25);
    assert_eq!((c.outcome, c.d_permille), (Outcome::Confirmed, 250));
    let down: Vec<i64> = gain.iter().map(|x| -x).collect();
    assert_eq!(milestone(&down, 3, 1, -0.3, 0.2, 0.25).outcome, Outcome::Regression);
    // NI failure alone (p_inf > 0.05): a regression only while m ≤ margin_max
    let soft = [-1i64, -1, -1, 1, 0, 0, -1, 0];
    assert!(!p_inf(&soft).le_permille(50));
    assert_eq!(milestone(&soft, 3, 1, -0.3, 0.2, 0.25).outcome, Outcome::Regression);
    assert_eq!(milestone(&soft, 3, 1, -0.3, 0.3, 0.25).outcome, Outcome::NotConfirmed);
    assert_eq!(milestone(&[1, -1, 0, 1], 3, 1, -0.05, 0.2, 0.25).outcome, Outcome::NotConfirmed);
    assert_eq!(ratchet_k_next(Outcome::Confirmed, 1, 4), 4);
    assert_eq!(ratchet_k_next(Outcome::NotConfirmed, 4, 4), 2);
    assert_eq!(ratchet_k_next(Outcome::NotConfirmed, 1, 4), 1);
    assert_eq!(ratchet_k_next(Outcome::Regression, 3, 4), 1);
    assert_eq!(ratchet_k_next(Outcome::Partial, 3, 4), 3);
    assert_eq!(Outcome::NotConfirmed.as_str(), "not_confirmed");
}

/// M7-R13's rounding, the cost-per-pass ratio, the sentinel (M7-R09's rule), band and knobs.
#[test]
fn m7_r13_rounding_cpp_sentinel_band() {
    assert_eq!([round_to(1224, 50), round_to(1225, 50), round_to(-1225, 50), round_to(-24, 50)], [1200, 1250, -1250, 0]);
    assert_eq!([round_to(216, 10), round_to(214, 10), round_to(955, 50)], [220, 210, 950]);
    assert_eq!(cpp_ratio_permille(300, 10, 300, 10), 1000);
    assert_eq!(cpp_ratio_permille(600, 10, 300, 10), 2000);
    assert_eq!(cpp_ratio_permille(450, 15, 300, 10), 1000, "more spend for proportionally more passes");
    assert_eq!(cpp_ratio_permille(300, 0, 300, 10), 10_000);
    assert_eq!((cpp_ratio_permille(0, 0, 0, 0), cpp_ratio_permille(1, 0, 0, 0)), (1000, u64::MAX));
    // Σp̂ = 3.5, Σp̂(1−p̂) = 1.75 → threshold 2.5·√1.75 ≈ 3.307
    assert!(!sentinel_drift(6, &[500; 7]) && sentinel_drift(7, &[500; 7]) && sentinel_drift(0, &[500; 7]));
    // the floor at 1: saturated cache, threshold 2.5
    assert!(!sentinel_drift(2, &[0; 7]) && sentinel_drift(3, &[0; 7]));
    assert!((in_band(1, 6), in_band(5, 6), in_band(0, 6), in_band(6, 6), in_band(1, 2)) == (true, true, false, false, true));
    assert_eq!([knob_step(199), knob_step(200), knob_step(800), knob_step(801)], [-1, 0, 0, 1]);
}

#[test]
fn clopper_pearson_reference_values() {
    // textbook values: 0/10 → (0, 0.3085); 5/10 → (0.1871, 0.8129); 10/10 → (0.6915, 1)
    assert_eq!(clopper_pearson_permille(0, 10), (0, 309));
    assert_eq!(clopper_pearson_permille(5, 10), (187, 813));
    assert_eq!(clopper_pearson_permille(10, 10), (691, 1000));
    // 0/2000 → upper 1 − 0.025^(1/2000) = 0.001843
    assert_eq!(clopper_pearson_permille(0, 2000), (0, 2));
    let (lo, hi) = clopper_pearson_permille(72, 2000);
    assert!(lo < 36 && hi > 36 && hi - lo < 20, "{lo} {hi}");
    assert_eq!(clopper_pearson_permille(0, 0), (0, 1000));
}

/// The tier table's run counts and costs (m7 §3.2.6 at $0.003/run) and §3.1.7's expected false accepts.
#[test]
fn tiers_runs_cost_and_expected_false_accepts() {
    let got: Vec<(u32, u32, u64, u64)> =
        TIERS.iter().map(|t| (t.runs(false), t.runs(true), t.cost_musd(false, 3000), t.cost_musd(true, 3000))).collect();
    assert_eq!(got, [(127, 72, 381_000, 216_000), (223, 122, 669_000, 366_000), (351, 186, 1_053_000, 558_000)]);
    assert_eq!(tier("standard"), Some(STANDARD));
    assert_eq!(tier("auto"), None);
    assert_eq!(expected_false_accepts_milli(20, 36), 720);
}

// ---- the design check: m7 §3.2.6 and §3.2.13, computed exactly ----

mod design {
    /// m7 §3.2.6's task model: incumbent pass rates 80% U[0.15, 0.85] + 20% U[0, 1]; the candidate gains `g`
    /// on every task (uniform), or `g/0.4` on each task independently with probability 0.4 (concentrated),
    /// capped to [0, 1]. Returns the per-task distribution of k = c − p passes (index k + R), integrated by the
    /// midpoint rule.
    pub fn pi_task(r: usize, g: f64, conc: bool) -> Vec<f64> {
        if conc {
            let (a, b) = (pi_task(r, 0.0, false), pi_task(r, g / 0.4, false));
            return a.iter().zip(&b).map(|(x, y)| 0.6 * x + 0.4 * y).collect();
        }
        const M: usize = 4000;
        let pmf = |p: f64| -> Vec<f64> {
            (0..=r).map(|a| binom(r, a) * p.powi(a as i32) * (1.0 - p).powi((r - a) as i32)).collect()
        };
        let mut pi = vec![0.0; 2 * r + 1];
        for (w, lo, hi) in [(0.8, 0.15, 0.85), (0.2, 0.0, 1.0)] {
            for i in 0..M {
                let p = lo + (hi - lo) * (i as f64 + 0.5) / M as f64;
                let (bp, bq) = (pmf(p), pmf((p + g).clamp(0.0, 1.0)));
                for a in 0..=r {
                    for b in 0..=r {
                        pi[a + r - b] += w / M as f64 * bq[a] * bp[b];
                    }
                }
            }
        }
        pi
    }

    fn binom(n: usize, k: usize) -> f64 {
        (0..k).fold(1.0, |acc, i| acc * (n - i) as f64 / (i + 1) as f64)
    }

    struct Ctx {
        n: usize,
        r: usize,
        w: Vec<f64>,
        plus: Vec<f64>,
        alpha_pm: f64,
        margin: bool,
        total: f64,
    }

    /// P(p_sup ≤ α [∧ Σk ≥ R]) for n iid tasks with the k distribution `pi`, exactly: a sum over the multisets of
    /// |k| (multinomial weights) of the alternative's mass on the exact test's rejection region for that
    /// multiset. Subtrees below 1e-15 total weight are skipped (error < 1e-9).
    pub fn p_reject(n: usize, pi: &[f64], alpha_pm: u32, margin: bool) -> f64 {
        let r = (pi.len() - 1) / 2;
        let w: Vec<f64> = (0..=r).map(|j| if j == 0 { pi[r] } else { pi[r + j] + pi[r - j] }).collect();
        let plus: Vec<f64> = (0..=r).map(|j| if j == 0 || w[j] == 0.0 { 0.5 } else { pi[r + j] / w[j] }).collect();
        let mut c = Ctx { n, r, w, plus, alpha_pm: f64::from(alpha_pm), margin, total: 0.0 };
        let size = n * r + 1;
        let (mut cnt, mut alt) = (vec![0.0; size], vec![0.0; size]);
        cnt[0] = 1.0;
        alt[0] = 1.0;
        dfs(&mut c, 1, 0, 0, 0, &cnt, &alt, 1.0);
        c.total
    }

    #[allow(clippy::too_many_arguments)]
    fn dfs(c: &mut Ctx, j: usize, used: usize, a: usize, m: usize, cnt: &[f64], alt: &[f64], wt: f64) {
        if wt < 1e-15 {
            return;
        }
        if j > c.r {
            let wt = wt * c.w[0].powi((c.n - used) as i32);
            let need = if c.margin { (a + c.r).div_ceil(2) } else { 0 };
            let bound = c.alpha_pm * (m as f64).exp2();
            let (mut tail, mut mass) = (0.0, 0.0);
            for s in (need..=a).rev() {
                tail += cnt[s];
                if tail * 1000.0 > bound {
                    break;
                }
                mass += alt[s];
            }
            c.total += wt * mass;
            return;
        }
        let (mut cnt, mut alt, mut w, rem) = (cnt.to_vec(), alt.to_vec(), wt, c.n - used);
        let (pj, wj) = (c.plus[j], c.w[j]);
        for k in 0..=rem {
            // C(rem, k)·w_j^k is unimodal in k: past the mode and below the floor, every later term is smaller
            if k > 0 && w < 1e-15 && wj * (rem - k + 1) as f64 <= k as f64 {
                break;
            }
            if k > 0 {
                let top = a + k * j;
                for s in (j..=top).rev() {
                    cnt[s] += cnt[s - j];
                    alt[s] = (1.0 - pj) * alt[s] + pj * alt[s - j];
                }
                for s in 0..j.min(top + 1) {
                    alt[s] *= 1.0 - pj;
                }
                w *= wj * (rem - k + 1) as f64 / k as f64;
            }
            dfs(c, j + 1, used + k, a + k * j, m + k, &cnt, &alt, w);
        }
    }

    /// P(the screen does not reject on net ≤ −4) over 12 tasks at R = 1 (m7 §3.2.2).
    pub fn screen_pass(g: f64, conc: bool) -> f64 {
        let pi = pi_task(1, g, conc);
        let mut dist = vec![1.0]; // net + 12·0 offset grows by one each task
        for _ in 0..12 {
            let mut next = vec![0.0; dist.len() + 2];
            for (i, p) in dist.iter().enumerate() {
                for (d, q) in pi.iter().enumerate() {
                    next[i + d] += p * q;
                }
            }
            dist = next;
        }
        // index i ↔ net = i − 12; reject at net ≤ −4 ↔ i ≤ 8
        1.0 - dist[..=8].iter().sum::<f64>()
    }
}

/// The numbers m7 §3.2.6 and §3.2.13 print, recomputed exactly. Run with `--nocapture` to see the table.
fn tier_row(t: Tier, g: f64, conc: bool) -> (f64, f64, f64) {
    let pi = design::pi_task(t.repeats as usize, g, conc);
    let sup = design::p_reject(t.n, &pi, ALPHA_GAIN_PERMILLE, true);
    let ratchet = design::p_reject(t.n, &pi, ALPHA_RATCHET_PERMILLE, true);
    (design::screen_pass(g, conc), sup, ratchet)
}

fn screen_sup(t: Tier, g: f64, conc: bool) -> (f64, f64) {
    let pi = design::pi_task(t.repeats as usize, g, conc);
    (design::screen_pass(g, conc), design::p_reject(t.n, &pi, ALPHA_GAIN_PERMILLE, true))
}

#[test]
fn design_screen_error_rates() {
    // m7 §3.2.2: an equal candidate is screened out 6%; +15 ≤ 1.1%; −10 16%; −40 about 70%
    let out = |g: f64| 1.0 - design::screen_pass(g, false);
    let got = [out(0.0), out(0.15), out(-0.10), out(-0.40)];
    eprintln!("screen reject at 0, +15, −10, −40: {got:?}");
    for (x, want) in got[..3].iter().zip([0.06, 0.011, 0.16]) {
        assert!((x - want).abs() <= 0.02, "{got:?}");
    }
    assert!(got[1] <= 0.011);
    // discrepancy (build/status/ROOT-S1b.md): under §3.2.6's task model a −40 candidate is screened out 63.9%,
    // not "about 70%"; pinned at the exact value
    assert!((got[3] - 0.6393).abs() < 0.001, "{got:?}");
}

/// m7 §3.2.6's rows: FA, power cells (uniform / concentrated) at +10, +15, +20, +25.
const TIER_TABLE: [(Tier, f64, [(f64, f64); 4]); 3] = [
    (LEAN, 0.030, [(0.21, 0.19), (0.38, 0.31), (0.61, 0.41), (0.80, 0.48)]),
    (STANDARD, 0.036, [(0.37, 0.34), (0.70, 0.55), (0.90, 0.69), (0.98, 0.74)]),
    (STRONG, 0.034, [(0.56, 0.49), (0.87, 0.69), (0.98, 0.82), (1.00, 0.85)]),
];

#[test]
fn design_tier_table_false_accepts_and_power() {
    // lean and standard in full; strong's FA here, its power row (R = 5, slow in debug) in the ignored test below
    check_tiers(&TIER_TABLE[..2], true);
    check_tiers(&TIER_TABLE[2..], false);
}

#[test]
#[ignore]
fn design_tier_table_strong_power() {
    check_tiers(&TIER_TABLE[2..], true);
}

/// P(`better`) ≈ P(screen passes) · P(superior); the slice and the screen use disjoint tasks.
fn check_tiers(rows: &[(Tier, f64, [(f64, f64); 4])], power: bool) {
    for &(t, fa_design, cells) in rows {
        let (screen0, sup0) = screen_sup(t, 0.0, false);
        let fa = screen0 * sup0;
        eprintln!("{:>8}: FA {:.4} (design {fa_design}) sup|null {sup0:.4}; 20 looks → {:.2} false accepts", t.name, fa, 20.0 * fa);
        assert!(sup0 <= 0.05, "the exact bound");
        assert!((fa - fa_design).abs() <= 0.005, "{} FA {fa}", t.name);
        let fa_pm = (fa * 1000.0).round() as u32;
        assert!(expected_false_accepts_milli(20, fa_pm) <= 1000, "under one expected false accept per epoch");
        for (i, g) in [0.10, 0.15, 0.20, 0.25].into_iter().enumerate().filter(|_| power) {
            let (su, pu) = screen_sup(t, g, false);
            let (sc, pc) = screen_sup(t, g, true);
            eprintln!("          +{:.0}: uniform {:.3} (design {}) · concentrated {:.3} (design {})", g * 100.0, su * pu, cells[i].0,
                sc * pc, cells[i].1);
            assert!((su * pu - cells[i].0).abs() <= 0.03 && (sc * pc - cells[i].1).abs() <= 0.03, "{} +{g}", t.name);
        }
        // the analytic MDE (calibration's refit) at the model's within-task variance 0.2007, against the exact
        // gain at which the superiority test reaches 80% power (bisection on the exact engine)
        if power {
            let mde = t.mde_permille(201);
            let (mut lo, mut hi) = (0.0, 0.4);
            for _ in 0..12 {
                let mid = 0.5 * (lo + hi);
                let p = design::p_reject(t.n, &design::pi_task(t.repeats as usize, mid, false), ALPHA_GAIN_PERMILLE, true);
                if p >= 0.8 { hi = mid } else { lo = mid }
            }
            eprintln!("          MDE: analytic {mde}‰, exact 80% power at {:.0}‰", hi * 1000.0);
            assert!((f64::from(mde) - hi * 1000.0).abs() <= 15.0, "{} MDE {mde} vs {hi}", t.name);
        }
    }
}

#[test]
fn design_ratchet_operating_characteristic() {
    // m7 §3.2.13 stage 1 at standard: P(better), P(lineage_gain) ≈ P(screen)·(P(ratchet signal) − P(superior)),
    // taking NI and m ≤ margin_max as given whenever D ≥ 1/n (m ≈ 0.21 < 0.25 at standard)
    let design = [(-0.10, 0.000, 0.006), (-0.05, 0.004, 0.045), (0.0, 0.035, 0.145), (0.05, 0.142, 0.309), (0.10, 0.377, 0.366)];
    for (g, better_d, lineage_d) in design {
        let (s, sup, rat) = tier_row(STANDARD, g, false);
        eprintln!("ratchet {g:+.2}: better {:.3} (design {better_d}), lineage_gain {:.3} (design {lineage_d})", s * sup, s * (rat - sup));
        assert!((s * sup - better_d).abs() <= 0.01 && (s * (rat - sup) - lineage_d).abs() <= 0.015, "{g}");
    }
}

#[test]
fn design_milestone_power() {
    // m7 §3.2.13: P(confirmed) = P(p_sup ≤ 0.05 ∧ D ≥ 1/n) on the reserve, both arms fresh, no screen;
    // false "regression" at 0 = P(p_inf ≤ 0.05) (the NI path is not included here)
    let design = [((12, 3), [0.031, 0.15, 0.28, 0.45, 0.62], 0.025), ((24, 3), [0.034, 0.29, 0.56, 0.79, 0.93], 0.037), ((24, 5), [0.039, 0.45, 0.78, 0.95, 0.99], 0.038)];
    for ((n, r), cells, reg_d) in design {
        let got: Vec<f64> = [0.0, 0.10, 0.15, 0.20, 0.25]
            .into_iter()
            .map(|g| design::p_reject(n, &design::pi_task(r, g, false), ALPHA_MILESTONE_PERMILLE, true))
            .collect();
        let reg = design::p_reject(n, &design::pi_task(r, 0.0, false), ALPHA_MILESTONE_PERMILLE, false);
        eprintln!("milestone {n}×{r}: confirm {got:.3?} (design {cells:?}); false regression {reg:.3} (design {reg_d})");
        assert!(got[0] <= 0.05 && reg <= 0.05, "exact bounds");
        assert!(got.iter().zip(cells).all(|(x, y)| (x - y).abs() <= 0.015) && (reg - reg_d).abs() <= 0.005, "{n}×{r}");
    }
}

// ---- M7-R14: calibration on synthetic R = 6 data ----

/// n tasks, pass rates from m7 §3.2.6's model, 6 binary runs each (permille scores).
fn synthetic_r6(n: usize, id: &str) -> Vec<Vec<i64>> {
    let mut rng = Rng(seed(id, "synthetic"));
    let u = |rng: &mut Rng| (rng.next() >> 11) as f64 / (1u64 << 53) as f64;
    (0..n)
        .map(|_| {
            let p = if u(&mut rng) < 0.8 { 0.15 + 0.7 * u(&mut rng) } else { u(&mut rng) };
            (0..6).map(|_| if u(&mut rng) < p { 1000 } else { 0 }).collect()
        })
        .collect()
}

/// The exact pseudo-A/A false-accept rate given the data: a task with s passes of 6 splits into k = 2X − s
/// (X hypergeometric), whose sign is fair given |k|; so FA = Σ over the |k| multisets of P(multiset) times the
/// share of sign patterns the exact test rejects (with the margin Σk ≥ 3).
fn exact_aa_fa(runs: &[Vec<i64>]) -> f64 {
    let s: Vec<usize> = runs.iter().map(|t| t.iter().filter(|x| **x == 1000).count()).collect();
    let n1 = s.iter().filter(|x| **x == 1 || **x == 5).count();
    let n2 = s.iter().filter(|x| **x == 2 || **x == 4).count(); // |k| = 2 w.p. 0.4, else 0
    let n3 = s.iter().filter(|x| **x == 3).count(); // |k| = 3 w.p. 0.1, else 1
    let bin = |n: usize, k: usize, p: f64| (0..k).fold(1.0, |a, i| a * (n - i) as f64 / (i + 1) as f64) * p.powi(k as i32) * (1.0 - p).powi((n - k) as i32);
    let mut fa = 0.0;
    for a in 0..=n2 {
        for b in 0..=n3 {
            let mut mags = vec![1i64; n1 + n3 - b];
            mags.extend(vec![2; a]);
            mags.extend(vec![3; b]);
            // share of sign patterns with p_sup ≤ 0.05 ∧ Σk ≥ 3: enumerate T through the count DP
            let total: i64 = mags.iter().sum();
            let mut cnt = vec![0f64; total as usize + 1];
            cnt[0] = 1.0;
            for &x in &mags {
                for t in (x as usize..=total as usize).rev() {
                    cnt[t] += cnt[t - x as usize];
                }
            }
            let den = (mags.len() as f64).exp2();
            let (mut tail, mut rej) = (0.0, 0.0);
            for sp in (0..=total as usize).rev() {
                tail += cnt[sp];
                let t = 2 * sp as i64 - total;
                if tail * 1000.0 > 50.0 * den || t < 3 {
                    break;
                }
                rej += cnt[sp];
            }
            fa += bin(n2, a, 0.4) * bin(n3, b, 0.1) * rej / den;
        }
    }
    fa
}

#[test]
fn m7_r14_calibration_on_synthetic_data() {
    let runs = synthetic_r6(32, "m7-r14");
    // the pseudo-A/A's false-accept rate over CALIBRATE_B splits, on the exact split stream `calibrate` uses
    let mut rng = Rng(seed("cal-r14", "calibrate:aa"));
    let flags: Vec<bool> = (0..CALIBRATE_B)
        .map(|_| {
            let (k, _) = aa_split(&mut rng, &runs);
            slice_status(&k, 3, 1000, 0.0, 0.1, 0.25).status == Status::Superior
        })
        .collect();
    let fa = flags.iter().filter(|x| **x).count() as f64 / CALIBRATE_B as f64;
    let exact = exact_aa_fa(&runs);
    eprintln!("pseudo-A/A FA {fa:.4} vs exact {exact:.4}");
    assert!((fa - exact).abs() <= 0.010, "FA within ±1 point of the analytic value");
    assert!(exact <= 0.05);
    // the full path (δ, bootstrap, status) on the first splits counts the same superiors
    let b = 40;
    let c = calibrate_b(&runs, "cal-r14", 0.25, b).unwrap();
    let want = flags[..b].iter().filter(|x| **x).count() as u32;
    assert_eq!(i64::from(c.fa_permille), div_round(1000 * i128::from(want), b as i128));
    eprintln!("{c:?}");
    // variance components against a direct computation; δ near its analytic one-sided 95% half-width
    let ph: Vec<f64> = runs.iter().map(|t| t.iter().sum::<i64>() as f64 / 6000.0).collect();
    let within = ph.iter().map(|p| p * (1.0 - p) * 6.0 / 5.0).sum::<f64>() / 32.0;
    assert_eq!(c.var_within_permille, (within * 1000.0).round() as u32);
    let analytic = |r: f64| 1.645 * (ph.iter().map(|p| 2.0 * p * (1.0 - p) * 6.0 / 5.0 / r).sum::<f64>()).sqrt() / 32.0;
    assert!((c.delta_r3 - analytic(3.0)).abs() < 0.03 && (c.delta_r2 - analytic(2.0)).abs() < 0.04, "{c:?}");
    assert!((c.delta_r5 - c.delta_r3 * 0.6f64.sqrt()).abs() < 1e-12);
    assert_eq!(c.mde_standard_permille, STANDARD.mde_permille(c.var_within_permille));
    assert!(c.mde_lean_permille > c.mde_standard_permille && c.mde_standard_permille > c.mde_strong_permille);
    assert_eq!(c.json()["delta_permille"]["r3"], (c.delta_r3 * 1000.0).round() as i64);
    assert!(calibrate_b(&runs[..1].iter().map(|t| t[..5].to_vec()).collect::<Vec<_>>(), "x", 0.25, 1).is_err());
}

/// m7 §3.2.6's last column, P(accept a cheaper regression) ≈ P(screen) · P(status = tie) (the tie guard), and
/// the pseudo-A/A's FR. Both need the Root's δ and bootstrap, so they are simulated (seeded) with the real
/// `stats::noise` and `stats::lower`, no δ history. Slow in a debug build: `cargo test … -- --ignored`.
#[test]
#[ignore]
fn design_tie_guard_and_aa_fr_simulated() {
    use super::super::stats::noise;
    let gates = 400;
    for (t, g, want) in [(STANDARD, -0.10, 0.058), (STANDARD, -0.05, 0.20), (STRONG, -0.10, 0.020), (STRONG, -0.05, 0.14)] {
        let mut rng = Rng(seed("tie-guard", &format!("{}{g}", t.name)));
        let u = |rng: &mut Rng| (rng.next() >> 11) as f64 / (1u64 << 53) as f64;
        let mut ties = 0;
        for j in 0..gates {
            let (mut k, mut inc) = (Vec::new(), Vec::new());
            for _ in 0..t.n {
                let p = if u(&mut rng) < 0.8 { 0.15 + 0.7 * u(&mut rng) } else { u(&mut rng) };
                let q = (p + g).clamp(0.0, 1.0);
                let ri: Vec<f64> = (0..t.repeats).map(|_| f64::from(u8::from(u(&mut rng) < p))).collect();
                let rc: f64 = (0..t.repeats).map(|_| f64::from(u8::from(u(&mut rng) < q))).sum();
                k.push((rc - ri.iter().sum::<f64>()) as i64);
                inc.push(ri);
            }
            let id = format!("tg{j}");
            let m = margin(noise(&inc, &id).unwrap_or(0.0), t.n);
            let s = slice_status(&k, t.repeats, 1, lower(&d_ratios(&k, t.repeats, 1), &id, "ni"), m, 0.25).status;
            ties += u32::from(s == Status::Tie);
        }
        let p = design::screen_pass(g, false) * f64::from(ties) / f64::from(gates);
        eprintln!("{} {g:+.2}: P(cheaper accepted) {p:.3} (design {want})", t.name);
    }
    let c = calibrate_b(&synthetic_r6(32, "m7-r14"), "cal-fr", 0.25, 400).unwrap();
    eprintln!("pseudo-A/A over 400 splits: FA {}‰ [{}, {}], FR {}‰ [{}, {}], m {}‰, verdict {:?}", c.fa_permille,
        c.fa_lo_permille, c.fa_hi_permille, c.fr_permille, c.fr_lo_permille, c.fr_hi_permille, c.m_standard_permille, c.verdict);
}
