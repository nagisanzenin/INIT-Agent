//! Deterministic randomness for the gate statistics (kernel/gate.md §3.8.3 "Primitives").
//!
//! Every draw is reproducible bit for bit from `(gate_id, purpose)`, so a re-run of a gate selects the
//! same tasks, the same bootstrap resamples and the same noise splits.

use sha2::{Digest, Sha256};

/// `seed_u64(gate_id, purpose)`: the first 8 bytes, big-endian, of `sha256(gate_id + "/" + purpose)`.
pub fn seed_u64(gate_id: &str, purpose: &str) -> u64 {
    let mut h = Sha256::new();
    h.update(gate_id.as_bytes());
    h.update(b"/");
    h.update(purpose.as_bytes());
    let digest = h.finalize();
    let mut b = [0u8; 8];
    b.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(b)
}

/// SplitMix64 (Steele, Lea, Flood 2014), with wrapping arithmetic.
#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }

    /// `SplitMix64::new(seed_u64(gate_id, purpose))`.
    pub fn for_gate(gate_id: &str, purpose: &str) -> Self {
        Self::new(seed_u64(gate_id, purpose))
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `idx(rng, n) = ((rng.next() as u128 * n as u128) >> 64) as usize` (Lemire's multiply-shift).
    /// `n` must be > 0.
    pub fn idx(&mut self, n: usize) -> usize {
        debug_assert!(n > 0);
        ((self.next() as u128 * n as u128) >> 64) as usize
    }

    /// In-place Fisher–Yates shuffle using [`SplitMix64::idx`]: for i = n−1 down to 1, swap i with
    /// `idx(i + 1)`.
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        let n = v.len();
        for i in (1..n).rev() {
            let j = self.idx(i + 1);
            v.swap(i, j);
        }
    }

    /// A uniformly random permutation of `0..n` (the `fisher_yates(0..R, rng)` of §3.8.2).
    pub fn permutation(&mut self, n: usize) -> Vec<usize> {
        let mut p: Vec<usize> = (0..n).collect();
        self.shuffle(&mut p);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_reference_values() {
        // Reference sequence for seed 0 (Vigna's splitmix64.c).
        let mut r = SplitMix64::new(0);
        assert_eq!(r.next(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(r.next(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(r.next(), 0x06C4_5D18_8009_454F);
    }

    #[test]
    fn seed_is_sha256_prefix_big_endian() {
        // sha256("j-000043/ni") computed independently: first 8 bytes, big-endian.
        let s = seed_u64("j-000043", "ni");
        let mut h = Sha256::new();
        h.update(b"j-000043/ni");
        let d = h.finalize();
        assert_eq!(s.to_be_bytes(), d[..8]);
        assert_ne!(seed_u64("j-000043", "ni"), seed_u64("j-000043", "cap"));
    }

    #[test]
    fn idx_in_range_and_permutation_complete() {
        let mut r = SplitMix64::for_gate("j-000001", "t");
        for n in 1..50 {
            for _ in 0..100 {
                assert!(r.idx(n) < n);
            }
        }
        let mut p = r.permutation(17);
        p.sort_unstable();
        assert_eq!(p, (0..17).collect::<Vec<_>>());
    }

    #[test]
    fn deterministic_per_gate_and_purpose() {
        let a: Vec<u64> = {
            let mut r = SplitMix64::for_gate("j-000043", "select");
            (0..5).map(|_| r.next()).collect()
        };
        let b: Vec<u64> = {
            let mut r = SplitMix64::for_gate("j-000043", "select");
            (0..5).map(|_| r.next()).collect()
        };
        assert_eq!(a, b);
    }
}
