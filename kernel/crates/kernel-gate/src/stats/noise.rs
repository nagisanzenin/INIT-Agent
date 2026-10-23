//! The noise floor δ(m) (kernel/gate.md §3.8.2): sign-flip splits of the LKG arm's own repeats,
//! smoothed across gates through `vault/gate/noise.json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::data::{Arm, Metric, TaskRuns, TaskSet, arm_stat, scaled};
use super::rng::SplitMix64;

/// `quantile_hi(v, q)` on a sorted vector: the element at `ceil(q·B) − 1` (the same index rule as the
/// bootstrap's upper bound, §3.8.3).
pub fn quantile_hi(sorted: &[f64], q: f64) -> f64 {
    let b = sorted.len();
    if b == 0 {
        return 0.0;
    }
    let k = ((q * b as f64) - 1e-9).ceil() as usize;
    sorted[k.clamp(1, b) - 1]
}

/// `noise(m, tasks, R)` of §3.8.2 on the LKG arm. `key` names the seed purpose (`"noise:" + key`), e.g.
/// `pass_rate`, `pass_rate@pool`, `cost_usd`. Returns `None` when R < 2 (no within-gate estimate) or no
/// task is given. R is the smallest LKG repeat count over the tasks.
pub fn noise(
    gate_id: &str,
    key: &str,
    m: Metric,
    tasks: &[&TaskRuns],
    b_noise: usize,
) -> Option<f64> {
    if tasks.is_empty() || b_noise == 0 {
        return None;
    }
    let r = tasks.iter().map(|t| t.p.len()).min().unwrap_or(0);
    if r < 2 {
        return None;
    }
    let h = r / 2;
    let mut rng = SplitMix64::for_gate(gate_id, &format!("noise:{key}"));
    let mut diff = Vec::with_capacity(b_noise);
    let mut a = vec![Vec::with_capacity(h); tasks.len()];
    let mut a2 = vec![Vec::with_capacity(h); tasks.len()];
    for _ in 0..b_noise {
        for i in 0..tasks.len() {
            let pi = rng.permutation(r);
            a[i].clear();
            a[i].extend_from_slice(&pi[..h]);
            a2[i].clear();
            a2[i].extend_from_slice(&pi[h..2 * h]);
        }
        let x = scaled(m, tasks, Arm::P, &|i| a[i].as_slice());
        let y = scaled(m, tasks, Arm::P, &|i| a2[i].as_slice());
        diff.push(x - y);
    }
    diff.sort_by(|x, y| x.total_cmp(y));
    let delta_half = quantile_hi(&diff, 0.95);
    Some(delta_half * (h as f64 / r as f64).sqrt())
}

/// One `noise.json` entry: `{"delta_log_ppm"|"delta_permille": int, "n", "repeats", "at"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoiseEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_log_ppm: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_permille: Option<i64>,
    pub n: u64,
    pub repeats: u64,
    pub at: String,
}

impl NoiseEntry {
    /// δ on the metric's comparison scale (fraction for linear metrics, natural log for log metrics).
    pub fn delta(&self) -> Option<f64> {
        match (self.delta_permille, self.delta_log_ppm) {
            (Some(p), _) => Some(p as f64 / 1000.0),
            (None, Some(l)) => Some(l as f64 / 1e6),
            _ => None,
        }
    }
}

/// `vault/gate/noise.json`: `{"v":1,"models":{"<model>":{"<metric key>": NoiseEntry}}}`. Metric keys are
/// [`Metric::name`], plus `pass_rate@pool` and `pass_rate@holdout` for the split-specific δ_p and δ_h.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoiseHistory {
    pub v: u32,
    pub models: BTreeMap<String, BTreeMap<String, NoiseEntry>>,
}

impl Default for NoiseHistory {
    fn default() -> Self {
        NoiseHistory {
            v: 1,
            models: BTreeMap::new(),
        }
    }
}

impl NoiseHistory {
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Serialized with sorted keys (BTreeMap) and integers only (ICJ-compatible).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("noise history serializes")
    }

    /// The entry for (model, key) if it is at most `max_age_days` old at `now_unix`.
    pub fn recent(
        &self,
        model: &str,
        key: &str,
        now_unix: i64,
        max_age_days: u32,
    ) -> Option<&NoiseEntry> {
        let e = self.models.get(model)?.get(key)?;
        let at = parse_rfc3339(&e.at)?;
        let age = now_unix - at;
        if age >= 0 && age <= max_age_days as i64 * 86_400 {
            Some(e)
        } else {
            None
        }
    }
}

/// Smoothing across gates (§3.8.2): `δ_used = sqrt(w·δ_gate² + (1 − w)·δ_hist²)` with
/// `δ_hist = δ_prev·sqrt(n_prev / n)`, w = 0.5 when a recent history value exists, else 1. With no
/// within-gate value (R = 1) δ_hist alone is used; with neither the result is `None`
/// (`inconclusive: no_noise_estimate`).
pub fn smooth(delta_gate: Option<f64>, hist: Option<&NoiseEntry>, n: usize) -> Option<f64> {
    let delta_hist = hist.and_then(|e| {
        let d = e.delta()?;
        if n == 0 || e.n == 0 {
            return Some(d);
        }
        Some(d * (e.n as f64 / n as f64).sqrt())
    });
    match (delta_gate, delta_hist) {
        (Some(g), Some(h)) => Some((0.5 * g * g + 0.5 * h * h).sqrt()),
        (Some(g), None) => Some(g),
        (None, Some(h)) => Some(h),
        (None, None) => None,
    }
}

/// δ_used for every metric of §3.8.1, plus the split-specific pass-rate floors.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Deltas {
    /// δ(pass_rate) over pool ∪ holdout.
    pub pass_all: f64,
    /// δ_p: pool only.
    pub pass_pool: f64,
    /// δ_h: holdout only.
    pub pass_holdout: f64,
    /// Every metric with a measured δ (log scale for log metrics). `pass_rate` equals `pass_all`.
    pub metrics: BTreeMap<Metric, f64>,
}

impl Deltas {
    /// δ_used(m); 0 when unmeasured (evolution.md §3.10.1 then marks the verdict `delta_unmeasured`).
    pub fn get(&self, m: Metric) -> f64 {
        if m == Metric::PassRate {
            return self.pass_all;
        }
        self.metrics.get(&m).copied().unwrap_or(0.0)
    }

    /// Synthetic deltas for tests and offline simulation.
    pub fn uniform(pass: f64, log: f64) -> Self {
        let mut metrics = BTreeMap::new();
        for m in Metric::ALL {
            metrics.insert(m, if m.is_log() { log } else { pass });
        }
        Deltas {
            pass_all: pass,
            pass_pool: pass,
            pass_holdout: pass,
            metrics,
        }
    }
}

/// History context for [`compute_deltas`].
pub struct HistoryCtx<'a> {
    pub history: &'a NoiseHistory,
    pub model: &'a str,
    pub now_unix: i64,
    /// `gate.g4.noise_history_days` (30).
    pub max_age_days: u32,
}

/// Why δ could not be computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoNoiseEstimate {
    pub missing: Vec<String>,
}

/// Computes δ_used for all metrics and splits, and the `noise.json` entries to store back.
/// `pass_rate` (all, pool, holdout), `cost_usd`, `wall_s` and `cost_x_wall` are required; a missing one
/// gives `no_noise_estimate`. Other metrics are reported only when measurable.
pub fn compute_deltas(
    gate_id: &str,
    tasks: &[&TaskRuns],
    hist: Option<&HistoryCtx<'_>>,
    b_noise: usize,
    now_rfc3339: &str,
) -> Result<(Deltas, BTreeMap<String, NoiseEntry>), NoNoiseEstimate> {
    let pool: Vec<&TaskRuns> = tasks
        .iter()
        .copied()
        .filter(|t| t.set == TaskSet::Pool)
        .collect();
    let holdout: Vec<&TaskRuns> = tasks
        .iter()
        .copied()
        .filter(|t| t.set == TaskSet::Holdout)
        .collect();
    let repeats = tasks.iter().map(|t| t.p.len()).min().unwrap_or(0) as u64;
    let mut out = Deltas::default();
    let mut store = BTreeMap::new();
    let mut missing = Vec::new();

    let mut one = |key: &str, m: Metric, set: &[&TaskRuns], required: bool| -> Option<f64> {
        let g = noise(gate_id, key, m, set, b_noise);
        let h = hist.and_then(|c| c.history.recent(c.model, key, c.now_unix, c.max_age_days));
        let used = smooth(g, h, set.len());
        match used {
            Some(d) => {
                let entry = if m.is_log() {
                    NoiseEntry {
                        delta_log_ppm: Some((d * 1e6).round() as i64),
                        delta_permille: None,
                        n: set.len() as u64,
                        repeats,
                        at: now_rfc3339.to_string(),
                    }
                } else {
                    NoiseEntry {
                        delta_log_ppm: None,
                        delta_permille: Some((d * 1000.0).round() as i64),
                        n: set.len() as u64,
                        repeats,
                        at: now_rfc3339.to_string(),
                    }
                };
                store.insert(key.to_string(), entry);
            }
            None if required => missing.push(key.to_string()),
            None => {}
        }
        used
    };

    let all = one("pass_rate", Metric::PassRate, tasks, true);
    let p = one("pass_rate@pool", Metric::PassRate, &pool, true);
    let h = one("pass_rate@holdout", Metric::PassRate, &holdout, true);
    for m in Metric::ALL {
        if m == Metric::PassRate {
            continue;
        }
        let required = matches!(m, Metric::CostUsd | Metric::WallS | Metric::CostXWall);
        if let Some(d) = one(m.name(), m, tasks, required) {
            out.metrics.insert(m, d);
        }
    }
    if !missing.is_empty() {
        return Err(NoNoiseEstimate { missing });
    }
    out.pass_all = all.unwrap_or(0.0);
    out.pass_pool = p.unwrap_or(0.0);
    out.pass_holdout = h.unwrap_or(0.0);
    out.metrics.insert(Metric::PassRate, out.pass_all);
    Ok((out, store))
}

/// Reported value of δ in the metric's natural unit at the LKG level (§3.8.2): permille for linear
/// metrics; `round(T_m(p)·(e^δ − 1))` (µUSD, ms, tokens, count) for log metrics.
pub fn natural_delta(m: Metric, delta: f64, tasks: &[&TaskRuns]) -> i64 {
    if m.is_log() {
        let t = if m == Metric::CostXWall {
            arm_stat(Metric::CostUsd, tasks, Arm::P) * arm_stat(Metric::WallS, tasks, Arm::P)
        } else {
            arm_stat(m, tasks, Arm::P)
        };
        (t * (delta.exp() - 1.0)).round() as i64
    } else {
        (delta * 1000.0).round() as i64
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parses the RFC 3339 UTC form the kernel writes (`YYYY-MM-DDTHH:MM:SS[.fff]Z`) to Unix seconds.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse::<i64>().ok() };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let rest = &s[19..];
    let rest = rest
        .strip_prefix('.')
        .map(|r| r.trim_start_matches(|c: char| c.is_ascii_digit()))
        .unwrap_or(rest);
    if rest != "Z"
        || !(1..=12).contains(&mo)
        || !(1..=31).contains(&d)
        || h > 23
        || mi > 59
        || se > 60
    {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se)
}

/// Formats Unix seconds as `YYYY-MM-DDTHH:MM:SS.000Z`.
pub fn format_rfc3339(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // civil_from_days (Hinnant)
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::super::data::fixtures::task;
    use super::super::data::{RunValues, Source, TaskSet};
    use super::*;

    /// G-26: R = 2 binary data with known p → δ within 10% of the analytic
    /// `1.645·√(Σ p_i(1−p_i)) / n`. 100 tasks with p = 0.5; exactly the expected half disagree.
    #[test]
    fn g26_noise_matches_analytic_binary_r2() {
        let n = 100;
        let tasks: Vec<TaskRuns> = (0..n)
            .map(|i| {
                let p: &[u32] = if i % 2 == 0 {
                    &[1000, 0]
                } else if i % 4 == 1 {
                    &[1000, 1000]
                } else {
                    &[0, 0]
                };
                task(
                    &format!("t{i}"),
                    TaskSet::Pool,
                    Source::Owner,
                    &[1000, 1000],
                    p,
                )
            })
            .collect();
        let refs: Vec<&TaskRuns> = tasks.iter().collect();
        let d = noise("j-000026", "pass_rate", Metric::PassRate, &refs, 2000).unwrap();
        let analytic = 1.645 * (n as f64 * 0.25).sqrt() / n as f64;
        assert!(
            (d - analytic).abs() / analytic < 0.10,
            "δ = {d}, analytic = {analytic}"
        );
        // Bit-for-bit reproducible.
        assert_eq!(
            d,
            noise("j-000026", "pass_rate", Metric::PassRate, &refs, 2000).unwrap()
        );
    }

    #[test]
    fn noise_needs_two_repeats_and_is_zero_for_deterministic_tasks() {
        let t1 = task("a", TaskSet::Pool, Source::Owner, &[1000], &[1000]);
        assert_eq!(noise("j", "pass_rate", Metric::PassRate, &[&t1], 100), None);
        let t2 = task(
            "b",
            TaskSet::Pool,
            Source::Owner,
            &[1000, 1000],
            &[1000, 1000],
        );
        assert_eq!(
            noise("j", "pass_rate", Metric::PassRate, &[&t2], 100),
            Some(0.0)
        );
    }

    #[test]
    fn noise_on_log_metric_scales_with_spread() {
        let mut t = task(
            "a",
            TaskSet::Pool,
            Source::Owner,
            &[1000, 1000],
            &[1000, 1000],
        );
        t.p[0] = RunValues {
            cost_ref_musd: 10_000,
            ..t.p[0]
        };
        t.p[1] = RunValues {
            cost_ref_musd: 40_000,
            ..t.p[1]
        };
        let d = noise("j", "cost_usd", Metric::CostUsd, &[&t], 1000).unwrap();
        // h = 1, R = 2: |ln 40000 − ln 10000| · sqrt(1/2)
        assert!((d - 4f64.ln() * 0.5f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn smoothing_rules() {
        let e = NoiseEntry {
            delta_log_ppm: None,
            delta_permille: Some(100),
            n: 16,
            repeats: 2,
            at: "2026-10-19T00:00:00.000Z".into(),
        };
        // both present: sqrt(0.5·g² + 0.5·h²), h scaled by sqrt(n_prev / n)
        let s = smooth(Some(0.2), Some(&e), 64).unwrap();
        let h = 0.1 * (16f64 / 64.0).sqrt();
        assert!((s - (0.5 * 0.04 + 0.5 * h * h).sqrt()).abs() < 1e-12);
        assert_eq!(smooth(Some(0.2), None, 16), Some(0.2));
        assert_eq!(smooth(None, Some(&e), 16), Some(0.1));
        assert_eq!(smooth(None, None, 16), None);
    }

    #[test]
    fn history_recency_and_round_trip() {
        let mut h = NoiseHistory::default();
        let e = NoiseEntry {
            delta_log_ppm: Some(95_000),
            delta_permille: None,
            n: 16,
            repeats: 2,
            at: "2026-10-01T00:00:00.000Z".into(),
        };
        h.models
            .entry("deepseek/deepseek-flash".into())
            .or_default()
            .insert("cost_usd".into(), e.clone());
        let now = parse_rfc3339("2026-10-20T12:00:00.000Z").unwrap();
        assert_eq!(
            h.recent("deepseek/deepseek-flash", "cost_usd", now, 30),
            Some(&e)
        );
        let later = parse_rfc3339("2026-11-15T00:00:00Z").unwrap();
        assert_eq!(
            h.recent("deepseek/deepseek-flash", "cost_usd", later, 30),
            None
        );
        let back = NoiseHistory::parse(&h.to_json()).unwrap();
        assert_eq!(back, h);
        assert!(h.to_json().contains(r#""delta_log_ppm":95000"#));
        assert_eq!(format_rfc3339(now), "2026-10-20T12:00:00.000Z");
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn compute_deltas_requires_estimates() {
        let t1 = task("a", TaskSet::Pool, Source::Owner, &[1000], &[1000]);
        let t2 = task("b", TaskSet::Holdout, Source::Owner, &[1000], &[1000]);
        let err =
            compute_deltas("j", &[&t1, &t2], None, 100, "2026-10-20T00:00:00.000Z").unwrap_err();
        assert!(err.missing.contains(&"pass_rate".to_string()));
        let t3 = task("c", TaskSet::Pool, Source::Owner, &[1000, 0], &[1000, 0]);
        let t4 = task("d", TaskSet::Holdout, Source::Owner, &[1000, 0], &[0, 1000]);
        let (d, store) =
            compute_deltas("j", &[&t3, &t4], None, 200, "2026-10-20T00:00:00.000Z").unwrap();
        assert!(d.pass_all > 0.0 && d.pass_pool > 0.0 && d.pass_holdout > 0.0);
        assert!(store.contains_key("pass_rate@pool") && store.contains_key("cost_x_wall"));
        assert_eq!(d.get(Metric::PassRate), d.pass_all);
    }
}
