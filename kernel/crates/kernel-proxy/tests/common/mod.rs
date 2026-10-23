//! Test harness: a home under `kernel/target/test-homes/wp5/`, `init-mockllm` (WP2) as the upstream,
//! and an in-test fake Root egress socket in front of it (the kernel-facing egress contract of
//! root/egress.md §3.6, §4.1, §4.2: `GET /_root/ceilings`, typed refusals with
//! `x-init-egress-error`, `x-egress-*` response headers, epochs and a peak-priced spend counter).

#![allow(dead_code)]

use http_body_util::{BodyExt, Full};
use hyper::body::{Body, Bytes, Frame, Incoming, SizeHint};
use hyper::{Request, Response};
use init_testkit::mockllm::client::{HttpResponse, request_unix};
use init_testkit::mockllm::http::{Listen, MockServer};
use init_testkit::mockllm::{Mock, MockConfig, MockRequest, Mode, Script};
use kernel_proxy::tap::{Family, Tap};
use kernel_proxy::{Alert, Label, Level, Proxy, ProxyConfig, ProxyHandle, StartOptions};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A fresh test home (exact path, owned by this test).
pub fn home(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp5");
    std::fs::create_dir_all(&base).unwrap();
    let d = base.canonicalize().unwrap().join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn rfc3339(ms: u64) -> String {
    let days = (ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let rem = ms % 86_400_000;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60
    )
}

/// One request as the fake egress saw it.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn header(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
    }
}

/// A model of the fake Root's table: (provider, model, min_level, input µUSD/MTok, output µUSD/MTok).
pub type RootModelRow = (&'static str, &'static str, u8, u64, u64);

/// The fake Root egress state.
pub struct EgressState {
    pub level: AtomicU64,
    pub epoch: AtomicU64,
    pub day_limit_musd: AtomicU64,
    pub total_limit_musd: AtomicU64,
    /// pico-USD, peak-priced (root/egress.md §3.4).
    pub day_spent_pusd: AtomicU64,
    pub total_spent_pusd: AtomicU64,
    pub reserved_pusd: AtomicU64,
    pub models: Mutex<Vec<RootModelRow>>,
    pub seen: Mutex<Vec<Seen>>,
    pub refusals: Mutex<VecDeque<(u16, String)>>,
    /// Requests the fake itself had to refuse on a ceiling (should never happen behind a correct guard).
    pub ceiling_refusals: AtomicU64,
    pub ceilings_calls: AtomicU64,
    pub mock_addr: String,
    pub publish_models: AtomicBool,
}

impl EgressState {
    pub fn models_json(&self) -> Value {
        let level = self.level.load(Ordering::SeqCst) as u8;
        Value::Array(
            self.models
                .lock()
                .unwrap()
                .iter()
                .map(|(p, m, l, i, o)| {
                    json!({"provider": p, "model": m, "min_level": l, "input_musd_per_mtok": i,
                           "output_musd_per_mtok": o, "routable": *l <= level})
                })
                .collect(),
        )
    }

    pub fn ceilings(&self) -> Value {
        let now = now_ms();
        let start = now / 86_400_000 * 86_400_000;
        let mut v = json!({"v": 1, "epoch": self.epoch.load(Ordering::SeqCst), "level": self.level.load(Ordering::SeqCst),
            "money": {"day_limit_musd": self.day_limit_musd.load(Ordering::SeqCst),
                      "day_spent_musd": self.day_spent_pusd.load(Ordering::SeqCst) / 1_000_000,
                      "day_reserved_musd": 0,
                      "day_starts_at": rfc3339(start), "day_resets_at": rfc3339(start + 86_400_000),
                      "total_limit_musd": self.total_limit_musd.load(Ordering::SeqCst),
                      "total_spent_musd": self.total_spent_pusd.load(Ordering::SeqCst) / 1_000_000,
                      "sealed_reserve_musd": 0},
            "classes": []});
        if self.publish_models.load(Ordering::SeqCst) {
            v["models"] = self.models_json();
        }
        v
    }

    pub fn set_spent_musd(&self, day: u64, total: u64) {
        self.day_spent_pusd.store(day * 1_000_000, Ordering::SeqCst);
        self.total_spent_pusd
            .store(total * 1_000_000, Ordering::SeqCst);
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    pub fn seen_posts(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.method == "POST")
            .cloned()
            .collect()
    }
}

/// The fake egress server (its own tokio runtime).
pub struct FakeEgress {
    pub st: Arc<EgressState>,
    pub path: PathBuf,
    rt: Option<tokio::runtime::Runtime>,
    stop: Arc<tokio::sync::Notify>,
}

type FBody = http_body_util::combinators::BoxBody<Bytes, BoxError>;

fn full(b: impl Into<Bytes>) -> FBody {
    Full::new(b.into())
        .map_err(|n: std::convert::Infallible| match n {})
        .boxed()
}

/// Settles the fake Root's peak-priced spend when the forwarded body ends.
struct SettleBody {
    inner: FBody,
    tap: Option<Tap>,
    st: Arc<EgressState>,
    r: u64,
    prices: (u64, u64),
    done: bool,
}

impl SettleBody {
    fn settle(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        let mut cost = self.r;
        if let Some(mut t) = self.tap.take() {
            t.on_end(Instant::now());
            if let Some(u) = t.result().usage {
                cost = u.input_total() * self.prices.0 + u.output * self.prices.1;
            }
        }
        self.st.reserved_pusd.fetch_sub(self.r, Ordering::SeqCst);
        self.st.day_spent_pusd.fetch_add(cost, Ordering::SeqCst);
        self.st.total_spent_pusd.fetch_add(cost, Ordering::SeqCst);
        self.st.epoch.fetch_add(1, Ordering::SeqCst);
    }
}

impl Body for SettleBody {
    type Data = Bytes;
    type Error = BoxError;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        let r = Pin::new(&mut this.inner).poll_frame(cx);
        match &r {
            Poll::Ready(Some(Ok(f))) => {
                if let (Some(d), Some(t)) = (f.data_ref(), this.tap.as_mut()) {
                    t.on_bytes(d, Instant::now());
                }
            }
            Poll::Ready(None) | Poll::Ready(Some(Err(_))) => this.settle(),
            Poll::Pending => {}
        }
        r
    }
    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for SettleBody {
    fn drop(&mut self) {
        self.settle();
    }
}

async fn egress_handle(
    req: Request<Incoming>,
    st: Arc<EgressState>,
) -> Result<Response<FBody>, std::convert::Infallible> {
    let method = req.method().as_str().to_string();
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_default();
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let body = req
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes())
        .unwrap_or_default();
    if path == "/_root/ceilings" {
        st.ceilings_calls.fetch_add(1, Ordering::SeqCst);
        let mut r = Response::new(full(serde_json::to_vec(&st.ceilings()).unwrap()));
        r.headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        return Ok(r);
    }
    st.seen.lock().unwrap().push(Seen {
        method: method.clone(),
        path: path.clone(),
        headers: headers.clone(),
        body: body.to_vec(),
    });
    let refusal = |status: u16, code: &str, init: Value| {
        let mut r = Response::new(full(
            serde_json::to_vec(&json!({"error": {"type": "init_ceiling_exceeded", "code": code, "message": "refused"}, "init": init})).unwrap(),
        ));
        *r.status_mut() = hyper::StatusCode::from_u16(status).unwrap();
        r.headers_mut()
            .insert("x-init-egress-error", code.parse().unwrap());
        r.headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        r
    };
    if let Some((status, code)) = st.refusals.lock().unwrap().pop_front() {
        let init = json!({"source": "root", "code": code, "limit_musd": st.day_limit_musd.load(Ordering::SeqCst),
                          "spent_musd": st.day_spent_pusd.load(Ordering::SeqCst) / 1_000_000, "reserved_musd": 0,
                          "epoch": st.epoch.load(Ordering::SeqCst), "retryable": code == "ceiling.day", "demotes": false});
        return Ok(refusal(status, &code, init));
    }
    // /<provider>/<rest>
    let rest = path
        .trim_start_matches('/')
        .split_once('/')
        .map(|(_, r)| format!("/{r}"))
        .unwrap_or_default();
    // Peak-priced reservation against the ceiling (root/egress.md §3.4).
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let model = v["model"].as_str().unwrap_or("").to_string();
    let prices = st
        .models
        .lock()
        .unwrap()
        .iter()
        .find(|m| m.1 == model)
        .map(|m| (m.3, m.4))
        .unwrap_or((300_000, 1_200_000));
    let max_out = v["max_tokens"].as_u64().unwrap_or(65_536);
    let r = (body.len() as u64).div_ceil(3) * prices.0 + max_out * prices.1;
    if method == "POST" {
        let limit = st.day_limit_musd.load(Ordering::SeqCst) * 1_000_000;
        let used =
            st.day_spent_pusd.load(Ordering::SeqCst) + st.reserved_pusd.load(Ordering::SeqCst);
        if used + r > limit {
            st.ceiling_refusals.fetch_add(1, Ordering::SeqCst);
            let init = json!({"source": "root", "code": "ceiling.day", "limit_musd": limit / 1_000_000, "spent_musd": used / 1_000_000,
                              "reserved_musd": 0, "retryable": true, "demotes": false});
            return Ok(refusal(429, "ceiling.day", init));
        }
        st.reserved_pusd.fetch_add(r, Ordering::SeqCst);
    }
    // Forward to the mock (upstream mode), every header except hop-by-hop and the x-egress-* labels.
    let up = async {
        let tcp = tokio::net::TcpStream::connect(&st.mock_addr).await?;
        let (mut s, conn) = hyper::client::conn::http1::handshake::<_, Full<Bytes>>(
            hyper_util::rt::TokioIo::new(tcp),
        )
        .await?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let mut b = Request::builder()
            .method(method.as_str())
            .uri(rest.as_str());
        for (k, v) in &headers {
            if matches!(
                k.as_str(),
                "host" | "content-length" | "transfer-encoding" | "connection"
            ) || k.starts_with("x-egress-")
            {
                continue;
            }
            b = b.header(k.as_str(), v.as_str());
        }
        b = b.header("host", "mock");
        let resp = s.send_request(b.body(Full::new(body.clone()))?).await?;
        Ok::<_, BoxError>(resp)
    };
    match up.await {
        Err(e) => {
            if method == "POST" {
                st.reserved_pusd.fetch_sub(r, Ordering::SeqCst);
            }
            let mut resp = Response::new(full(format!("upstream: {e}")));
            *resp.status_mut() = hyper::StatusCode::BAD_GATEWAY;
            Ok(resp)
        }
        Ok(resp) => {
            let (parts, inner) = resp.into_parts();
            let streaming = parts
                .headers
                .get("content-type")
                .and_then(|c| c.to_str().ok())
                .is_some_and(|c| c.starts_with("text/event-stream"));
            let tap = (method == "POST").then(|| Tap::new(Family::OpenAiChat, streaming, 1 << 20));
            let body = SettleBody {
                inner: inner.map_err(|e| Box::new(e) as BoxError).boxed(),
                tap,
                st: st.clone(),
                r: if method == "POST" { r } else { 0 },
                prices,
                done: method != "POST",
            };
            let mut out = Response::new(body.boxed());
            *out.status_mut() = parts.status;
            for (k, v) in parts.headers.iter() {
                if !matches!(
                    k.as_str(),
                    "content-length" | "transfer-encoding" | "connection"
                ) {
                    out.headers_mut().append(k.clone(), v.clone());
                }
            }
            let id = format!("eg-{}", st.seen.lock().unwrap().len());
            out.headers_mut()
                .insert("x-egress-req-id", id.parse().unwrap());
            out.headers_mut()
                .insert("x-egress-retries", "0".parse().unwrap());
            Ok(out)
        }
    }
}

impl FakeEgress {
    pub fn start(path: &Path, mock_addr: String, models: Vec<RootModelRow>) -> FakeEgress {
        let st = Arc::new(EgressState {
            level: AtomicU64::new(0),
            epoch: AtomicU64::new(1),
            day_limit_musd: AtomicU64::new(10_000_000),
            total_limit_musd: AtomicU64::new(25_000_000),
            day_spent_pusd: AtomicU64::new(0),
            total_spent_pusd: AtomicU64::new(0),
            reserved_pusd: AtomicU64::new(0),
            models: Mutex::new(models),
            seen: Mutex::new(Vec::new()),
            refusals: Mutex::new(VecDeque::new()),
            ceiling_refusals: AtomicU64::new(0),
            ceilings_calls: AtomicU64::new(0),
            mock_addr,
            publish_models: AtomicBool::new(true),
        });
        Self::start_with(path, st)
    }

    pub fn start_with(path: &Path, st: Arc<EgressState>) -> FakeEgress {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(path);
        let l = {
            let _g = rt.enter();
            tokio::net::UnixListener::bind(path).unwrap()
        };
        let stop = Arc::new(tokio::sync::Notify::new());
        let (st2, stop2) = (st.clone(), stop.clone());
        rt.spawn(async move {
            loop {
                let a = tokio::select! {
                    _ = stop2.notified() => break,
                    a = l.accept() => a,
                };
                let Ok((s, _)) = a else { continue };
                let st3 = st2.clone();
                tokio::spawn(async move {
                    let svc =
                        hyper::service::service_fn(move |req| egress_handle(req, st3.clone()));
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(s), svc)
                        .await;
                });
            }
        });
        FakeEgress {
            st,
            path: path.to_path_buf(),
            rt: Some(rt),
            stop,
        }
    }

    /// Stop serving, close every connection and remove the socket (exact path).
    pub fn stop(mut self) -> Arc<EgressState> {
        self.stop.notify_one();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(Duration::from_millis(200));
        }
        let _ = std::fs::remove_file(&self.path);
        self.st.clone()
    }
}

impl Drop for FakeEgress {
    fn drop(&mut self) {
        self.stop.notify_one();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(Duration::from_millis(200));
        }
    }
}

/// The standard test models: priced in kernel config and in the fake Root's table.
pub fn default_root_models() -> Vec<RootModelRow> {
    vec![
        ("mock", "mock-frontier-1", 0, 300_000, 1_200_000),
        ("mock", "mock-pro", 1, 1_320_000, 3_960_000),
    ]
}

pub const CONFIG: &str = r#"
[model]
provider = "mock"
id = "mock-frontier-1"

[proxy]
test_mode = true
root_poll_s = 1

[budget]
day_usd = "5.00"
task_usd = "0.50"
request_max_usd = "1.00"

[providers.mock]
preset = "deepseek"

[providers.mock.models."mock-frontier-1"]
max_output = 65536
prices.standard = { input = "0.30", cache_read = "0.006", output = "1.20" }

[providers.mock.models."mock-pro"]
max_output = 65536
prices.standard = { input = "1.32", cache_read = "0.044", output = "3.96" }
"#;

/// Everything one test needs.
pub struct Rig {
    pub home: PathBuf,
    pub mock: MockServer,
    pub egress: Option<FakeEgress>,
    pub proxy: ProxyHandle,
    pub alerts: Arc<Mutex<Vec<Alert>>>,
    pub sock: PathBuf,
}

pub struct RigOpts<'a> {
    pub script: &'a str,
    pub config: &'a str,
    pub level: u64,
    pub day_limit_musd: u64,
    pub strict: bool,
    pub judge: bool,
    pub models: Vec<RootModelRow>,
}

impl Default for RigOpts<'_> {
    fn default() -> Self {
        RigOpts {
            script: r#"{"v":1,"kind":"meta","provider":"deepseek","model":"mock-frontier-1","name":"wp5","chunk":8}"#,
            config: CONFIG,
            level: 0,
            day_limit_musd: 10_000_000,
            strict: false,
            judge: false,
            models: default_root_models(),
        }
    }
}

impl Rig {
    pub fn new(name: &str, o: RigOpts) -> Rig {
        let home = home(name);
        let script = Script::parse(o.script, Path::new(".")).unwrap();
        let mut mc = MockConfig::new(Mode::Upstream, script);
        mc.strict_deepseek = o.strict;
        let mock =
            MockServer::start(Mock::new(mc).unwrap(), Listen::Tcp("127.0.0.1:0".into())).unwrap();
        let egress_path = home.join("root/run/egress.sock");
        let egress = FakeEgress::start(&egress_path, mock.addr().unwrap().to_string(), o.models);
        egress.st.level.store(o.level, Ordering::SeqCst);
        egress
            .st
            .day_limit_musd
            .store(o.day_limit_musd, Ordering::SeqCst);
        let cfg: ProxyConfig = toml::from_str(o.config).unwrap();
        let alerts = Arc::new(Mutex::new(Vec::new()));
        let a2 = alerts.clone();
        let proxy = Proxy::start(
            &cfg,
            StartOptions {
                home: home.clone(),
                egress_sock: egress_path,
                live_socket: None,
                label: Label {
                    sha: "0e786c7".into(),
                    boot_reason: "test".into(),
                    model: "mock-frontier-1".into(),
                    level: Level::L0,
                },
                judge: o.judge,
            },
            Arc::new(move |a: Alert| a2.lock().unwrap().push(a)),
        )
        .unwrap();
        let sock = home.join("run/proxy.sock");
        Rig {
            home,
            mock,
            egress: Some(egress),
            proxy,
            alerts,
            sock,
        }
    }

    pub fn eg(&self) -> &Arc<EgressState> {
        &self.egress.as_ref().unwrap().st
    }

    pub fn alert_codes(&self) -> Vec<String> {
        self.alerts
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.code.clone())
            .collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.proxy.shutdown(Duration::from_secs(2));
    }
}

/// A DeepSeek chat request body.
pub fn chat(model: &str, user: &str, max_tokens: u64) -> Value {
    json!({"model": model, "stream": true, "max_tokens": max_tokens, "thinking": {"type": "enabled"},
           "messages": [{"role": "system", "content": "You are INIT."}, {"role": "user", "content": user}],
           "tools": [{"type": "function", "function": {"name": "bash", "description": "run", "parameters": {"type": "object"}}}]})
}

/// POST through the proxy socket.
pub fn post(sock: &Path, path: &str, body: &Value, headers: &[(&str, &str)]) -> HttpResponse {
    let mut h: Vec<(&str, &str)> = vec![
        ("content-type", "application/json"),
        ("accept", "text/event-stream"),
    ];
    h.extend_from_slice(headers);
    request_unix(
        sock,
        &MockRequest::post(path, &h, serde_json::to_vec(body).unwrap()),
    )
    .unwrap()
}

pub fn get(sock: &Path, path: &str) -> HttpResponse {
    request_unix(sock, &MockRequest::get(path)).unwrap()
}

pub fn get_json(sock: &Path, path: &str) -> Value {
    let r = get(sock, path);
    assert_eq!(r.status, 200, "{path}: {}", r.text());
    serde_json::from_slice(&r.body).unwrap()
}

pub fn json_of(r: &HttpResponse) -> Value {
    serde_json::from_slice(&r.body).unwrap_or(Value::Null)
}

/// ICJ: integers only, anywhere.
pub fn assert_integers_only(v: &Value, path: &str) {
    match v {
        Value::Number(n) => assert!(n.is_u64() || n.is_i64(), "float at {path}: {n}"),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .for_each(|(i, x)| assert_integers_only(x, &format!("{path}/{i}"))),
        Value::Object(m) => m
            .iter()
            .for_each(|(k, x)| assert_integers_only(x, &format!("{path}/{k}"))),
        _ => {}
    }
}

/// Every file under `dir`, recursively.
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// Poll until `f` holds (accounting runs after the response's last byte).
pub fn wait_for(mut f: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}
