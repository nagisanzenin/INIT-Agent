//! `kernel-proxy`: the model proxy (kernel/proxy.md v0.3, ADR-002). It runs inside
//! `init-kernel supervise` on its own tokio runtime and is controlled through [`ProxyHandle`].
//!
//! It holds no keys, no TLS and no hard ceilings: its only upstream is the Root's egress socket
//! (root/egress.md). It routes allowlisted `POST /<provider>/<path>` requests, strips the `x-init-*`
//! headers, enforces the level's model-routing policy and the soft budgets (clamped under the Root's
//! ceilings, with a guard on the Root's own spend and the `x-egress-guard` epoch header, D6), meters
//! usage per wire family, computes the head hashes and `tools_sha256`, writes traces deduplicated by
//! prefix, and serves `GET /_init/*`. Sealed traffic never passes through it (D5).

pub mod budget;
pub mod config;
pub mod egress;
pub mod errors;
pub mod hash;
pub mod money;
pub mod pricing;
pub mod request;
pub mod routes;
pub mod scope;
pub mod server;
pub mod stats;
pub mod tap;
pub mod time;
pub mod trace;
pub mod types;

pub use config::{Compiled, ProxyConfig, SoftBudgets};
pub use errors::ProxyError;
pub use scope::RunCaps;
pub use server::{Alert, AlertSink};
pub use types::{GateKind, Label, Level, ScopeKind, Visibility};

use budget::{Accounts, RootView};
use egress::Egress;
use serde_json::{Value, json};
use server::Shared;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::{Notify, mpsc};
use trace::TraceWriter;

/// This crate's name, for diagnostics.
pub const CRATE: &str = env!("CARGO_PKG_NAME");

/// How the proxy is started (§3.1).
#[derive(Clone, Debug)]
pub struct StartOptions {
    /// `$INIT_HOME`.
    pub home: PathBuf,
    /// The Root egress socket (`hello.paths.egress_sock`).
    pub egress_sock: PathBuf,
    /// The live lane socket; default `$INIT_HOME/run/proxy.sock`.
    pub live_socket: Option<PathBuf>,
    /// The live scope's label at start.
    pub label: Label,
    /// `judge` mode (kernel/gate.md §4.1a): no trace store, no ledger file (P-25).
    pub judge: bool,
}

/// `open_scope` arguments (§4.4).
#[derive(Clone, Debug)]
pub struct OpenScope {
    pub scope_id: String,
    pub kind: ScopeKind,
    /// The gate job (gate scopes); `None` for a fleet member.
    pub job_id: Option<String>,
    pub visibility: Visibility,
    pub cap_musd: u64,
    pub label: Label,
    pub socket: PathBuf,
}

/// `close_gate` result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateSpend {
    pub spent_pusd: u64,
    pub cost_ref_pusd: u64,
}

/// For initctl `status` (§4.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyStatus {
    pub day_cap_musd: u64,
    pub day_spent_musd: u64,
    pub root_day_limit_musd: u64,
    pub root_day_spent_musd: u64,
    pub root_total_limit_musd: u64,
    pub root_total_spent_musd: u64,
    pub gate_cap_musd: Option<u64>,
    pub gate_spent_musd: Option<u64>,
    pub cache_hit_permille_last50: Option<u32>,
    pub level: Level,
}

/// What `reload` re-reads.
#[derive(Clone, Debug)]
pub enum Reload {
    Config(Box<ProxyConfig>),
    RootState,
    All(Box<ProxyConfig>),
}

/// The kernel's view of the Root (§4.4 `RootState`).
#[derive(Clone, Debug)]
pub struct RootState {
    pub ceilings: Value,
    pub level: Level,
    pub models: BTreeMap<String, std::collections::BTreeSet<String>>,
    pub at: SystemTime,
}

/// The proxy: `Proxy::start` returns its handle.
pub struct Proxy;

/// The in-process control API (§4.4). Cheap to clone.
#[derive(Clone)]
pub struct ProxyHandle {
    st: Arc<Shared>,
    rt: Arc<Mutex<Option<tokio::runtime::Runtime>>>,
    handle: tokio::runtime::Handle,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Proxy {
    /// The start sequence of §3.1. Config errors come back all at once (`E_CONFIG`).
    pub fn start(
        cfg: &ProxyConfig,
        opts: StartOptions,
        alerts: AlertSink,
    ) -> Result<ProxyHandle, Vec<String>> {
        let compiled = cfg.compile()?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(compiled.raw.proxy.worker_threads.max(1))
            .thread_name("init-proxy")
            .enable_all()
            .build()
            .map_err(|e| vec![format!("proxy runtime: {e}")])?;
        let traces_root = opts.home.join("vault/traces/v1");
        let writer = if opts.judge {
            TraceWriter::disabled()
        } else {
            TraceWriter::start(&traces_root, &compiled.raw.traces)
                .map_err(|e| vec![format!("trace store: {e}")])?
        };
        let live_socket = opts
            .live_socket
            .clone()
            .unwrap_or_else(|| opts.home.join("run/proxy.sock"));
        let (tx, rx) = mpsc::unbounded_channel();
        let p = &compiled.raw.proxy;
        let st = Arc::new(Shared {
            acct: Mutex::new(Accounts::new(
                compiled.budgets,
                compiled.raw.budget.root_guard_permille,
                compiled.soft_day_fraction_permille,
                time::now_ms(),
            )),
            egress: Egress::new(
                &opts.egress_sock,
                p.egress_pool_max_idle,
                Duration::from_secs(p.upstream_connect_timeout_s.max(1)),
            ),
            cfg: RwLock::new(Arc::new(compiled)),
            home: opts.home.clone(),
            traces_root,
            live_socket: live_socket.clone(),
            level: Mutex::new(Level::L0),
            writer,
            scopes: Mutex::new(BTreeMap::new()),
            closers: Mutex::new(BTreeMap::new()),
            alerts,
            alert_last: Mutex::new(BTreeMap::new()),
            own_uid: Mutex::new(None),
            seq: AtomicU64::new(0),
            completions: tx,
            root_ok_ms: AtomicU64::new(0),
            egress_down_since: Mutex::new(None),
            providers: Mutex::new(BTreeMap::new()),
            cost_mismatch: AtomicU64::new(0),
            inflight: AtomicU64::new(0),
            accepting: AtomicBool::new(true),
            rebuilt: AtomicBool::new(false),
            judge: opts.judge,
        });
        let handle = rt.handle().clone();
        // Step 2 + 4: the Root state, then today's totals (on the first figure).
        {
            let st2 = st.clone();
            rt.block_on(async move {
                st2.refresh_root().await;
            });
        }
        handle.spawn(server::accounting(st.clone(), rx));
        // The Root poller (§3.1 "Root state refresh").
        {
            let st2 = st.clone();
            let every = Duration::from_secs(st.cfg().raw.proxy.root_poll_s.max(1));
            handle.spawn(async move {
                let mut tick = tokio::time::interval(every);
                tick.tick().await;
                loop {
                    tick.tick().await;
                    if !st2.accepting.load(Ordering::Acquire) {
                        break;
                    }
                    st2.refresh_root().await;
                }
            });
        }
        let ph = ProxyHandle {
            st,
            rt: Arc::new(Mutex::new(Some(rt))),
            handle,
        };
        // Step 5: bind the live lane.
        let label = opts.label.clone();
        let ring = ph.st.cfg().raw.proxy.stats_ring;
        let scope = Arc::new(scope::Scope::new(
            "live".into(),
            ScopeKind::Live,
            None,
            Visibility::Agent,
            None,
            label,
            Some(live_socket.clone()),
            ring,
            time::now_ms(),
        ));
        ph.bind(scope)
            .map_err(|e| vec![format!("bind {}: {e}", live_socket.display())])?;
        Ok(ph)
    }
}

impl ProxyHandle {
    fn bind(&self, scope: Arc<scope::Scope>) -> std::io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        let path = scope
            .socket
            .clone()
            .ok_or_else(|| std::io::Error::other("scope without a socket"))?;
        let std_l = server::bind_socket(&path)?;
        let uid = std::fs::metadata(&path)?.uid();
        *lock(&self.st.own_uid) = Some(uid);
        let _guard = self.handle.enter();
        let l = tokio::net::UnixListener::from_std(std_l)?;
        let stop = Arc::new(Notify::new());
        lock(&self.st.closers).insert(scope.id.clone(), stop.clone());
        lock(&self.st.scopes).insert(scope.id.clone(), scope.clone());
        let st = self.st.clone();
        self.st.writer.scope_record(json!({"t": "open", "ts": time::rfc3339_ms(time::now_ms()), "scope": scope.id,
            "kind": scope.kind.as_str(), "job_id": scope.job_id, "vis": scope.visibility.as_str(), "label": scope.label().to_json()}));
        self.handle.spawn(server::serve_scope(l, scope, st, stop));
        Ok(())
    }

    /// The live socket path.
    pub fn live_socket(&self) -> &Path {
        &self.st.live_socket
    }

    /// Bind a lane (§3.4). Gate scopes are checked against their gate's budget (`budget.gate`),
    /// member lanes against the soft day minus the evolution share (`budget.member`).
    pub fn open_scope(&self, req: OpenScope) -> Result<(), ProxyError> {
        let home = &self.st.home;
        let probe_root = home.join("probe");
        if !(req.socket.starts_with(&probe_root)
            || req.socket == self.st.live_socket
            || req.socket.starts_with(home.join("run/fleet")))
        {
            return Err(ProxyError::new(
                400,
                "scope.bad_socket",
                "a lane socket must lie under $INIT_HOME/probe/ or run/fleet/",
            ));
        }
        if req.socket.as_os_str().len() > 103 {
            return Err(ProxyError::new(
                400,
                "scope.bad_socket",
                "socket path longer than 103 bytes",
            ));
        }
        if !valid_scope_id(&req.scope_id) || lock(&self.st.scopes).contains_key(&req.scope_id) {
            return Err(ProxyError::new(
                400,
                "scope.bad_id",
                "invalid or duplicate scope id",
            ));
        }
        {
            let mut a = lock(&self.st.acct);
            match &req.kind {
                ScopeKind::Member(_) => a.open_member(&req.scope_id, req.cap_musd)?,
                k if k.is_gate() => {
                    let job = req.job_id.clone().unwrap_or_default();
                    a.open_gate_scope(&req.scope_id, &job, req.cap_musd)?
                }
                _ => {
                    return Err(ProxyError::new(
                        400,
                        "scope.bad_kind",
                        "the live scope is opened by the proxy itself",
                    ));
                }
            }
        }
        let ring = self.st.cfg().raw.proxy.stats_ring;
        let scope = Arc::new(scope::Scope::new(
            req.scope_id.clone(),
            req.kind.clone(),
            req.job_id.clone(),
            req.visibility,
            Some(req.cap_musd),
            req.label,
            Some(req.socket),
            ring,
            time::now_ms(),
        ));
        self.bind(scope).map_err(|e| {
            let mut a = lock(&self.st.acct);
            a.scopes.remove(&req.scope_id);
            a.members.remove(&req.scope_id);
            ProxyError::new(500, "scope.bind", format!("bind failed: {e}"))
        })
    }

    /// Unbind a lane and remove its socket by exact path.
    pub fn close_scope(&self, scope_id: &str) -> Result<serde_json::Value, ProxyError> {
        let scope = lock(&self.st.scopes)
            .remove(scope_id)
            .ok_or_else(|| ProxyError::new(404, "scope.unknown", "unknown scope"))?;
        if let Some(n) = lock(&self.st.closers).remove(scope_id) {
            n.notify_one();
        }
        if let Some(p) = &scope.socket {
            let _ = std::fs::remove_file(p);
        }
        if let Ok(mut s) = scope.state.lock() {
            s.closed_ms = Some(time::now_ms());
        }
        {
            let mut a = lock(&self.st.acct);
            a.scopes.remove(scope_id);
            a.members.remove(scope_id);
        }
        self.st.writer.scope_record(
            json!({"t": "close", "ts": time::rfc3339_ms(time::now_ms()), "scope": scope_id}),
        );
        Ok(scope.summary())
    }

    /// `ScopeSummary` (§4.4) as JSON.
    pub fn scope_summary(&self, scope_id: &str) -> Result<serde_json::Value, ProxyError> {
        let s = lock(&self.st.scopes)
            .get(scope_id)
            .cloned()
            .ok_or_else(|| ProxyError::new(404, "scope.unknown", "unknown scope"))?;
        Ok(s.summary())
    }

    pub fn set_label(&self, scope_id: &str, label: Label) -> Result<(), ProxyError> {
        let s = lock(&self.st.scopes)
            .get(scope_id)
            .cloned()
            .ok_or_else(|| ProxyError::new(404, "scope.unknown", "unknown scope"))?;
        *lock(&s.label) = label;
        self.st.writer.scope_record(json!({"t": "label", "ts": time::rfc3339_ms(time::now_ms()), "scope": scope_id, "label": s.label().to_json()}));
        Ok(())
    }

    /// Per-run caps on actuals for a gate `run` lane (m7-design.md §3.1.3, §3.2.8; GATE-V2b). Set
    /// before the run's body is spawned; setting them again replaces the caps and keeps the counters.
    /// Refused (`scope.bad_kind`) on any other lane kind, and (`scope.bad_caps`) for a zero cap.
    pub fn set_run_caps(&self, scope_id: &str, caps: RunCaps) -> Result<(), ProxyError> {
        let s = lock(&self.st.scopes)
            .get(scope_id)
            .cloned()
            .ok_or_else(|| ProxyError::new(404, "scope.unknown", "unknown scope"))?;
        if s.kind != ScopeKind::Run {
            return Err(ProxyError::new(400, "scope.bad_kind", "run caps apply to gate run lanes only"));
        }
        if [caps.max_turns, caps.max_output_tokens, caps.max_input_tokens].contains(&Some(0)) {
            return Err(ProxyError::new(400, "scope.bad_caps", "a run cap must be at least 1"));
        }
        let mut st = lock(&s.state);
        match st.run.as_mut() {
            Some(r) => r.caps = caps,
            None => st.run = Some(scope::CapState { caps, ..Default::default() }),
        }
        drop(st);
        self.st.writer.scope_record(json!({"t": "caps", "ts": time::rfc3339_ms(time::now_ms()), "scope": scope_id,
            "max_turns": caps.max_turns, "max_output_tokens": caps.max_output_tokens, "max_input_tokens": caps.max_input_tokens}));
        Ok(())
    }

    /// `x-egress-body` for a scope (the live body's boot id; Q-SV1).
    pub fn set_body_id(&self, scope_id: &str, body_id: &str) -> Result<(), ProxyError> {
        let s = lock(&self.st.scopes)
            .get(scope_id)
            .cloned()
            .ok_or_else(|| ProxyError::new(404, "scope.unknown", "unknown scope"))?;
        *lock(&s.body_id) = body_id.to_string();
        Ok(())
    }

    /// Open a gate: `budget.evolution` check, level and routable-set snapshot (§3.5a).
    pub fn open_gate(&self, job_id: &str, kind: GateKind, cap_musd: u64) -> Result<(), ProxyError> {
        let cfg = self.st.cfg();
        let root = self.st.root();
        let level = self.st.level();
        let set = self.st.routable_set(&cfg, root.as_ref(), level);
        lock(&self.st.acct).open_gate(job_id, kind, cap_musd, level, set)
    }

    pub fn close_gate(&self, job_id: &str) -> Result<GateSpend, ProxyError> {
        let g = lock(&self.st.acct)
            .gates
            .remove(job_id)
            .ok_or_else(|| ProxyError::new(404, "gate.unknown", "unknown gate"))?;
        Ok(GateSpend {
            spent_pusd: g.c.spent,
            cost_ref_pusd: g.cost_ref,
        })
    }

    /// One request's reservation floor for `provider/model` in µUSD
    /// ([`pricing::request_reservation_pusd`]); `None` for a model the configuration does not price.
    pub fn request_reservation_musd(&self, provider: &str, model: &str) -> Option<u64> {
        self.st.cfg().model(provider, model).map(|m| money::pusd_to_musd(pricing::request_reservation_pusd(m)))
    }

    /// Whether this proxy refuses requests for money: the Root's `money.enforced` (false under the observe-only
    /// policy of 2026-10-23, so no budget or cap refuses a request; true for a Root that says nothing).
    pub fn money_enforced(&self) -> bool {
        lock(&self.st.acct).money_enforced()
    }

    pub fn status(&self) -> ProxyStatus {
        let a = lock(&self.st.acct).clone();
        let gate = a.gates.values().next();
        let live = lock(&self.st.scopes).get("live").cloned();
        let cache = live.and_then(|s| {
            let st = s.state.lock().ok()?;
            let (r, t) = st
                .cache
                .iter()
                .rev()
                .take(50)
                .fold((0u64, 0u64), |acc, x| (acc.0 + x.1, acc.1 + x.2));
            (t > 0).then(|| (r * 1000 / t) as u32)
        });
        let r = a.root.as_ref();
        ProxyStatus {
            day_cap_musd: money::pusd_to_musd(a.eff_day()),
            day_spent_musd: money::pusd_to_musd(a.day.spent),
            root_day_limit_musd: r.map(|r| r.day_limit_musd).unwrap_or(0),
            root_day_spent_musd: r.map(|r| r.day_spent_musd).unwrap_or(0),
            root_total_limit_musd: r.map(|r| r.total_limit_musd).unwrap_or(0),
            root_total_spent_musd: r.map(|r| r.total_spent_musd).unwrap_or(0),
            gate_cap_musd: gate.map(|g| money::pusd_to_musd(g.cap)),
            gate_spent_musd: gate.map(|g| money::pusd_to_musd(g.c.spent)),
            cache_hit_permille_last50: cache,
            level: self.st.level(),
        }
    }

    /// Tool calls observed since `since` in scopes of `vis` (`"tool/bash"`, `"ext/pty"` → count).
    /// Counted per open scope (in memory).
    pub fn tools_usage(&self, _since: SystemTime, vis: Visibility) -> BTreeMap<String, u64> {
        let mut out = BTreeMap::new();
        for s in lock(&self.st.scopes).values() {
            if s.visibility != vis {
                continue;
            }
            if let Ok(st) = s.state.lock() {
                for (k, n) in &st.obs.tool_calls {
                    *out.entry(k.clone()).or_default() += n;
                }
            }
        }
        out
    }

    /// Re-read config, Root state or both. Returns the enabled providers.
    pub fn reload(&self, what: Reload) -> Result<Vec<String>, ProxyError> {
        let apply_cfg = |c: &ProxyConfig| -> Result<(), ProxyError> {
            let compiled = c
                .compile()
                .map_err(|e| ProxyError::new(400, "config.invalid", e.join("; ")))?;
            {
                let mut a = lock(&self.st.acct);
                if a.source == "config" {
                    a.configured = compiled.budgets;
                }
                a.guard_permille = compiled.raw.budget.root_guard_permille as u64;
                a.soft_day_fraction_permille = compiled.soft_day_fraction_permille as u64;
            }
            if let Ok(mut w) = self.st.cfg.write() {
                *w = Arc::new(compiled);
            }
            Ok(())
        };
        match &what {
            Reload::Config(c) => apply_cfg(c)?,
            Reload::RootState => {
                let st = self.st.clone();
                self.block_on(async move { st.refresh_root().await });
            }
            Reload::All(c) => {
                apply_cfg(c)?;
                let st = self.st.clone();
                self.block_on(async move { st.refresh_root().await });
            }
        }
        Ok(self
            .st
            .cfg()
            .providers
            .iter()
            .filter(|(_, p)| p.enabled)
            .map(|(k, _)| k.clone())
            .collect())
    }

    fn block_on<F: std::future::Future<Output = R> + Send + 'static, R: Send + 'static>(
        &self,
        f: F,
    ) -> Option<R> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.handle.spawn(async move {
            let _ = tx.send(f.await);
        });
        rx.recv_timeout(Duration::from_secs(30)).ok()
    }

    /// The last Root state read (§3.1).
    pub fn root_state(&self) -> RootState {
        let r = self.st.root();
        let mut models: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
        if let Some(ms) = r.as_ref().and_then(|r| r.models.as_ref()) {
            for m in ms.iter().filter(|m| m.routable) {
                models
                    .entry(m.provider.clone())
                    .or_default()
                    .insert(m.model.clone());
            }
        }
        RootState {
            ceilings: r
                .as_ref()
                .map(|r| r.ceilings.clone())
                .unwrap_or(Value::Null),
            level: self.st.level(),
            models,
            at: SystemTime::UNIX_EPOCH
                + Duration::from_millis(self.st.root_ok_ms.load(Ordering::Relaxed)),
        }
    }

    /// A Root level change (§3.5a): applies to the next admission; running gates keep their snapshot.
    /// A demotion to L0 resets agent-set soft budgets to the config defaults (evolution.md §3.15.3).
    pub fn set_level(&self, level: Level) {
        *lock(&self.st.level) = level;
        if level == Level::L0 {
            let defaults = self.st.cfg().budgets;
            let mut a = lock(&self.st.acct);
            if a.source != "config" {
                a.configured = defaults;
                a.source = "config";
                a.since_ms = time::now_ms();
                drop(a);
                let _ = self.write_budget_json();
            }
        }
    }

    /// L1+ soft budgets (§3.8). `BudgetInvalid` → initctl `E_BUDGET` (`code = "budget.invalid"`).
    pub fn set_budgets(&self, b: SoftBudgets) -> Result<SoftBudgets, ProxyError> {
        let level = self.st.level();
        {
            let mut a = lock(&self.st.acct);
            a.check_budgets(&b, level)
                .map_err(|m| ProxyError::new(400, "budget.invalid", m))?;
            a.configured = b;
            a.source = "agent";
            a.since_ms = time::now_ms();
        }
        self.write_budget_json()
            .map_err(|e| ProxyError::new(500, "budget.write", e.to_string()))?;
        Ok(b)
    }

    fn write_budget_json(&self) -> std::io::Result<()> {
        if self.st.judge {
            return Ok(());
        }
        let a = lock(&self.st.acct).clone();
        let b = a.configured;
        let v = json!({"v": 1, "source": a.source, "since": time::rfc3339_ms(a.since_ms),
            "budgets": {"day_musd": b.day_musd, "task_musd": b.task_musd, "gate_musd": b.gate_musd,
                        "quick_gate_musd": b.quick_gate_musd, "probe_musd": b.probe_musd,
                        "request_max_musd": b.request_max_musd, "evolution_share_permille": b.evolution_share_permille}});
        let dir = self.st.home.join("state/kernel");
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join("budget.json.tmp");
        let mut bytes = serde_json::to_vec(&v).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, dir.join("budget.json"))
    }

    /// Supervise relays `ceilings.get` and `ceilings` events (§3.1).
    pub fn set_ceilings(&self, c: serde_json::Value) {
        if let Ok(view) = RootView::parse(&c, time::now_ms()) {
            self.st.apply_root(view);
        }
    }

    /// Stop accepting, wait up to `grace` for in-flight streams, flush the traces, remove the sockets.
    pub fn shutdown(&self, grace: Duration) {
        self.st.accepting.store(false, Ordering::Release);
        let deadline = Instant::now() + grace;
        while self.st.inflight.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let ids: Vec<String> = lock(&self.st.scopes).keys().cloned().collect();
        for id in ids {
            let _ = self.close_scope(&id);
        }
        // Let the accounting task drain.
        std::thread::sleep(Duration::from_millis(50));
        self.st.writer.flush();
        self.st.writer.shutdown();
        if let Some(rt) = lock(&self.rt).take() {
            rt.shutdown_timeout(Duration::from_secs(1));
        }
    }

    /// Block until queued trace records are on disk (tests, CLI).
    pub fn flush_traces(&self) {
        self.st.writer.flush();
    }

    /// The trace root (`$INIT_HOME/vault/traces/v1`).
    pub fn traces_root(&self) -> PathBuf {
        self.st.traces_root.clone()
    }

    /// Wait until no request is in flight and the accounting queue is drained (tests).
    pub fn quiesce(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.st.inflight.load(Ordering::Acquire) == 0 {
                std::thread::sleep(Duration::from_millis(30));
                if self.st.inflight.load(Ordering::Acquire) == 0 {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// The `/_init/stats` object of a scope (for in-process callers).
    pub fn stats(&self, scope_id: &str) -> Option<serde_json::Value> {
        let s = lock(&self.st.scopes).get(scope_id).cloned()?;
        Some(stats::stats(&self.st, &s))
    }
}

/// `live`, or `<job_id>/<name>`, or `member:<bid>`: `[a-zA-Z0-9][a-zA-Z0-9._/:-]{0,127}`.
fn valid_scope_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 128
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'/' | b'-' | b':'))
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name() {
        assert_eq!(super::CRATE, "kernel-proxy");
    }
}
