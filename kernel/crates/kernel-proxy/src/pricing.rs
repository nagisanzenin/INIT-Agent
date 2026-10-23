//! Measurement cost (kernel/proxy.md §3.10.3) and reservations (§3.6): price windows, long-context
//! tiers, multipliers. Integer pico-USD only.

use crate::config::{ModelTable, Prices, Provider};
use crate::money::mul_div;
use crate::tap::Usage;
use crate::time::{civil_from_days, date_at_offset, weekday};

/// ×1 in parts per million.
pub const PPM_ONE: u64 = 1_000_000;

/// The price window in force at `ms` (proxy.md §3.10.3): `default_dates` first, then the first
/// matching window (half-open `[from, to)` in the schedule's offset), else the default. No schedule:
/// `standard`.
pub fn window_at(p: &Provider, ms: u64) -> String {
    let Some(s) = &p.schedule else {
        return "standard".into();
    };
    if !s.default_dates.is_empty()
        && s.default_dates
            .contains(&date_at_offset(ms, s.dates_offset_min))
    {
        return s.default.clone();
    }
    let local = ms as i64 + s.offset_min * 60_000;
    let days = local.div_euclid(86_400_000);
    let minute = (local.rem_euclid(86_400_000) / 60_000) as u32;
    let wd = weekday(days);
    let _ = civil_from_days(days);
    for w in &s.windows {
        if w.days & (1 << wd) != 0 && minute >= w.from_min && minute < w.to_min {
            return w.use_.clone();
        }
    }
    s.default.clone()
}

/// The prices of `window`, honouring long-context tiers by `input_tokens`.
pub fn prices_for(m: &ModelTable, window: &str, input_tokens: u64) -> Prices {
    let mut p = m.prices.get(window).copied().unwrap_or_else(|| highest(m));
    for (above, tp) in &m.tiers {
        if input_tokens > *above
            && let Some(x) = tp.get(window)
        {
            p = *x;
        }
    }
    p
}

/// Per-class maximum over every window and tier: the "highest configured window" over-count.
pub fn highest(m: &ModelTable) -> Prices {
    let mut h = Prices::default();
    let all = m
        .prices
        .values()
        .chain(m.tiers.iter().flat_map(|(_, t)| t.values()));
    for p in all {
        h.input = h.input.max(p.input);
        h.cache_read = h.cache_read.max(p.cache_read);
        h.cache_write = h.cache_write.max(p.cache_write);
        h.cache_write_1h = h.cache_write_1h.max(p.cache_write_1h);
        h.output = h.output.max(p.output);
    }
    h
}

/// `Σ tokens[class] × price[class] × multiplier`.
pub fn cost_pusd(u: &Usage, p: &Prices, mult_ppm: u64) -> u64 {
    let base = (u.input_uncached as u128) * (p.input as u128)
        + (u.input_cache_read as u128) * (p.cache_read as u128)
        + (u.input_cache_write as u128) * (p.cache_write as u128)
        + (u.input_cache_write_1h as u128) * (p.cache_write_1h as u128)
        + (u.output as u128) * (p.output as u128);
    let v = base * (mult_ppm as u128) / (PPM_ONE as u128);
    u64::try_from(v).unwrap_or(u64::MAX)
}

/// `ceil(req_bytes / bytes_per_token) × input + max_out × output`, × the multiplier (§3.6).
pub fn reserve_pusd(
    req_bytes: u64,
    bytes_per_token: u64,
    max_out: u64,
    p: &Prices,
    mult_ppm: u64,
) -> u64 {
    let input_tokens = req_bytes.div_ceil(bytes_per_token.max(1));
    let base = (input_tokens as u128) * (p.input as u128) + (max_out as u128) * (p.output as u128);
    let v = base * (mult_ppm as u128) / (PPM_ONE as u128);
    u64::try_from(v).unwrap_or(u64::MAX)
}

/// One request's reservation for `m` (pico-USD) when it leaves `max_tokens` to the proxy, in the
/// dearest price window: the model's `reserve_output_tokens` at its highest window output price, no
/// input, ×1. It is the floor a G4 task's cost cap must clear (`gate.g4.task_cost_cap`, K-GATE). A
/// lower cap admits no such request in that window, so the run's outcome would depend on the time
/// of day. Long-context tiers are left out, because they apply only to large inputs. Install sets
/// `reserve_output_tokens` to the seed's max_tokens.
pub fn request_reservation_pusd(m: &ModelTable) -> u64 {
    let out = m.prices.values().map(|p| p.output).max().unwrap_or(0);
    u64::try_from((m.reserve_output_tokens as u128) * (out as u128)).unwrap_or(u64::MAX)
}

/// The estimate when no usage arrived (§3.10.3 "No usage"): input at `ceil(bytes/3)`, output at
/// `ceil(body_bytes/3)`, capped at the reservation.
pub fn estimate_pusd(req_bytes: u64, body_bytes: u64, p: &Prices, mult_ppm: u64, cap: u64) -> u64 {
    let u = Usage {
        input_uncached: req_bytes.div_ceil(3),
        output: body_bytes.div_ceil(3),
        ..Usage::default()
    };
    cost_pusd(&u, p, mult_ppm).min(cap)
}

/// Values a provider reports for the default tier: priced at ×1 unless configured.
pub fn neutral(v: &str) -> bool {
    matches!(
        v,
        "default" | "standard" | "global" | "auto" | "not_available" | ""
    )
}

/// The multiplier for a set of `"<key>:<value>"` modifiers. Returns `(ppm, unpriced)`: a modifier
/// without a configured multiplier is charged at the largest configured one and flagged.
pub fn multiplier_ppm(p: &Provider, modifiers: &[String]) -> (u64, bool) {
    let mut m: u128 = PPM_ONE as u128;
    let mut unpriced = false;
    for k in modifiers {
        let f = match p.multipliers.get(k) {
            Some(f) => *f,
            None => {
                let v = k.split_once(':').map(|x| x.1).unwrap_or("");
                if neutral(v) {
                    continue;
                }
                unpriced = true;
                p.multipliers
                    .values()
                    .copied()
                    .max()
                    .unwrap_or(PPM_ONE)
                    .max(PPM_ONE)
            }
        };
        m = m * (f as u128) / (PPM_ONE as u128);
    }
    (u64::try_from(m).unwrap_or(u64::MAX), unpriced)
}

/// pico-USD at a Root price table given in micro-USD per 1M tokens (= pico-USD per token), with
/// cache reads at the input price (root/egress.md §3.4, the safe direction).
pub fn root_cost_pusd(u: &Usage, input_musd_per_mtok: u64, output_musd_per_mtok: u64) -> u64 {
    mul_div(u.input_total(), input_musd_per_mtok, 1).saturating_add(mul_div(
        u.output,
        output_musd_per_mtok,
        1,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProxyConfig;
    use crate::time::parse_rfc3339;

    fn ds() -> crate::config::Compiled {
        let c: ProxyConfig = toml::from_str(crate::config::tests_support::DEEPSEEK).unwrap();
        c.compile().unwrap()
    }

    /// K-GATE: the floor is `reserve_output_tokens` at the dearest window's output price
    /// (deepseek-flash here: 131,072 × $1.20/Mtok peak ≈ $0.157; not the $0.60 off-peak price).
    #[test]
    fn request_reservation_is_the_default_output_at_the_dearest_price() {
        let k = ds();
        let m = k.model("deepseek", "deepseek-flash").unwrap();
        assert_eq!(request_reservation_pusd(m), 131_072 * 1_200_000);
        assert_eq!(crate::money::pusd_to_musd(request_reservation_pusd(m)), 157_286);
    }

    /// P-07.
    #[test]
    fn p07_windows_and_cost() {
        let k = ds();
        let p = &k.providers["deepseek"];
        let at = |s: &str| window_at(p, parse_rfc3339(s).unwrap());
        // 2026-10-19 is a Monday; 2026-10-24 a Saturday; 2026-10-01 is listed (a Thursday, +08:00 date).
        assert_eq!(at("2026-10-19T00:59:59Z"), "offpeak");
        assert_eq!(at("2026-10-19T01:00:00Z"), "peak");
        assert_eq!(at("2026-10-19T03:59:59Z"), "peak");
        assert_eq!(at("2026-10-19T04:00:00Z"), "offpeak");
        assert_eq!(at("2026-10-19T06:00:00Z"), "peak");
        assert_eq!(at("2026-10-24T02:00:00Z"), "offpeak");
        assert_eq!(at("2026-10-01T02:00:00Z"), "offpeak");
        assert_eq!(at("2026-10-02T02:00:00Z"), "peak");
        let m = k.model("deepseek", "deepseek-flash").unwrap();
        let u = Usage {
            input_uncached: 875,
            input_cache_read: 118_016,
            output: 214,
            reasoning: 96,
            ..Usage::default()
        };
        // peak: 875 × 300,000 + 118,016 × 6,000 + 214 × 1,200,000 = 1,227,396,000 pusd.
        assert_eq!(
            cost_pusd(&u, &prices_for(m, "peak", u.input_total()), PPM_ONE),
            1_227_396_000
        );
        assert_eq!(
            cost_pusd(&u, &prices_for(m, "offpeak", u.input_total()), PPM_ONE),
            613_698_000
        );
        // cost_ref is always the reference window (peak).
        assert_eq!(m.reference_window, "peak");
        assert_eq!(
            reserve_pusd(9, 3, 10, &prices_for(m, "peak", 3), PPM_ONE),
            3 * 300_000 + 10 * 1_200_000
        );
        assert_eq!(
            estimate_pusd(9, 30, &prices_for(m, "peak", 3), PPM_ONE, 1),
            1
        );
    }

    #[test]
    fn multipliers() {
        let mut k = ds();
        let p = k.providers.get_mut("deepseek").unwrap();
        p.multipliers.insert("service_tier:flex".into(), 500_000);
        p.multipliers.insert("speed:fast".into(), 2_000_000);
        assert_eq!(
            multiplier_ppm(p, &["service_tier:flex".into()]),
            (500_000, false)
        );
        assert_eq!(
            multiplier_ppm(p, &["service_tier:default".into()]),
            (PPM_ONE, false)
        );
        assert_eq!(
            multiplier_ppm(p, &["service_tier:priority".into()]),
            (2_000_000, true)
        );
        assert_eq!(
            root_cost_pusd(
                &Usage {
                    input_uncached: 10,
                    input_cache_read: 90,
                    output: 5,
                    ..Usage::default()
                },
                300_000,
                1_200_000
            ),
            100 * 300_000 + 5 * 1_200_000
        );
    }
}
