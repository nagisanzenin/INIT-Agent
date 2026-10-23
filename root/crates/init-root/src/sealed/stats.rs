//! The Root's own copy of kernel/gate.md §3.8.2–§3.8.3 for the pass rate (same algorithms, seeds and
//! PRNG primitives): from L3 the kernel's copy is agent code (sealed-eval.md §3.5).

/// `seed_u64(id, purpose)`: the first 8 bytes, big-endian, of `sha256(id + "/" + purpose)`.
pub fn seed(id: &str, purpose: &str) -> u64 {
    use sha2::Digest;
    let h = sha2::Sha256::digest(format!("{id}/{purpose}").as_bytes());
    u64::from_be_bytes(h[..8].try_into().unwrap_or_default())
}

/// SplitMix64 with Lemire's multiply-shift `idx`.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let z = (self.0 ^ (self.0 >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn idx(&mut self, n: usize) -> usize {
        ((u128::from(self.next()) * n as u128) >> 64) as usize
    }
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.idx(i + 1);
            v.swap(i, j);
        }
    }
}

fn mean(x: &[f64]) -> f64 {
    if x.is_empty() { 0.0 } else { x.iter().sum::<f64>() / x.len() as f64 }
}

/// §3.8.2 `noise(pass_rate)`: the sign-flip split of each task's repeats, B = 2,000, the one-sided 95%
/// quantile scaled from half-size groups to R-vs-R. `s[i]` holds task i's repeat scores in [0, 1].
pub fn noise(s: &[Vec<f64>], id: &str) -> Option<f64> {
    let r = s.iter().map(Vec::len).min().filter(|r| *r >= 2)?;
    let (h, mut rng, b) = (r / 2, Rng(seed(id, "noise:pass_rate")), 2000);
    let mut diff: Vec<f64> = (0..b).map(|_| {
        let (mut a, mut a2) = (Vec::new(), Vec::new());
        for t in s {
            let mut p: Vec<usize> = (0..r).collect();
            rng.shuffle(&mut p);
            a.push(mean(&p[..h].iter().map(|&k| t[k]).collect::<Vec<_>>()));
            a2.push(mean(&p[h..2 * h].iter().map(|&k| t[k]).collect::<Vec<_>>()));
        }
        mean(&a) - mean(&a2)
    }).collect();
    diff.sort_by(f64::total_cmp);
    Some(diff[(0.95 * b as f64).ceil() as usize - 1] * (h as f64 / r as f64).sqrt())
}

/// §3.8.3: the one-sided paired cluster bootstrap over tasks (B = 10,000); returns `L`, the 5% quantile of
/// the resampled mean difference.
pub fn lower(d: &[f64], id: &str, purpose: &str) -> f64 {
    if d.is_empty() {
        return 0.0;
    }
    let (mut rng, b) = (Rng(seed(id, purpose)), 10_000);
    let mut v: Vec<f64> = (0..b).map(|_| (0..d.len()).map(|_| d[rng.idx(d.len())]).sum::<f64>() / d.len() as f64).collect();
    v.sort_by(f64::total_cmp);
    v[(0.05 * b as f64).floor() as usize]
}

/// Smoothing across audits: `sqrt(w·δ² + (1 − w)·δ_hist²)` with `δ_hist = δ_prev·sqrt(n_prev / n)`.
pub fn smooth(delta: f64, n: usize, prev: Option<(f64, usize)>) -> f64 {
    match prev {
        Some((dp, np)) if np > 0 && n > 0 => (0.5 * delta * delta + 0.5 * dp * dp * np as f64 / n as f64).sqrt(),
        _ => delta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_follow_gate_md() {
        // SplitMix64 reference values for state 0 (Vigna's splitmix64.c)
        let mut r = Rng(0);
        assert_eq!([r.next(), r.next()], [0xE220_A839_7B1D_CDAF, 0x6E78_9E6A_A1B9_65F4]);
        assert_eq!(seed("j-000001", "ni"), seed("j-000001", "ni"));
        assert_ne!(seed("j-000001", "ni"), seed("j-000002", "ni"));
        let mut v: Vec<usize> = (0..10).collect();
        Rng(7).shuffle(&mut v);
        let mut w = v.clone();
        w.sort_unstable();
        assert_eq!(w, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn noise_and_bootstrap_behave() {
        // identical repeats: no noise; binary flips at p = 0.5 on 20 tasks: about 1.645·sqrt(20·0.25)/20 = 0.18
        assert_eq!(noise(&vec![vec![1.0, 1.0]; 8], "a"), Some(0.0));
        let flips: Vec<Vec<f64>> = (0..20).map(|i| if i % 2 == 0 { vec![1.0, 0.0] } else { vec![0.0, 1.0] }).collect();
        let d = noise(&flips, "a").unwrap();
        assert!((0.1..0.3).contains(&d), "{d}");
        assert_eq!(noise(&[vec![1.0]], "a"), None, "R = 1 has no within-audit estimate");
        assert_eq!(lower(&[0.0; 10], "a", "ni"), 0.0);
        assert!(lower(&[-1.0, -1.0, -1.0, 0.0], "a", "ni") <= -0.5);
        assert!((smooth(0.2, 10, Some((0.2, 10))) - 0.2).abs() < 1e-12);
    }
}
