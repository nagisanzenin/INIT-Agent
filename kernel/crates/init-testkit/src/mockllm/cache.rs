//! Opt-in prefix-cache simulation (`--simulate-cache`, testing.md §3.4.3).
//!
//! `cache_read = u × ⌊⌈LCP / 4⌉ / u⌋` tokens, where LCP is the longest common byte prefix between
//! the rendered prompt R and any earlier R of the same model in this run, and u is the unit size.

use std::collections::BTreeMap;

/// Default cache unit, in tokens (`--cache-unit-tokens`).
pub const DEFAULT_UNIT_TOKENS: u64 = 64;

/// The simulation state: earlier prompts per model.
#[derive(Debug, Default)]
pub struct CacheSim {
    unit: u64,
    by_model: BTreeMap<String, Vec<Vec<u8>>>,
}

/// Length of the longest common prefix of two byte strings.
pub fn lcp(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

impl CacheSim {
    pub fn new(unit_tokens: u64) -> CacheSim {
        CacheSim {
            unit: unit_tokens.max(1),
            by_model: BTreeMap::new(),
        }
    }

    /// The simulated `cache_read` for prompt `r` without remembering it.
    pub fn peek(&self, model: &str, r: &[u8]) -> u64 {
        let best = self
            .by_model
            .get(model)
            .map(|v| v.iter().map(|s| lcp(r, s)).max().unwrap_or(0))
            .unwrap_or(0);
        let tokens = best.div_ceil(4) as u64;
        self.unit * (tokens / self.unit)
    }

    /// The simulated `cache_read` for prompt `r`; then remember `r`.
    ///
    /// A stored prompt that is a prefix of `r` is dropped: for any later X, LCP(X, prefix) ≤ LCP(X, r),
    /// so a linear conversation keeps one entry per model instead of one per request.
    pub fn observe(&mut self, model: &str, r: &[u8]) -> u64 {
        let hit = self.peek(model, r);
        let v = self.by_model.entry(model.to_string()).or_default();
        if !v.iter().any(|s| s.starts_with(r)) {
            v.retain(|s| !r.starts_with(s));
            v.push(r.to_vec());
        }
        hit
    }

    /// Prompts remembered for `model` (tests).
    pub fn entries(&self, model: &str) -> usize {
        self.by_model.get(model).map_or(0, Vec::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_request_misses_then_prefix_hits_in_units() {
        let mut c = CacheSim::new(64);
        let a = vec![b'a'; 1000];
        assert_eq!(c.observe("m", &a), 0);
        let mut b = a.clone();
        b.extend_from_slice(&[b'b'; 500]);
        // LCP 1000 bytes → 250 tokens → 3 units of 64 = 192.
        assert_eq!(c.observe("m", &b), 192);
        assert_eq!(c.entries("m"), 1, "the prefix entry is subsumed");
        // A restart sends the same prompt again: full hit, floored to units (1500 → 375 → 320).
        assert_eq!(c.observe("m", &b), 320);
        // Another model shares nothing.
        assert_eq!(c.observe("other", &b), 0);
    }

    #[test]
    fn diverging_prompts_keep_both_and_short_lcp_misses() {
        let mut c = CacheSim::new(64);
        c.observe("m", &[b'x'; 400]);
        let mut y = vec![b'x'; 100];
        y.extend_from_slice(&[b'y'; 400]);
        // LCP 100 bytes → 25 tokens < one unit.
        assert_eq!(c.observe("m", &y), 0);
        assert_eq!(c.entries("m"), 2);
        let mut c1 = CacheSim::new(1);
        c1.observe("m", b"abcdefgh");
        assert_eq!(c1.observe("m", b"abcdeXXX"), 2); // ⌈5/4⌉ = 2
    }
}
