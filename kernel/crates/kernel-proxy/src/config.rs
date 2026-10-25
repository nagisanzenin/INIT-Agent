//! The proxy-owned sections of `kernel/config.toml` (kernel/config.md §4.1–§4.5: `[model]`, `[proxy]`,
//! `[budget]`, `[traces]`, `[providers.*]`) and their compiled, integer-only form.
//!
//! The assembled `Config` and its loader belong to kernel/config.md; this module gives it the section
//! structs (`deny_unknown_fields`, defaults filled in) and validates what proxy.md owns.

use crate::money::{parse_decimal_e6, price_pusd_per_token, ratio_to_permille, usd_to_musd};
use crate::routes::{Preset, Route};
use crate::tap::Family;
use crate::time::parse_offset;
pub use crate::trace::TracesCfg;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// `[model]` (config.md §4.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ModelCfg {
    /// Body wire/limit metadata; grants no provider route or authorization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<init_proto::files::ModelDescriptor>,
    pub provider: String,
    pub id: String,
    pub effort: String,
    /// `"<provider>/<model>"`; empty = every priced model the Root forwards.
    pub routable: Vec<String>,
}

impl Default for ModelCfg {
    fn default() -> Self {
        ModelCfg {
            descriptor: None,
            provider: "deepseek".into(),
            id: "deepseek-flash".into(),
            effort: "high".into(),
            routable: vec![],
        }
    }
}

/// `[proxy]` (config.md §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ProxyCfg {
    pub worker_threads: usize,
    pub max_request_bytes: usize,
    pub max_sse_line_bytes: usize,
    pub header_read_timeout_s: u64,
    pub upstream_connect_timeout_s: u64,
    pub upstream_header_timeout_s: u64,
    pub upstream_idle_timeout_s: u64,
    pub upstream_total_timeout_s: u64,
    pub retry_max: u32,
    pub retry_max_wait_s: u64,
    pub retry_budget_s: u64,
    pub shutdown_grace_s: u64,
    pub stats_ring: usize,
    pub balance_poll_s: u64,
    pub root_poll_s: u64,
    pub egress_pool_max_idle: usize,
    pub test_mode: bool,
}

impl Default for ProxyCfg {
    fn default() -> Self {
        ProxyCfg {
            worker_threads: 2,
            max_request_bytes: 33_554_432,
            max_sse_line_bytes: 1_048_576,
            header_read_timeout_s: 30,
            upstream_connect_timeout_s: 10,
            upstream_header_timeout_s: 660,
            upstream_idle_timeout_s: 330,
            upstream_total_timeout_s: 3600,
            retry_max: 2,
            retry_max_wait_s: 20,
            retry_budget_s: 30,
            shutdown_grace_s: 30,
            stats_ring: 256,
            balance_poll_s: 600,
            root_poll_s: 10,
            egress_pool_max_idle: 16,
            test_mode: false,
        }
    }
}

/// `[budget]` (config.md §4.3). Money keys are decimal USD strings; the count keys are serve's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BudgetCfg {
    pub day_usd: String,
    pub task_usd: String,
    pub gate_usd: String,
    pub quick_gate_usd: String,
    pub probe_usd: String,
    pub request_max_usd: String,
    pub evolution_share: f64,
    pub reserve_bytes_per_token: u64,
    pub root_guard_permille: u32,
    pub promote_attempts_task: u32,
    pub promotes_day: u32,
    pub quick_gates_day: u32,
    pub task_submits_day: u32,
}

impl Default for BudgetCfg {
    fn default() -> Self {
        BudgetCfg {
            day_usd: "5.00".into(),
            task_usd: "0.50".into(),
            gate_usd: "3.00".into(),
            quick_gate_usd: "0.50".into(),
            probe_usd: "0.25".into(),
            request_max_usd: "1.00".into(),
            evolution_share: 0.6,
            reserve_bytes_per_token: 3,
            root_guard_permille: 20,
            promote_attempts_task: 3,
            promotes_day: 12,
            quick_gates_day: 20,
            task_submits_day: 10,
        }
    }
}

/// An extra route of `[providers.<id>] routes` (config.md §4.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteCfg {
    pub method: String,
    pub path: String,
    pub family: String,
    #[serde(default)]
    pub metered: bool,
    #[serde(default)]
    pub auth: Option<String>,
}

/// One time-of-day window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowCfg {
    pub days: String,
    pub from: String,
    pub to: String,
    #[serde(rename = "use")]
    pub use_: String,
}

/// `[providers.<id>.schedule]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleCfg {
    pub tz: String,
    pub default: String,
    #[serde(default)]
    pub windows: Vec<WindowCfg>,
    #[serde(default)]
    pub default_dates_tz: Option<String>,
    #[serde(default)]
    pub default_dates: Vec<String>,
}

/// `prices.<window>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceCfg {
    pub input: String,
    #[serde(default)]
    pub cache_read: Option<String>,
    #[serde(default)]
    pub cache_write: Option<String>,
    #[serde(default)]
    pub cache_write_1h: Option<String>,
    pub output: String,
}

/// A long-context tier.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierCfg {
    pub above_input_tokens: u64,
    pub prices: BTreeMap<String, PriceCfg>,
}

/// `[providers.<id>.models."<model>"]`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPriceCfg {
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub max_output: Option<u64>,
    #[serde(default)]
    pub reserve_output_tokens: Option<u64>,
    #[serde(default)]
    pub reference_window: Option<String>,
    #[serde(default)]
    pub prices: BTreeMap<String, PriceCfg>,
    #[serde(default)]
    pub tiers: Vec<TierCfg>,
}

/// `[providers.<id>]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCfg {
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub websocket: bool,
    #[serde(default)]
    pub routes: Vec<RouteCfg>,
    #[serde(default)]
    pub multipliers: BTreeMap<String, String>,
    #[serde(default)]
    pub schedule: Option<ScheduleCfg>,
    #[serde(default)]
    pub models: BTreeMap<String, ModelPriceCfg>,
}

fn yes() -> bool {
    true
}

/// Everything the proxy reads from the config file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ProxyConfig {
    pub model: ModelCfg,
    pub proxy: ProxyCfg,
    pub budget: BudgetCfg,
    pub traces: TracesCfg,
    pub providers: BTreeMap<String, ProviderCfg>,
    /// `evolution.soft_day_fraction` (evolution.md §4.6, config.md §4.7).
    pub soft_day_fraction: f64,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        ProxyConfig {
            model: ModelCfg::default(),
            proxy: ProxyCfg::default(),
            budget: BudgetCfg::default(),
            traces: TracesCfg::default(),
            providers: BTreeMap::new(),
            soft_day_fraction: 0.9,
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Compiled form
// ------------------------------------------------------------------------------------------------

/// Prices in pico-USD per token.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prices {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cache_write_1h: u64,
    pub output: u64,
}

/// One priced model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelTable {
    pub prices: BTreeMap<String, Prices>,
    /// `(above_input_tokens, prices by window)`, ascending.
    pub tiers: Vec<(u64, BTreeMap<String, Prices>)>,
    pub reserve_output_tokens: u64,
    pub reference_window: String,
}

/// A parsed time-of-day window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// Bit `d` set = weekday `d` (0 = Monday).
    pub days: u8,
    pub from_min: u32,
    pub to_min: u32,
    pub use_: String,
}

/// A parsed schedule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub offset_min: i64,
    pub default: String,
    pub windows: Vec<Window>,
    pub dates_offset_min: i64,
    pub default_dates: BTreeSet<String>,
}

/// A configured provider, compiled.
#[derive(Clone, Debug)]
pub struct Provider {
    pub id: String,
    pub preset: Preset,
    pub enabled: bool,
    pub routes: Vec<Route>,
    /// `"service_tier:flex"` → multiplier × 10⁶.
    pub multipliers: BTreeMap<String, u64>,
    pub schedule: Option<Schedule>,
    pub models: BTreeMap<String, ModelTable>,
}

/// Soft budgets in micro-USD (proxy.md §4.4 `SoftBudgets`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftBudgets {
    pub day_musd: u64,
    pub task_musd: u64,
    pub gate_musd: u64,
    pub quick_gate_musd: u64,
    pub probe_musd: u64,
    pub request_max_musd: u64,
    pub evolution_share_permille: u32,
}

/// The validated, integer-only configuration the proxy runs on.
#[derive(Clone, Debug)]
pub struct Compiled {
    pub raw: ProxyConfig,
    pub default_provider: String,
    pub default_model: String,
    pub routable: BTreeSet<(String, String)>,
    pub budgets: SoftBudgets,
    pub soft_day_fraction_permille: u32,
    pub providers: BTreeMap<String, Provider>,
}

/// Fallback output reservation when a model gives neither `reserve_output_tokens` nor `max_output`
/// (DeepSeek's 128K default at `reasoning_effort: max`; larger only makes the reservation safer).
pub const DEFAULT_RESERVE_OUTPUT_TOKENS: u64 = 131_072;

fn valid_provider_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
        && id != "_init"
}

fn parse_days(s: &str) -> Option<u8> {
    const NAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let idx = |n: &str| NAMES.iter().position(|x| *x == n);
    let mut mask = 0u8;
    for part in s.split(',') {
        let part = part.trim();
        if let Some((a, b)) = part.split_once('-') {
            let (a, b) = (idx(a)?, idx(b)?);
            if a > b {
                return None;
            }
            for d in a..=b {
                mask |= 1 << d;
            }
        } else {
            mask |= 1 << idx(part)?;
        }
    }
    (mask != 0).then_some(mask)
}

fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    if h > 24 || m > 59 || (h == 24 && m != 0) {
        return None;
    }
    Some(h * 60 + m)
}

fn money_ok(s: &str) -> bool {
    parse_decimal_e6(s).is_some()
}

fn compile_prices(p: &PriceCfg, key: &str, errs: &mut Vec<String>) -> Prices {
    let mut one = |v: Option<&String>, name: &str| -> u64 {
        match v {
            None => 0,
            Some(s) => price_pusd_per_token(s).unwrap_or_else(|| {
                errs.push(format!(
                    "{key}.{name}: money must match ^[0-9]{{1,7}}(\\.[0-9]{{1,6}})?$, got {s:?}"
                ));
                0
            }),
        }
    };
    Prices {
        input: one(Some(&p.input), "input"),
        cache_read: one(p.cache_read.as_ref(), "cache_read"),
        cache_write: one(p.cache_write.as_ref(), "cache_write"),
        cache_write_1h: one(p.cache_write_1h.as_ref(), "cache_write_1h"),
        output: one(Some(&p.output), "output"),
    }
}

impl ProxyConfig {
    /// Validate and compile (config.md §3.2 rules 2–5, 9, 11 and the `[budget]` warnings of rule 6,
    /// which are returned as errors only when they make the proxy unusable). All errors at once.
    pub fn compile(&self) -> Result<Compiled, Vec<String>> {
        let mut errs = Vec::new();
        if let Some(d) = &self.model.descriptor
            && let Err(message) = d.validate()
        {
            errs.push(format!("model.descriptor: {message}"));
        }
        let mut providers = BTreeMap::new();
        for (id, p) in &self.providers {
            let key = format!("providers.{id}");
            if !valid_provider_id(id) {
                errs.push(format!(
                    "{key}: provider id must match ^[a-z][a-z0-9_-]{{0,31}}$ and not be _init"
                ));
                continue;
            }
            if id == "mock" && !self.proxy.test_mode {
                errs.push(format!(
                    "{key}: the provider id \"mock\" is allowed only with proxy.test_mode = true"
                ));
            }
            let selected_api = (id == &self.model.provider).then_some(self.model.descriptor.as_ref()).flatten().map(|d| d.api.as_str());
            let preset_name = p.preset.clone().unwrap_or_else(|| match selected_api {
                Some("chat_completions") => "deepseek".into(),
                Some("messages") => "anthropic".into(),
                Some("responses") => "openai".into(),
                _ => id.clone(),
            });
            let Some(preset) = Preset::parse(&preset_name) else {
                errs.push(format!(
                    "{key}.preset: unknown preset {preset_name:?} (deepseek, anthropic, openai)"
                ));
                continue;
            };
            if p.websocket {
                errs.push(format!(
                    "{key}.websocket: true is not allowed until root/egress.md forwards WebSocket upgrades (kernel/proxy.md §3.15)"
                ));
            }
            let mut routes = preset.routes();
            // The body emits the canonical Chat path, even when the owner chooses an
            // OpenAI preset. Root must independently authorize this exact route.
            if selected_api == Some("chat_completions") && !routes.iter().any(|r| r.method == "POST" && r.path == "chat/completions") {
                routes.push(Route { method: "POST".into(), path: "chat/completions".into(), family: Family::OpenAiChat, metered: true });
            }
            for (i, r) in p.routes.iter().enumerate() {
                let rk = format!("{key}.routes[{i}]");
                let Some(family) = Family::parse(&r.family) else {
                    errs.push(format!("{rk}.family: unknown family {:?}", r.family));
                    continue;
                };
                let method = r.method.to_ascii_uppercase();
                if method != "GET" && method != "POST" {
                    errs.push(format!("{rk}.method: only GET and POST are routable"));
                    continue;
                }
                let path = r.path.trim_start_matches('/').to_string();
                if path.is_empty() || crate::routes::bad_path(&path) {
                    errs.push(format!("{rk}.path: invalid path {:?}", r.path));
                    continue;
                }
                routes.retain(|x| !(x.method == method && x.path == path));
                routes.push(Route {
                    method,
                    path,
                    family,
                    metered: r.metered,
                });
            }
            let mut multipliers = BTreeMap::new();
            for (k, v) in &p.multipliers {
                match parse_decimal_e6(v) {
                    Some(m) if k.split_once(':').is_some_and(|(a, b)| {
                        matches!(a, "service_tier" | "speed" | "inference_geo") && !b.is_empty()
                    }) =>
                    {
                        multipliers.insert(k.clone(), m);
                    }
                    _ => errs.push(format!("{key}.multipliers.{k}: expected \"<service_tier|speed|inference_geo>:<v>\" = \"<decimal>\"")),
                }
            }
            let schedule = p.schedule.as_ref().and_then(|s| {
                let sk = format!("{key}.schedule");
                let Some(offset_min) = parse_offset(&s.tz) else {
                    errs.push(format!("{sk}.tz: expected a fixed offset like \"+00:00\""));
                    return None;
                };
                let dates_offset_min = match &s.default_dates_tz {
                    None => offset_min,
                    Some(t) => parse_offset(t).unwrap_or_else(|| {
                        errs.push(format!(
                            "{sk}.default_dates_tz: expected a fixed offset like \"+08:00\""
                        ));
                        0
                    }),
                };
                let mut windows = Vec::new();
                for (i, w) in s.windows.iter().enumerate() {
                    match (parse_days(&w.days), parse_hhmm(&w.from), parse_hhmm(&w.to)) {
                        (Some(days), Some(from_min), Some(to_min)) if from_min < to_min => windows
                            .push(Window {
                                days,
                                from_min,
                                to_min,
                                use_: w.use_.clone(),
                            }),
                        _ => errs.push(format!("{sk}.windows[{i}]: bad days/from/to")),
                    }
                }
                for d in &s.default_dates {
                    if crate::time::parse_rfc3339(&format!("{d}T00:00:00Z")).is_none()
                        || d.len() != 10
                    {
                        errs.push(format!("{sk}.default_dates: bad date {d:?}"));
                    }
                }
                Some(Schedule {
                    offset_min,
                    default: s.default.clone(),
                    windows,
                    dates_offset_min,
                    default_dates: s.default_dates.iter().cloned().collect(),
                })
            });
            let selectable: BTreeSet<String> = match &schedule {
                None => ["standard".to_string()].into(),
                Some(s) => {
                    let mut w: BTreeSet<String> =
                        s.windows.iter().map(|w| w.use_.clone()).collect();
                    w.insert(s.default.clone());
                    w
                }
            };
            let mut models = BTreeMap::new();
            for (mid, m) in &p.models {
                let mk = format!("{key}.models.\"{mid}\"");
                let mut prices = BTreeMap::new();
                for (w, pc) in &m.prices {
                    prices.insert(
                        w.clone(),
                        compile_prices(pc, &format!("{mk}.prices.{w}"), &mut errs),
                    );
                }
                let reference_window = m.reference_window.clone().unwrap_or_else(|| {
                    if schedule.is_some() {
                        "peak".into()
                    } else {
                        "standard".into()
                    }
                });
                let mut need = selectable.clone();
                need.insert(reference_window.clone());
                for w in &need {
                    if !prices.contains_key(w) {
                        errs.push(format!(
                            "{mk}.prices.{w}: missing (the schedule or reference_window selects it)"
                        ));
                    }
                }
                let mut tiers = Vec::new();
                for (i, t) in m.tiers.iter().enumerate() {
                    let mut tp = BTreeMap::new();
                    for (w, pc) in &t.prices {
                        tp.insert(
                            w.clone(),
                            compile_prices(pc, &format!("{mk}.tiers[{i}].prices.{w}"), &mut errs),
                        );
                    }
                    tiers.push((t.above_input_tokens, tp));
                }
                tiers.sort_by_key(|t| t.0);
                models.insert(
                    mid.clone(),
                    ModelTable {
                        prices,
                        tiers,
                        reserve_output_tokens: m
                            .reserve_output_tokens
                            .or(m.max_output)
                            .unwrap_or(DEFAULT_RESERVE_OUTPUT_TOKENS),
                        reference_window,
                    },
                );
            }
            if models.is_empty() && routes.iter().any(|r| r.metered) && p.enabled {
                errs.push(format!(
                    "{key}.models: a metered provider needs at least one priced model"
                ));
            }
            providers.insert(
                id.clone(),
                Provider {
                    id: id.clone(),
                    preset,
                    enabled: p.enabled,
                    routes,
                    multipliers,
                    schedule,
                    models,
                },
            );
        }
        // Rule 2.
        match providers.get(&self.model.provider) {
            None => errs.push(format!(
                "model.provider: {:?} is not a configured provider",
                self.model.provider
            )),
            Some(p) if !p.models.contains_key(&self.model.id) => errs.push(format!(
                "model.id: {:?} has no prices under providers.{}.models",
                self.model.id, p.id
            )),
            _ => {}
        }
        let mut routable = BTreeSet::new();
        for r in &self.model.routable {
            match r.split_once('/') {
                Some((p, m)) if providers.get(p).is_some_and(|x| x.models.contains_key(m)) => {
                    routable.insert((p.to_string(), m.to_string()));
                }
                _ => errs.push(format!(
                    "model.routable: {r:?} is not a priced \"<provider>/<model>\""
                )),
            }
        }
        // Budget money (rule 5) and bounds.
        let b = &self.budget;
        let mut money = |name: &str, v: &str| -> u64 {
            if !money_ok(v) {
                errs.push(format!(
                    "budget.{name}: money must match ^[0-9]{{1,7}}(\\.[0-9]{{1,6}})?$, got {v:?}"
                ));
            }
            usd_to_musd(v).unwrap_or(0)
        };
        let day_musd = money("day_usd", &b.day_usd);
        let task_musd = money("task_usd", &b.task_usd);
        let gate_musd = money("gate_usd", &b.gate_usd);
        let quick_gate_musd = money("quick_gate_usd", &b.quick_gate_usd);
        let probe_musd = money("probe_usd", &b.probe_usd);
        let request_max_musd = money("request_max_usd", &b.request_max_usd);
        let evolution_share_permille = ratio_to_permille(b.evolution_share).unwrap_or_else(|| {
            errs.push("budget.evolution_share: must be in [0, 1]".into());
            0
        });
        if b.root_guard_permille > 200 {
            errs.push("budget.root_guard_permille: must be in 0..=200".into());
        }
        if b.reserve_bytes_per_token == 0 {
            errs.push("budget.reserve_bytes_per_token: must be ≥ 1".into());
        }
        let soft_day_fraction_permille =
            ratio_to_permille(self.soft_day_fraction).unwrap_or_else(|| {
                errs.push("evolution.soft_day_fraction: must be in [0, 1]".into());
                900
            });
        if self.proxy.worker_threads == 0 {
            errs.push("proxy.worker_threads: must be ≥ 1".into());
        }
        if !errs.is_empty() {
            return Err(errs);
        }
        Ok(Compiled {
            raw: self.clone(),
            default_provider: self.model.provider.clone(),
            default_model: self.model.id.clone(),
            routable,
            budgets: SoftBudgets {
                day_musd,
                task_musd,
                gate_musd,
                quick_gate_musd,
                probe_musd,
                request_max_musd,
                evolution_share_permille,
            },
            soft_day_fraction_permille,
            providers,
        })
    }
}

impl Compiled {
    /// The configured warnings of config.md §3.2 rule 6 (not errors).
    pub fn warnings(&self) -> Vec<String> {
        let b = &self.budgets;
        let mut w = Vec::new();
        if b.gate_musd as u128 * 1000 > b.evolution_share_permille as u128 * b.day_musd as u128 {
            w.push(
                "budget.gate_usd > evolution_share × day_usd: no promote gate could start".into(),
            );
        }
        if b.evolution_share_permille > 800 {
            w.push("budget.evolution_share > 0.8".into());
        }
        if b.probe_musd > b.quick_gate_musd {
            w.push("budget.probe_usd > budget.quick_gate_usd".into());
        }
        w
    }

    /// The model table of `(provider, model)`.
    pub fn model(&self, provider: &str, model: &str) -> Option<&ModelTable> {
        self.providers.get(provider)?.models.get(model)
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    pub const DEEPSEEK: &str = r#"
[model]
provider = "deepseek"
id = "deepseek-flash"

[providers.deepseek.schedule]
tz      = "+00:00"
default = "offpeak"
windows = [
  { days = "mon-fri", from = "01:00", to = "04:00", use = "peak" },
  { days = "mon-fri", from = "06:00", to = "10:00", use = "peak" },
]
default_dates_tz = "+08:00"
default_dates    = ["2026-10-01"]

[providers.deepseek.models."deepseek-flash"]
max_output            = 393216
reserve_output_tokens = 131072
reference_window      = "peak"
prices.peak    = { input = "0.30", cache_read = "0.006", output = "1.20" }
prices.offpeak = { input = "0.15", cache_read = "0.003", output = "0.60" }

[providers.deepseek.models."deepseek-v4-pro"]
max_output            = 393216
prices.peak    = { input = "1.32", cache_read = "0.044", output = "3.96" }
prices.offpeak = { input = "0.66", cache_read = "0.022", output = "1.98" }
"#;
}

#[cfg(test)]
mod tests {
    use super::tests_support::DEEPSEEK;
    use super::*;

    #[test]
    fn deepseek_example_compiles() {
        let c: ProxyConfig = toml::from_str(DEEPSEEK).unwrap();
        let k = c.compile().unwrap();
        let flash = k.model("deepseek", "deepseek-flash").unwrap();
        assert_eq!(flash.prices["peak"].input, 300_000);
        assert_eq!(flash.prices["offpeak"].cache_read, 3_000);
        assert_eq!(flash.reserve_output_tokens, 131_072);
        assert_eq!(
            k.model("deepseek", "deepseek-v4-pro")
                .unwrap()
                .reserve_output_tokens,
            393_216
        );
        assert_eq!(k.budgets.day_musd, 5_000_000);
        assert_eq!(k.budgets.evolution_share_permille, 600);
        let ds = &k.providers["deepseek"];
        assert_eq!(ds.schedule.as_ref().unwrap().windows[0].days, 0b0011111);
        assert!(k.warnings().is_empty());
    }

    #[test]
    fn descriptor_does_not_grant_provider_or_prices() {
        let text = r#"[model]
provider = "local"
id = "custom"
[model.descriptor]
api = "chat_completions"
context_window = 131072
max_output = 32768
default_max_tokens = 8192
request_max_tokens = 4096
"#;
        let c: ProxyConfig = toml::from_str(text).unwrap();
        assert_eq!(c.model.descriptor.as_ref().unwrap().request_max_tokens, 4096);
        assert!(c.compile().unwrap_err().iter().any(|e| e.contains("model.provider")));
        let invalid: ProxyConfig = toml::from_str(&text.replace("request_max_tokens = 4096", "request_max_tokens = 0")).unwrap();
        assert!(invalid.compile().unwrap_err().iter().any(|e| e.contains("model.descriptor")));
        assert!(toml::from_str::<ProxyConfig>(&format!("{text}images = true\n")).is_err());
    }

    #[test]
    fn descriptor_alias_infers_native_routes_and_chat_canonical_path() {
        for (api, path, preset) in [
            ("chat_completions", "chat/completions", ""),
            ("chat_completions", "chat/completions", "preset = \"openai\""),
            ("messages", "v1/messages", ""),
            ("responses", "v1/responses", ""),
        ] {
            let text = format!(r#"[model]
provider = "custom"
id = "custom-model"
[model.descriptor]
api = "{api}"
context_window = 131072
max_output = 32768
default_max_tokens = 8192
request_max_tokens = 4096
chat_stream_usage = false
native_extensions = false
[providers.custom]
{preset}
[providers.custom.models.custom-model]
prices.standard = {{ input = "0", output = "0" }}
"#);
            let config: ProxyConfig = toml::from_str(&text).unwrap();
            let compiled = config.compile().unwrap();
            assert!(compiled.providers["custom"].routes.iter().any(|r| r.path == path && r.method == "POST" && r.metered));
            assert_eq!(config.model.descriptor.unwrap().chat_stream_usage, Some(false));
        }
    }

    #[test]
    fn errors_are_collected() {
        let bad = r#"
[model]
provider = "mock"
id = "nope"
[budget]
day_usd = "5.0000001"
[providers.mock]
preset = "deepseek"
websocket = true
[providers.mock.models."m"]
prices.peak = { input = "0.1", output = "0.1" }
"#;
        let c: ProxyConfig = toml::from_str(bad).unwrap();
        let errs = c.compile().unwrap_err();
        let all = errs.join("\n");
        assert!(all.contains("test_mode"), "{all}");
        assert!(all.contains("websocket"), "{all}");
        assert!(all.contains("prices.standard: missing"), "{all}");
        assert!(all.contains("model.id"), "{all}");
        assert!(all.contains("budget.day_usd"), "{all}");
        // Unknown keys are refused by serde.
        assert!(toml::from_str::<ProxyConfig>("[proxy]\npool_idle_timeout_s = 3\n").is_err());
    }
}
