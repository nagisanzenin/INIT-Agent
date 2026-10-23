//! Exact tests for the sentinel (ADR-011 §3.2–§3.3, reusing ADR-010 §3): the exceedance test, the binomial upper
//! tail and the one-sided Fisher tail, as exact fractions of unsigned big integers. No floats anywhere: a tail is
//! compared with α = a/b by cross-multiplication. RF-10 checks these against Python's `fractions` vectors
//! (tests/fixtures/sentinel/tails.json, made by tails.py next to it).

use std::cmp::Ordering;

/// A rational threshold `num/den` (α = 0.002 is `(2, 1000)`).
pub type Alpha = (u64, u64);
/// The hard α per test per look (ADR-011 §3.2).
pub const HARD: Alpha = (2, 1000);
/// The soft α of the close check (S1 → `flagged`).
pub const SOFT: Alpha = (5, 100);

/// An unsigned big integer: base-2³² limbs, least significant first, no trailing zero limbs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Big(Vec<u32>);

impl Big {
    pub fn from(mut v: u128) -> Big {
        let mut l = Vec::new();
        while v > 0 {
            l.push(v as u32);
            v >>= 32;
        }
        Big(l)
    }

    #[cfg_attr(not(test), allow(dead_code))] // RF-10 and X6
    pub fn is_zero(&self) -> bool {
        self.0.is_empty()
    }

    /// `self × m`.
    pub fn mul(&mut self, m: u64) {
        let (lo, hi) = (m & 0xffff_ffff, m >> 32);
        let a = self.mul32(lo);
        if hi == 0 {
            *self = a;
            return;
        }
        let mut b = self.mul32(hi);
        b.0.insert(0, 0);
        b.trim();
        *self = a;
        self.add(&b);
    }

    fn mul32(&self, m: u64) -> Big {
        let mut out = Vec::with_capacity(self.0.len() + 1);
        let mut carry = 0u64;
        for &x in &self.0 {
            let t = u64::from(x) * m + carry;
            out.push(t as u32);
            carry = t >> 32;
        }
        if carry > 0 {
            out.push(carry as u32);
        }
        let mut b = Big(out);
        b.trim();
        b
    }

    /// `self ÷ d`, returning the remainder (`d` < 2³²).
    pub fn div_rem(&mut self, d: u32) -> u32 {
        let mut rem = 0u64;
        for x in self.0.iter_mut().rev() {
            let cur = (rem << 32) | u64::from(*x);
            *x = (cur / u64::from(d)) as u32;
            rem = cur % u64::from(d);
        }
        self.trim();
        rem as u32
    }

    /// `self ÷ d` where the caller knows the division is exact.
    fn div_exact(&mut self, d: u64) {
        let r = self.div_rem(u32::try_from(d).expect("divisor below 2^32"));
        debug_assert!(r == 0, "inexact division");
    }

    pub fn add(&mut self, o: &Big) {
        let n = self.0.len().max(o.0.len());
        self.0.resize(n, 0);
        let mut carry = 0u64;
        for i in 0..n {
            let t = u64::from(self.0[i]) + u64::from(*o.0.get(i).unwrap_or(&0)) + carry;
            self.0[i] = t as u32;
            carry = t >> 32;
        }
        if carry > 0 {
            self.0.push(carry as u32);
        }
    }

    fn trim(&mut self) {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
    }

    /// Decimal digits (RF-10 compares these with Python's integers).
    #[cfg_attr(not(test), allow(dead_code))] // RF-10 and X6
    pub fn decimal(&self) -> String {
        let mut x = self.clone();
        let mut parts = Vec::new();
        while !x.is_zero() {
            parts.push(x.div_rem(1_000_000_000));
        }
        match parts.split_last() {
            None => "0".into(),
            Some((top, rest)) => rest.iter().rev().fold(top.to_string(), |s, p| format!("{s}{p:09}")),
        }
    }
}

impl Ord for Big {
    fn cmp(&self, o: &Big) -> Ordering {
        self.0.len().cmp(&o.0.len()).then_with(|| self.0.iter().rev().cmp(o.0.iter().rev()))
    }
}

impl PartialOrd for Big {
    fn partial_cmp(&self, o: &Big) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// C(n, k).
pub fn binom(n: u64, k: u64) -> Big {
    if k > n {
        return Big::from(0);
    }
    let k = k.min(n - k);
    let mut b = Big::from(1);
    for i in 0..k {
        b.mul(n - i);
        b.div_exact(i + 1); // C(n, i+1) = C(n, i)·(n−i)/(i+1) is an integer
    }
    b
}

/// A fraction `num/den` of big integers.
#[derive(Clone, Debug, PartialEq)]
pub struct Tail {
    pub num: Big,
    pub den: Big,
}

impl Tail {
    /// `num/den ≤ a/b`, exactly.
    pub fn le(&self, (a, b): Alpha) -> bool {
        let (mut l, mut r) = (self.num.clone(), self.den.clone());
        l.mul(b);
        r.mul(a);
        l <= r
    }

    /// The tail in parts per million, rounded down (records carry integers only).
    #[cfg_attr(not(test), allow(dead_code))] // RF-10 and X6
    pub fn ppm(&self) -> u64 {
        let mut scaled = self.num.clone();
        scaled.mul(1_000_000);
        let (mut lo, mut hi) = (0u64, 1_000_000u64);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let mut d = self.den.clone();
            d.mul(mid);
            if d <= scaled { lo = mid } else { hi = mid - 1 }
        }
        lo
    }
}

/// The exact exceedance test (ADR-010 §3): the threshold is the j-th largest of m baseline values and K the number of
/// the n window values strictly above it. Under exchangeability
/// `P(K = k) = C(j−1+k, k)·C(m−j+n−k, n−k) / C(m+n, n)`; this is `P(K ≥ k)`.
pub fn exceed_tail(m: u64, j: u64, n: u64, k: u64) -> Tail {
    let mut num = Big::from(0);
    for i in k..=n {
        num.add(&mul_big(&binom(j - 1 + i, i), &binom(m - j + n - i, n - i)));
    }
    Tail { num, den: binom(m + n, n) }
}

/// `a × b` for two bigs (schoolbook; the operands here have a few limbs).
pub fn mul_big(a: &Big, b: &Big) -> Big {
    let mut out = vec![0u64; a.0.len() + b.0.len() + 1];
    for (i, &x) in a.0.iter().enumerate() {
        let mut carry = 0u64;
        for (j, &y) in b.0.iter().enumerate() {
            let t = out[i + j] + u64::from(x) * u64::from(y) + carry;
            out[i + j] = t & 0xffff_ffff;
            carry = t >> 32;
        }
        let mut p = i + b.0.len();
        while carry > 0 {
            let t = out[p] + carry;
            out[p] = t & 0xffff_ffff;
            carry = t >> 32;
            p += 1;
        }
    }
    let mut r = Big(out.into_iter().map(|x| x as u32).collect());
    r.trim();
    r
}

/// The smallest K whose exceedance tail is ≤ α, or None when even K = n is not.
pub fn exceed_k(m: u64, j: u64, n: u64, alpha: Alpha) -> Option<u64> {
    (0..=n).find(|&k| exceed_tail(m, j, n, k).le(alpha))
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// `p = a/d` in lowest terms.
pub fn reduce(a: u64, d: u64) -> (u64, u64) {
    let g = gcd(a, d).max(1);
    (a / g, d / g)
}

/// The larger of two rates `a1/d1`, `a2/d2`, reduced.
pub fn max_rate(r1: (u64, u64), r2: (u64, u64)) -> (u64, u64) {
    let r = if u128::from(r1.0) * u128::from(r2.1) >= u128::from(r2.0) * u128::from(r1.1) { r1 } else { r2 };
    reduce(r.0, r.1)
}

/// The binomial upper tail `P(X ≥ k)` for `X ~ Bin(n, a/d)`: `Σ_{i≥k} C(n,i)·aⁱ·(d−a)ⁿ⁻ⁱ / dⁿ`. The terms are
/// stepped down exactly from `U_n = aⁿ`: `U_{i−1} = U_i·i·(d−a) / ((n−i+1)·a)`. `0 < a < d < 2³²`.
pub fn binom_tail(n: u64, k: u64, a: u64, d: u64) -> Tail {
    let mut terms = Terms::new(n, a, d);
    let mut num = Big::from(0);
    while let Some((i, u)) = terms.next() {
        if i < k {
            break;
        }
        num.add(u);
    }
    Tail { num, den: terms.den }
}

/// The binomial terms `U_n, U_{n−1}, …` and the denominator `dⁿ`.
struct Terms {
    n: u64,
    a: u64,
    b: u64,
    i: Option<u64>,
    u: Big,
    den: Big,
}

impl Terms {
    fn new(n: u64, a: u64, d: u64) -> Terms {
        let (mut u, mut den) = (Big::from(1), Big::from(1));
        for _ in 0..n {
            u.mul(a);
            den.mul(d);
        }
        Terms { n, a, b: d - a, i: None, u, den }
    }

    fn next(&mut self) -> Option<(u64, &Big)> {
        let i = match self.i {
            None => self.n,
            Some(0) => return None,
            Some(i) => {
                self.u.mul(i);
                self.u.mul(self.b);
                self.u.div_exact(self.n - i + 1);
                self.u.div_exact(self.a);
                i - 1
            }
        };
        self.i = Some(i);
        Some((i, &self.u))
    }
}

/// The smallest k with `P(X ≥ k) ≤ α`, or None. The tail grows as k falls, so the sum walks down from n and
/// stops at the first k whose tail passes α.
pub fn binom_k(n: u64, a: u64, d: u64, alpha: Alpha) -> Option<u64> {
    let mut terms = Terms::new(n, a, d);
    let mut rhs = terms.den.clone();
    rhs.mul(alpha.0);
    let (mut num, mut best) = (Big::from(0), None);
    while let Some((i, u)) = terms.next() {
        num.add(u);
        let mut lhs = num.clone();
        lhs.mul(alpha.1);
        if lhs > rhs {
            break;
        }
        best = Some(i);
    }
    best
}

/// The one-sided Fisher exact tail (ADR-011 §6.3, for X6): `P(X ≥ a)` with X hypergeometric — N items, K marked,
/// n drawn: `Σ_{x≥a} C(K,x)·C(N−K,n−x) / C(N,n)`.
#[cfg_attr(not(test), allow(dead_code))] // RF-10 and X6
pub fn fisher_tail(big_n: u64, big_k: u64, n: u64, a: u64) -> Tail {
    let mut num = Big::from(0);
    for x in a..=n.min(big_k) {
        if n - x <= big_n - big_k {
            num.add(&mul_big(&binom(big_k, x), &binom(big_n - big_k, n - x)));
        }
    }
    Tail { num, den: binom(big_n, n) }
}

/// Twice the median of `v` (so an even count stays an integer: the sum of the two middle values).
pub fn median2(v: &[u64]) -> u64 {
    let mut s = v.to_vec();
    s.sort_unstable();
    match s.len() {
        0 => 0,
        n if n % 2 == 1 => 2 * s[n / 2],
        n => s[n / 2 - 1] + s[n / 2],
    }
}

/// The j-th largest of `ring` (1-based), the exceedance threshold.
pub fn jth_largest(ring: &[u64], j: usize) -> Option<u64> {
    let mut s = ring.to_vec();
    s.sort_unstable_by(|a, b| b.cmp(a));
    s.get(j.checked_sub(1)?).copied()
}
