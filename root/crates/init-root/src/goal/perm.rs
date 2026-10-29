//! L3 D1 (v0.5.3): the exact two-sample permutation test of a card reading. A reading compares the change's first n
//! owner turns with up to 40 turns of the incumbent before it (5 against 40 by default; 5 against 5 when changes
//! stack). Welch's z against a fixed 2 is a large-sample rule: at these sizes, and on the cards' skewed (µUSD, ms) and
//! discrete (corrections, errors) values, its one-sided rates are far from the 2.3 % ADR-013 states (lane M #2/#3,
//! build/status/STATS-053.md: 34 % false `better` on the corrections card at 5 against 40). The permutation test needs
//! only exchangeability of the turns when the change does nothing (foundations.md A_exch, at the turn level here):
//! then every split of the pooled values into n_after and n_before is equally likely, and the p-value of the observed
//! after-sum is exact at every n and for every distribution (Fisher 1935; Pitman 1937). Ties count against the
//! change (≤ and ≥), so discrete cards are conservative, never anti-conservative.
//!
//! The statistic is the sum of the after values, a monotone function of Δ = mean_after − mean_before for fixed sizes.
//! p_le = P(S* ≤ S) and p_ge = P(S* ≥ S) over the C(N, k) equally likely relabelings, N = n_before + n_after. They are
//! counted exactly when C(N, k) ≤ [`EXACT_MAX`] (every default reading: C(45, 5) = 1,221,759), by a branch-and-bound
//! count on the sorted pool; otherwise from [`MC_DRAWS`] random relabelings, as p = (1 + #{S* ≤ S}) / (1 + B), which
//! is a valid p-value at any B (Phipson & Smyth 2010, "Permutation p-values should never be zero"), seeded from the
//! pooled values only (not their labels), so the draws are independent of which turns came after.

use crate::util::sha256_hex;

/// The largest relabeling count enumerated exactly: 2²¹, above C(45, 5) = 1,221,759 (5 readings against a full
/// 40-turn baseline, the default), so a default reading is always exact; the count visits far fewer nodes than
/// relabelings (bounds prune whole subtrees), a few ms in a release build.
pub const EXACT_MAX: u128 = 1 << 21;
/// Monte Carlo relabelings beyond `EXACT_MAX`: (1 + B) = 100,000 makes the smallest p 10 ppm, below the strictest
/// `met` level the grammar allows (5 % / 1000 rounds = 50 ppm, `MET_ALPHA_PPM`).
pub const MC_DRAWS: u64 = 99_999;

/// One-sided permutation p-values of the after-sum, in ppm rounded up (so `p_ppm ≤ α_ppm` holds exactly when p ≤ α),
/// and whether they were counted exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct P {
    pub le_ppm: u64,
    pub ge_ppm: u64,
    pub exact: bool,
}

fn ppm_up(num: u128, den: u128) -> u64 {
    (num * 1_000_000).div_ceil(den.max(1)) as u64
}

/// C(n, k) in u128 (n ≤ 80 here: C(80, 40) ≈ 1.1·10²³).
pub fn binom(n: usize, k: usize) -> u128 {
    if k > n {
        return 0;
    }
    let k = k.min(n - k);
    (0..k).fold(1u128, |c, i| c * (n - i) as u128 / (i as u128 + 1))
}

/// The number of k-subsets of `v` (sorted ascending, `pre` its prefix sums) from index `i` on whose sum is ≤ `s`.
fn count_le(v: &[i64], pre: &[i128], i: usize, k: usize, s: i128) -> u128 {
    let n = v.len();
    if k == 0 {
        return u128::from(s >= 0);
    }
    if n - i < k || pre[i + k] - pre[i] > s {
        return 0; // too few left, or even the k smallest exceed s
    }
    if pre[n] - pre[n - k] <= s {
        return binom(n - i, k); // even the k largest fit: every subset counts
    }
    if k == 1 {
        return v[i..].partition_point(|&x| i128::from(x) <= s) as u128;
    }
    if k == 2 {
        // pairs by two pointers: with v[lo] + v[hi] ≤ s, every partner of v[lo] up to hi fits
        let (mut lo, mut hi, mut c) = (i, n - 1, 0u128);
        while lo < hi {
            if i128::from(v[lo]) + i128::from(v[hi]) <= s {
                c += (hi - lo) as u128;
                lo += 1;
            } else {
                hi -= 1;
            }
        }
        return c;
    }
    let mut c = 0;
    for j in i..=n - k {
        if pre[j + k] - pre[j] > s {
            break; // v[j] and the k − 1 next-smallest already exceed s; later j only grow
        }
        c += count_le(v, pre, j + 1, k - 1, s - i128::from(v[j]));
    }
    c
}

/// The p-values of `after` against `before` (both non-empty).
pub fn test(before: &[i64], after: &[i64]) -> P {
    let (nb, na) = (before.len(), after.len());
    if nb == 0 || na == 0 {
        return P { le_ppm: 1_000_000, ge_ppm: 1_000_000, exact: true };
    }
    let mut pool: Vec<i64> = before.iter().chain(after).copied().collect();
    pool.sort_unstable();
    let s: i128 = after.iter().map(|&x| i128::from(x)).sum();
    let k = na.min(nb);
    let total = binom(nb + na, k);
    if total <= EXACT_MAX {
        let mut pre = vec![0i128; pool.len() + 1];
        for (i, &x) in pool.iter().enumerate() {
            pre[i + 1] = pre[i] + i128::from(x);
        }
        let le = |t: i128| count_le(&pool, &pre, 0, k, t);
        // enumerate the smaller side: with k = n_before, S_after = T − S_before
        let (c_le, c_ge) = if na <= nb {
            (le(s), total - le(s - 1))
        } else {
            let sb = pre[pool.len()] - s;
            (total - le(sb - 1), le(sb))
        };
        return P { le_ppm: ppm_up(c_le, total), ge_ppm: ppm_up(c_ge, total), exact: true };
    }
    // Monte Carlo: seeded by the pooled multiset and the split sizes, never by which values came after
    let key: String = pool.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",") + &format!("/{na}");
    let mut st = u64::from_str_radix(&sha256_hex(key.as_bytes())[..16], 16).unwrap_or(0);
    let mut next = || {
        // SplitMix64 (Steele, Lea, Flood 2014)
        st = st.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = st;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let (mut c_le, mut c_ge) = (1u128, 1u128); // the observed split counts once (Phipson & Smyth)
    let mut idx: Vec<usize> = (0..pool.len()).collect();
    for _ in 0..MC_DRAWS {
        // a partial Fisher–Yates shuffle draws na of the N values uniformly (Lemire's multiply-shift for the index)
        let mut sum = 0i128;
        for i in 0..na {
            let j = i + ((u128::from(next()) * (pool.len() - i) as u128) >> 64) as usize;
            idx.swap(i, j);
            sum += i128::from(pool[idx[i]]);
        }
        c_le += u128::from(sum <= s);
        c_ge += u128::from(sum >= s);
    }
    let den = u128::from(MC_DRAWS) + 1;
    P { le_ppm: ppm_up(c_le, den), ge_ppm: ppm_up(c_ge, den), exact: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Brute force over every relabeling, for checking the branch-and-bound count.
    fn brute(before: &[i64], after: &[i64]) -> (u128, u128, u128) {
        let pool: Vec<i64> = before.iter().chain(after).copied().collect();
        let (n, k) = (pool.len(), after.len());
        let s: i64 = after.iter().sum();
        let (mut le, mut ge, mut total) = (0, 0, 0);
        for mask in 0u64..(1 << n) {
            if mask.count_ones() as usize == k {
                let t: i64 = (0..n).filter(|i| mask >> i & 1 == 1).map(|i| pool[i]).sum();
                (le, ge, total) = (le + u128::from(t <= s), ge + u128::from(t >= s), total + 1);
            }
        }
        (le, ge, total)
    }

    /// The exact count equals brute force on small pools (ties, negatives, either side smaller), and a fully
    /// separated 5 against 5 is 1/252 each way (C(10, 5) = 252).
    #[test]
    fn exact_counts_equal_brute_force() {
        let cases: [(&[i64], &[i64]); 7] = [(&[5, 5, 5, 5, 5], &[4, 4, 4, 4, 4]), (&[3, 3, 2, 2, 2], &[1, 1, 1, 1, 1]), (&[0, 0, 0, 1, 0, 0, 0, 0], &[0, 0, 1]),
            (&[10, -3, 7, 7, 2, 9], &[1, 7, 30]), (&[1, 2], &[3, 4, 5, 6, 0]), (&[100, 104, 96, 102, 98, 100], &[50, 52, 48, 51, 49]), (&[7; 9], &[7; 4])];
        for (b, a) in cases {
            let (le, ge, total) = brute(b, a);
            assert_eq!(binom(b.len() + a.len(), a.len()), total);
            let p = test(b, a);
            assert!(p.exact);
            assert_eq!((p.le_ppm, p.ge_ppm), (ppm_up(le, total), ppm_up(ge, total)), "{b:?} vs {a:?}");
        }
        // and on 2,000 random pools (sizes 1–7 each side, small values so ties are common)
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = |m: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % m
        };
        for _ in 0..2_000 {
            let (nb, na) = (1 + next(7) as usize, 1 + next(7) as usize);
            let b: Vec<i64> = (0..nb).map(|_| next(7) as i64 - 2).collect();
            let a: Vec<i64> = (0..na).map(|_| next(7) as i64 - 2).collect();
            let (le, ge, total) = brute(&b, &a);
            assert_eq!(test(&b, &a), P { le_ppm: ppm_up(le, total), ge_ppm: ppm_up(ge, total), exact: true }, "{b:?} vs {a:?}");
        }
        assert_eq!(test(&[5; 5], &[4; 5]).le_ppm, 3_969, "1/252 = 3,968.25 ppm, rounded up");
        assert_eq!(test(&[5; 5], &[5; 5]), P { le_ppm: 1_000_000, ge_ppm: 1_000_000, exact: true }, "all tied: no evidence either way");
        assert_eq!(binom(45, 5), 1_221_759);
        assert_eq!(binom(80, 40), 107_507_208_733_336_176_461_620);
    }

    /// Beyond `EXACT_MAX` the Monte Carlo p is valid ((1 + count)/(1 + B) never below 10 ppm), close to exact, and
    /// reproducible (the same pool gives the same draws).
    #[test]
    fn monte_carlo_beyond_the_exact_bound() {
        let before: Vec<i64> = (0..40).map(|i| 1000 + (i * 37) % 200).collect();
        let after: Vec<i64> = (0..20).map(|i| 900 + (i * 53) % 200).collect();
        assert!(binom(60, 20) > EXACT_MAX);
        let p = test(&before, &after);
        assert!(!p.exact && p.le_ppm >= 10 && p.le_ppm < 50_000 && p.ge_ppm > 900_000, "{p:?}");
        assert_eq!(p, test(&before, &after), "seeded");
        let mut swapped = before.clone();
        swapped.swap(0, 39);
        assert_eq!(p, test(&swapped, &after), "the seed depends on the pooled values, not their order");
    }
}
