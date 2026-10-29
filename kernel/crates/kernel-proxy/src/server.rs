//! The HTTP/1.1 server on the lane sockets, request handling, forwarding to the Root egress, the
//! metered response body and the completion path (kernel/proxy.md §3.2, §3.6, §3.7, §3.9, §3.10).

use crate::budget::{Accounts, AdmitReq, Reservation, RootView};
use crate::config::{Compiled, ModelTable, Provider};
use crate::egress::{Egress, EgressError, Sender};
use crate::errors::ProxyError;
use crate::money::pusd_to_musd;
use crate::pricing;
use crate::request::{self, Attribution, FORWARD_REQUEST, forward_response_header};
use crate::routes::{self, Route, RouteTable, Routed};
use crate::scope::{RequestObs, Scope, counts_as_turn};
use crate::tap::{Family, Finish, Tap, TapResult, Usage};
use crate::time::{now_ms, rfc3339_ms};
use crate::trace::{EndJob, RequestBody, StartJob, TraceWriter};
use crate::types::Level;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::{Body, Bytes, Frame, Incoming, SizeHint};
use hyper::http::{HeaderMap, HeaderValue};
use hyper::{Request, Response};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type ProxyBody = BoxBody<Bytes, BoxError>;
type Resp = Response<ProxyBody>;

/// `x-init-caps` (ADR-011 §5.2): at most 1 KiB, else not forwarded (the Root's own bound).
pub const MAX_CAPS_HEADER: usize = 1024;

/// One alert (proxy.md §4.7); supervise writes it to `state/kernel/alerts.jsonl`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub ts_ms: u64,
    pub code: String,
    pub message: String,
    pub provider: Option<String>,
    pub scope: Option<String>,
}

impl Alert {
    /// `{"v":1,"ts","source":"proxy","code","message","provider"?,"scope"?}`.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("v".into(), json!(1));
        m.insert("ts".into(), json!(rfc3339_ms(self.ts_ms)));
        m.insert("source".into(), json!("proxy"));
        m.insert("code".into(), json!(self.code));
        m.insert("message".into(), json!(self.message));
        if let Some(p) = &self.provider {
            m.insert("provider".into(), json!(p));
        }
        if let Some(s) = &self.scope {
            m.insert("scope".into(), json!(s));
        }
        Value::Object(m)
    }
}

/// Where alerts go.
pub type AlertSink = Arc<dyn Fn(Alert) + Send + Sync>;

/// Per-provider counters for `/_init/stats`.
#[derive(Clone, Debug, Default)]
pub struct ProviderStats {
    pub requests: u64,
    pub errors_upstream: u64,
    pub errors_proxy: u64,
    pub retries: u64,
}

/// State shared by every task of the proxy.
pub struct Shared {
    pub cfg: RwLock<Arc<Compiled>>,
    pub home: PathBuf,
    pub traces_root: PathBuf,
    pub live_socket: PathBuf,
    pub acct: Mutex<Accounts>,
    pub level: Mutex<Level>,
    pub egress: Egress,
    pub writer: TraceWriter,
    pub scopes: Mutex<BTreeMap<String, Arc<Scope>>>,
    pub closers: Mutex<BTreeMap<String, Arc<Notify>>>,
    pub alerts: AlertSink,
    pub alert_last: Mutex<BTreeMap<String, u64>>,
    pub own_uid: Mutex<Option<u32>>,
    pub seq: AtomicU64,
    pub completions: mpsc::UnboundedSender<Completion>,
    pub root_ok_ms: AtomicU64,
    pub egress_down_since: Mutex<Option<u64>>,
    pub providers: Mutex<BTreeMap<String, ProviderStats>>,
    pub cost_mismatch: AtomicU64,
    pub inflight: AtomicU64,
    pub accepting: AtomicBool,
    pub rebuilt: AtomicBool,
    pub judge: bool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    pub fn cfg(&self) -> Arc<Compiled> {
        self.cfg
            .read()
            .map(|c| c.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }

    pub fn level(&self) -> Level {
        *lock(&self.level)
    }

    pub fn root(&self) -> Option<RootView> {
        lock(&self.acct).root.clone()
    }

    /// `req_id`: 12 hex digits of Unix ms + a 4-hex per-process counter (§3.13).
    pub fn next_req_id(&self) -> String {
        let n = self.seq.fetch_add(1, Ordering::Relaxed) & 0xffff;
        format!("{:012x}{:04x}", now_ms() & 0xffff_ffff_ffff, n)
    }

    /// Raise an alert; `per_hour` limits it to the first occurrence per hour.
    pub fn alert(
        &self,
        code: &str,
        message: String,
        provider: Option<&str>,
        scope: Option<&str>,
        per_hour: bool,
    ) {
        let now = now_ms();
        if per_hour {
            let mut last = lock(&self.alert_last);
            if last
                .get(code)
                .is_some_and(|t| now.saturating_sub(*t) < 3_600_000)
            {
                return;
            }
            last.insert(code.to_string(), now);
        }
        (self.alerts)(Alert {
            ts_ms: now,
            code: code.to_string(),
            message,
            provider: provider.map(str::to_string),
            scope: scope.map(str::to_string),
        });
    }

    fn provider_stat(&self, id: &str, f: impl FnOnce(&mut ProviderStats)) {
        f(lock(&self.providers).entry(id.to_string()).or_default());
    }

    /// The routable set at `level` (§3.5a): priced in kernel config ∩ forwarded by the Root ∩
    /// `model.routable` when non-empty; at L0 only the default model.
    pub fn routable_set(
        &self,
        cfg: &Compiled,
        root: Option<&RootView>,
        level: Level,
    ) -> BTreeSet<(String, String)> {
        let fwd = |p: &str, m: &str| root.is_none_or(|r| r.forwards_model(p, m));
        let mut s = BTreeSet::new();
        if fwd(&cfg.default_provider, &cfg.default_model)
            && cfg
                .model(&cfg.default_provider, &cfg.default_model)
                .is_some()
        {
            s.insert((cfg.default_provider.clone(), cfg.default_model.clone()));
        }
        if level >= Level::L1 {
            for (pid, p) in &cfg.providers {
                if !p.enabled {
                    continue;
                }
                for m in p.models.keys() {
                    let key = (pid.clone(), m.clone());
                    if fwd(pid, m) && (cfg.routable.is_empty() || cfg.routable.contains(&key)) {
                        s.insert(key);
                    }
                }
            }
        }
        s
    }

    /// Take a Root figure (a `GET /_root/ceilings` answer or a relayed `ceilings` event).
    pub fn apply_root(&self, view: RootView) {
        let level = view.level;
        let (changed, _rolled) = {
            let mut a = lock(&self.acct);
            a.set_root(view)
        };
        *lock(&self.level) = level;
        self.root_ok_ms.store(now_ms(), Ordering::Relaxed);
        if changed == Some(true) {
            let a = lock(&self.acct);
            let msg = format!(
                "the Root's day ceiling forces the soft day down to {} micro-USD (configured {})",
                pusd_to_musd(a.eff_day()),
                a.configured.day_musd
            );
            drop(a);
            self.alert("soft_day_clamped", msg, None, None, false);
        }
        if !self.rebuilt.swap(true, Ordering::AcqRel) {
            self.rebuild_ledger();
        }
    }

    /// Rebuild today's soft-budget totals from `vault/traces/v1/req/` (§3.10.4).
    fn rebuild_ledger(&self) {
        if self.judge {
            return;
        }
        let (from, cfg) = (lock(&self.acct).day_start_ms, self.cfg());
        let Ok(entries) = crate::trace::read_ledger(&self.traces_root, from, now_ms() + 1) else {
            return;
        };
        let mut a = lock(&self.acct);
        for e in entries {
            let amount = e.cost_pusd.unwrap_or(e.reserved_pusd);
            let gate = e.job_id.is_some();
            a.rebuild_entry(
                &e.scope,
                gate,
                e.task.as_deref(),
                &format!("{}/{}", e.provider, e.model),
                amount,
            );
        }
        let _ = cfg;
    }

    /// Fetch the Root's figure and apply it; on failure, track the egress outage.
    pub async fn refresh_root(&self) -> bool {
        let fetched = now_ms();
        match self.egress.ceilings(Duration::from_secs(5)).await {
            Ok(v) => match RootView::parse(&v, fetched) {
                Ok(view) => {
                    *lock(&self.egress_down_since) = None;
                    self.apply_root(view);
                    true
                }
                Err(_) => false,
            },
            Err(_) => {
                self.egress.clear();
                self.note_egress_down();
                false
            }
        }
    }

    fn note_egress_down(&self) {
        let now = now_ms();
        let since = *lock(&self.egress_down_since).get_or_insert(now);
        if now.saturating_sub(since) >= 30_000 {
            self.alert(
                "egress_down",
                "the Root egress socket has been unreachable for 30 s".into(),
                None,
                None,
                true,
            );
        }
    }
}

struct Table<'a> {
    cfg: &'a Compiled,
    root: Option<&'a RootView>,
}

impl RouteTable for Table<'_> {
    fn provider(&self, id: &str) -> Option<(bool, &[Route])> {
        self.cfg
            .providers
            .get(id)
            .map(|p| (p.enabled, p.routes.as_slice()))
    }
    fn root_forwards(&self, id: &str) -> bool {
        self.root.is_none_or(|r| r.forwards_provider(id))
    }
}

pub fn full(b: impl Into<Bytes>) -> ProxyBody {
    Full::new(b.into())
        .map_err(|never: Infallible| match never {})
        .boxed()
}

fn err_resp(e: &ProxyError, anthropic_shaped: bool, req_id: Option<&str>) -> Resp {
    let (status, headers, body) = e.render(anthropic_shaped);
    let mut r = Response::new(full(body));
    *r.status_mut() = hyper::StatusCode::from_u16(status).unwrap_or(hyper::StatusCode::BAD_GATEWAY);
    for (k, v) in headers {
        if let Ok(v) = HeaderValue::from_str(&v) {
            r.headers_mut().insert(k, v);
        }
    }
    if let Some(id) = req_id.and_then(|i| HeaderValue::from_str(i).ok()) {
        r.headers_mut().insert("x-init-req-id", id);
    }
    r
}

pub fn json_resp(status: u16, v: &Value, content_type: &str) -> Resp {
    let mut r = Response::new(full(serde_json::to_vec(v).unwrap_or_default()));
    *r.status_mut() = hyper::StatusCode::from_u16(status).unwrap_or(hyper::StatusCode::OK);
    if let Ok(ct) = HeaderValue::from_str(content_type) {
        r.headers_mut().insert("content-type", ct);
    }
    r
}

/// Serve one lane socket until `stop` is notified (§3.2).
pub async fn serve_scope(
    listener: tokio::net::UnixListener,
    scope: Arc<Scope>,
    st: Arc<Shared>,
    stop: Arc<Notify>,
) {
    let header_timeout = Duration::from_secs(st.cfg().raw.proxy.header_read_timeout_s.max(1));
    loop {
        let accepted = tokio::select! {
            _ = stop.notified() => break,
            a = listener.accept() => a,
        };
        let Ok((stream, _)) = accepted else { continue };
        if !st.accepting.load(Ordering::Acquire) {
            continue;
        }
        // Defence in depth: the socket is 0600, but also refuse other uids.
        let own = *lock(&st.own_uid);
        match (stream.peer_cred(), own) {
            (Ok(c), Some(uid)) if c.uid() == uid => {}
            _ => continue,
        }
        let (scope, st) = (scope.clone(), st.clone());
        tokio::spawn(async move {
            let svc_scope = scope.clone();
            let svc_st = st.clone();
            let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
                handle(req, svc_scope.clone(), svc_st.clone())
            });
            let conn = hyper::server::conn::http1::Builder::new()
                .keep_alive(true)
                .timer(hyper_util::rt::TokioTimer::new())
                .header_read_timeout(header_timeout)
                .max_headers(64)
                .serve_connection(hyper_util::rt::TokioIo::new(stream), svc);
            let _ = conn.await;
        });
    }
}

/// One request (§3.2 `handle`).
pub async fn handle(
    req: Request<Incoming>,
    scope: Arc<Scope>,
    st: Arc<Shared>,
) -> Result<Resp, Infallible> {
    let t_start = Instant::now();
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(str::to_string);
    let cfg = st.cfg();
    let root = st.root();
    let routed = {
        let table = Table {
            cfg: &cfg,
            root: root.as_ref(),
        };
        routes::route(&method, &path, query.as_deref(), &table)
    };
    match routed {
        Err(e) => Ok(err_resp(&e, false, None)),
        Ok(Routed::Local(rest)) => Ok(crate::stats::local(
            &st,
            &scope,
            &method,
            &rest,
            query.as_deref(),
        )),
        Ok(Routed::Upstream {
            provider,
            rest,
            egress_path,
            route,
        }) => Ok(upstream(
            req,
            scope,
            st,
            cfg,
            root,
            provider,
            rest,
            egress_path,
            route,
            t_start,
        )
        .await),
    }
}

/// How a request that got no streamed response is charged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Charge {
    /// From the tap's usage, else the no-usage estimate.
    Measured,
    /// Nothing (never reached the provider, or rejected before generation).
    Zero,
    /// The whole reservation (it may have been billed).
    Reservation,
}

/// What the completion path needs to know about a request.
pub struct Ctx {
    pub st: Arc<Shared>,
    pub scope: Arc<Scope>,
    pub req_id: String,
    pub provider: String,
    pub model: String,
    pub routed: bool,
    pub family: Family,
    pub window: String,
    pub model_table: ModelTable,
    pub req_mods: Vec<String>,
    pub res: Reservation,
    pub req_bytes: u64,
    pub session: Option<String>,
    pub task_key: Option<String>,
    pub task: Option<String>,
    pub prefix_hash: String,
    pub t_read: Instant,
    pub t_sent: Instant,
    pub t_headers: Option<Instant>,
    pub status: u16,
    pub retries: u32,
    pub upstream_request_id: Option<String>,
    pub egress_req_id: Option<String>,
    pub egress_retries: Option<u64>,
    pub resp_type: Option<String>,
    pub gap_ms: Option<u64>,
    pub done: bool,
}

/// One finished request, sent to the accounting task.
pub struct Completion {
    pub ctx: Box<Ctx>,
    pub tap: Option<TapResult>,
    pub outcome: &'static str,
    pub charge: Charge,
    pub response: Option<Vec<u8>>,
    pub resp_truncated: bool,
    pub t_end: Instant,
}

fn header_str(h: &HeaderMap, k: &str) -> Option<String> {
    h.get(k).and_then(|v| v.to_str().ok()).map(str::to_string)
}

fn upstream_request_id(h: &HeaderMap) -> Option<String> {
    for k in ["x-ds-trace-id", "request-id", "x-request-id"] {
        if let Some(v) = header_str(h, k) {
            return Some(format!("{k}:{v}"));
        }
    }
    None
}

/// Backoff for retry attempt `n` (1-based): `min(8 s, 0.5 s × 2^(n−1)) × U(0.75, 1.0)`.
fn backoff(n: u32) -> Duration {
    let base_ms = (500u64 << (n.saturating_sub(1).min(4))).min(8000);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as u64;
    let jitter = 750 + (nanos.wrapping_mul(2_654_435_761) >> 7) % 251; // 750..=1000 permille
    Duration::from_millis(base_ms * jitter / 1000)
}

/// The server-provided delay: `retry-after-ms`, then `retry-after` (seconds).
fn server_delay(h: &HeaderMap) -> Option<Duration> {
    if let Some(ms) = header_str(h, "retry-after-ms").and_then(|v| v.trim().parse::<u64>().ok()) {
        return Some(Duration::from_millis(ms));
    }
    header_str(h, "retry-after")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// A 429 that means "out of money", never retried (§3.9).
fn spend_limit_429(body: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let e = &v["error"];
    if e["details"]["error_code"] == "enforced_spend_limit_reached" {
        return true;
    }
    let code = e["code"]
        .as_str()
        .or_else(|| e["type"].as_str())
        .unwrap_or("");
    matches!(
        code,
        "insufficient_quota" | "credit_balance_exhausted" | "organization_usage_limit_exceeded"
    ) || (code.ends_with("_spend_limit_exceeded"))
}

fn retryable_status(s: u16) -> bool {
    matches!(s, 429 | 500 | 502 | 503 | 504 | 529)
}

/// A per-run cap refusal (m7-design.md §3.1.3): 429 `budget.<cap>`, `x-init-cap: <cap>`, never
/// retryable (the run is over: it ends `cost_capped` and is scored on what it left).
fn run_cap_error(scope: &str, cap: &str, limit: u64, value: u64) -> ProxyError {
    let code = format!("budget.{cap}");
    let mut init = Map::new();
    init.insert("cap".into(), json!(cap));
    init.insert("code".into(), json!(code));
    init.insert("limit".into(), json!(limit));
    init.insert("retryable".into(), json!(false));
    init.insert("scope".into(), json!(scope));
    init.insert("task".into(), Value::Null);
    init.insert("value".into(), json!(value));
    let what = match cap {
        "turns" => "turns",
        "tokens_out" => "output tokens",
        _ => "input tokens",
    };
    ProxyError::cap(&code, cap, init, None, format!("run cap reached: {value} of {limit} {what}"))
}

/// Map a Root refusal (`x-init-egress-error`) to the kernel's shape (§4.2).
fn map_root_refusal(
    code: &str,
    root_body: &[u8],
    h: &HeaderMap,
    scope: &str,
    task: Option<&str>,
) -> ProxyError {
    let root_init = serde_json::from_slice::<Value>(root_body)
        .ok()
        .map(|v| v["init"].clone())
        .unwrap_or(Value::Null);
    let n = |k: &str| root_init.get(k).and_then(Value::as_u64);
    let mut init = Map::new();
    init.insert("root".into(), root_init.clone());
    init.insert("scope".into(), json!(scope));
    init.insert("task".into(), json!(task));
    let retryable = root_init
        .get("retryable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    init.insert("retryable".into(), json!(retryable));
    let message = format!("the Root refused the request: {code}");
    let (status, kcode, cap) = if let Some(c) = code.strip_prefix("ceiling.") {
        (429, format!("budget.root_{c}"), Some(format!("root_{c}")))
    } else if let Some(q) = code.strip_prefix("quota.") {
        (429, format!("budget.root_{q}"), Some(format!("root_{q}")))
    } else if let Some(x) = code.strip_prefix("class.") {
        (403, format!("egress.{x}"), None)
    } else if code.starts_with("root.") {
        (503, "egress.unavailable".to_string(), None)
    } else {
        (403, format!("egress.{}", code.replace('.', "_")), None)
    };
    if let Some(cap) = &cap {
        init.insert("cap".into(), json!(cap));
        init.insert("limit_musd".into(), json!(n("limit_musd")));
        init.insert("spent_musd".into(), json!(n("spent_musd")));
        init.insert(
            "reserved_musd".into(),
            json!(n("reserved_musd").unwrap_or(0)),
        );
        init.insert(
            "resets_at".into(),
            root_init.get("resets_at").cloned().unwrap_or(Value::Null),
        );
        if code.starts_with("ceiling.") {
            init.insert("ceiling_musd".into(), json!(n("limit_musd")));
        }
    }
    init.insert("code".into(), json!(kcode));
    let retry_after = retryable
        .then(|| server_delay(h).map(|d| d.as_secs()))
        .flatten();
    let mut e = match cap {
        Some(cap) => ProxyError::cap(&kcode, &cap, init, retry_after, message),
        None => {
            let mut e = ProxyError::new(status, &kcode, message);
            e.init = init;
            e
        }
    };
    e.status = status;
    e
}

enum Sent {
    Stream(Response<Incoming>, Sender),
    Buffered(hyper::http::response::Parts, Bytes),
    Refused(ProxyError),
    Failed(ProxyError, Charge, &'static str),
}

/// Forward with the retry policy of §3.9. `attempts` counts retries done.
#[allow(clippy::too_many_arguments)]
async fn send_with_retries(
    st: &Shared,
    method: &str,
    egress_path: &str,
    headers: &HeaderMap,
    body: &Bytes,
    scope_id: &str,
    task: Option<&str>,
    retries: &mut u32,
) -> Sent {
    let p = &st.cfg().raw.proxy;
    let header_timeout = Duration::from_secs(p.upstream_header_timeout_s.max(1));
    let budget = Duration::from_secs(p.retry_budget_s);
    let max_wait = Duration::from_secs(p.retry_max_wait_s);
    let mut waited = Duration::ZERO;
    loop {
        let mut b = Request::builder().method(method).uri(egress_path);
        if let Some(h) = b.headers_mut() {
            *h = headers.clone();
        }
        let Ok(req) = b.body(Full::new(body.clone())) else {
            return Sent::Failed(
                ProxyError::new(400, "route.bad_path", "unrepresentable request"),
                Charge::Zero,
                "proxy_error",
            );
        };
        match st.egress.send(req, header_timeout).await {
            Err(EgressError::Connect(e)) => {
                st.note_egress_down();
                let wait = backoff(*retries + 1);
                if *retries < p.retry_max && waited + wait <= budget {
                    *retries += 1;
                    waited += wait;
                    tokio::time::sleep(wait).await;
                    continue;
                }
                return Sent::Failed(
                    ProxyError::new(
                        503,
                        "egress.unavailable",
                        format!("the Root egress socket cannot be reached: {e}"),
                    ),
                    Charge::Zero,
                    "egress_unavailable",
                );
            }
            Err(EgressError::Transport(e)) => {
                return Sent::Failed(
                    ProxyError::new(
                        502,
                        "upstream.transport",
                        format!("the egress connection broke: {e}"),
                    ),
                    Charge::Reservation,
                    "transport",
                );
            }
            Err(EgressError::Timeout) => {
                return Sent::Failed(
                    ProxyError::new(
                        504,
                        "upstream.timeout",
                        "no response headers within the header timeout",
                    ),
                    Charge::Reservation,
                    "timeout",
                );
            }
            Ok((resp, sender)) => {
                *lock(&st.egress_down_since) = None;
                let status = resp.status().as_u16();
                if let Some(code) = header_str(resp.headers(), "x-init-egress-error") {
                    let (parts, body) = resp.into_parts();
                    let bytes = Limited::new(body, 1 << 16)
                        .collect()
                        .await
                        .map(|c| c.to_bytes())
                        .unwrap_or_default();
                    st.egress.checkin(sender);
                    return Sent::Refused(map_root_refusal(
                        &code,
                        &bytes,
                        &parts.headers,
                        scope_id,
                        task,
                    ));
                }
                let should_retry_hdr =
                    header_str(resp.headers(), "x-should-retry").as_deref() != Some("false");
                if retryable_status(status) && should_retry_hdr && *retries < p.retry_max {
                    let (parts, body) = resp.into_parts();
                    let bytes = Limited::new(body, 1 << 16)
                        .collect()
                        .await
                        .map(|c| c.to_bytes())
                        .unwrap_or_default();
                    st.egress.checkin(sender);
                    if status == 429 && spend_limit_429(&bytes) {
                        return Sent::Buffered(parts, bytes);
                    }
                    let wait =
                        server_delay(&parts.headers).unwrap_or_else(|| backoff(*retries + 1));
                    if wait > max_wait || waited + wait > budget {
                        return Sent::Buffered(parts, bytes);
                    }
                    *retries += 1;
                    waited += wait;
                    tokio::time::sleep(wait).await;
                    continue;
                }
                return Sent::Stream(resp, sender);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn upstream(
    req: Request<Incoming>,
    scope: Arc<Scope>,
    st: Arc<Shared>,
    cfg: Arc<Compiled>,
    root: Option<RootView>,
    provider: String,
    _rest: String,
    egress_path: String,
    route: Route,
    t_start: Instant,
) -> Resp {
    let Some(pcfg) = cfg.providers.get(&provider).cloned() else {
        return err_resp(
            &ProxyError::new(404, "route.unknown_provider", "unknown provider"),
            false,
            None,
        );
    };
    let anthropic_shaped = route.family == Family::Anthropic
        || (route.family == Family::None && pcfg.preset.anthropic_shaped());
    let in_headers = req.headers().clone();
    let attrib = Attribution::from_headers(
        in_headers
            .iter()
            .filter_map(|(k, v)| Some((k.as_str(), v.to_str().ok()?))),
    );
    let max = cfg.raw.proxy.max_request_bytes;
    let body = match Limited::new(req.into_body(), max).collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => {
            st.provider_stat(&provider, |p| p.errors_proxy += 1);
            let pe = if e.downcast_ref::<LengthLimitError>().is_some() {
                ProxyError::new(
                    413,
                    "request.too_large",
                    format!("request body over {max} bytes"),
                )
            } else {
                ProxyError::new(
                    400,
                    "request.bad_json",
                    "the request body could not be read",
                )
            };
            return err_resp(&pe, anthropic_shaped, None);
        }
    };
    let t_read = Instant::now();
    // Egress headers (§3.6): the allowlist, then the proxy's own.
    let mut eh = HeaderMap::new();
    for name in FORWARD_REQUEST {
        for v in in_headers.get_all(name) {
            eh.append(name, v.clone());
        }
    }
    if route.family == Family::Anthropic && !eh.contains_key("anthropic-version") {
        eh.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    }
    eh.insert("host", HeaderValue::from_static("egress"));
    eh.insert(
        "user-agent",
        HeaderValue::from_static(concat!("init-kernel/", env!("CARGO_PKG_VERSION"))),
    );
    eh.insert("accept-encoding", HeaderValue::from_static("identity"));
    eh.insert(
        "x-egress-scope",
        HeaderValue::from_static(scope.kind.egress_scope()),
    );
    let body_id = lock(&scope.body_id).clone();
    eh.insert(
        "x-egress-body",
        HeaderValue::from_str(if body_id.is_empty() {
            "_none"
        } else {
            &body_id
        })
        .unwrap_or(HeaderValue::from_static("_none")),
    );
    // ADR-011 §5.2 (X5): the body's zone-evidence caps travel to the Root as `x-egress-caps` (the
    // `x-init-*` original is never forwarded; the Root validates the ids and strips the header).
    if let Some(v) = in_headers
        .get("x-init-caps")
        .filter(|v| v.len() <= MAX_CAPS_HEADER)
    {
        eh.insert("x-egress-caps", v.clone());
    }
    // MERGE (body/tools.md §3.7, root/egress.md): a sub-task's label travels to the Root as
    // `x-egress-subtask`, so its per-turn signals count the main loop apart from `delegate` sub-tasks.
    // Only a well-formed `x-init-subtask` (§3.6 grammar, ≤ 64 bytes) is forwarded; the Root strips it
    // before the provider.
    if let Some(v) = in_headers
        .get("x-init-subtask")
        .filter(|v| v.to_str().is_ok_and(request::valid_id))
    {
        eh.insert("x-egress-subtask", v.clone());
    }
    st.provider_stat(&provider, |p| p.requests += 1);

    if !route.metered {
        return unmetered(
            &st,
            &route,
            &egress_path,
            eh,
            body,
            &provider,
            anthropic_shaped,
            root.as_ref(),
        )
        .await;
    }

    // ---- metered: parse, route policy, pre-flight, admission ----
    let fail = |st: &Shared, e: ProxyError| {
        st.provider_stat(&provider, |p| p.errors_proxy += 1);
        err_resp(&e, anthropic_shaped, None)
    };
    let parsed = match request::parse(&provider, route.family, &body) {
        Ok(p) => p,
        Err(e) => {
            return fail(
                &st,
                ProxyError::new(
                    400,
                    "request.bad_json",
                    format!("request body is not valid JSON: {e}"),
                ),
            );
        }
    };
    let Some(model) = parsed.model.clone() else {
        return fail(
            &st,
            ProxyError::new(
                400,
                "request.model_not_allowed",
                "the request names no model",
            ),
        );
    };
    let label = scope.label();
    let default_model = if label.model.is_empty() {
        cfg.default_model.clone()
    } else {
        label.model.clone()
    };
    let routed = !(model == default_model && provider == cfg.default_provider);
    let (level, snapshot) = match (&scope.kind, &scope.job_id) {
        (k, Some(job)) if k.is_gate() => {
            let a = lock(&st.acct);
            match a.gates.get(job) {
                Some(g) => (g.level, Some(g.routable.clone())),
                None => (st.level(), None),
            }
        }
        _ => (st.level(), None),
    };
    if level == Level::L0 && routed {
        return fail(
            &st,
            ProxyError::new(
                400,
                "request.model_not_routed",
                format!("model {model:?} is not the default model; routing unlocks at L1"),
            ),
        );
    }
    let Some(table) = cfg.model(&provider, &model).cloned() else {
        return fail(
            &st,
            ProxyError::new(
                400,
                "request.model_not_allowed",
                format!("model {model:?} is not priced in kernel config"),
            ),
        );
    };
    let allowed = snapshot.unwrap_or_else(|| st.routable_set(&cfg, root.as_ref(), level));
    if !allowed.contains(&(provider.clone(), model.clone())) {
        return fail(
            &st,
            ProxyError::new(
                400,
                "request.model_not_allowed",
                format!("model {provider}/{model} is not in the routable set"),
            ),
        );
    }
    let mut req_mods = Vec::new();
    for (k, v) in [
        ("service_tier", &parsed.service_tier),
        ("speed", &parsed.speed),
    ] {
        if let Some(v) = v {
            let key = format!("{k}:{v}");
            if !pricing::neutral(v) && !pcfg.multipliers.contains_key(&key) {
                return fail(
                    &st,
                    ProxyError::new(
                        400,
                        "request.tier_not_allowed",
                        format!("{key} has no configured multiplier"),
                    ),
                );
            }
            req_mods.push(key);
        }
    }
    if pcfg.preset == routes::Preset::Openai
        && route.family == Family::OpenAiChat
        && parsed.stream
        && parsed.include_usage != Some(true)
    {
        return fail(
            &st,
            ProxyError::new(
                400,
                "request.usage_required",
                "stream_options.include_usage must be true",
            ),
        );
    }
    let now = now_ms();
    let window = pricing::window_at(&pcfg, now);
    let bpt = cfg.raw.budget.reserve_bytes_per_token;
    let req_bytes = body.len() as u64;
    let prices = pricing::prices_for(&table, &window, req_bytes.div_ceil(bpt.max(1)));
    let (mult, _) = pricing::multiplier_ppm(&pcfg, &req_mods);
    let max_out = parsed.max_out.unwrap_or(table.reserve_output_tokens);
    let r = pricing::reserve_pusd(req_bytes, bpt, max_out, &prices, mult);
    let r_out = pricing::reserve_pusd(0, bpt, max_out, &prices, mult); // D11: the output term alone
    let r_root = match root.as_ref().and_then(|rv| rv.model(&provider, &model)) {
        Some(m) => req_bytes
            .div_ceil(3)
            .saturating_mul(m.input_musd_per_mtok)
            .saturating_add(max_out.saturating_mul(m.output_musd_per_mtok)),
        None => pricing::reserve_pusd(req_bytes, 3, max_out, &pricing::highest(&table), mult),
    };
    let task =
        (!scope.kind.is_gate()).then(|| attrib.task.clone().unwrap_or_else(|| "_none".into()));
    // Per-run caps on actuals (m7-design.md §3.1.3): checked before the money admission, with no
    // reservation, so a first request is never refused by the token caps.
    let refusal = {
        let mut s = lock(&scope.state);
        let r = s.run.as_ref().and_then(|r| r.refusal());
        if let Some((cap, _, _)) = r {
            s.capped = true;
            s.caps_hit.get_or_insert(cap);
        }
        r
    };
    if let Some((cap, limit, value)) = refusal {
        return fail(&st, run_cap_error(&scope.id, cap, limit, value));
    }
    let admitted = {
        let mut a = lock(&st.acct);
        a.admit(&AdmitReq {
            scope_id: &scope.id,
            kind: &scope.kind,
            job_id: scope.job_id.as_deref(),
            task: task.as_deref(),
            r,
            r_out,
            r_root,
            now_ms: now,
        })
    };
    let res = match admitted {
        Ok(res) => res,
        Err(e) => {
            if e.status == 429 {
                {
                    let mut s = lock(&scope.state);
                    s.capped = true;
                    if s.run.is_some() {
                        s.caps_hit.get_or_insert("cost");
                    }
                }
                let code = match e.code.as_str() {
                    "budget.day" => Some("budget_day_reached"),
                    "budget.root_day" | "budget.root_total" => Some("budget_root_guard"),
                    "budget.evolution" => Some("budget_evolution_reached"),
                    _ => None,
                };
                if let Some(c) = code {
                    st.alert(c, e.message.clone(), Some(&provider), Some(&scope.id), true);
                }
            }
            return fail(&st, e);
        }
    };
    // ---- admitted: records ----
    let req_id = st.next_req_id();
    eh.insert(
        "x-egress-guard",
        HeaderValue::from_str(&format!("epoch={}", res.epoch))
            .unwrap_or(HeaderValue::from_static("epoch=0")),
    );
    if let Ok(v) = HeaderValue::from_str(&req_id) {
        eh.insert("x-egress-reservation", v);
    }
    let gap_ms = {
        let mut s = lock(&scope.state);
        s.requests += 1;
        s.inflight += 1;
        let sess = attrib.session.clone().unwrap_or_else(|| "_none".into());
        let gap = s
            .prev_end
            .get(&sess)
            .and_then(|(end, tools)| tools.then(|| now.saturating_sub(*end)));
        if counts_as_turn(attrib.purpose.as_deref())
            && let Some(r) = s.run.as_mut()
        {
            r.turns += 1;
        }
        s.obs.on_request(
            RequestObs {
                req_id: req_id.clone(),
                model: model.clone(),
                family: route.family,
                prefix_hash: parsed.prefix_hash.clone(),
                head_bytes_hash: parsed.head_bytes_hash.clone(),
                tools_sha256: parsed.tools_sha256.clone(),
                session: attrib.session.clone(),
                tools: parsed.tools.clone(),
            },
            &parsed.tool_result_ids,
        );
        s.last_session = attrib.session.clone();
        s.last_task = attrib.task.clone();
        gap
    };
    st.inflight.fetch_add(1, Ordering::AcqRel);
    let mut f = Map::new();
    f.insert("req_id".into(), json!(req_id));
    f.insert("ts".into(), json!(rfc3339_ms(now)));
    f.insert("scope".into(), json!(scope.id));
    f.insert("vis".into(), json!(scope.visibility.as_str()));
    f.insert("job_id".into(), json!(scope.job_id));
    f.insert(
        "task".into(),
        json!(if scope.kind.is_gate() {
            None
        } else {
            attrib.task.clone()
        }),
    );
    f.insert("subtask".into(), json!(attrib.subtask));
    f.insert("session".into(), json!(attrib.session));
    f.insert("purpose".into(), json!(attrib.purpose));
    f.insert("attempt".into(), json!(attrib.attempt));
    f.insert("label".into(), label.to_json());
    f.insert("provider".into(), json!(provider));
    f.insert(
        "route".into(),
        json!(format!("{} {}", route.method, route.path)),
    );
    f.insert("family".into(), json!(route.family.as_str()));
    f.insert("model".into(), json!(model));
    f.insert("default_model".into(), json!(default_model));
    f.insert("routed".into(), json!(routed));
    f.insert("level".into(), json!(level.as_str()));
    f.insert("stream".into(), json!(parsed.stream));
    f.insert("req_bytes".into(), json!(req_bytes));
    f.insert("reserved_pusd".into(), json!(r));
    f.insert("window".into(), json!(window));
    f.insert("prefix_hash".into(), json!(parsed.prefix_hash));
    f.insert("head_bytes_hash".into(), json!(parsed.head_bytes_hash));
    f.insert("tools_sha256".into(), json!(parsed.tools_sha256));
    let session_key = attrib.session.clone().unwrap_or_else(|| "_none".into());
    let store = cfg.raw.traces.store_bodies;
    st.writer.start_record(StartJob {
        req_id: req_id.clone(),
        chain_key: format!("{}\0{}", scope.id, session_key),
        fields: f,
        body: store.then(|| RequestBody {
            bytes: Arc::from(&body[..]),
            list: parsed.list,
            elements: parsed.elements.clone(),
        }),
    });
    let mut ctx = Box::new(Ctx {
        st: st.clone(),
        scope: scope.clone(),
        req_id: req_id.clone(),
        provider: provider.clone(),
        model: model.clone(),
        routed,
        family: route.family,
        window,
        model_table: table,
        req_mods,
        res,
        req_bytes,
        session: attrib.session.clone(),
        task_key: task.as_ref().map(|t| format!("{}\0{}", scope.id, t)),
        task: attrib.task.clone(),
        prefix_hash: parsed.prefix_hash.clone(),
        t_read,
        t_sent: Instant::now(),
        t_headers: None,
        status: 0,
        retries: 0,
        upstream_request_id: None,
        egress_req_id: None,
        egress_retries: None,
        resp_type: None,
        gap_ms,
        done: false,
    });
    let _ = t_start;
    // ---- forward ----
    let mut retries = 0u32;
    let sent = send_with_retries(
        &st,
        &route.method,
        &egress_path,
        &eh,
        &body,
        &scope.id,
        attrib.task.as_deref(),
        &mut retries,
    )
    .await;
    ctx.retries = retries;
    if retries > 0 {
        st.provider_stat(&provider, |p| p.retries += retries as u64);
        lock(&scope.state).retries += retries as u64;
    }
    ctx.t_headers = Some(Instant::now());
    let extra = |r: &mut Resp, ctx: &Ctx| {
        let h = r.headers_mut();
        for (k, v) in [
            ("x-init-req-id", ctx.req_id.clone()),
            ("x-init-retries", ctx.retries.to_string()),
            ("x-init-head-hash", ctx.prefix_hash.clone()),
            (
                "x-init-routed",
                if ctx.routed { "1".into() } else { "0".into() },
            ),
        ] {
            if let Ok(v) = HeaderValue::from_str(&v) {
                h.insert(k, v);
            }
        }
    };
    let complete_now =
        |mut ctx: Box<Ctx>, outcome: &'static str, charge: Charge, response: Option<Vec<u8>>| {
            ctx.done = true;
            let st = ctx.st.clone();
            let _ = st.completions.send(Completion {
                ctx,
                tap: None,
                outcome,
                charge,
                response,
                resp_truncated: false,
                t_end: Instant::now(),
            });
        };
    match sent {
        Sent::Failed(e, charge, outcome) => {
            st.provider_stat(&provider, |p| p.errors_proxy += 1);
            ctx.status = e.status;
            let mut r = err_resp(&e, anthropic_shaped, None);
            extra(&mut r, &ctx);
            complete_now(ctx, outcome, charge, None);
            r
        }
        Sent::Refused(e) => {
            st.provider_stat(&provider, |p| p.errors_upstream += 1);
            lock(&st.acct).root_refusals += 1;
            st.alert(
                "root_refused",
                format!("the Root refused a request: {}", e.code),
                Some(&provider),
                Some(&scope.id),
                false,
            );
            ctx.status = e.status;
            let mut r = err_resp(&e, anthropic_shaped, None);
            extra(&mut r, &ctx);
            complete_now(ctx, "root_refused", Charge::Zero, None);
            r
        }
        Sent::Buffered(parts, bytes) => {
            st.provider_stat(&provider, |p| p.errors_upstream += 1);
            ctx.status = parts.status.as_u16();
            ctx.egress_req_id = header_str(&parts.headers, "x-egress-req-id");
            ctx.egress_retries =
                header_str(&parts.headers, "x-egress-retries").and_then(|v| v.parse().ok());
            ctx.upstream_request_id = upstream_request_id(&parts.headers);
            ctx.resp_type = header_str(&parts.headers, "content-type");
            let mut r = Response::new(full(bytes.clone()));
            *r.status_mut() = parts.status;
            copy_response_headers(&parts.headers, r.headers_mut());
            extra(&mut r, &ctx);
            complete_now(ctx, "upstream_error", Charge::Zero, Some(bytes.to_vec()));
            r
        }
        Sent::Stream(resp, sender) => {
            let status = resp.status().as_u16();
            ctx.status = status;
            let (parts, inbody) = resp.into_parts();
            ctx.egress_req_id = header_str(&parts.headers, "x-egress-req-id");
            ctx.egress_retries =
                header_str(&parts.headers, "x-egress-retries").and_then(|v| v.parse().ok());
            ctx.upstream_request_id = upstream_request_id(&parts.headers);
            ctx.resp_type = header_str(&parts.headers, "content-type");
            if status == 401 || status == 403 {
                // Defence in depth (§3.3, §3.9): withhold the body, it can echo a key fragment.
                drop(inbody);
                drop(sender);
                st.provider_stat(&provider, |p| p.errors_upstream += 1);
                st.alert(
                    "upstream_auth",
                    format!("the provider rejected the Root's key (HTTP {status})"),
                    Some(&provider),
                    None,
                    false,
                );
                let e = ProxyError::new(
                    401,
                    "upstream.auth",
                    "the provider rejected the request's credentials",
                );
                let mut r = err_resp(&e, anthropic_shaped, None);
                extra(&mut r, &ctx);
                ctx.status = 401;
                complete_now(ctx, "upstream_auth", Charge::Zero, None);
                return r;
            }
            if status == 402 {
                st.alert(
                    "provider_balance",
                    "the provider reports an insufficient balance (HTTP 402)".into(),
                    Some(&provider),
                    None,
                    true,
                );
            }
            if status >= 400 {
                st.provider_stat(&provider, |p| p.errors_upstream += 1);
            }
            let streaming = ctx
                .resp_type
                .as_deref()
                .is_some_and(|c| c.starts_with("text/event-stream"));
            let p = &cfg.raw.proxy;
            let tap = Tap::new(route.family, streaming, p.max_sse_line_bytes);
            let body = MeteredBody::new(
                inbody.map_err(|e| Box::new(e) as BoxError).boxed(),
                tap,
                ctx,
                Some(sender),
                cfg.raw.traces.max_response_bytes,
                Duration::from_secs(p.upstream_idle_timeout_s.max(1)),
                Duration::from_secs(p.upstream_total_timeout_s.max(1)),
            );
            let mut r = Response::new(body.boxed());
            *r.status_mut() = parts.status;
            copy_response_headers(&parts.headers, r.headers_mut());
            let h = r.headers_mut();
            for (k, v) in [
                ("x-init-req-id", req_id),
                ("x-init-retries", retries.to_string()),
                ("x-init-head-hash", parsed.prefix_hash.clone()),
                (
                    "x-init-routed",
                    if routed { "1".into() } else { "0".into() },
                ),
            ] {
                if let Ok(v) = HeaderValue::from_str(&v) {
                    h.insert(k, v);
                }
            }
            r
        }
    }
}

fn copy_response_headers(from: &HeaderMap, to: &mut HeaderMap) {
    for (k, v) in from {
        if forward_response_header(k.as_str()) {
            to.append(k.clone(), v.clone());
        }
    }
}

/// An unmetered route (`none` family): forwarded and passed through, no budget, no trace record.
#[allow(clippy::too_many_arguments)]
async fn unmetered(
    st: &Arc<Shared>,
    route: &Route,
    egress_path: &str,
    mut eh: HeaderMap,
    body: Bytes,
    provider: &str,
    anthropic_shaped: bool,
    root: Option<&RootView>,
) -> Resp {
    if let Some(r) = root
        && let Ok(v) = HeaderValue::from_str(&format!("epoch={}", r.epoch))
    {
        eh.insert("x-egress-guard", v);
    }
    let mut retries = 0;
    match send_with_retries(
        st,
        &route.method,
        egress_path,
        &eh,
        &body,
        "-",
        None,
        &mut retries,
    )
    .await
    {
        Sent::Failed(e, _, _) | Sent::Refused(e) => {
            st.provider_stat(provider, |p| p.errors_proxy += 1);
            err_resp(&e, anthropic_shaped, None)
        }
        Sent::Buffered(parts, bytes) => {
            let mut r = Response::new(full(bytes));
            *r.status_mut() = parts.status;
            copy_response_headers(&parts.headers, r.headers_mut());
            r
        }
        Sent::Stream(resp, sender) => {
            let status = resp.status().as_u16();
            if status == 401 || status == 403 {
                st.alert(
                    "upstream_auth",
                    format!("the provider rejected the Root's key (HTTP {status})"),
                    Some(provider),
                    None,
                    false,
                );
                return err_resp(
                    &ProxyError::new(
                        401,
                        "upstream.auth",
                        "the provider rejected the request's credentials",
                    ),
                    anthropic_shaped,
                    None,
                );
            }
            let (parts, inbody) = resp.into_parts();
            let st2 = st.clone();
            let body = CheckinBody {
                inner: inbody.map_err(|e| Box::new(e) as BoxError).boxed(),
                sender: Some(sender),
                st: st2,
            };
            let mut r = Response::new(body.boxed());
            *r.status_mut() = parts.status;
            copy_response_headers(&parts.headers, r.headers_mut());
            r
        }
    }
}

/// Passes an unmetered body through and returns the egress connection to the pool at its end.
struct CheckinBody {
    inner: ProxyBody,
    sender: Option<Sender>,
    st: Arc<Shared>,
}

impl Body for CheckinBody {
    type Data = Bytes;
    type Error = BoxError;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        let r = Pin::new(&mut this.inner).poll_frame(cx);
        // hyper stops polling once `is_end_stream()` is true, so the end is also detected right
        // after the last frame.
        let ended = matches!(r, Poll::Ready(None))
            || (matches!(r, Poll::Ready(Some(Ok(_)))) && this.inner.is_end_stream());
        if ended && let Some(s) = this.sender.take() {
            this.st.egress.checkin(s);
        }
        r
    }
    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// Forwards frames unchanged and taps them (§3.10.1).
pub struct MeteredBody {
    inner: ProxyBody,
    tap: Option<Tap>,
    ctx: Option<Box<Ctx>>,
    sender: Option<Sender>,
    trace: Vec<u8>,
    max_trace: usize,
    truncated: bool,
    idle: Pin<Box<tokio::time::Sleep>>,
    idle_dur: Duration,
    deadline: Pin<Box<tokio::time::Sleep>>,
    done: bool,
}

impl MeteredBody {
    fn new(
        inner: ProxyBody,
        tap: Tap,
        ctx: Box<Ctx>,
        sender: Option<Sender>,
        max_trace: usize,
        idle_dur: Duration,
        total: Duration,
    ) -> MeteredBody {
        let now = tokio::time::Instant::now();
        MeteredBody {
            inner,
            tap: Some(tap),
            ctx: Some(ctx),
            sender,
            trace: Vec::new(),
            max_trace,
            truncated: false,
            idle: Box::pin(tokio::time::sleep_until(now + idle_dur)),
            idle_dur,
            deadline: Box::pin(tokio::time::sleep_until(now + total)),
            done: false,
        }
    }

    fn finish(&mut self, outcome: &'static str) {
        self.done = true;
        let now = Instant::now();
        let Some(mut ctx) = self.ctx.take() else {
            return;
        };
        let mut tap = self.tap.take();
        if outcome == "ok" {
            if let Some(t) = tap.as_mut() {
                t.on_end(now);
            }
            if let Some(s) = self.sender.take() {
                ctx.st.egress.checkin(s);
            }
        }
        let result = tap.map(|t| t.result().clone());
        if let Some(r) = &result
            && let Ok(mut s) = ctx.scope.state.lock()
        {
            // Run caps on actuals: counted here, before the body can see the end of this response.
            if let (Some(rc), Some(u)) = (s.run.as_mut(), r.usage) {
                rc.tokens_out = rc.tokens_out.saturating_add(u.output);
                rc.tokens_in = rc.tokens_in.saturating_add(u.input_total());
            }
            if let Some(m) = &r.model_served {
                s.model_served = Some(m.clone());
            }
        }
        let outcome = match (&result, outcome) {
            (Some(r), "ok") if r.stream_error => "stream_error",
            (_, "ok") if ctx.status >= 400 => "upstream_error",
            (_, o) => o,
        };
        let charge = if ctx.status >= 400 && outcome == "upstream_error" {
            Charge::Zero
        } else {
            Charge::Measured
        };
        ctx.done = true;
        let st = ctx.st.clone();
        let response = std::mem::take(&mut self.trace);
        let _ = st.completions.send(Completion {
            ctx,
            tap: result,
            outcome,
            charge,
            response: Some(response),
            resp_truncated: self.truncated,
            t_end: now,
        });
    }
}

impl Body for MeteredBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        if this.done {
            return Poll::Ready(None);
        }
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    let now = Instant::now();
                    if let Some(t) = this.tap.as_mut() {
                        t.on_bytes(data, now);
                    }
                    if this.trace.len() + data.len() <= this.max_trace {
                        this.trace.extend_from_slice(data);
                    } else {
                        this.truncated = true;
                    }
                }
                let next = tokio::time::Instant::now() + this.idle_dur;
                this.idle.as_mut().reset(next);
                // hyper stops polling once `is_end_stream()` is true (a content-length body after
                // its last frame), so settle here rather than waiting for a `None` that never comes.
                if this.inner.is_end_stream() {
                    this.finish("ok");
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(e))) => {
                this.finish("stream_error");
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                this.finish("ok");
                Poll::Ready(None)
            }
            Poll::Pending => {
                if this.idle.as_mut().poll(cx).is_ready()
                    || this.deadline.as_mut().poll(cx).is_ready()
                {
                    this.finish("timeout");
                    return Poll::Ready(Some(Err("upstream idle timeout".into())));
                }
                Poll::Pending
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done || self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for MeteredBody {
    fn drop(&mut self) {
        if !self.done {
            // The body disconnected mid-stream.
            self.finish("abort");
        }
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        if !self.done {
            // A request dropped before any completion (a cancelled handler): release its hold.
            let mut a = lock(&self.st.acct);
            a.settle(&self.res, self.res.r, 0, self.res.r_root, now_ms());
            drop(a);
            self.st.inflight.fetch_sub(1, Ordering::AcqRel);
            if let Ok(mut s) = self.scope.state.lock() {
                s.inflight = s.inflight.saturating_sub(1);
            }
        }
    }
}

fn ms_between(a: Instant, b: Option<Instant>) -> Option<u64> {
    b.map(|b| b.saturating_duration_since(a).as_millis() as u64)
}

/// The accounting task: settle, observe, write the `end` record, refresh the Root figure.
pub async fn accounting(st: Arc<Shared>, mut rx: mpsc::UnboundedReceiver<Completion>) {
    while let Some(c) = rx.recv().await {
        process(&st, c);
        while let Ok(c) = rx.try_recv() {
            process(&st, c);
        }
        st.refresh_root().await;
    }
}

/// Settle one completion (§3.10.3, §3.10.4, §3.11, §3.12).
pub fn process(st: &Shared, c: Completion) {
    let Completion {
        ctx,
        tap,
        outcome,
        charge,
        response,
        resp_truncated,
        t_end,
    } = c;
    let cfg = st.cfg();
    let pcfg: Option<&Provider> = cfg.providers.get(&ctx.provider);
    let usage: Option<Usage> = tap.as_ref().and_then(|t| t.usage);
    let mut flags: Vec<&'static str> = tap.as_ref().map(|t| t.flags.clone()).unwrap_or_default();
    let mut mods = ctx.req_mods.clone();
    if let Some(t) = &tap {
        for m in &t.modifiers {
            if !mods.contains(m) {
                mods.push(m.clone());
            }
        }
    }
    let (mult, unpriced) = pcfg
        .map(|p| pricing::multiplier_ppm(p, &mods))
        .unwrap_or((pricing::PPM_ONE, false));
    if unpriced {
        flags.push("unpriced_modifier");
    }
    let r = &ctx.res;
    let root = st.root();
    let root_model = root
        .as_ref()
        .and_then(|rv| rv.model(&ctx.provider, &ctx.model).cloned());
    let (cost, cost_ref, cost_root) = match (charge, usage) {
        (Charge::Zero, _) => (0, 0, 0),
        (Charge::Reservation, _) => (r.r, r.r, r.r_root),
        (Charge::Measured, Some(u)) => {
            let total = u.input_total();
            let p = pricing::prices_for(&ctx.model_table, &ctx.window, total);
            let pref =
                pricing::prices_for(&ctx.model_table, &ctx.model_table.reference_window, total);
            let root_cost = match &root_model {
                Some(m) => {
                    pricing::root_cost_pusd(&u, m.input_musd_per_mtok, m.output_musd_per_mtok)
                }
                None => pricing::cost_pusd(&u, &pricing::highest(&ctx.model_table), mult),
            };
            (
                pricing::cost_pusd(&u, &p, mult),
                pricing::cost_pusd(&u, &pref, mult),
                root_cost,
            )
        }
        (Charge::Measured, None) => {
            flags.push("usage_estimated");
            let body_bytes = tap.as_ref().map(|t| t.body_bytes).unwrap_or(0);
            let p = pricing::prices_for(&ctx.model_table, &ctx.window, ctx.req_bytes.div_ceil(3));
            let pref = pricing::prices_for(
                &ctx.model_table,
                &ctx.model_table.reference_window,
                ctx.req_bytes.div_ceil(3),
            );
            (
                pricing::estimate_pusd(ctx.req_bytes, body_bytes, &p, mult, r.r),
                pricing::estimate_pusd(ctx.req_bytes, body_bytes, &pref, mult, u64::MAX),
                r.r_root,
            )
        }
    };
    let now = now_ms();
    let model_key = format!("{}/{}", ctx.provider, ctx.model);
    // Timings (§3.11).
    let tr = tap.clone().unwrap_or_default();
    let queue_ms = ctx.t_sent.saturating_duration_since(ctx.t_read).as_millis() as u64;
    let ttfb_ms = ms_between(ctx.t_sent, ctx.t_headers);
    let ttft_ms = ms_between(ctx.t_sent, tr.first_token_at);
    let ttfc_ms = ms_between(ctx.t_sent, tr.first_content_at);
    let gen_ms = match (tr.first_token_at, tr.last_token_at) {
        (Some(a), Some(b)) => Some(b.saturating_duration_since(a).as_millis() as u64),
        _ => None,
    };
    let tps = match (gen_ms, usage) {
        (Some(g), Some(u)) if g >= 50 => Some(u.output * 1000 / g),
        _ => None,
    };
    let duration_ms = t_end.saturating_duration_since(ctx.t_read).as_millis() as u64;
    let finish = tr.finish;
    let cache_permille = usage.and_then(|u| u.cache_permille());
    // Scope state and observations.
    {
        let mut s = lock(&ctx.scope.state);
        s.inflight = s.inflight.saturating_sub(1);
        if outcome != "ok" {
            s.errors += 1;
        }
        s.spent_pusd = s.spent_pusd.saturating_add(cost);
        s.cost_ref_pusd = s.cost_ref_pusd.saturating_add(cost_ref);
        if let Some(u) = usage {
            s.usage.input_uncached += u.input_uncached;
            s.usage.input_cache_read += u.input_cache_read;
            s.usage.input_cache_write += u.input_cache_write;
            s.usage.input_cache_write_1h += u.input_cache_write_1h;
            s.usage.output += u.output;
            s.usage.reasoning += u.reasoning;
            if u.input_total() > 0 {
                s.cache
                    .push_back((now, u.input_cache_read, u.input_total()));
                if s.cache.len() > 4096 {
                    s.cache.pop_front();
                }
                let e = s
                    .cache_by_session
                    .entry(ctx.session.clone().unwrap_or_else(|| "_none".into()))
                    .or_default();
                e.0 += u.input_cache_read;
                e.1 += u.input_total();
            }
        }
        let bm = s.obs.by_model.entry(model_key.clone()).or_default();
        bm.requests += 1;
        bm.routed += ctx.routed as u64;
        bm.spent_pusd = bm.spent_pusd.saturating_add(cost);
        bm.cost_ref_pusd = bm.cost_ref_pusd.saturating_add(cost_ref);
        s.obs.on_tool_calls(&tr.tool_calls);
        if let Some(f) = finish {
            s.obs.last_finish = Some((f.as_str().to_string(), now));
        }
        if let Some(v) = ttft_ms {
            s.ttft.push(now, v);
        }
        if let Some(v) = ttfc_ms {
            s.ttfc.push(now, v);
        }
        if let Some(v) = tps {
            s.tps.push(now, v);
        }
        if let Some(v) = ctx.gap_ms {
            s.gap.push(now, v);
        }
        let sess = ctx.session.clone().unwrap_or_else(|| "_none".into());
        s.prev_end
            .insert(sess, (now, finish == Some(Finish::ToolUse)));
        s.last_head_hash = Some(ctx.prefix_hash.clone());
        s.last = Some(json!({
            "req_id": ctx.req_id, "provider": ctx.provider, "model": ctx.model, "status": ctx.status, "outcome": outcome,
            "finish": finish.map(|f| f.as_str()), "retries": ctx.retries, "window": ctx.window,
            "usage": usage.map(|u| serde_json::to_value(u).unwrap_or(Value::Null)),
            "cost_musd": pusd_to_musd(cost), "cache_permille": cache_permille,
            "ttft_ms": ttft_ms, "ttfc_ms": ttfc_ms, "tps": tps, "duration_ms": duration_ms, "gap_ms": ctx.gap_ms,
        }));
    }
    // The `end` record (§3.13).
    let mut f = Map::new();
    f.insert("req_id".into(), json!(ctx.req_id));
    f.insert("ts".into(), json!(rfc3339_ms(now)));
    f.insert("status".into(), json!(ctx.status));
    f.insert("outcome".into(), json!(outcome));
    f.insert("retries".into(), json!(ctx.retries));
    f.insert(
        "usage".into(),
        usage
            .map(|u| serde_json::to_value(u).unwrap_or(Value::Null))
            .unwrap_or(Value::Null),
    );
    f.insert("usage_flags".into(), json!(flags));
    f.insert("cost_pusd".into(), json!(cost));
    f.insert("cost_ref_pusd".into(), json!(cost_ref));
    f.insert("root_cost_musd".into(), Value::Null);
    f.insert("cache_permille".into(), json!(cache_permille));
    f.insert("queue_ms".into(), json!(queue_ms));
    f.insert("ttfb_ms".into(), json!(ttfb_ms));
    f.insert("ttft_ms".into(), json!(ttft_ms));
    f.insert("ttfc_ms".into(), json!(ttfc_ms));
    f.insert("gen_ms".into(), json!(gen_ms));
    f.insert("tps".into(), json!(tps));
    f.insert("duration_ms".into(), json!(duration_ms));
    f.insert("gap_ms".into(), json!(ctx.gap_ms));
    f.insert("finish".into(), json!(finish.map(|x| x.as_str())));
    f.insert(
        "tool_calls".into(),
        Value::Array(
            tr.tool_calls
                .iter()
                .map(|t| json!({"args": t.args, "id": t.id, "name": t.name}))
                .collect(),
        ),
    );
    f.insert("upstream_request_id".into(), json!(ctx.upstream_request_id));
    f.insert("egress_req_id".into(), json!(ctx.egress_req_id));
    f.insert("egress_retries".into(), json!(ctx.egress_retries));
    f.insert("resp_type".into(), json!(ctx.resp_type));
    f.insert("resp_truncated".into(), json!(resp_truncated));
    let store = cfg.raw.traces.store_bodies;
    st.writer.end_record(EndJob {
        req_id: ctx.req_id.clone(),
        fields: f,
        response: if store { response } else { None },
    });
    let dropped = st.writer.dropped();
    if dropped > 0 {
        st.alert(
            "trace_dropped",
            format!("{dropped} trace payloads dropped (writer queue full)"),
            None,
            None,
            true,
        );
    }
    // Settle last, so that a reader who sees the money moved also sees the request's observations.
    {
        let mut a = lock(&st.acct);
        a.settle(&ctx.res, cost, cost_ref, cost_root, now);
        a.attribute(&model_key, ctx.task_key.as_deref(), ctx.routed, cost);
    }
    st.inflight.fetch_sub(1, Ordering::AcqRel);
    let mut ctx = ctx;
    ctx.done = true;
}

/// Bind a lane socket (§3.2): a stale socket at exactly this path is replaced; mode 0600.
pub fn bind_socket(path: &Path) -> std::io::Result<std::os::unix::net::UnixListener> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if path.as_os_str().len() > 103 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "socket path longer than 103 bytes",
        ));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::symlink_metadata(path).is_ok() {
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "a live socket is bound at this path",
            ));
        }
        std::fs::remove_file(path)?;
    }
    let l = std::os::unix::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    l.set_nonblocking(true)?;
    let _ = std::fs::metadata(path)?.uid();
    Ok(l)
}

/// A boxed future, for spawning from std threads.
pub type BoxFut = Pin<Box<dyn Future<Output = ()> + Send>>;
