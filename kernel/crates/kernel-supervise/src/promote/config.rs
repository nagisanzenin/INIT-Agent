//! The `[gate]`, `[budget]` and `[evolution]` keys the pipeline reads (kernel/config.md §4.6,
//! §4.4; gate.md §4.3), with their defaults. The full cross-section validation is config.md's
//! loader; this reader is lenient (unknown keys ignored) and never weakens a default silently: a
//! key of the wrong type is an error.

use kernel_gate::accept::G4Config;
use std::path::Path;
use toml::Table;

/// `[gate]` and the soft budgets the gate uses.
#[derive(Clone, Debug, PartialEq)]
pub struct GateCfg {
    /// `gate.g0.timeout_s`.
    pub g0_timeout_s: u64,
    /// `gate.g2.window_s` (15; tests shorten it).
    pub g2_window_s: u64,
    /// `gate.g2.ready_timeout_s` (90).
    pub g2_ready_timeout_s: u64,
    /// The pause before the one parent-probe retry (30 s).
    pub g2_parent_retry_s: u64,
    /// `gate.g3.mode`: `model` | `scripted`.
    pub g3_mode: String,
    pub g3_agent_timeout_s: u64,
    pub g3_inner_timeout_s: u64,
    /// The quiet period that ends an agent part (`last_finish` stop, nothing in flight; 5 s).
    pub quiet_ms: u64,
    pub g4: G4Config,
    pub pool_k: usize,
    pub holdout_h: usize,
    pub canaries: usize,
    pub repeats: u32,
    pub repeats_max: u32,
    pub parallel: usize,
    pub wall_s: u64,
    pub task_wall_s: u64,
    pub futility_alpha: f64,
    pub noise_history_days: u32,
    /// `budget.gate_usd`, `budget.quick_gate_usd`, `budget.probe_usd` (µUSD).
    pub gate_musd: u64,
    pub quick_gate_musd: u64,
    pub probe_musd: u64,
    /// The effective soft day (µUSD) and `budget.evolution_share` (permille) for `cost_guard`.
    pub soft_day_musd: u64,
    pub evolution_share_permille: u64,
    /// `evolution.g4_escalate_repeats` (4), `evolution.provisional_max_age_s` (86,400).
    pub escalate_repeats: u32,
    pub provisional_max_age_s: u64,
    /// `evolution.tasks.agent_stratum_max` (6).
    pub agent_stratum_max: usize,
    /// F12: G4 infra errors retried at most this many times.
    pub g4_infra_retries: u32,
    /// M7 `gate.g4.tier`: `auto` (default), `lean`, `standard` or `strong` (m7-design.md §4.3).
    pub g4_tier: kernel_gate::g4v2::TierCfg,
    /// G4 v2's pre-flight per-run estimate (µUSD): `gate.g4.cost_prior_usd` when set (M7: the cost
    /// probe's p90), else m7-design.md §3.2.6's $0.005. config.md's $0.05 default is not used here:
    /// at $0.05 per run no tier fits a $3 gate budget (build/status/GATE-V2a.md, deviation).
    pub g4_run_estimate_musd: u64,
}

impl Default for GateCfg {
    fn default() -> Self {
        GateCfg {
            g0_timeout_s: 900,
            g2_window_s: kernel_gate::g2::WINDOW_S,
            g2_ready_timeout_s: kernel_gate::g2::READY_TIMEOUT_S,
            g2_parent_retry_s: kernel_gate::g2::PARENT_RETRY_S,
            g3_mode: "model".into(),
            g3_agent_timeout_s: kernel_gate::g3::AGENT_TIMEOUT_S,
            g3_inner_timeout_s: kernel_gate::g3::INNER_TIMEOUT_S,
            quiet_ms: kernel_gate::g3::QUIET_S * 1000,
            g4: G4Config::default(),
            pool_k: 12,
            holdout_h: 8,
            canaries: 2,
            repeats: 2,
            repeats_max: 3,
            parallel: 4,
            wall_s: 5400,
            task_wall_s: 900,
            futility_alpha: 0.01,
            noise_history_days: 30,
            gate_musd: 3_000_000,
            quick_gate_musd: 500_000,
            probe_musd: 250_000,
            soft_day_musd: 4_000_000,
            evolution_share_permille: 600,
            escalate_repeats: 4,
            provisional_max_age_s: 86_400,
            agent_stratum_max: 6,
            g4_infra_retries: 2,
            g4_tier: kernel_gate::g4v2::TierCfg::Auto,
            g4_run_estimate_musd: kernel_gate::g4v2::DEFAULT_RUN_MUSD,
        }
    }
}

/// G3's time bounds (ADR-011 X0c, RELEASE_PLAN G3-STAGE-TIMEOUT), derived from the build timeouts.
/// The fixed 300 s stage defaults were shorter than a cold slot's body build (about 280 s) plus the
/// model's turns, so a promote right after the slot was cleared failed G3 with `timeout` (M7 D,
/// j-000010).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct G3Bounds {
    /// The agent part: `gate.g3.agent_timeout_s` for the model's turns, plus a cold build, because the
    /// agent's `selfmap --write` builds the body in the probe's fresh target.
    pub agent: std::time::Duration,
    /// The inner gate's G0–G2 after its build (`gate.g3.inner_timeout_s`). The stage clock starts
    /// when the inner build ends: the build is bounded by the slot's own timeout (warm or cold).
    pub inner_after_build: std::time::Duration,
    /// `[supervise] build_timeout_cold_s` · `build_timeout_warm_s`.
    pub build_cold: std::time::Duration,
    pub build_warm: std::time::Duration,
}

impl G3Bounds {
    /// The whole inner stage's bound for a build that was cold or warm.
    pub fn inner_limit(&self, cold: bool) -> std::time::Duration {
        self.inner_after_build + if cold { self.build_cold } else { self.build_warm }
    }

    /// Whether the inner gate finished in time: `total` since it started, of which `build` was the
    /// build's own wall time (bounded by the slot, so never counted against the stage).
    pub fn inner_in_time(&self, total: std::time::Duration, build: std::time::Duration) -> bool {
        total.saturating_sub(build) <= self.inner_after_build
    }
}

impl GateCfg {
    /// [`G3Bounds`] for this gate configuration and the supervisor's build timeouts.
    pub fn g3_bounds(&self, sup: &crate::config::SuperviseCfg) -> G3Bounds {
        use std::time::Duration;
        let build_cold = Duration::from_secs(sup.build_timeout_cold_s);
        G3Bounds {
            agent: Duration::from_secs(self.g3_agent_timeout_s) + build_cold,
            inner_after_build: Duration::from_secs(self.g3_inner_timeout_s),
            build_cold,
            build_warm: Duration::from_secs(sup.build_timeout_warm_s),
        }
    }
}

fn usd_to_musd(s: &str) -> Option<u64> {
    let (i, f) = s.split_once('.').unwrap_or((s, ""));
    if f.len() > 6 || i.is_empty() || !i.bytes().all(|b| b.is_ascii_digit()) || !f.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let frac: u64 = format!("{f:0<6}").parse().ok()?;
    i.parse::<u64>().ok()?.checked_mul(1_000_000)?.checked_add(frac)
}

impl GateCfg {
    /// Read `kernel/config.toml` (missing file: defaults).
    pub fn load(path: &Path) -> Result<GateCfg, Vec<String>> {
        match std::fs::read_to_string(path) {
            Ok(t) => GateCfg::parse(&t),
            Err(_) => Ok(GateCfg::default()),
        }
    }

    pub fn parse(text: &str) -> Result<GateCfg, Vec<String>> {
        let t: Table = toml::from_str(text).map_err(|e| vec![format!("config.toml: {e}")])?;
        let mut c = GateCfg::default();
        let mut errs = Vec::new();
        let get = |path: &[&str]| -> Option<&toml::Value> {
            let mut v: &toml::Value = t.get(path[0])?;
            for k in &path[1..] {
                v = v.get(k)?;
            }
            Some(v)
        };
        macro_rules! int {
            ($field:expr, $($k:literal).+) => {{
                let p = [$($k),+];
                if let Some(v) = get(&p) {
                    match v.as_integer().and_then(|i| i.try_into().ok()) {
                        Some(x) => $field = x,
                        None => errs.push(format!("{}: expected a non-negative integer", p.join("."))),
                    }
                }
            }};
        }
        macro_rules! float {
            ($field:expr, $($k:literal).+) => {{
                let p = [$($k),+];
                if let Some(v) = get(&p) {
                    match v.as_float().or_else(|| v.as_integer().map(|i| i as f64)) {
                        Some(x) => $field = x,
                        None => errs.push(format!("{}: expected a number", p.join("."))),
                    }
                }
            }};
        }
        macro_rules! usd {
            ($field:expr, $($k:literal).+) => {{
                let p = [$($k),+];
                if let Some(v) = get(&p) {
                    match v.as_str().and_then(usd_to_musd) {
                        Some(x) => $field = x,
                        None => errs.push(format!("{}: expected a decimal string like \"3.00\"", p.join("."))),
                    }
                }
            }};
        }
        int!(c.g0_timeout_s, "gate"."g0"."timeout_s");
        int!(c.g2_window_s, "gate"."g2"."window_s");
        int!(c.g2_ready_timeout_s, "gate"."g2"."ready_timeout_s");
        int!(c.g2_parent_retry_s, "gate"."g2"."parent_retry_s");
        if let Some(v) = get(&["gate", "g3", "mode"]) {
            match v.as_str() {
                Some(m @ ("model" | "scripted")) => c.g3_mode = m.into(),
                _ => errs.push("gate.g3.mode: expected \"model\" or \"scripted\"".into()),
            }
        }
        int!(c.g3_agent_timeout_s, "gate"."g3"."agent_timeout_s");
        int!(c.g3_inner_timeout_s, "gate"."g3"."inner_timeout_s");
        int!(c.quiet_ms, "gate"."quiet_ms");
        int!(c.pool_k, "gate"."g4"."pool_k");
        int!(c.holdout_h, "gate"."g4"."holdout_h");
        int!(c.canaries, "gate"."g4"."canaries");
        int!(c.repeats, "gate"."g4"."repeats");
        int!(c.repeats_max, "gate"."g4"."repeats_max");
        int!(c.parallel, "gate"."g4"."parallel");
        int!(c.wall_s, "gate"."g4"."wall_s");
        int!(c.task_wall_s, "gate"."g4"."task_wall_s");
        int!(c.noise_history_days, "gate"."g4"."noise_history_days");
        int!(c.g4.min_pool, "gate"."g4"."min_pool");
        int!(c.g4.min_holdout, "gate"."g4"."min_holdout");
        int!(c.g4.min_canaries, "gate"."g4"."min_canaries");
        int!(c.g4.bootstrap_b, "gate"."g4"."bootstrap_b");
        int!(c.g4.noise_b, "gate"."g4"."noise_b");
        int!(c.g4.capability_min_tasks, "gate"."g4"."capability_min_tasks");
        float!(c.g4.alpha, "gate"."g4"."alpha");
        float!(c.futility_alpha, "gate"."g4"."futility_alpha");
        float!(c.g4.margin_factor, "gate"."g4"."margin_factor");
        float!(c.g4.margin_max, "gate"."g4"."margin_max");
        float!(c.g4.rho_max, "gate"."g4"."rho_max");
        if let Some(v) = get(&["gate", "g4", "ct_include_wall"]) {
            match v.as_bool() {
                Some(b) => c.g4.ct_include_wall = b,
                None => errs.push("gate.g4.ct_include_wall: expected a boolean".into()),
            }
        }
        if let Some(v) = get(&["gate", "g4", "tier"]) {
            match v.as_str().and_then(kernel_gate::g4v2::TierCfg::parse) {
                Some(t) => c.g4_tier = t,
                None => errs.push("gate.g4.tier: expected \"auto\", \"lean\", \"standard\" or \"strong\"".into()),
            }
        }
        usd!(c.g4_run_estimate_musd, "gate"."g4"."cost_prior_usd");
        usd!(c.gate_musd, "budget"."gate_usd");
        usd!(c.quick_gate_musd, "budget"."quick_gate_usd");
        usd!(c.probe_musd, "budget"."probe_usd");
        usd!(c.soft_day_musd, "budget"."day_usd");
        if let Some(v) = get(&["budget", "evolution_share"]) {
            match v.as_float() {
                Some(x) if (0.0..=1.0).contains(&x) => c.evolution_share_permille = (x * 1000.0).round() as u64,
                _ => errs.push("budget.evolution_share: expected a number in [0, 1]".into()),
            }
        }
        int!(c.escalate_repeats, "evolution"."g4_escalate_repeats");
        int!(c.provisional_max_age_s, "evolution"."provisional_max_age_s");
        int!(c.agent_stratum_max, "evolution"."tasks"."agent_stratum_max");
        // config.md §3.2 rule 7 (the gate's part).
        if c.g4.min_pool > c.pool_k || c.g4.min_holdout > c.holdout_h || c.repeats < 2 || c.repeats > c.repeats_max || c.repeats_max > 5 {
            errs.push("gate.g4: need min_pool ≤ pool_k, min_holdout ≤ holdout_h, 2 ≤ repeats ≤ repeats_max ≤ 5".into());
        }
        if errs.is_empty() { Ok(c) } else { Err(errs) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let c = GateCfg::parse("").unwrap();
        assert_eq!((c.pool_k, c.repeats, c.g3_mode.as_str(), c.gate_musd), (12, 2, "model", 3_000_000));
        let c = GateCfg::parse("[gate.g3]\nmode = \"scripted\"\n[gate.g4]\npool_k = 2\nmin_pool = 2\nmargin_max = 0.2\n[budget]\nprobe_usd = \"0.10\"\n").unwrap();
        assert_eq!((c.g3_mode.as_str(), c.pool_k, c.g4.min_pool, c.probe_musd), ("scripted", 2, 2, 100_000));
        assert!((c.g4.margin_max - 0.2).abs() < 1e-12);
        assert!(GateCfg::parse("[gate.g4]\nrepeats = 1\n").is_err());
        assert!(GateCfg::parse("[gate.g3]\nmode = \"magic\"\n").is_err());
        assert_eq!(usd_to_musd("0.5"), Some(500_000));
        assert_eq!(usd_to_musd("1.0000001"), None);
    }
}
