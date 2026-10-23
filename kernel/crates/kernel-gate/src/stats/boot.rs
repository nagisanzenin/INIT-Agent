//! Paired cluster bootstrap (kernel/gate.md §3.8.3), the futility look (§3.8.4) and the capability test
//! (§3.9.1). Every resample is drawn from [`SplitMix64`] seeded by `(gate_id, purpose)`.

use serde::Serialize;

use super::rng::SplitMix64;

/// Bootstrap result: the point estimate and the two one-sided bounds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Interval {
    /// `D = mean_i d[i]`.
    pub mean: f64,
    /// `L = D*[floor(α·B)]`.
    pub lower: f64,
    /// `U = D*[ceil((1 − α)·B) − 1]`.
    pub upper: f64,
}

/// Sorted bootstrap means: `D*[b] = mean over j in 1..n of d[idx(rng, n)]`, b in 0..B.
pub fn resample_means(rng: &mut SplitMix64, d: &[f64], b: usize) -> Vec<f64> {
    let n = d.len();
    let mut out = Vec::with_capacity(b);
    if n == 0 {
        out.resize(b, 0.0);
        return out;
    }
    for _ in 0..b {
        let mut s = 0.0;
        for _ in 0..n {
            s += d[rng.idx(n)];
        }
        out.push(s / n as f64);
    }
    out.sort_by(|x, y| x.total_cmp(y));
    out
}

/// `floor(α·B)`, guarded against binary rounding of α (0.05·10,000 must be 500).
fn lo_index(alpha: f64, b: usize) -> usize {
    (((alpha * b as f64) + 1e-9).floor() as usize).min(b.saturating_sub(1))
}

/// `ceil((1 − α)·B) − 1`, guarded the same way (index 9,499 at α = 0.05, B = 10,000).
fn hi_index(alpha: f64, b: usize) -> usize {
    let k = (((1.0 - alpha) * b as f64) - 1e-9).ceil() as usize;
    k.clamp(1, b.max(1)) - 1
}

/// One-sided percentile bootstrap of the mean of `d` at level α, seeded by `(gate_id, purpose)`.
pub fn bootstrap(gate_id: &str, purpose: &str, d: &[f64], b: usize, alpha: f64) -> Interval {
    let mean = if d.is_empty() {
        0.0
    } else {
        d.iter().sum::<f64>() / d.len() as f64
    };
    let mut rng = SplitMix64::for_gate(gate_id, purpose);
    let s = resample_means(&mut rng, d, b);
    if s.is_empty() {
        return Interval {
            mean,
            lower: mean,
            upper: mean,
        };
    }
    Interval {
        mean,
        lower: s[lo_index(alpha, b)],
        upper: s[hi_index(alpha, b)],
    }
}

/// The non-inferiority bootstrap of §3.8.3 (seed purpose `"ni"`).
pub fn bootstrap_ni(gate_id: &str, d: &[f64], b: usize, alpha: f64) -> Interval {
    bootstrap(gate_id, "ni", d, b, alpha)
}

/// The minimum noise floor δ_min of a set of n tasks: half a task's worth, 0.5 / n in score units
/// (decision D14). A deterministic measurement with zero observed variance does not mean zero true
/// noise, so the divergence guard's δ_p and δ_h are at least this (§3.8.3), and so is the margin. The
/// split guards keep the measured δ: flooring them raised false accepts (build/status/FIX2.md). 0 for an
/// empty set (no evidence, no floor).
pub fn delta_min(n: usize) -> f64 {
    if n == 0 { 0.0 } else { 0.5 / n as f64 }
}

/// δ_used floored at δ_min (D14): `max(δ_used, 0.5 / n)`.
pub fn floored(delta: f64, n: usize) -> f64 {
    delta.max(delta_min(n))
}

/// `m = max(margin_factor × δ_used(pass_rate), 0.5 / n)`: the floor is half a task, the same δ_min the
/// divergence guard uses (D14). The floor bounds m itself and is not multiplied by `margin_factor`, so
/// D14 leaves the margin exactly as §3.8.3 had it (and G-21 unchanged).
pub fn margin(delta_pass: f64, n: usize, margin_factor: f64) -> f64 {
    let floor = if n == 0 { f64::INFINITY } else { delta_min(n) };
    (margin_factor * delta_pass).max(floor)
}

/// `n_needed ≈ n · (m / margin_max)²` (§3.8.3 top-up), rounded up.
pub fn n_needed(n: usize, m: f64, margin_max: f64) -> usize {
    (n as f64 * (m / margin_max).powi(2)).ceil() as usize
}

/// Result of the futility look (§3.8.4).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FutilityLook {
    pub u99: f64,
    pub margin: f64,
    /// `U99 < −m`: stop with `reject: score_inferior_early`.
    pub stop: bool,
}

/// The futility look over the first ⌈n/2⌉ tasks in schedule order (`d_first`), with δ from history only.
/// Returns `None` (skip the look) without a history δ. Seed purpose `"futility"`; `futility_alpha` is
/// `gate.g4.futility_alpha` (0.01), so U99 = `D*[ceil(0.99·B) − 1]`.
pub fn futility(
    gate_id: &str,
    d_first: &[f64],
    delta_hist: Option<f64>,
    margin_factor: f64,
    b: usize,
    futility_alpha: f64,
) -> Option<FutilityLook> {
    let delta = delta_hist?;
    if d_first.is_empty() {
        return None;
    }
    let m = margin(delta, d_first.len(), margin_factor);
    let iv = bootstrap(gate_id, "futility", d_first, b, futility_alpha);
    Some(FutilityLook {
        u99: iv.upper,
        margin: m,
        stop: iv.upper < -m,
    })
}

/// One family of the capability set A: per-arm mean scores over its sealed instances (in [0, 1]).
/// At L1–L2 the gate computes them from its own runs ([`CapFamily::from_instances`]); at L3+ the Root
/// returns exactly these aggregates (§3.7.5).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CapFamily {
    pub family: String,
    pub n_instances: usize,
    pub c_mean: f64,
    pub p_mean: f64,
}

impl CapFamily {
    /// From per-instance repeat scores (permille) of both arms: `(c repeats, p repeats)` per instance.
    pub fn from_instances(family: &str, instances: &[(Vec<u32>, Vec<u32>)]) -> Self {
        let mean = |v: &[u32]| -> f64 {
            if v.is_empty() {
                0.0
            } else {
                v.iter().map(|&x| x as f64 / 1000.0).sum::<f64>() / v.len() as f64
            }
        };
        let n = instances.len();
        let (mut c, mut p) = (0.0, 0.0);
        for (ci, pi) in instances {
            c += mean(ci);
            p += mean(pi);
        }
        let k = n.max(1) as f64;
        CapFamily {
            family: family.to_string(),
            n_instances: n,
            c_mean: c / k,
            p_mean: p / k,
        }
    }

    pub fn d(&self) -> f64 {
        self.c_mean - self.p_mean
    }
}

/// `capability.status` (§3.9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapStatus {
    /// The test ran.
    Tested,
    /// Fewer than `capability_min_tasks` instances, or a single family: the path does not apply.
    Insufficient,
}

/// The capability test result for the record (`G4.capability`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CapResult {
    pub status: CapStatus,
    pub n: usize,
    pub n_families: usize,
    /// Mean over families of the paired difference.
    pub d: f64,
    /// Lower 95% bound, family-clustered (`l_cap`); 0 when insufficient.
    pub l: f64,
}

impl CapResult {
    /// A significant gain on the sealed half ⇔ status tested and `l_cap > 0`.
    pub fn gain(&self) -> bool {
        self.status == CapStatus::Tested && self.l > 0.0
    }
}

/// The capability test (§3.9.1 step 4): the §3.8.3 bootstrap on A alone, seed purpose `"cap"`, with the
/// **family** as the cluster.
pub fn capability_test(
    gate_id: &str,
    fams: &[CapFamily],
    min_tasks: usize,
    b: usize,
    alpha: f64,
) -> CapResult {
    let n: usize = fams.iter().map(|f| f.n_instances).sum();
    let n_families = fams.iter().filter(|f| f.n_instances > 0).count();
    let d: Vec<f64> = fams
        .iter()
        .filter(|f| f.n_instances > 0)
        .map(CapFamily::d)
        .collect();
    let mean = if d.is_empty() {
        0.0
    } else {
        d.iter().sum::<f64>() / d.len() as f64
    };
    if n < min_tasks || n_families < 2 {
        return CapResult {
            status: CapStatus::Insufficient,
            n,
            n_families,
            d: mean,
            l: 0.0,
        };
    }
    let iv = bootstrap(gate_id, "cap", &d, b, alpha);
    CapResult {
        status: CapStatus::Tested,
        n,
        n_families,
        d: iv.mean,
        l: iv.lower,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_rules() {
        assert_eq!(lo_index(0.05, 10_000), 500);
        assert_eq!(hi_index(0.05, 10_000), 9_499);
        assert_eq!(hi_index(0.01, 10_000), 9_899);
        assert_eq!(lo_index(0.05, 1), 0);
        assert_eq!(hi_index(0.05, 1), 0);
    }

    /// G-20: fixed gate id, fixed data → exact L and U, reproducible bit for bit.
    #[test]
    fn g20_bootstrap_reproducible() {
        let d = [
            0.0, 0.0, 0.5, -0.5, 1.0, 0.0, 0.0, -1.0, 0.5, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0,
        ];
        let a = bootstrap_ni("j-000043", &d, 10_000, 0.05);
        let b = bootstrap_ni("j-000043", &d, 10_000, 0.05);
        assert_eq!(a.lower.to_bits(), b.lower.to_bits());
        assert_eq!(a.upper.to_bits(), b.upper.to_bits());
        assert!((a.mean - 0.0625).abs() < 1e-12);
        assert!(a.lower < a.mean && a.mean < a.upper);
        // Resample means of 16 values are multiples of 1/32: the bounds land exactly on that grid.
        assert_eq!((a.lower * 32.0).fract(), 0.0);
        assert_eq!((a.upper * 32.0).fract(), 0.0);
        // Pinned values for this seed (regression guard for the primitives).
        assert_eq!((a.lower * 32.0, a.upper * 32.0), (PINNED_L32, PINNED_U32));
    }

    // L·32 and U·32 for gate "j-000043" (first run, 2026-10-20); any change to seed_u64, SplitMix64,
    // idx or the index rules moves them.
    const PINNED_L32: f64 = -4.0;
    const PINNED_U32: f64 = 8.0;

    #[test]
    fn constant_differences_have_degenerate_interval() {
        let iv = bootstrap_ni("j", &[0.25; 10], 1000, 0.05);
        assert_eq!((iv.mean, iv.lower, iv.upper), (0.25, 0.25, 0.25));
    }

    #[test]
    fn margin_floor_and_n_needed() {
        assert!((margin(0.0, 16, 2.0) - 0.5 / 16.0).abs() < 1e-15);
        assert!((margin(0.103, 16, 2.0) - 0.206).abs() < 1e-12);
        assert_eq!(n_needed(16, 0.30, 0.15), 64);
    }

    /// D14: δ_min = half a task of the set; the floor only raises δ; the margin keeps §3.8.3's formula.
    #[test]
    fn d14_delta_min_is_half_a_task() {
        assert_eq!(delta_min(4), 0.125);
        assert_eq!(delta_min(1), 0.5);
        assert_eq!(delta_min(0), 0.0);
        assert_eq!(floored(0.0, 4), 0.125);
        assert_eq!(floored(0.2, 4), 0.2);
        for n in 1..40 {
            for k in 0..30 {
                let d = k as f64 * 0.01;
                let old = (2.0 * d).max(0.5 / n as f64);
                assert_eq!(margin(d, n, 2.0), old, "n={n} δ={d}");
            }
        }
    }

    /// G-23: true effect −0.5 with a history δ → the look at n/2 stops with `score_inferior_early`.
    #[test]
    fn g23_futility_stops_clear_failures() {
        // 8 tasks of 16 complete (R = 2); true effect −0.5: the candidate loses half its passes.
        let d_first = [-1.0, -0.5, -0.5, -0.5, -0.5, -0.5, 0.0, -0.5];
        let look = futility("j-000023", &d_first, Some(0.08), 2.0, 10_000, 0.01).unwrap();
        assert!(look.stop, "{look:?}");
        assert!(look.u99 < -look.margin);
        // No history → the look is skipped.
        assert_eq!(
            futility("j-000023", &d_first, None, 2.0, 10_000, 0.01),
            None
        );
        // A candidate as good as LKG is never stopped.
        let ok = futility(
            "j-000023",
            &[0.0, 0.5, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0],
            Some(0.08),
            2.0,
            10_000,
            0.01,
        )
        .unwrap();
        assert!(!ok.stop);
    }

    #[test]
    fn capability_test_family_clustered() {
        // Two families, two sealed instances each, R = 2: candidate passes all, LKG fails all.
        let f1 = CapFamily::from_instances(
            "fam-a",
            &[
                (vec![1000, 1000], vec![0, 0]),
                (vec![1000, 1000], vec![0, 0]),
            ],
        );
        let f2 = CapFamily::from_instances(
            "fam-b",
            &[
                (vec![1000, 1000], vec![0, 0]),
                (vec![1000, 1000], vec![0, 0]),
            ],
        );
        let r = capability_test("j-000042", &[f1.clone(), f2.clone()], 4, 10_000, 0.05);
        assert_eq!(r.status, CapStatus::Tested);
        assert_eq!((r.n, r.n_families), (4, 2));
        assert!(r.gain() && r.l > 0.0 && (r.d - 1.0).abs() < 1e-12);
        // One family ties: the lower bound is that family's d = 0 → no significant gain.
        let tie = CapFamily::from_instances(
            "fam-b",
            &[
                (vec![1000, 1000], vec![1000, 1000]),
                (vec![0, 0], vec![0, 0]),
            ],
        );
        let r2 = capability_test("j-000042", &[f1.clone(), tie], 4, 10_000, 0.05);
        assert!(!r2.gain());
        // A single family, or fewer than 4 instances: insufficient.
        let r3 = capability_test("j-000042", std::slice::from_ref(&f1), 2, 10_000, 0.05);
        assert_eq!(r3.status, CapStatus::Insufficient);
        let small = CapFamily::from_instances("fam-c", &[(vec![1000, 1000], vec![0, 0])]);
        let r4 = capability_test("j-000042", &[f1, small], 4, 10_000, 0.05);
        assert_eq!((r4.status, r4.n), (CapStatus::Insufficient, 3));
        assert!(!r4.gain());
    }
}
