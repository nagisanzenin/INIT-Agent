//! Soft budgets under the Root's ceilings, the guard on the Root's own spend, reservations and
//! settlement (kernel/proxy.md §3.8, D6). All money is pico-USD in `u64`.

use crate::config::SoftBudgets;
use crate::errors::ProxyError;
use crate::money::{mul_div, musd_to_pusd, pusd_to_musd};
use crate::time::{parse_rfc3339, rfc3339_s};
use crate::types::{GateKind, Level, ScopeKind};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// One model of the Root's published price and level table (root/egress.md §4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootModel {
    pub provider: String,
    pub model: String,
    pub min_level: u8,
    pub input_musd_per_mtok: u64,
    pub output_musd_per_mtok: u64,
    pub routable: bool,
}

/// The kernel's view of the Root (§3.1 `RootState`), parsed from `GET /_root/ceilings`.
#[derive(Clone, Debug, PartialEq)]
pub struct RootView {
    /// The object verbatim (folded into `/_init/stats` as `root`).
    pub ceilings: Value,
    pub epoch: u64,
    pub level: Level,
    pub day_limit_musd: u64,
    pub day_spent_musd: u64,
    pub day_reserved_musd: u64,
    pub total_limit_musd: u64,
    pub total_spent_musd: u64,
    pub day_starts_at_ms: u64,
    pub day_resets_at_ms: u64,
    /// `None` when the Root publishes no model table (then the Root's own allowlist alone decides).
    pub models: Option<Vec<RootModel>>,
    /// Wall-clock ms when the request for this figure was sent.
    pub fetched_ms: u64,
}

impl RootView {
    /// Parse the money block (`init_proto::root_link::Ceilings` shape, root/egress.md §4.2).
    pub fn parse(v: &Value, fetched_ms: u64) -> Result<RootView, String> {
        let c: init_proto::root_link::Ceilings =
            serde_json::from_value(v.clone()).map_err(|e| format!("ceilings: {e}"))?;
        let money = c.money.ok_or("ceilings: no money block")?;
        let day_starts_at_ms =
            parse_rfc3339(&money.day_starts_at).ok_or("ceilings: bad day_starts_at")?;
        let day_resets_at_ms =
            parse_rfc3339(&money.day_resets_at).ok_or("ceilings: bad day_resets_at")?;
        let level = c.level.unwrap_or(0);
        let models = c.models.map(|ms| {
            ms.iter()
                .filter_map(|m| {
                    let s = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
                    let n = |k: &str| m.get(k).and_then(Value::as_u64);
                    let min_level = n("min_level").unwrap_or(0) as u8;
                    Some(RootModel {
                        provider: s("provider")?,
                        model: s("model")?,
                        min_level,
                        input_musd_per_mtok: n("input_musd_per_mtok").unwrap_or(0),
                        output_musd_per_mtok: n("output_musd_per_mtok").unwrap_or(0),
                        routable: m
                            .get("routable")
                            .and_then(Value::as_bool)
                            .unwrap_or(min_level <= level),
                    })
                })
                .collect()
        });
        Ok(RootView {
            ceilings: v.clone(),
            epoch: c.epoch.unwrap_or(0),
            level: Level::from_u8(level),
            day_limit_musd: money.day_limit_musd,
            day_spent_musd: money.day_spent_musd,
            day_reserved_musd: money.day_reserved_musd,
            total_limit_musd: money.total_limit_musd,
            total_spent_musd: money.total_spent_musd,
            day_starts_at_ms,
            day_resets_at_ms,
            models,
            fetched_ms,
        })
    }

    /// Does the Root forward this provider at all?
    pub fn forwards_provider(&self, provider: &str) -> bool {
        match &self.models {
            None => true,
            Some(ms) => ms.iter().any(|m| m.provider == provider),
        }
    }

    /// Does the Root forward this model at its current level?
    pub fn forwards_model(&self, provider: &str, model: &str) -> bool {
        match &self.models {
            None => true,
            Some(ms) => ms
                .iter()
                .any(|m| m.provider == provider && m.model == model && m.routable),
        }
    }

    pub fn model(&self, provider: &str, model: &str) -> Option<&RootModel> {
        self.models
            .as_ref()?
            .iter()
            .find(|m| m.provider == provider && m.model == model)
    }
}

/// Spent and reserved pico-USD of one limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counter {
    pub spent: u64,
    pub reserved: u64,
}

impl Counter {
    fn used(&self) -> u64 {
        self.spent.saturating_add(self.reserved)
    }
}

/// Which counters a reservation holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LimitKey {
    Day,
    Evolution,
    /// `<scope>\0<task>`.
    Task(String),
    Member(String),
    Gate(String),
    Scope(String),
}

/// A running gate (`open_gate` … `close_gate`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateBudget {
    pub kind: GateKind,
    pub cap: u64,
    pub c: Counter,
    pub cost_ref: u64,
    /// The level and routable set snapshotted at `open_gate` (§3.5a).
    pub level: Level,
    pub routable: BTreeSet<(String, String)>,
}

/// An admitted request's hold on the budgets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reservation {
    pub r: u64,
    pub r_root: u64,
    pub keys: Vec<LimitKey>,
    /// The epoch of the Root figure the guard used (`x-egress-guard: epoch=<E>`, D6).
    pub epoch: u64,
}

/// What `admit` needs to know about the request.
#[derive(Clone, Debug)]
pub struct AdmitReq<'a> {
    pub scope_id: &'a str,
    pub kind: &'a ScopeKind,
    pub job_id: Option<&'a str>,
    /// `x-init-task` or `_none` (live and member lanes only).
    pub task: Option<&'a str>,
    /// Reservation at the chosen model's window prices.
    pub r: u64,
    /// The same reservation at the Root's peak prices.
    pub r_root: u64,
    pub now_ms: u64,
}

/// The member share default (`fleet.default_share_musd_day`, $0.50; §3.4).
pub const DEFAULT_MEMBER_SHARE_MUSD: u64 = 500_000;
/// The smallest settable soft budget ($0.01; §3.8).
pub const MIN_BUDGET_MUSD: u64 = 10_000;

/// All soft-budget state, under one mutex.
#[derive(Clone, Debug)]
pub struct Accounts {
    pub configured: SoftBudgets,
    /// `config` or `agent` (§3.8 persistence).
    pub source: &'static str,
    pub since_ms: u64,
    pub guard_permille: u64,
    pub soft_day_fraction_permille: u64,
    pub root: Option<RootView>,
    pub day_start_ms: u64,
    pub day_end_ms: u64,
    pub day: Counter,
    pub evolution: Counter,
    pub tasks: BTreeMap<String, Counter>,
    /// Open member lanes: counter and daily share.
    pub members: BTreeMap<String, (Counter, u64)>,
    pub gates: BTreeMap<String, GateBudget>,
    /// Gate scopes: counter and cap.
    pub scopes: BTreeMap<String, (Counter, u64)>,
    /// Kernel in-flight reservations at Root prices.
    pub root_reserved: u64,
    /// Root-priced estimates of requests settled since the last Root figure was requested.
    pub settled_since: Vec<(u64, u64)>,
    /// Today: `"<provider>/<model>"` → (requests, spent pusd).
    pub by_model: BTreeMap<String, (u64, u64)>,
    /// Today: (task key, `"<provider>/<model>"`) → spent pusd.
    pub by_task_model: BTreeMap<(String, String), u64>,
    pub requests_today: u64,
    pub routed_today: u64,
    /// The last clamp state, so `soft_day_clamped` is raised once per change.
    pub clamped: bool,
    pub root_refusals: u64,
}

fn task_key(scope: &str, task: &str) -> String {
    format!("{scope}\0{task}")
}

impl Accounts {
    pub fn new(
        configured: SoftBudgets,
        guard_permille: u32,
        soft_day_fraction_permille: u32,
        now_ms: u64,
    ) -> Accounts {
        Accounts {
            configured,
            source: "config",
            since_ms: now_ms,
            guard_permille: guard_permille as u64,
            soft_day_fraction_permille: soft_day_fraction_permille as u64,
            root: None,
            day_start_ms: 0,
            day_end_ms: 0,
            day: Counter::default(),
            evolution: Counter::default(),
            tasks: BTreeMap::new(),
            members: BTreeMap::new(),
            gates: BTreeMap::new(),
            scopes: BTreeMap::new(),
            root_reserved: 0,
            settled_since: Vec::new(),
            by_model: BTreeMap::new(),
            by_task_model: BTreeMap::new(),
            requests_today: 0,
            routed_today: 0,
            clamped: false,
            root_refusals: 0,
        }
    }

    /// New Root versions publish observe-only money policy. Missing flags retain
    /// compatibility with older Roots that still enforce their monetary ceilings.
    pub fn money_enforced(&self) -> bool {
        self.root.as_ref().and_then(|r| r.ceilings["money"]["enforced"].as_bool()).unwrap_or(true)
    }

    fn request_max(&self) -> u64 {
        musd_to_pusd(self.configured.request_max_musd)
    }

    /// The clamp bound for `day_usd`: `min(ceiling − request_max, soft_day_fraction × ceiling)`.
    pub fn day_bound(&self) -> Option<u64> {
        if !self.money_enforced() { return None; }
        let root = self.root.as_ref()?;
        let ceiling = musd_to_pusd(root.day_limit_musd);
        Some(ceiling.saturating_sub(self.request_max()).min(mul_div(
            ceiling,
            self.soft_day_fraction_permille,
            1000,
        )))
    }

    /// The effective soft day (D6): `min(day_usd, ceiling − request_max, fraction × ceiling)`.
    pub fn eff_day(&self) -> u64 {
        let d = musd_to_pusd(self.configured.day_musd);
        match self.day_bound() {
            Some(b) => d.min(b),
            None => d,
        }
    }

    /// The effective soft total: `ceiling_total − request_max`.
    pub fn eff_total(&self) -> Option<u64> {
        if !self.money_enforced() { return None; }
        let root = self.root.as_ref()?;
        Some(musd_to_pusd(root.total_limit_musd).saturating_sub(self.request_max()))
    }

    pub fn evolution_cap(&self) -> u64 {
        mul_div(
            self.eff_day(),
            self.configured.evolution_share_permille as u64,
            1000,
        )
    }

    /// The Root's day figure plus the bridge estimate (§3.8 `spent_view`).
    pub fn root_day_view(&self) -> u64 {
        let Some(r) = &self.root else { return 0 };
        let bridge: u64 = self.settled_since.iter().fold(0u64, |sum, x| sum.saturating_add(x.1));
        musd_to_pusd(r.day_spent_musd.saturating_add(r.day_reserved_musd)).saturating_add(bridge)
    }

    pub fn root_total_view(&self) -> u64 {
        let Some(r) = &self.root else { return 0 };
        let bridge: u64 = self.settled_since.iter().fold(0u64, |sum, x| sum.saturating_add(x.1));
        musd_to_pusd(r.total_spent_musd.saturating_add(r.day_reserved_musd)).saturating_add(bridge)
    }

    fn guarded(&self, ceiling_musd: u64) -> u64 {
        mul_div(
            musd_to_pusd(ceiling_musd),
            1000u64.saturating_sub(self.guard_permille),
            1000,
        )
    }

    /// Take a new Root figure. Returns `Some(true|false)` when the clamp state changed (for the
    /// `soft_day_clamped` alert) and whether the day rolled over.
    pub fn set_root(&mut self, view: RootView) -> (Option<bool>, bool) {
        let fetched = view.fetched_ms;
        self.settled_since.retain(|(ms, _)| *ms >= fetched);
        let rolled = self.day_start_ms != 0 && view.day_starts_at_ms > self.day_start_ms;
        if rolled {
            self.roll_day();
        }
        self.day_start_ms = view.day_starts_at_ms;
        self.day_end_ms = view.day_resets_at_ms;
        self.root = Some(view);
        let clamped = self.eff_day() < musd_to_pusd(self.configured.day_musd);
        let changed = (clamped != self.clamped).then_some(clamped);
        self.clamped = clamped;
        (changed, rolled)
    }

    /// A new Root day: daily spend resets; in-flight reservations stay held.
    pub fn roll_day(&mut self) {
        self.day.spent = 0;
        self.evolution.spent = 0;
        for c in self.tasks.values_mut() {
            c.spent = 0;
        }
        self.tasks.retain(|_, c| c.reserved > 0);
        for (c, _) in self.members.values_mut() {
            c.spent = 0;
        }
        self.by_model.clear();
        self.by_task_model.clear();
        self.requests_today = 0;
        self.routed_today = 0;
    }

    /// Advance the day locally when the Root's figure is stale past its reset time.
    pub fn tick(&mut self, now_ms: u64) {
        if self.day_end_ms != 0 && now_ms >= self.day_end_ms {
            let len = self.day_end_ms.saturating_sub(self.day_start_ms).max(1);
            while now_ms >= self.day_end_ms {
                self.day_start_ms = self.day_end_ms;
                self.day_end_ms += len;
            }
            self.roll_day();
        }
    }

    fn retry_after(&self, now_ms: u64) -> u64 {
        self.day_end_ms.saturating_sub(now_ms).div_ceil(1000)
    }

    #[allow(clippy::too_many_arguments)]
    fn cap_error(
        &self,
        code: &str,
        cap: &str,
        limit: u64,
        c: Counter,
        retryable: bool,
        q: &AdmitReq,
        extra: &[(&str, Value)],
    ) -> ProxyError {
        let resets_at = (retryable && self.day_end_ms > 0).then(|| rfc3339_s(self.day_end_ms));
        let mut init = Map::new();
        init.insert("cap".into(), json!(cap));
        init.insert("limit_musd".into(), json!(pusd_to_musd(limit)));
        init.insert("reserved_musd".into(), json!(pusd_to_musd(c.reserved)));
        init.insert("resets_at".into(), json!(resets_at));
        init.insert("retryable".into(), json!(retryable));
        init.insert("scope".into(), json!(q.scope_id));
        init.insert("spent_musd".into(), json!(pusd_to_musd(c.spent)));
        init.insert("task".into(), json!(q.task));
        for (k, v) in extra {
            init.insert((*k).into(), v.clone());
        }
        let message = format!(
            "{} reached: spent {} of {} micro-USD{}",
            cap.replace('_', " "),
            pusd_to_musd(c.spent),
            pusd_to_musd(limit),
            resets_at
                .as_deref()
                .map(|r| format!("; resets {r}"))
                .unwrap_or_default()
        );
        ProxyError::cap(
            code,
            cap,
            init,
            retryable.then(|| self.retry_after(q.now_ms)),
            message,
        )
    }

    /// The admission rule (§3.8): every applicable limit must hold with the reservation added.
    pub fn admit(&mut self, q: &AdmitReq) -> Result<Reservation, ProxyError> {
        self.tick(q.now_ms);
        let Some(root) = self.root.clone() else {
            return Err(ProxyError::new(
                503,
                "egress.unavailable",
                "no Root state yet: the Root egress has not answered",
            ));
        };
        let enforce = self.money_enforced();
        let r = q.r;
        let mut keys = Vec::new();
        let request_max = self.request_max();
        if enforce && r > request_max {
            return Err(self.cap_error(
                "budget.request",
                "request",
                request_max,
                Counter {
                    spent: r,
                    reserved: 0,
                },
                false,
                q,
                &[],
            ));
        }
        if q.kind.is_gate() {
            if let Some((c, cap)) = self.scopes.get(q.scope_id) {
                if enforce && c.used().saturating_add(r) > *cap {
                    return Err(self.cap_error("budget.scope", "scope", *cap, *c, false, q, &[]));
                }
                keys.push(LimitKey::Scope(q.scope_id.to_string()));
            }
            if let Some(job) = q.job_id
                && let Some(g) = self.gates.get(job)
            {
                if enforce && g.c.used().saturating_add(r) > g.cap {
                    return Err(self.cap_error("budget.gate", "gate", g.cap, g.c, false, q, &[]));
                }
                keys.push(LimitKey::Gate(job.to_string()));
            }
            let cap = self.evolution_cap();
            if enforce && self.evolution.used().saturating_add(r) > cap {
                return Err(self.cap_error(
                    "budget.evolution",
                    "evolution",
                    cap,
                    self.evolution,
                    true,
                    q,
                    &[],
                ));
            }
            keys.push(LimitKey::Evolution);
        } else {
            let task = q.task.unwrap_or("_none");
            let tk = task_key(q.scope_id, task);
            let tc = self.tasks.get(&tk).copied().unwrap_or_default();
            let tcap = musd_to_pusd(self.configured.task_musd);
            if enforce && tc.used().saturating_add(r) > tcap {
                return Err(self.cap_error("budget.task", "task", tcap, tc, true, q, &[]));
            }
            keys.push(LimitKey::Task(tk));
            if let ScopeKind::Member(_) = q.kind
                && let Some((c, share)) = self.members.get(q.scope_id)
            {
                let cap = musd_to_pusd(*share);
                if enforce && c.used().saturating_add(r) > cap {
                    return Err(self.cap_error("budget.member", "member", cap, *c, true, q, &[]));
                }
                keys.push(LimitKey::Member(q.scope_id.to_string()));
            }
        }
        let day = self.eff_day();
        if enforce && self.day.used().saturating_add(r) > day {
            return Err(self.cap_error("budget.day", "day", day, self.day, true, q, &[]));
        }
        keys.push(LimitKey::Day);
        // Soft total, against the Root's lifetime figure (the only lifetime figure the kernel has).
        let total_view = self.root_total_view();
        if let Some(total) = self.eff_total()
            && total_view
                .saturating_add(self.root_reserved)
                .saturating_add(q.r_root)
                > total
        {
            let c = Counter {
                spent: total_view,
                reserved: self.root_reserved,
            };
            return Err(self.cap_error("budget.total", "total", total, c, false, q, &[]));
        }
        // The guard on the Root's own spend (§3.8).
        let day_view = self.root_day_view();
        let g_day = self.guarded(root.day_limit_musd);
        if enforce && day_view
            .saturating_add(self.root_reserved)
            .saturating_add(q.r_root)
            > g_day
        {
            let c = Counter {
                spent: day_view,
                reserved: self.root_reserved,
            };
            return Err(self.cap_error(
                "budget.root_day",
                "root_day",
                g_day,
                c,
                true,
                q,
                &[("ceiling_musd", json!(root.day_limit_musd))],
            ));
        }
        let g_total = self.guarded(root.total_limit_musd);
        if enforce && total_view
            .saturating_add(self.root_reserved)
            .saturating_add(q.r_root)
            > g_total
        {
            let c = Counter {
                spent: total_view,
                reserved: self.root_reserved,
            };
            return Err(self.cap_error(
                "budget.root_total",
                "root_total",
                g_total,
                c,
                false,
                q,
                &[("ceiling_musd", json!(root.total_limit_musd))],
            ));
        }
        // Admit: hold every counter.
        for k in &keys {
            if let Some(c) = self.counter(k) {
                c.reserved = c.reserved.saturating_add(r);
            }
        }
        self.root_reserved = self.root_reserved.saturating_add(q.r_root);
        Ok(Reservation {
            r,
            r_root: q.r_root,
            keys,
            epoch: root.epoch,
        })
    }

    fn counter(&mut self, k: &LimitKey) -> Option<&mut Counter> {
        match k {
            LimitKey::Day => Some(&mut self.day),
            LimitKey::Evolution => Some(&mut self.evolution),
            LimitKey::Task(t) => Some(self.tasks.entry(t.clone()).or_default()),
            LimitKey::Member(m) => self.members.get_mut(m).map(|x| &mut x.0),
            LimitKey::Gate(g) => self.gates.get_mut(g).map(|x| &mut x.c),
            LimitKey::Scope(s) => self.scopes.get_mut(s).map(|x| &mut x.0),
        }
    }

    /// Release the reservation and charge the actual cost (on completion, error or abort).
    pub fn settle(
        &mut self,
        res: &Reservation,
        actual: u64,
        actual_ref: u64,
        actual_root: u64,
        now_ms: u64,
    ) {
        for k in &res.keys {
            if let Some(c) = self.counter(k) {
                c.reserved = c.reserved.saturating_sub(res.r);
                c.spent = c.spent.saturating_add(actual);
            }
            if let LimitKey::Gate(g) = k
                && let Some(g) = self.gates.get_mut(g)
            {
                g.cost_ref = g.cost_ref.saturating_add(actual_ref);
            }
        }
        self.root_reserved = self.root_reserved.saturating_sub(res.r_root);
        if actual_root > 0 {
            self.settled_since.push((now_ms, actual_root));
        }
    }

    /// Record per-model attribution of a settled request (stats §4.3).
    pub fn attribute(
        &mut self,
        model_key: &str,
        task_key_live: Option<&str>,
        routed: bool,
        actual: u64,
    ) {
        let e = self.by_model.entry(model_key.to_string()).or_default();
        e.0 = e.0.saturating_add(1);
        e.1 = e.1.saturating_add(actual);
        if let Some(t) = task_key_live {
            let spent = self.by_task_model.entry((t.to_string(), model_key.to_string())).or_default();
            *spent = spent.saturating_add(actual);
        }
        self.requests_today = self.requests_today.saturating_add(1);
        if routed {
            self.routed_today = self.routed_today.saturating_add(1);
        }
    }

    /// Charge a ledger entry while rebuilding today's totals (§3.10.4).
    pub fn rebuild_entry(
        &mut self,
        scope: &str,
        kind_is_gate: bool,
        task: Option<&str>,
        model_key: &str,
        amount: u64,
    ) {
        self.day.spent = self.day.spent.saturating_add(amount);
        if kind_is_gate {
            self.evolution.spent = self.evolution.spent.saturating_add(amount);
        } else {
            let tk = task_key(scope, task.unwrap_or("_none"));
            let task = self.tasks.entry(tk.clone()).or_default();
            task.spent = task.spent.saturating_add(amount);
            let spent = self.by_task_model.entry((tk, model_key.to_string())).or_default();
            *spent = spent.saturating_add(amount);
        }
        let e = self.by_model.entry(model_key.to_string()).or_default();
        e.0 = e.0.saturating_add(1);
        e.1 = e.1.saturating_add(amount);
        self.requests_today = self.requests_today.saturating_add(1);
    }

    /// `open_gate`: refused `budget.evolution` when the day's evolution share is used up.
    pub fn open_gate(
        &mut self,
        job_id: &str,
        kind: GateKind,
        cap_musd: u64,
        level: Level,
        routable: BTreeSet<(String, String)>,
    ) -> Result<(), ProxyError> {
        let cap = self.evolution_cap();
        if self.money_enforced() && self.evolution.used() >= cap {
            let mut init = Map::new();
            init.insert("cap".into(), json!("evolution"));
            init.insert("limit_musd".into(), json!(pusd_to_musd(cap)));
            init.insert(
                "spent_musd".into(),
                json!(pusd_to_musd(self.evolution.spent)),
            );
            return Err(ProxyError::cap(
                "budget.evolution",
                "evolution",
                init,
                None,
                "evolution share used up".into(),
            ));
        }
        let kind_cap = match kind {
            GateKind::Promote => self.configured.gate_musd,
            GateKind::Quick => self.configured.quick_gate_musd,
        };
        self.gates.insert(
            job_id.to_string(),
            GateBudget {
                kind,
                cap: musd_to_pusd(cap_musd.min(kind_cap)),
                c: Counter::default(),
                cost_ref: 0,
                level,
                routable,
            },
        );
        Ok(())
    }

    /// `open_scope` for a gate scope: refused `budget.gate` when the gate's budget is used up.
    pub fn open_gate_scope(
        &mut self,
        scope_id: &str,
        job_id: &str,
        cap_musd: u64,
    ) -> Result<(), ProxyError> {
        if let Some(g) = self.gates.get(job_id)
            && self.money_enforced() && g.c.used() >= g.cap
        {
            let mut init = Map::new();
            init.insert("cap".into(), json!("gate"));
            init.insert("limit_musd".into(), json!(pusd_to_musd(g.cap)));
            init.insert("spent_musd".into(), json!(pusd_to_musd(g.c.spent)));
            return Err(ProxyError::cap(
                "budget.gate",
                "gate",
                init,
                None,
                "gate budget used up".into(),
            ));
        }
        self.scopes.insert(
            scope_id.to_string(),
            (Counter::default(), musd_to_pusd(cap_musd)),
        );
        Ok(())
    }

    /// `open_scope` for a fleet member: the open shares may not exceed the soft day minus the
    /// evolution share (§3.8 "Member share").
    pub fn open_member(&mut self, scope_id: &str, share_musd: u64) -> Result<(), ProxyError> {
        let open: u64 = self
            .members
            .iter()
            .filter(|(k, _)| k.as_str() != scope_id)
            .map(|(_, (_, s))| musd_to_pusd(*s))
            .fold(0u64, u64::saturating_add);
        let bound = self.eff_day().saturating_sub(self.evolution_cap());
        if self.money_enforced() && open.saturating_add(musd_to_pusd(share_musd)) > bound {
            let mut init = Map::new();
            init.insert("cap".into(), json!("member"));
            init.insert("limit_musd".into(), json!(pusd_to_musd(bound)));
            init.insert("spent_musd".into(), json!(pusd_to_musd(open)));
            return Err(ProxyError::cap(
                "budget.member",
                "member",
                init,
                None,
                "member shares would exceed the soft day".into(),
            ));
        }
        self.members
            .insert(scope_id.to_string(), (Counter::default(), share_musd));
        Ok(())
    }

    /// Validate an L1+ `set_budgets` (§3.8 "Who sets the soft budgets").
    pub fn check_budgets(&self, b: &SoftBudgets, level: Level) -> Result<(), String> {
        if level < Level::L1 {
            return Err("soft budgets are maintainer defaults at L0".into());
        }
        if level >= Level::L3 || !self.money_enforced() {
            return Ok(());
        }
        let money = [
            ("day_usd", b.day_musd),
            ("task_usd", b.task_musd),
            ("gate_usd", b.gate_musd),
            ("quick_gate_usd", b.quick_gate_musd),
            ("probe_usd", b.probe_musd),
            ("request_max_usd", b.request_max_musd),
        ];
        for (k, v) in money {
            if v < MIN_BUDGET_MUSD {
                return Err(format!("{k} below $0.01"));
            }
        }
        if let Some(root) = &self.root {
            let ceiling = musd_to_pusd(root.day_limit_musd);
            let bound = ceiling
                .saturating_sub(musd_to_pusd(b.request_max_musd))
                .min(mul_div(ceiling, self.soft_day_fraction_permille, 1000));
            if musd_to_pusd(b.day_musd) > bound {
                return Err(format!(
                    "day_usd above the clamp bound {} micro-USD",
                    pusd_to_musd(bound)
                ));
            }
        }
        if b.evolution_share_permille > 800 {
            return Err("evolution_share above 0.8".into());
        }
        if (b.gate_musd as u128) * 1000
            > (b.evolution_share_permille as u128) * (b.day_musd as u128)
        {
            return Err("gate_usd above evolution_share × day_usd".into());
        }
        if b.task_musd > b.day_musd {
            return Err("task_usd above day_usd".into());
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn root_json(
        day_limit: u64,
        day_spent: u64,
        total_limit: u64,
        total_spent: u64,
        epoch: u64,
    ) -> Value {
        json!({"v":1,"epoch":epoch,"level":0,
               "money":{"day_limit_musd":day_limit,"day_spent_musd":day_spent,"day_reserved_musd":0,
                        "day_starts_at":"2026-10-19T17:00:00Z","day_resets_at":"2026-10-20T17:00:00Z",
                        "total_limit_musd":total_limit,"total_spent_musd":total_spent,"sealed_reserve_musd":0},
               "models":[{"provider":"deepseek","model":"deepseek-flash","min_level":0,"input_musd_per_mtok":300000,"output_musd_per_mtok":1200000,"routable":true},
                         {"provider":"deepseek","model":"deepseek-v4-pro","min_level":1,"input_musd_per_mtok":1320000,"output_musd_per_mtok":3960000,"routable":false}]})
    }

    fn budgets() -> SoftBudgets {
        SoftBudgets {
            day_musd: 5_000_000,
            task_musd: 500_000,
            gate_musd: 3_000_000,
            quick_gate_musd: 500_000,
            probe_musd: 250_000,
            request_max_musd: 1_000_000,
            evolution_share_permille: 600,
        }
    }

    const NOW: u64 = 1_792_465_200_000; // 2026-10-20T03:00:00Z, inside the test day

    fn q<'a>(kind: &'a ScopeKind, r: u64) -> AdmitReq<'a> {
        AdmitReq {
            scope_id: "live",
            kind,
            job_id: None,
            task: None,
            r,
            r_root: r,
            now_ms: NOW,
        }
    }

    #[test]
    fn clamp_d6_and_rt_c1() {
        // RT-C1: charter day $0.10, request_max $0.02 → soft day $0.08.
        let mut b = budgets();
        b.request_max_musd = 20_000;
        b.task_musd = 1_000_000;
        let mut a = Accounts::new(b, 20, 900, NOW);
        let (changed, _) =
            a.set_root(RootView::parse(&root_json(100_000, 0, 25_000_000, 0, 1), NOW).unwrap());
        assert_eq!(changed, Some(true));
        assert_eq!(a.eff_day(), musd_to_pusd(80_000));
        let live = ScopeKind::Live;
        let per = musd_to_pusd(20_000);
        let mut admitted = 0;
        for _ in 0..10 {
            match a.admit(&q(&live, per)) {
                Ok(res) => {
                    admitted += 1;
                    a.settle(&res, per, per, 0, NOW);
                }
                Err(e) => {
                    assert_eq!(e.code, "budget.day");
                    assert_eq!(e.status, 429);
                    assert_eq!(e.cap.as_deref(), Some("day"));
                    assert!(e.retry_after_s.is_some());
                    assert_eq!(e.init["limit_musd"], 80_000);
                    break;
                }
            }
        }
        assert_eq!(admitted, 4);
    }

    /// P-08 (a): concurrent reservations never exceed the soft day.
    #[test]
    fn p08a_reservations_bound_concurrency() {
        let mut b = budgets();
        b.task_musd = 10_000_000; // above the day, so only the day binds
        let mut a = Accounts::new(b, 20, 900, NOW);
        a.set_root(RootView::parse(&root_json(10_000_000, 0, 25_000_000, 0, 1), NOW).unwrap());
        let live = ScopeKind::Live;
        let r = musd_to_pusd(300_000);
        let held: Vec<_> = (0..20).filter_map(|_| a.admit(&q(&live, r)).ok()).collect();
        assert_eq!(held.len(), 16); // 16 × $0.30 = $4.80 ≤ $5.00
        assert!(a.day.reserved <= a.eff_day());
        let e = a.admit(&q(&live, r)).unwrap_err();
        assert_eq!(e.code, "budget.day");
        for h in &held {
            a.settle(h, r / 2, r / 2, 0, NOW);
        }
        assert_eq!(a.day.reserved, 0);
        assert_eq!(a.day.spent, 16 * r / 2);
    }

    /// P-08 (b): the guard reads the Root's own (peak-priced) figure.
    #[test]
    fn p08b_root_guard() {
        let mut b = budgets();
        b.task_musd = 5_000_000;
        let mut a = Accounts::new(b, 20, 900, NOW);
        a.set_root(
            RootView::parse(
                &root_json(10_000_000, 9_700_000, 25_000_000, 9_700_000, 7),
                NOW,
            )
            .unwrap(),
        );
        a.day.spent = musd_to_pusd(4_000_000);
        let live = ScopeKind::Live;
        let ok = a.admit(&q(&live, musd_to_pusd(90_000))).unwrap();
        assert_eq!(ok.epoch, 7);
        let e = a.admit(&q(&live, musd_to_pusd(20_000))).unwrap_err();
        assert_eq!(e.code, "budget.root_day");
        assert_eq!(e.init["ceiling_musd"], 10_000_000);
        assert_eq!(e.init["limit_musd"], 9_800_000);
        assert_eq!(e.init["spent_musd"], 9_700_000);
        assert!(e.retry_after_s.is_some());
        // Settled requests count until a fresher Root figure arrives.
        a.settle(&ok, musd_to_pusd(10_000), 0, musd_to_pusd(10_000), NOW + 5);
        assert_eq!(a.root_day_view(), musd_to_pusd(9_710_000));
        a.set_root(
            RootView::parse(
                &root_json(10_000_000, 9_710_000, 25_000_000, 9_710_000, 8),
                NOW + 10,
            )
            .unwrap(),
        );
        assert_eq!(a.root_day_view(), musd_to_pusd(9_710_000));
    }

    #[test]
    fn tasks_gates_members() {
        let mut a = Accounts::new(budgets(), 20, 900, NOW);
        a.set_root(RootView::parse(&root_json(5_000_000, 0, 25_000_000, 0, 1), NOW).unwrap());
        // Soft day clamped to $4.00 by the $5.00 ceiling (D13 template).
        assert_eq!(a.eff_day(), musd_to_pusd(4_000_000));
        let live = ScopeKind::Live;
        let mut t = q(&live, musd_to_pusd(400_000));
        t.task = Some("t1");
        let res = a.admit(&t).unwrap();
        a.settle(&res, musd_to_pusd(400_000), 0, 0, NOW);
        assert_eq!(a.admit(&t).unwrap_err().code, "budget.task");
        t.task = None; // `_none` has its own budget
        assert!(a.admit(&t).is_ok());
        // Gate: evolution share 0.6 × $4.00 = $2.40; gate cap min($3.00 asked, $3.00).
        a.open_gate(
            "j-000001",
            GateKind::Promote,
            3_000_000,
            Level::L0,
            BTreeSet::new(),
        )
        .unwrap();
        a.open_gate_scope("j-000001/probe-c", "j-000001", 250_000)
            .unwrap();
        let probe = ScopeKind::Probe;
        let g = AdmitReq {
            scope_id: "j-000001/probe-c",
            kind: &probe,
            job_id: Some("j-000001"),
            task: None,
            r: musd_to_pusd(300_000),
            r_root: 0,
            now_ms: NOW,
        };
        assert_eq!(a.admit(&g).unwrap_err().code, "budget.scope");
        let g2 = AdmitReq {
            r: musd_to_pusd(200_000),
            ..g.clone()
        };
        let res = a.admit(&g2).unwrap();
        a.settle(&res, musd_to_pusd(200_000), musd_to_pusd(250_000), 0, NOW);
        assert_eq!(a.gates["j-000001"].cost_ref, musd_to_pusd(250_000));
        assert_eq!(a.evolution.spent, musd_to_pusd(200_000));
        // Members: soft day − evolution share = $1.60; two $0.50 shares fit, a fourth does not.
        a.open_member("member:b1", 500_000).unwrap();
        a.open_member("member:b2", 500_000).unwrap();
        a.open_member("member:b3", 500_000).unwrap();
        assert_eq!(
            a.open_member("member:b4", 500_000).unwrap_err().code,
            "budget.member"
        );
        let m = ScopeKind::Member("b1".into());
        let mq = AdmitReq {
            scope_id: "member:b1",
            kind: &m,
            job_id: None,
            task: Some("x"),
            r: musd_to_pusd(450_000),
            r_root: 0,
            now_ms: NOW,
        };
        let mut b = a.configured;
        b.task_musd = 4_000_000;
        a.configured = b;
        let res = a.admit(&mq).unwrap();
        a.settle(&res, musd_to_pusd(450_000), 0, 0, NOW);
        let e = a.admit(&mq).unwrap_err();
        assert_eq!(e.code, "budget.member");
        assert!(e.retry_after_s.is_some());
        // Request cap.
        let big = q(&live, musd_to_pusd(1_000_001));
        let e = a.admit(&big).unwrap_err();
        assert_eq!((e.code.as_str(), e.retry_after_s), ("budget.request", None));
    }

    /// P-22 bounds.
    #[test]
    fn p22_set_budget_bounds() {
        let mut a = Accounts::new(budgets(), 20, 900, NOW);
        a.set_root(RootView::parse(&root_json(5_000_000, 0, 25_000_000, 0, 1), NOW).unwrap());
        let mut b = budgets();
        b.day_musd = 4_000_000;
        b.gate_musd = 2_000_000;
        assert!(a.check_budgets(&b, Level::L1).is_ok());
        assert!(a.check_budgets(&b, Level::L0).is_err());
        let mut x = b;
        x.evolution_share_permille = 850;
        assert!(
            a.check_budgets(&x, Level::L1)
                .unwrap_err()
                .contains("evolution_share")
        );
        let mut x = b;
        x.gate_musd = 3_000_000;
        assert!(
            a.check_budgets(&x, Level::L1)
                .unwrap_err()
                .contains("gate_usd")
        );
        let mut x = b;
        x.day_musd = 4_500_000;
        assert!(
            a.check_budgets(&x, Level::L1)
                .unwrap_err()
                .contains("clamp bound")
        );
        let mut x = b;
        x.probe_musd = 9_999;
        assert!(a.check_budgets(&x, Level::L2).is_err());
        // A lowered ceiling clamps at once and flags the change.
        let (changed, _) =
            a.set_root(RootView::parse(&root_json(2_000_000, 0, 25_000_000, 0, 2), NOW).unwrap());
        assert_eq!(changed, None); // already clamped under $5.00
        assert_eq!(a.eff_day(), musd_to_pusd(1_000_000));
    }

    #[test]
    fn observe_policy_keeps_accounting_without_money_refusals() {
        let mut a = Accounts::new(budgets(), 20, 900, NOW);
        let mut view = root_json(1, 10_000_000, 1, 20_000_000, 4);
        view["money"]["enforced"] = json!(false);
        a.set_root(RootView::parse(&view, NOW).unwrap());
        assert!(!a.money_enforced());
        assert_eq!(a.day_bound(), None);
        assert_eq!(a.eff_total(), None);
        let amount = musd_to_pusd(50_000_000); // exceeds every configured USD threshold
        let live = ScopeKind::Live;
        let held = a.admit(&q(&live, amount)).unwrap();
        assert_eq!(a.day.reserved, amount);
        a.settle(&held, amount, amount, amount, NOW);
        assert_eq!(a.day.spent, amount);
        assert_eq!(a.day.reserved, 0);
        a.evolution.spent = amount;
        a.open_gate("job", GateKind::Quick, 1, Level::L0, BTreeSet::new()).unwrap();
        a.open_gate_scope("scope", "job", 1).unwrap();
        a.open_member("member", 50_000_000).unwrap();
        // Routing/model-level policy remains available independently of money.
        assert!(!a.root.as_ref().unwrap().forwards_model("deepseek", "deepseek-v4-pro"));
    }

    #[test]
    fn observe_only_accounting_saturates_without_wrapping_or_panicking() {
        let mut a = Accounts::new(budgets(), 20, 900, NOW);
        let mut view = root_json(1, 1, 1, 1, 1);
        view["money"]["enforced"] = json!(false);
        a.set_root(RootView::parse(&view, NOW).unwrap());
        a.settled_since = vec![(NOW, u64::MAX - 1), (NOW, 10)];
        assert_eq!(a.root_day_view(), u64::MAX);
        assert_eq!(a.root_total_view(), u64::MAX);
        // Summing multiple unbounded member shares must remain observe-only.
        a.open_member("a", u64::MAX).unwrap();
        a.open_member("b", u64::MAX).unwrap();
        a.open_member("c", u64::MAX).unwrap();
        assert_eq!(a.members.len(), 3);
        let tk = task_key("live", "task");
        let model = "deepseek/deepseek-flash";
        a.by_model.insert(model.into(), (u64::MAX, u64::MAX - 1));
        a.by_task_model.insert((tk.clone(), model.into()), u64::MAX - 1);
        a.requests_today = u64::MAX;
        a.routed_today = u64::MAX;
        a.attribute(model, Some(&tk), true, 10);
        assert_eq!(a.by_model[model], (u64::MAX, u64::MAX));
        assert_eq!(a.by_task_model[&(tk.clone(), model.into())], u64::MAX);
        assert_eq!(a.requests_today, u64::MAX);
        assert_eq!(a.routed_today, u64::MAX);
        a.tasks.insert(tk.clone(), Counter { spent: u64::MAX - 1, reserved: 7 });
        a.rebuild_entry("live", false, Some("task"), model, 10);
        assert_eq!(a.tasks[&tk], Counter { spent: u64::MAX, reserved: 7 });
        assert_eq!(a.by_task_model[&(tk, model.into())], u64::MAX);
        assert_eq!(a.by_model[model], (u64::MAX, u64::MAX));
        assert_eq!(a.requests_today, u64::MAX);
    }

    #[test]
    fn ordinary_attribution_and_rebuild_remain_exact_below_saturation() {
        let mut a = Accounts::new(budgets(), 20, 900, NOW);
        let tk = task_key("live", "task");
        a.attribute("p/m", Some(&tk), true, 3);
        a.rebuild_entry("live", false, Some("task"), "p/m", 5);
        assert_eq!(a.by_model["p/m"], (2, 8));
        assert_eq!(a.by_task_model[&(tk.clone(), "p/m".into())], 8);
        assert_eq!(a.tasks[&tk].spent, 5);
        assert_eq!((a.requests_today, a.routed_today), (2, 1));
    }

    #[test]
    fn day_rolls_over() {
        let mut a = Accounts::new(budgets(), 20, 900, NOW);
        a.set_root(RootView::parse(&root_json(10_000_000, 0, 25_000_000, 0, 1), NOW).unwrap());
        a.day.spent = 7;
        a.day.reserved = 3;
        a.tick(parse_rfc3339("2026-10-20T17:00:01Z").unwrap());
        assert_eq!((a.day.spent, a.day.reserved), (0, 3));
        assert_eq!(
            a.day_start_ms,
            parse_rfc3339("2026-10-20T17:00:00Z").unwrap()
        );
    }
}
