//! Exact integer money (kernel/proxy.md §3.8; kernel/config.md §3.1).
//!
//! Config money is a decimal string in USD, `^[0-9]{1,7}(\.[0-9]{1,6})?$`, parsed exactly. Internal
//! accounting is pico-USD (`pusd`, 10⁻¹² USD) in `u64`; everything that leaves the kernel is micro-USD
//! (`musd`), truncated toward zero. A price in USD per 1M tokens, scaled by 10⁶, is micro-USD per 1M
//! tokens, which equals pico-USD per token.

/// pico-USD per micro-USD.
pub const PUSD_PER_MUSD: u64 = 1_000_000;

/// Parse a decimal string with at most 7 integer and 6 fractional digits into units of 10⁻⁶.
/// `"0.003"` → 3,000; `"1.20"` → 1,200,000; `"5"` → 5,000,000.
pub fn parse_decimal_e6(s: &str) -> Option<u64> {
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    if int.is_empty() || int.len() > 7 || !int.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut v: u64 = int.parse().ok()?;
    v = v.checked_mul(1_000_000)?;
    if let Some(f) = frac {
        if f.is_empty() || f.len() > 6 || !f.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let mut digits = f.to_string();
        while digits.len() < 6 {
            digits.push('0');
        }
        v = v.checked_add(digits.parse::<u64>().ok()?)?;
    }
    Some(v)
}

/// A money string in USD → micro-USD.
pub fn usd_to_musd(s: &str) -> Option<u64> {
    parse_decimal_e6(s)
}

/// A price string in USD per 1M tokens → pico-USD per token.
pub fn price_pusd_per_token(s: &str) -> Option<u64> {
    parse_decimal_e6(s)
}

/// pico-USD → micro-USD, truncated toward zero (ICJ output rule).
pub fn pusd_to_musd(p: u64) -> u64 {
    p / PUSD_PER_MUSD
}

/// micro-USD → pico-USD (saturating).
pub fn musd_to_pusd(m: u64) -> u64 {
    m.saturating_mul(PUSD_PER_MUSD)
}

/// pico-USD → micro-USD, rounded up (used where a conservative upper bound is wanted).
pub fn pusd_to_musd_ceil(p: u64) -> u64 {
    p.div_ceil(PUSD_PER_MUSD)
}

/// A ratio in [0, 1] (TOML float) → permille, rounded to nearest.
pub fn ratio_to_permille(r: f64) -> Option<u32> {
    if !(0.0..=1.0).contains(&r) || r.is_nan() {
        return None;
    }
    Some((r * 1000.0).round() as u32)
}

/// `a × b / d` in u128, saturating to u64.
pub fn mul_div(a: u64, b: u64, d: u64) -> u64 {
    if d == 0 {
        return u64::MAX;
    }
    let v = (a as u128) * (b as u128) / (d as u128);
    u64::try_from(v).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_parse() {
        assert_eq!(price_pusd_per_token("0.003"), Some(3_000));
        assert_eq!(price_pusd_per_token("1.20"), Some(1_200_000));
        assert_eq!(usd_to_musd("5.00"), Some(5_000_000));
        assert_eq!(usd_to_musd("0"), Some(0));
        assert_eq!(usd_to_musd("9999999.999999"), Some(9_999_999_999_999));
        for bad in [
            "",
            ".5",
            "1.",
            "1.1234567",
            "12345678",
            "-1",
            "1e3",
            "0x1",
            " 1",
            "1,0",
        ] {
            assert_eq!(parse_decimal_e6(bad), None, "{bad:?}");
        }
        assert_eq!(pusd_to_musd(1_999_999), 1);
        assert_eq!(pusd_to_musd_ceil(1_000_001), 2);
        assert_eq!(ratio_to_permille(0.6), Some(600));
        assert_eq!(ratio_to_permille(1.2), None);
        assert_eq!(mul_div(u64::MAX, 2, 4), u64::MAX / 2);
    }
}
