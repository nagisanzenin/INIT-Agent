//! R2/R3: the egress gateway (root/egress.md): `ROOT_HOME/run/egress.sock`, the agent zone's only
//! network path. Provider keys, the metering forwarder (ureq on std threads), reservations against
//! observed costs in integer micro-USD, action classes, the refusal format and its lineage
//! records (D6), and `GET /_root/ceilings`. WP24 adds sealed lanes (§3.8).
//! Lock order: `st` is never held while calling into the Root (append, alert, its state).
// A refusal is built at most once per request, on the cold path; boxing it buys nothing.
#![allow(clippy::result_large_err)]

mod classes;
mod http;
mod lane;
mod money;
mod tap;

pub use classes::effective_actions;
pub use lane::{Caps, sealed_lane, twin_lane};
pub use money::owner_day;

use crate::charter::Charter;
use crate::install::VERSION;
use crate::link::{Root, lock};
use crate::sup::Slot;
use crate::util::{self, icj, now_ms, rfc3339};
use crate::sys;
use classes::{Auth, Class};
use http::Head;
use money::{Ledger, Limits, Money};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{self, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tap::{Family, Tap};

pub const AREA: &str = "egress";
/// `egress.max_inflight` (§3.1).
const MAX_INFLIGHT: usize = 64;
/// F3: `egress.max_conns`, kernel connections served at once (in flight, the kernel's idle pool of 16, local
/// GETs); one more is answered 503 `root.busy` and closed by the accept thread, which spawns nothing for it.
/// One descriptor each, so with ≤ 64 upstream sockets and a pool of 16 the egress stays near 176 descriptors,
/// under launchd's default soft limit of 256.
const MAX_CONNS: usize = 96;
/// F3: a connection may idle this long between requests, and a response write may stall this long.
const CONN_IDLE: Duration = Duration::from_secs(120);
/// F3: a request's head and body must arrive within this time of its first byte.
const REQ_READ: Duration = Duration::from_secs(30);
/// Kernel headers forwarded upstream (§4.1); every other header, auth included, is dropped.
const KERNEL_HEADERS: [&str; 6] = ["content-type", "accept", "anthropic-version", "anthropic-beta", "openai-beta", "idempotency-key"];
/// Provider headers relayed back (kernel/proxy.md §4.1 allowlist), plus the `*-ratelimit-*` families.
const RESP_HEADERS: [&str; 7] = ["content-type", "retry-after", "retry-after-ms", "request-id", "x-request-id", "x-ds-trace-id", "openai-processing-ms"];
const ALERT_REPEAT_MS: i64 = 600_000;

/// `x-egress-subtask` (MERGE, body/tools.md §3.7): the kernel forwards a `delegate` sub-task's label so the sentinel
/// counts it apart from the main loop (H10, H5b). Only the §3.6 id grammar counts (≤ 64 bytes,
/// `[a-zA-Z0-9][a-zA-Z0-9._-]*`); like every zone header it never reaches the provider (`send` relays `KERNEL_HEADERS`).
fn subtask(h: &Head) -> bool {
    h.get("x-egress-subtask").is_some_and(|v| {
        let b = v.as_bytes();
        !b.is_empty() && b.len() <= 64 && b[0].is_ascii_alphanumeric() && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    })
}

/// A provider key in memory: no `Debug`, `Display` or `Serialize`; zeroed on drop (§3.2).
pub struct Secret(Vec<u8>);

impl Drop for Secret {
    fn drop(&mut self) {
        sys::zero(&mut self.0);
    }
}

/// A refusal (§3.6) before it is rendered.
#[derive(Default)]
pub struct Refusal {
    pub code: &'static str,
    pub msg: String,
    pub class: Option<String>,
    pub demotes: bool,
    /// D6: `a` | `b` | `c` | `none`.
    pub rule: &'static str,
    /// Append a `ceiling_hit` record.
    pub record: bool,
    pub limit: Option<u64>,
    pub attempted: Option<u64>,
    pub retry_after: Option<u64>,
    pub money: Option<Value>,
    /// The upstream's own status for `upstream.auth`; otherwise derived from the code.
    pub status: Option<u16>,
}

fn refusal(code: &'static str, msg: impl Into<String>) -> Refusal {
    Refusal { code, msg: msg.into(), ..Refusal::default() }
}

fn status(code: &str) -> u16 {
    match code {
        "ceiling.day" | "ceiling.total" | "class.volume" | "class.rate" => 429,
        "request.invalid" => 400,
        "request.too_large" => 413,
        "root.unsupported" => 501,
        "upstream.transport" | "upstream.redirect" => 502,
        c if c.starts_with("root.") || c == "class.disabled" => 503,
        _ => 403,
    }
}

/// D6 (§3.6 table): which refusals demote, under which rule, and whether a `ceiling_hit` is written
/// (every demoting hit; at most one ordinary hit per ceiling per owner day).
fn classify(r: &mut Refusal, effectful: bool, m: &mut Money) {
    let (demotes, rule) = match r.code {
        "class.destination" | "class.method" | "class.path" | "class.unknown" | "class.provider_tool" => (true, "a"),
        "class.level" | "class.model" => (true, "b"),
        "class.volume" | "class.rate" => (effectful, if effectful { "a" } else { "none" }),
        "ceiling.day" | "ceiling.total" | "ceiling.lane" | "ceiling.twin" => { r.demotes = false; r.rule = "none"; r.record = false; return; },
        _ => return, // class.disabled, request.*, root.*, upstream.*: not a hit
    };
    (r.demotes, r.rule) = (demotes, rule);
    r.record = demotes || m.first_today(format!("{}:{}", r.code, r.class.as_deref().unwrap_or("")));
}

/// The compiled charter view: classes, keys and their states, test mode.
struct Cfg {
    benchmark: Result<Option<crate::benchmark::Policy>, String>,
    classes: BTreeMap<String, Class>,
    keys: BTreeMap<String, Arc<Secret>>,
    key_state: BTreeMap<String, &'static str>,
    test_mode: bool,
}

impl Cfg {
    /// Loads `keys/providers/<id>` for every provider of a model class (R-F4). Test mode accepts only
    /// keys marked `test-…`, so a test Root never loads a real key (testing.md §3.4.1).
    fn load(rh: &Path, ch: &Charter) -> (Cfg, Vec<(String, String)>) {
        let classes = classes::compile(&ch.actions);
        let test_mode = ch.root.test_mode;
        let (mut keys, mut key_state, mut alerts) = (BTreeMap::new(), BTreeMap::new(), Vec::new());
        for (pid, provider) in classes.values().filter(|c| c.model).flat_map(|c| c.providers.iter()) {
            if provider.no_auth && provider.local && classes::loopback(&provider.host) {
                key_state.insert(pid.clone(), "not_required");
                continue;
            }
            let state = match util::read_secret(&rh.join("keys/providers").join(pid)) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => "missing",
                Err(_) => "insecure",
                Ok(mut b) => {
                    while b.last().is_some_and(|c| *c == b'\n' || *c == b'\r') {
                        b.pop();
                    }
                    let ok = (1..=512).contains(&b.len()) && std::str::from_utf8(&b).is_ok_and(|s| !s.chars().any(char::is_control));
                    let state = if !ok { "insecure" } else if test_mode && !b.starts_with(b"test-") { "refused" } else { "present" };
                    if state == "present" {
                        keys.insert(pid.clone(), Arc::new(Secret(b)));
                    } else {
                        sys::zero(&mut b);
                    }
                    state
                }
            };
            match state {
                "insecure" => alerts.push(("key_file_insecure".into(), format!("[egress] keys/providers/{pid} is insecure (0600, yours, 1–512 bytes); provider {pid} is disabled"))),
                "refused" => alerts.push(("key_file_real".into(), format!("[egress] test mode refuses keys/providers/{pid}: test keys start with \"test-\""))),
                _ => {}
            }
            key_state.insert(pid.clone(), state);
        }
        (Cfg { benchmark: crate::benchmark::load(rh).map_err(|e| e.to_string()), classes, keys, key_state, test_mode }, alerts)
    }
}

/// An admitted request: where it goes and what it holds.
struct Admit {
    id: String,
    class: String,
    effectful: bool,
    provider: Option<String>,
    model: Option<String>,
    family: Family,
    url: String,
    dest: String,
    auth: Option<(Auth, Arc<Secret>)>,
    price: Option<classes::Model>,
    reserved: u64,
    day: i64,
    epoch: u64,
    response_max: u64,
    date: String,
}

pub struct Egress {
    root: Weak<Root>,
    rh: PathBuf,
    ledger: Ledger,
    st: Mutex<(Cfg, Money)>,
    level: AtomicI64,
    safe: AtomicBool,
    panicked: AtomicBool,
    inflight: AtomicUsize,
    /// F3: open kernel connections.
    open: AtomicUsize,
    seq: AtomicU64,
    boot: String,
    /// Open kernel connections, one descriptor each (F3: the read side, the write side and `panic` share it).
    conns: Mutex<BTreeMap<u64, Arc<UnixStream>>>,
    agent: ureq::Agent,
    alerted: Mutex<BTreeMap<String, i64>>,
}

/// F3: one counted kernel connection, uncounted on drop.
struct Open(Arc<Egress>);
impl Drop for Open { fn drop(&mut self) { self.0.open.fetch_sub(1, SeqCst); } }

struct Inflight<'a>(&'a AtomicUsize);
impl Drop for Inflight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, SeqCst);
    }
}

/// Starts the gateway (§4.3 `start`): rebuilds the counters from the ledger, then binds `run/egress.sock` (0600).
pub fn start(root: &Arc<Root>) -> io::Result<Arc<Egress>> {
    let (ch, level, safe, panicked) = {
        let st = lock(&root.st);
        (st.charter.as_ref().map(|l| l.ch.clone()), st.level.level, st.mode == "safe", matches!(st.slot, Slot::Panicked { .. }))
    };
    let ch = ch.ok_or_else(|| util::bad("no active charter"))?;
    let rh = root.p.rh.clone();
    let dir = rh.join("egress/usage");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    let now = now_ms();
    let _ = money::checkpoint(&dir, now);
    let m = money::rebuild(&dir, Limits::of(&ch), now);
    let (mut cfg, alerts) = Cfg::load(&rh, &ch);
    cfg.benchmark = Ok(root.benchmark.clone());
    let agent: ureq::Agent = ureq::Agent::config_builder().proxy(None).http_status_as_error(false).max_redirects(0).max_redirects_will_error(false)
        .user_agent(format!("init-root/{VERSION}")).timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(600))).timeout_recv_body(Some(Duration::from_secs(1800)))
        .max_idle_connections_per_host(16).build().into();
    let e = Arc::new(Egress { root: Arc::downgrade(root), rh: rh.clone(), ledger: Ledger { dir }, st: Mutex::new((cfg, m)),
        level: AtomicI64::new(level), safe: AtomicBool::new(safe), panicked: AtomicBool::new(panicked), inflight: AtomicUsize::new(0),
        open: AtomicUsize::new(0), seq: AtomicU64::new(0), boot: util::hex(&util::random::<4>()?), conns: Mutex::new(BTreeMap::new()), agent,
        alerted: Mutex::new(BTreeMap::new()) });
    root.egress.set(e.clone()).map_err(|_| util::bad("egress already started"))?;
    crate::extend::start(root); // X5: the sentinel's caps and closed-turn hooks; caps.json
    alerts.iter().for_each(|(c, m)| e.alert(c, m));
    let sock = rh.join("run/egress.sock");
    if std::fs::symlink_metadata(&sock).is_ok_and(|m| m.file_type().is_socket()) {
        std::fs::remove_file(&sock)?; // a stale socket of a dead Root; we hold run/root.lock
    }
    let l = UnixListener::bind(&sock)?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    let w = e.clone();
    std::thread::spawn(move || {
        for c in l.incoming() {
            // EMFILE and the like: no hot loop
            let Ok(mut c) = c else { std::thread::sleep(Duration::from_millis(50)); continue };
            if w.open.fetch_add(1, SeqCst) >= MAX_CONNS {
                w.open.fetch_sub(1, SeqCst);
                let _ = c.set_write_timeout(Some(Duration::from_secs(1)));
                w.reply(&mut c, Family::None, &refusal("root.busy", format!("more than {MAX_CONNS} egress connections are open")), Vec::new());
                continue;
            }
            let x = Open(w.clone()); // uncounted when the thread ends, or with the closure if the spawn fails
            let _ = std::thread::Builder::new().spawn(move || { let x = x; x.0.clone().conn(c) });
        }
    });
    let t = e.clone();
    std::thread::spawn(move || {
        for i in 1u64.. {
            std::thread::sleep(Duration::from_secs(60));
            t.publish();
            if i % 60 == 0 {
                let _ = money::checkpoint(&t.ledger.dir, now_ms());
            }
        }
    });
    e.publish();
    util::log(&rh, "info", "egress.sock up");
    Ok(e)
}

impl Egress {
    /// §4.3 `panic`: refuse new requests (503 `root.panicked`) and close every kernel connection; in-flight
    /// relays stop at their next upstream frame and settle at their reservation.
    pub fn panic(&self) {
        self.panicked.store(true, SeqCst);
        lock(&self.conns).values().for_each(|c| drop(c.shutdown(std::net::Shutdown::Both)));
    }

    pub fn resume(&self) {
        self.panicked.store(false, SeqCst);
    }

    /// Class and model gating (§4.3): safe mode means the `providers` class with the L0 models only.
    pub fn set_level(&self, level: i64, safe: bool) {
        self.level.store(level, SeqCst);
        self.safe.store(safe, SeqCst);
    }

    /// Keys, classes and money limits from a newly applied charter; in-flight requests keep the old ones.
    /// A lowered ceiling applies at the next admission and stops nothing (D6).
    pub fn reload(&self, ch: &Charter) {
        let (mut cfg, alerts) = Cfg::load(&self.rh, ch);
        if let Some(root) = self.root.upgrade() { cfg.benchmark = Ok(root.benchmark.clone()); }
        {
            let mut st = lock(&self.st);
            st.0 = cfg;
            st.1.lim = Limits::of(ch);
        }
        alerts.iter().for_each(|(c, m)| self.alert(c, m));
        self.publish();
    }

    /// `keys.reload` on the owner socket (after `init-root keys set`): the key states, never a key.
    pub fn reload_keys(&self) -> Value {
        let ch = self.root.upgrade().and_then(|r| lock(&r.st).charter.as_ref().map(|l| l.ch.clone()));
        if let Some(ch) = ch {
            self.reload(&ch);
        }
        json!({"keys": lock(&self.st).0.key_state, "reloaded": true})
    }

    /// §4.2: the money block, the model table and class headroom (ICJ integers, micro-USD).
    pub fn ceilings(&self) -> Value {
        let now = now_ms();
        let (level, safe) = (self.level.load(SeqCst), self.safe.load(SeqCst));
        let eff = if safe { 0 } else { level };
        let mut g = lock(&self.st);
        let (cfg, m) = &mut *g;
        m.roll(now);
        let mut models = Vec::new();
        for c in cfg.classes.values().filter(|c| c.model) {
            for (p, (id, md)) in c.providers.keys().flat_map(|p| c.models.iter().map(move |x| (p, x))) {
                models.push(json!({"input_musd_per_mtok": md.input, "min_level": md.min_level, "model": id, "output_musd_per_mtok": md.output,
                    "provider": p, "routable": i64::from(md.min_level) <= eff && i64::from(c.min_level) <= level && c.enabled}));
            }
        }
        let classes: Vec<Value> = cfg.classes.values().map(|c| {
            let tokens = m.rate_tokens(c, now);
            let v = m.vol.get(&c.id).cloned().unwrap_or_default();
            json!({"bytes_limit": c.bytes_per_day, "bytes_today": v.bytes, "enabled": c.enabled && (!safe || c.id == "providers"),
                "id": c.id, "kind": if c.model { "model" } else { "http" }, "min_level": c.min_level, "rate_limit": c.rate_per_min,
                "rate_tokens": tokens, "requests_limit": c.requests_per_day, "requests_today": v.requests})
        }).collect();
        json!({"benchmark": cfg.benchmark.as_ref().ok().and_then(|p| p.as_ref()).map(|p| p.view()), "classes": classes, "epoch": m.epoch, "level": level, "models": models, "v": 1,
            "money": {"enforced": false, "policy": "observe", "token_scope": "settled_live_metered_all_time",
                "reported_input_tokens": m.tokens.input, "reported_output_tokens": m.tokens.output,
                "token_totals_complete": m.tokens.complete,
                "total_input_tokens": m.tokens.complete.then_some(m.tokens.input), "total_output_tokens": m.tokens.complete.then_some(m.tokens.output), "total_reserved_musd": m.total_reserved, "day_limit_musd": m.lim.day, "day_reserved_musd": m.day_reserved, "day_resets_at": rfc3339(m.resets_at()),
                "day_spent_musd": m.day_spent, "day_starts_at": rfc3339(m.day_start), "sealed_reserve_musd": m.lim.reserve,
                "total_limit_musd": m.lim.total, "total_spent_musd": m.total_spent}})
    }

    /// `INIT_HOME/root/ceilings.json` (root/root.md §4.5): every 60 s and on every change, with the broker's counts.
    pub fn publish(&self) {
        let Some(root) = self.root.upgrade() else { return };
        let dir = root.p.i("root");
        if !dir.is_dir() {
            return;
        }
        let mut v = self.ceilings();
        (v["instances"], v["parallel_bodies"]) = crate::zone::counts(&root);
        if let Ok(s) = icj(&v) {
            let _ = util::write_atomic(&dir.join("ceilings.json"), format!("{s}\n").as_bytes(), 0o644);
        }
    }

    /// An alert with `source: "egress"`; the same alert is not repeated within 10 minutes.
    fn alert(&self, code: &str, msg: &str) {
        let now = now_ms();
        {
            let mut a = lock(&self.alerted);
            let k = format!("{code}\n{msg}");
            if a.get(&k).is_some_and(|t| now - t < ALERT_REPEAT_MS) {
                return;
            }
            a.insert(k, now);
        }
        if let Some(r) = self.root.upgrade() {
            r.alert_from("egress", code, msg);
        }
    }

    fn conn(self: Arc<Self>, s: UnixStream) {
        if !sys::peer(s.as_raw_fd()).is_ok_and(|(uid, _)| uid == sys::euid()) {
            return;
        }
        let id = self.seq.fetch_add(1, SeqCst);
        let s = Arc::new(s);
        lock(&self.conns).insert(id, s.clone());
        let _ = s.set_write_timeout(Some(CONN_IDLE));
        let mut rd = BufReader::new(Timed { s: s.clone(), idle: CONN_IDLE, read: REQ_READ, until: None });
        let mut w = &*s;
        loop {
            rd.get_mut().until = None;
            match http::read_request(&mut rd) {
                Ok(Some((h, body))) => {
                    if !self.handle(&h, &body, &mut w) || h.close {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) if rd.get_ref().until.is_none() => break, // idle past CONN_IDLE: closed without a reply
                Err(e) => {
                    let code = if e.kind() == io::ErrorKind::OutOfMemory { "request.too_large" } else { "request.invalid" };
                    self.reply(&mut w, Family::None, &refusal(code, e.to_string()), Vec::new());
                    break;
                }
            }
        }
        lock(&self.conns).remove(&id);
        let _ = w.shutdown(std::net::Shutdown::Both);
    }

    /// One request; false closes the connection.
    fn handle(&self, h: &Head, body: &[u8], w: &mut &UnixStream) -> bool {
        let path = h.target.split('?').next().unwrap_or("");
        let panicked = self.panicked.load(SeqCst);
        // F7: the local answers count against the in-flight bound too
        if self.inflight.fetch_add(1, SeqCst) >= MAX_INFLIGHT {
            self.inflight.fetch_sub(1, SeqCst);
            return self.reply(w, Family::None, &refusal("root.busy", format!("more than {MAX_INFLIGHT} requests in flight")), Vec::new());
        }
        let _slot = Inflight(&self.inflight);
        match (h.method.as_str(), path) {
            ("GET", "/_root/ceilings") => return self.json(w, &self.ceilings()),
            ("GET", "/_root/health") => {
                let test = lock(&self.st).0.test_mode;
                let v = json!({"level": self.level.load(SeqCst), "ok": !panicked, "panicked": panicked, "test_mode": test, "v": 1});
                return self.json(w, &v);
            }
            (_, "/_root/action") => return self.reply(w, Family::None, &refusal("root.unsupported", "helper verbs (git-push, email) are not in this Root release"), Vec::new()),
            _ => {}
        }
        if panicked {
            return self.reply(w, Family::None, &refusal("root.panicked", "the Root is panicked; init-root resume restarts it"), Vec::new());
        }
        let now = now_ms();
        let admitted = {
            let mut g = lock(&self.st);
            let (cfg, m) = &mut *g;
            m.roll(now);
            self.check(cfg, m, h, body, now).map_err(|(f, mut r)| {
                let eff = r.class.as_ref().and_then(|c| cfg.classes.get(c)).is_some_and(Class::effectful);
                classify(&mut r, eff, m);
                (f, r)
            })
        };
        match admitted {
            Ok(a) => self.forward(a, h, body, w, now),
            Err((family, r)) => self.refused(w, family, r, now),
        }
    }

    /// The matching order of §3.5: route → class → destination → method → path → level (class, then
    /// model) → provider tools → availability → day volume → rate → money.
    fn check(&self, cfg: &Cfg, m: &mut Money, h: &Head, body: &[u8], now: i64) -> Result<Admit, (Family, Refusal)> {
        let (level, safe) = (self.level.load(SeqCst), self.safe.load(SeqCst));
        let r = classes::route(&cfg.classes, &h.method, &h.target).map_err(|r| (Family::None, r))?;
        let (c, family) = (r.class, r.family);
        let no = |code, msg: String| (family, Refusal { code, msg, class: Some(c.id.clone()), ..Refusal::default() });
        if let Some(p) = cfg.benchmark.as_ref().map_err(|e| no("root.benchmark", e.clone()))? {
            p.request(&h.method, r.provider.map(|p| p.0), &r.dest, &r.path, body).map_err(|e| no("root.benchmark", e))?;
        }
        if i64::from(c.min_level) > level {
            return Err(no("class.level", format!("class {} needs L{}; the system is at L{level}", c.id, c.min_level)));
        }
        let metered = r.metered; // F2: `route` refused every model-class route without a metering rule
        let (mut model, mut price, mut reserved) = (None, None, 0);
        if metered {
            let ins = classes::inspect(family, c, r.provider, body).map_err(|r| (family, r))?;
            let md = c.models.get(&ins.model).ok_or_else(|| no("class.model", format!("model {:?} is not listed in class {}", ins.model, c.id)))?;
            if i64::from(md.min_level) > if safe { 0 } else { level } {
                return Err(no("class.level", format!("model {} needs L{}; the system is at L{level}{}", ins.model, md.min_level, if safe { " (safe mode)" } else { "" })));
            }
            reserved = money::reservation(body.len() as u64, ins.max_out, md).0; // review F1, F4
            (model, price) = (Some(ins.model), Some(md.clone()));
        }
        if !c.enabled || (safe && c.id != "providers") {
            return Err(no("class.disabled", format!("class {} is disabled{}", c.id, if safe { " in safe mode" } else { "" })));
        }
        if !classes::endpoint_allowed(cfg.test_mode, &r.dest, r.provider.map(|p| p.1)) {
            return Err(no("class.disabled", format!("{}: test mode requires loopback; production loopback requires explicit local provider", r.dest)));
        }
        let auth = match r.provider {
            Some((_, p)) if p.no_auth && p.local && classes::loopback(&p.host) => None,
            Some((pid, p)) => {
                let k = cfg.keys.get(pid).ok_or_else(|| no("class.disabled", format!("no usable key for provider {pid} (init-root keys check)")))?;
                let anthropic = r.path.starts_with(&format!("{}/anthropic/", p.base_path));
                Some((if anthropic { Auth::XApiKey } else { p.auth }, k.clone()))
            }
            None => None,
        };
        m.vol_check(c, body.len() as u64, now).map_err(|(code, limit, wait)| {
            let what = if code == "class.rate" { "per-minute rate" } else { "daily volume" };
            (family, Refusal { code, msg: format!("class {} {what} limit {limit} reached; retry in {wait} s", c.id), class: Some(c.id.clone()),
                limit: Some(limit), retry_after: Some(wait), ..Refusal::default() })
        })?;
        if metered { let _ = m.admit(reserved); } // accounting only: no automatic dollar stop
        let scheme = if classes::loopback(&r.dest) { "http" } else { "https" };
        let host = match r.dest.rsplit_once(':') {
            Some((h, "443")) if scheme == "https" => h,
            _ => r.dest.as_str(),
        };
        Ok(Admit { id: format!("{}-{}", self.boot, self.seq.fetch_add(1, SeqCst)), class: c.id.clone(), effectful: c.effectful(),
            provider: r.provider.map(|p| p.0.to_string()), model, family, url: format!("{scheme}://{host}{}", r.path), dest: r.dest.clone(),
            auth, price, reserved, day: m.day_start, epoch: m.epoch, response_max: c.response_max, date: money::date(now) })
    }

    /// Records a refusal (ledger, and lineage for hits) and answers it.
    fn refused(&self, w: &mut &UnixStream, family: Family, r: Refusal, now: i64) -> bool {
        if r.code.starts_with("class.") || r.code.starts_with("ceiling.") {
            let v = json!({"at": rfc3339(now), "class": r.class, "code": r.code, "demotes": r.demotes, "lane": "live", "t": "refuse", "v": 1});
            let _ = self.ledger.write(&money::date(now), &v, false);
        }
        if r.record {
            let ceiling = match r.code {
                "ceiling.day" => "money.day".to_string(),
                "ceiling.total" => "money.total".to_string(),
                _ => format!("actions.{}", r.class.as_deref().unwrap_or("")),
            };
            let data = json!({"attempted": r.attempted, "ceiling": ceiling, "class": r.class, "code": r.code, "demotes": r.demotes,
                "level": self.level.load(SeqCst), "limit": r.limit, "rule": r.rule, "source": "egress"});
            if let Some(root) = self.root.upgrade() {
                root.append("ceiling_hit", data);
            }
        }
        self.reply(w, family, &r, Vec::new())
    }

    /// §3.6: HTTP 403/429/5xx, `x-init-egress-error`, and a body shaped by the route's wire family with
    /// a top-level `init` object.
    fn reply(&self, w: &mut impl Write, family: Family, r: &Refusal, mut hs: Vec<(String, String)>) -> bool {
        let mut init = json!({"class": r.class, "code": r.code, "demotes": r.demotes, "level": self.level.load(SeqCst),
            "retryable": r.retry_after.is_some(), "source": "root"});
        match &r.money {
            Some(Value::Object(m)) => init.as_object_mut().into_iter().for_each(|o| o.extend(m.clone())),
            _ => init["limit"] = json!(r.limit),
        }
        let kind = if r.code.starts_with("ceiling.") { "init_ceiling_exceeded" } else if r.code.starts_with("class.") { "init_class_refused" } else { "init_egress_error" };
        let body = match family {
            Family::Messages => json!({"error": {"message": r.msg, "type": kind}, "init": init, "type": "error"}),
            _ => json!({"error": {"code": r.code, "message": r.msg, "type": kind}, "init": init}),
        };
        hs.extend([("content-type".to_string(), "application/json".to_string()), ("x-init-egress-error".into(), r.code.into())]);
        if let Some(s) = r.retry_after {
            hs.push(("retry-after".into(), s.to_string()));
        }
        let text = icj(&body).unwrap_or_default();
        http::full(w, r.status.unwrap_or_else(|| status(r.code)), hs, text.as_bytes()).is_ok()
    }

    fn json(&self, w: &mut impl Write, v: &Value) -> bool {
        let hs = vec![("content-type".to_string(), "application/json".to_string())];
        http::full(w, 200, hs, icj(v).unwrap_or_default().as_bytes()).is_ok()
    }

    /// §3.4: a synced `start` record, the upstream request with connect-phase retries, the relay with the
    /// usage tap, settlement and the `end` record. False closes the kernel connection.
    fn forward(&self, a: Admit, h: &Head, body: &[u8], w: &mut &UnixStream, now: i64) -> bool {
        let label = |k: &str| h.get(k).map(|v| v.chars().filter(char::is_ascii_graphic).take(64).collect::<String>());
        let start = json!({"at": rfc3339(now), "body": label("x-egress-body"), "class": a.class, "id": a.id, "kres": label("x-egress-reservation"),
            "lane": "live", "model": a.model, "provider": a.provider, "req_bytes": body.len(), "reserved_musd": a.reserved,
            "scope": label("x-egress-scope"), "t": "start", "v": 1});
        if let Err(e) = self.ledger.write(&a.date, &start, true) {
            {
                let mut g = lock(&self.st);
                let (cfg, m) = &mut *g;
                if a.price.is_some() {
                    m.settle(a.reserved, 0, a.day, now);
                }
                if let Some(c) = cfg.classes.get(&a.class) {
                    m.vol_undo(c, body.len() as u64);
                }
            }
            self.alert("egress_ledger", &format!("[egress] the usage ledger cannot be written ({e}); every request is refused until it can"));
            return self.reply(w, a.family, &refusal("root.ledger", "the Root cannot record usage; refusing (fail closed)"), Vec::new());
        }
        if let Some(root) = self.root.upgrade() {
            crate::sentinel::egress_start(&root, &a.id, body.len() as u64, subtask(h)); // X3: the turn tap
        }
        let mut retries = 0u32;
        let res = loop {
            match self.send(&a, h, body) {
                Err(e) if connect_phase(&e) && retries < 2 => {
                    retries += 1;
                    std::thread::sleep(Duration::from_millis(250 << retries));
                }
                r => break r,
            }
        };
        let mut hs: Vec<(String, String)> = vec![("x-egress-req-id".into(), a.id.clone()), ("x-egress-retries".into(), retries.to_string()),
            ("x-egress-reserved-musd".into(), a.reserved.to_string()), ("x-egress-epoch".into(), a.epoch.to_string())];
        let resp = match res {
            Err(e) => {
                // a connect-phase failure sent nothing; anything later may have been billed
                let cost = if connect_phase(&e) { 0 } else { a.reserved };
                self.finish(&a, h, body.len(), cost, 0, 0, (None, None), vec!["upstream_error"]);
                let msg = format!("upstream {}: {}", a.dest, e.to_string().chars().take(200).collect::<String>());
                return self.reply(w, a.family, &refusal("upstream.transport", msg), hs);
            }
            Ok(r) => r,
        };
        let status = resp.status().as_u16();
        if self.panicked.load(SeqCst) {
            self.finish(&a, h, body.len(), a.reserved, 0, status, (None, None), vec!["panic"]);
            return false;
        }
        if (300..400).contains(&status) || status == 401 || status == 403 {
            self.finish(&a, h, body.len(), 0, 0, status, (None, None), vec![]);
            if status >= 400 {
                let p = a.provider.as_deref().unwrap_or(&a.class);
                self.alert("upstream_auth", &format!("[egress] {p} rejected the Root's key (HTTP {status}); init-root keys set {p}"));
                let r = Refusal { status: Some(status), ..refusal("upstream.auth", format!("the provider rejected the Root's credentials (HTTP {status}); body withheld")) };
                return self.reply(w, a.family, &r, hs);
            }
            return self.reply(w, a.family, &refusal("upstream.redirect", format!("the upstream answered {status}; the Root never follows redirects")), hs);
        }
        for (k, v) in resp.headers() {
            let k = k.as_str();
            if (RESP_HEADERS.contains(&k) || k.starts_with("anthropic-ratelimit-") || k.starts_with("x-ratelimit-")) && let Ok(v) = v.to_str() {
                hs.push((k.into(), v.into()));
            }
        }
        let sse = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|c| c.contains("event-stream"));
        let head_only = h.method == "HEAD";
        if !head_only {
            hs.push(("transfer-encoding".into(), "chunked".into()));
        }
        let mut tap = Tap::new(a.family, sse).with_calls();
        let (mut bytes, mut flags) = (0u64, Vec::new());
        let mut ok = http::head(w, status, &hs).is_ok();
        let mut rd = resp.into_body().into_reader();
        let mut buf = vec![0u8; 16 * 1024];
        while ok && !head_only {
            if self.panicked.load(SeqCst) {
                (ok, flags) = (false, vec!["panic"]);
                break;
            }
            match rd.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    bytes += n as u64;
                    if bytes > a.response_max {
                        (ok, flags) = (false, vec!["response_max"]);
                        break;
                    }
                    if http::chunk(w, &buf[..n]).is_err() {
                        (ok, flags) = (false, vec!["client_gone"]);
                    }
                    tap.feed(&buf[..n]); // after the relay: parsing never delays the forwarded bytes (ADR-011 §5.2)
                }
                Err(_) => (ok, flags) = (false, vec!["upstream_error"]),
            }
        }
        drop(rd); // mid-stream, this closes the upstream connection instead of pooling it
        if ok && !head_only {
            ok = w.write_all(b"0\r\n\r\n").is_ok();
        }
        let (usage, finish, calls) = tap.finish_calls();
        let cost = match (usage, &a.price) {
            (Some(u), Some(md)) => u.cost(md),
            (None, Some(_)) if (200..300).contains(&status) => {
                flags.push("usage_estimated"); // RT-C7: never zero for a stream without usage
                a.reserved
            }
            _ => 0, // unmetered, or the provider refused the request
        };
        self.finish_with(&a, h, body.len(), cost, bytes, status, (usage, finish), flags, calls);
        ok
    }

    fn send(&self, a: &Admit, h: &Head, body: &[u8]) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
        let mut b = ureq::http::Request::builder().method(h.method.as_str()).uri(a.url.as_str());
        for (k, v) in h.headers.iter().filter(|(k, _)| KERNEL_HEADERS.contains(&k.as_str())) {
            b = b.header(k.as_str(), v.as_str());
        }
        b = b.header("accept-encoding", "identity");
        if let Some((auth, key)) = &a.auth {
            let (name, mut val) = match auth {
                Auth::Bearer => ("authorization", [&b"Bearer "[..], &key.0].concat()),
                Auth::XApiKey => ("x-api-key", key.0.clone()),
            };
            let hv = ureq::http::HeaderValue::from_bytes(&val);
            sys::zero(&mut val);
            let mut hv = hv.map_err(|e| ureq::Error::Http(e.into()))?;
            hv.set_sensitive(true);
            b = b.header(name, hv);
        }
        if body.is_empty() {
            self.agent.run(b.body(()).map_err(ureq::Error::Http)?)
        } else {
            self.agent.run(b.body(body).map_err(ureq::Error::Http)?)
        }
    }

    /// Settles the reservation, books the response bytes, writes the `end` record and, for a class with
    /// external effects, the `egress_action` lineage record (§3.6).
    #[allow(clippy::too_many_arguments)]
    fn finish(&self, a: &Admit, h: &Head, req: usize, cost: u64, bytes: u64, status: u16, tap: (Option<tap::Usage>, Option<&str>), flags: Vec<&str>) {
        self.finish_with(a, h, req, cost, bytes, status, tap, flags, BTreeMap::new());
    }

    /// `finish` with the caps the tap read from the response (ADR-011 §5.2, X5).
    #[allow(clippy::too_many_arguments)]
    fn finish_with(&self, a: &Admit, h: &Head, req: usize, cost: u64, bytes: u64, status: u16, tap: (Option<tap::Usage>, Option<&str>), flags: Vec<&str>,
        calls: BTreeMap<String, u32>) {
        // X5: the request's caps reach the sentinel before its end (X3 attributes them to the request's turn)
        if let Some(root) = self.root.upgrade() {
            crate::extend::request(&root, &a.id, calls, h.get("x-egress-caps"));
        }
        let now = now_ms();
        {
            let mut g = lock(&self.st);
            if a.price.is_some() {
                g.1.tokens.observe(tap.0);
                g.1.settle(a.reserved, cost, a.day, now);
            }
            if a.day == g.1.day_start && let Some(v) = g.1.vol.get_mut(&a.class) {
                v.bytes += bytes;
            }
        }
        let end = json!({"at": rfc3339(now), "bytes": bytes, "cost_musd": cost, "finish": tap.1, "flags": flags, "id": a.id, "status": status,
            "t": "end", "usage": tap.0.map(|u| u.json()), "v": 1});
        if let Err(e) = self.ledger.write(&a.date, &end, false) {
            util::log(&self.rh, "error", &format!("egress ledger end {}: {e}", a.id));
        }
        if let Some(root) = self.root.upgrade() {
            if let Some(u) = tap.0 {
                crate::sentinel::egress_usage(&root, &a.id, u.input, u.cache_read); // ADR-013: the cache-read card
            }
            crate::sentinel::egress_end(&root, &a.id, cost, status, tap.1); // X3: cost, H5a, H9
            crate::goal::egress_spent(&root, cost); // ADR-013: the active goal's meter, the same cost as the `end` record
        }
        if a.effectful && let Some(root) = self.root.upgrade() {
            // ADR-011 §5.3 (X5): the caps used in this effect's turn so far, and the groups open when it ran
            let groups: Vec<u64> = crate::sentinel::open_groups(&root).iter().filter_map(|g| g["seq"].as_u64()).collect();
            root.append("egress_action", json!({"bytes": req as u64 + bytes, "caps": crate::sentinel::turn_caps(&root), "class": a.class,
                "destination": a.dest, "open_groups": groups, "outcome": status, "verb": h.method}));
        }
    }
}

/// DNS, TCP or TLS failures before any request byte was written: the only failures the Root retries (§3.4).
fn connect_phase(e: &ureq::Error) -> bool {
    use std::io::ErrorKind as K;
    match e {
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Tls(_) | ureq::Error::Rustls(_) => true,
        ureq::Error::Timeout(t) => matches!(t, ureq::Timeout::Resolve | ureq::Timeout::Connect),
        ureq::Error::Io(io) => matches!(io.kind(), K::ConnectionRefused | K::AddrNotAvailable | K::NetworkUnreachable | K::HostUnreachable),
        _ => false,
    }
}

/// F3: a kernel connection's reads. A request's first byte may take up to `idle`; its head and body must
/// then arrive within `read`, so a held-open or trickling connection cannot pin a Root thread for long.
/// `until` is the deadline of the request being read, `None` until its first byte.
struct Timed { s: Arc<UnixStream>, idle: Duration, read: Duration, until: Option<Instant> }

impl Read for Timed {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let late = || io::Error::new(io::ErrorKind::TimedOut, "the request was not received in time");
        let left = self.until.map_or(Some(self.idle), |t| t.checked_duration_since(Instant::now())).filter(|d| !d.is_zero()).ok_or_else(late)?;
        let _ = self.s.set_read_timeout(Some(left)); // EINVAL on macOS once the peer is gone: the read cannot block then
        let n = (&*self.s).read(b).map_err(|e| if e.kind() == io::ErrorKind::WouldBlock { late() } else { e })?;
        self.until.get_or_insert_with(|| Instant::now() + self.read);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare(classes: BTreeMap<String, Class>) -> Egress {
        let keys = BTreeMap::from([("deepseek".to_string(), Arc::new(Secret(b"test-k".to_vec())))]);
        Egress { root: Weak::new(), rh: PathBuf::new(), ledger: Ledger { dir: PathBuf::new() },
            st: Mutex::new((Cfg { benchmark: Ok(None), classes, keys, key_state: BTreeMap::new(), test_mode: false },
                Money::new(Limits { day: 1_000_000, total: 1_000_000, reserve: 0, offset_ms: 0 }, 0))),
            level: AtomicI64::new(0), safe: AtomicBool::new(false), panicked: AtomicBool::new(false), inflight: AtomicUsize::new(0),
            open: AtomicUsize::new(0), seq: AtomicU64::new(0), boot: "b".into(), conns: Mutex::new(BTreeMap::new()), agent: ureq::Agent::new_with_defaults(),
            alerted: Mutex::new(BTreeMap::new()) }
    }

    /// MERGE: only a well-formed `x-egress-subtask` marks a sub-task request, and no zone header is ever relayed upstream.
    #[test]
    fn subtask_header_is_validated_and_never_relayed() {
        let head = |v: Option<&str>| Head { method: "POST".into(), target: "/".into(), close: false,
            headers: v.map(|v| vec![("x-egress-subtask".to_string(), v.to_string())]).unwrap_or_default() };
        assert!(subtask(&head(Some("delegate-call_7"))));
        let long = "a".repeat(65);
        for bad in [None, Some(""), Some("-x"), Some("a b"), Some("a/b"), Some(long.as_str())] {
            assert!(!subtask(&head(bad)), "{bad:?}");
        }
        assert!(subtask(&head(Some(&long[..64]))));
        assert!(KERNEL_HEADERS.iter().all(|k| !k.starts_with("x-")), "zone headers stay inside the Root");
    }

    #[test]
    fn local_no_auth_admission_still_requires_model_and_output_bound() {
        let user: toml::Value = toml::from_str("[providers]\ndestinations=[\"127.0.0.1:11434\"]\npaths=[\"chat/completions\"]\n[providers.providers.localmodel]\nhost=\"127.0.0.1:11434\"\napi=\"chat\"\nlocal=true\nauth=\"none\"\nbase_path=\"/v1\"\n").unwrap();
        let mut findings = Vec::new();
        let actions = effective_actions(Some(&user), false, &mut findings);
        assert!(findings.is_empty(), "{findings:?}");
        let mut ch = Charter { schema_version: 1, purpose: Default::default(), constitution: Default::default(), autonomy: Default::default(),
            money: Default::default(), quotas: Default::default(), actions: actions.clone(), sandbox: Default::default(), panic: Default::default(),
            notify: Default::default(), mirror: Default::default(), ladder: Default::default(), sealed: Default::default(), root: Default::default(), extend: Default::default() };
        let (loaded, alerts) = Cfg::load(Path::new("/init-no-key-fixture"), &ch);
        assert_eq!(loaded.key_state.get("localmodel"), Some(&"not_required"));
        assert!(loaded.keys.is_empty() && alerts.is_empty());
        ch.root.test_mode = true;
        let (loaded_test, _) = Cfg::load(Path::new("/init-no-key-fixture"), &ch);
        assert_eq!(loaded_test.key_state.get("localmodel"), Some(&"not_required"));
        let e = bare(classes::compile(&actions));
        let h = Head { method: "POST".into(), target: "/localmodel/chat/completions".into(), headers: vec![], close: true };
        let mut g = lock(&e.st);
        let (cfg, m) = &mut *g;
        let admitted = e.check(cfg, m, &h, br#"{"model":"deepseek-flash","max_tokens":5}"#, 0).unwrap_or_else(|(_, e)| panic!("{}", e.msg));
        assert!(admitted.auth.is_none());
        assert!(admitted.price.is_some());
        assert_eq!(admitted.url, "http://127.0.0.1:11434/v1/chat/completions");
        assert!(e.check(cfg, m, &h, br#"{"model":"deepseek-flash"}"#, 0).is_err());
        assert!(e.check(cfg, m, &h, br#"{"model":"not-configured","max_tokens":5}"#, 0).is_err());
    }

    #[test]
    fn levels_and_safe_mode_gate_classes_and_models() {
        let extra: toml::Value = toml::from_str::<toml::Table>("[providers.models.\"deepseek-flash\"]\nmin_level = 0\ninput_musd_per_mtok = 1\n\
            output_musd_per_mtok = 1\n[providers.models.\"deepseek-v4-pro\"]\nmin_level = 1\ninput_musd_per_mtok = 1\noutput_musd_per_mtok = 1\n").unwrap().into();
        let mut f = Vec::new();
        let e = bare(classes::compile(&effective_actions(Some(&extra), false, &mut f)));
        assert!(f.is_empty(), "{f:?}");
        let code = |method: &str, target: &str, model: &str| {
            let h = Head { method: method.into(), target: target.into(), headers: Vec::new(), close: true };
            let body = if model.is_empty() { Vec::new() } else { format!("{{\"model\":\"{model}\",\"max_tokens\":1}}").into_bytes() };
            let mut g = lock(&e.st);
            let (cfg, m) = &mut *g;
            e.check(cfg, m, &h, &body, 0).err().map(|x| x.1.code)
        };
        let (chat, crates) = ("/deepseek/chat/completions", "/x/crates/index.crates.io/se/rd/serde");
        assert_eq!((code("POST", chat, "deepseek-flash"), code("POST", chat, "deepseek-v4-pro"), code("GET", crates, "")), (None, Some("class.level"), None));
        e.set_level(1, false);
        assert_eq!(code("POST", chat, "deepseek-v4-pro"), None);
        // R3-07: safe mode is the providers class with the L0 models only
        e.set_level(1, true);
        assert_eq!((code("POST", chat, "deepseek-flash"), code("POST", chat, "deepseek-v4-pro"), code("GET", crates, "")),
            (None, Some("class.level"), Some("class.disabled")));
        assert_eq!(e.ceilings()["classes"][0]["enabled"], false);
    }

    /// F2: a model-class request is forwarded only on a route with a metering rule; billed routes reserve and
    /// are gated, free ones (S1's balance read) cost nothing, and a listed billable endpoint the Root cannot
    /// meter is refused before any key is attached.
    #[test]
    fn every_forwarded_model_route_is_metered_or_free() {
        let extra: toml::Value = toml::from_str::<toml::Table>("[providers]\npaths = [\"chat/completions\", \"embeddings\", \"models\", \"user/balance\"]\n").unwrap().into();
        let mut f = Vec::new();
        let e = bare(classes::compile(&effective_actions(Some(&extra), false, &mut f)));
        assert!(f.len() == 1 && f[0].warn, "C11 warns about embeddings: {f:?}");
        let check = |method: &str, target: &str, body: &[u8]| {
            let h = Head { method: method.into(), target: target.into(), headers: Vec::new(), close: true };
            let mut g = lock(&e.st);
            let (cfg, m) = &mut *g;
            e.check(cfg, m, &h, body, 0).map(|a| (a.reserved > 0, a.price.is_some(), a.family)).map_err(|(_, mut r)| {
                classify(&mut r, false, m);
                (r.code, r.demotes, r.record)
            })
        };
        let chat = br#"{"model":"deepseek-flash","max_tokens":10}"#;
        assert_eq!(check("POST", "/deepseek/chat/completions", chat), Ok((true, true, Family::Chat)));
        for body in [br#"{"model":"deepseek-flash"}"#.as_slice(), br#"{"model":"deepseek-flash","max_completion_tokens":10}"#.as_slice(),
            br#"{"model":"deepseek-flash","max_tokens":10,"n":2}"#.as_slice()] {
            assert_eq!(check("POST", "/deepseek/chat/completions", body).err().map(|x| x.0), Some("request.output_limit"));
        }
        assert_eq!(check("POST", "/deepseek/chat/completions", br#"{"model":"gpt-x","max_tokens":10}"#).err().map(|x| x.0), Some("class.model"));
        assert_eq!(check("GET", "/deepseek/user/balance", b""), Ok((false, false, Family::None)), "S1's balance read: forwarded at $0");
        assert_eq!(check("GET", "/deepseek/models", b""), Ok((false, false, Family::None)));
        for (m, p) in [("POST", "/deepseek/embeddings"), ("GET", "/deepseek/chat/completions"), ("POST", "/deepseek/user/balance")] {
            assert_eq!(check(m, p, chat), Err(("class.unmetered", false, false)), "{m} {p}: refused, not a hit");
        }
        assert_eq!(status("class.unmetered"), 403);
    }

    /// F7: the local answers count against the in-flight bound.
    #[test]
    fn local_answers_are_behind_the_inflight_bound() {
        let e = bare(BTreeMap::new());
        let (a, mut b) = UnixStream::pair().unwrap();
        let health = Head { method: "GET".into(), target: "/_root/health".into(), headers: Vec::new(), close: true };
        e.inflight.store(MAX_INFLIGHT, SeqCst);
        assert!(e.handle(&health, b"", &mut &a));
        e.inflight.store(0, SeqCst);
        assert!(e.handle(&health, b"", &mut &a) && e.inflight.load(SeqCst) == 0);
        drop(a);
        let mut s = String::new();
        b.read_to_string(&mut s).unwrap();
        let second = s.find("HTTP/1.1 200 ").unwrap();
        assert!(s.starts_with("HTTP/1.1 503 ") && s[..second].contains("x-init-egress-error: root.busy"), "{s}");
    }

    /// F3: an idle connection is closed quietly; a request trickling past its read deadline is refused.
    #[test]
    fn connection_reads_are_bounded_in_time() {
        let read = |send: &[u8], idle: u64, read: u64| {
            let (a, mut b) = UnixStream::pair().unwrap();
            b.write_all(send).unwrap();
            let t = Timed { s: Arc::new(a), idle: Duration::from_millis(idle), read: Duration::from_millis(read), until: None };
            let (mut rd, at) = (BufReader::new(t), Instant::now());
            let r = http::read_request(&mut rd).map(|o| o.map(|(h, _)| h.target)).map_err(|e| e.kind());
            (r, rd.get_ref().until.is_some(), at.elapsed() < Duration::from_secs(3))
        };
        assert_eq!(read(b"", 150, 60_000), (Err(io::ErrorKind::TimedOut), false, true), "idle");
        assert_eq!(read(b"POST /deepseek/chat/completions HTTP/1.1\r\nhost: x\r\n", 60_000, 200), (Err(io::ErrorKind::TimedOut), true, true), "a partial head");
        assert_eq!(read(b"GET /_root/health HTTP/1.1\r\n\r\n", 60_000, 60_000), (Ok(Some("/_root/health".into())), true, true));
    }

    #[test]
    fn d6_classification() {
        let mut m = Money::new(Limits { day: 1, total: 1, reserve: 0, offset_ms: 0 }, 0);
        let mut c = |code, class: Option<&str>, eff, demotes| {
            let mut r = Refusal { code, class: class.map(String::from), demotes, ..Refusal::default() };
            classify(&mut r, eff, &mut m);
            (r.demotes, r.rule, r.record)
        };
        assert_eq!(c("class.path", Some("providers"), false, false), (true, "a", true));
        assert_eq!(c("class.unknown", None, false, false), (true, "a", true));
        assert_eq!(c("class.level", Some("providers"), false, false), (true, "b", true));
        assert_eq!(c("class.model", Some("providers"), false, false), (true, "b", true));
        assert_eq!(c("ceiling.day", Some("providers"), false, true), (false, "none", false));
        assert_eq!(c("ceiling.day", Some("providers"), false, false), (false, "none", false));
        assert_eq!(c("ceiling.day", Some("providers"), false, false), (false, "none", false), "one ordinary hit per ceiling per day");
        assert_eq!(c("class.rate", Some("crates"), false, false), (false, "none", true));
        assert_eq!(c("class.rate", Some("post-api"), true, false), (true, "a", true));
        assert_eq!(c("class.disabled", Some("providers"), false, false), (false, "", false));
        assert_eq!((status("ceiling.total"), status("class.path"), status("class.disabled"), status("root.ledger")), (429, 403, 503, 503));
    }

    #[test]
    fn refusal_bodies_follow_the_wire_family() {
        let e = bare(BTreeMap::new());
        let r = Refusal { code: "ceiling.day", class: Some("providers".into()), retry_after: Some(38_521),
            money: Some(json!({"epoch": 7, "limit_musd": 5_000_000, "reserved_musd": 0, "resets_at": "2026-10-20T17:00:00.000Z", "spent_musd": 5_000_412})),
            ..refusal("ceiling.day", "root daily ceiling reached") };
        let mut out = Vec::new();
        assert!(e.reply(&mut out, Family::Chat, &r, Vec::new()));
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("HTTP/1.1 429 ") && s.contains("x-init-egress-error: ceiling.day\r\n") && s.contains("retry-after: 38521\r\n"), "{s}");
        let v: Value = serde_json::from_str(s.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(v["error"]["type"], "init_ceiling_exceeded");
        assert_eq!(v["init"], json!({"class": "providers", "code": "ceiling.day", "demotes": false, "epoch": 7, "level": 0, "limit_musd": 5_000_000,
            "reserved_musd": 0, "resets_at": "2026-10-20T17:00:00.000Z", "retryable": true, "source": "root", "spent_musd": 5_000_412}));
        let mut out = Vec::new();
        e.reply(&mut out, Family::Messages, &refusal("class.path", "no"), Vec::new());
        let s = String::from_utf8(out).unwrap();
        let v: Value = serde_json::from_str(s.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!((v["type"].as_str(), v["error"]["type"].as_str(), v["init"]["retryable"].as_bool()), (Some("error"), Some("init_class_refused"), Some(false)));
        assert!(s.starts_with("HTTP/1.1 403 "));
    }
}
