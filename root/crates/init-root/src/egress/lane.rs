//! Sealed lanes (§3.8, decision D5; WP24): one Unix socket inside a sealed run's root, on the kernel
//! proxy's wire (`POST /<provider>/<path>`, `GET /_init/stats|health`), forwarding only the `providers`
//! class at the audited level with the Root's key. Nothing is recorded but usage: one `t:"sealed"` line
//! per request in `egress/usage/` (run id, provider, model, usage, cost), which money.rs's rebuild leaves
//! out of the zone's money (the sealed reserve pays it; a twin lane's lines, tagged `pool:"twin"`, are the twin
//! pool's, money.rs). Dollar fields are advisory; request/time/token and security rules remain.

use super::classes::{self, Auth};
use super::tap::Tap;
use super::{Cfg, KERNEL_HEADERS, http, money};
use crate::charter::Charter;
use crate::util::{icj, now_ms, rfc3339};
use serde_json::{Value, json};
use std::io::{self, BufReader, Read};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const BODY_MAX: u64 = 32 << 20;
/// Review F3 on sealed lanes (as egress.rs `MAX_CONNS`): connections served at once on one lane socket. A sealed
/// run is one body (or one judge running its bodies one at a time) with a small keep-alive pool; one more is
/// answered 503 `root.busy` by the accept thread, which spawns nothing for it. Reads and writes are bounded in
/// time as on `egress.sock` (`CONN_IDLE`, `REQ_READ`).
const LANE_CONNS: usize = 32;

#[derive(Default)]
struct St {
    budget: u64,
    spent: AtomicU64,
    held: AtomicU64,
    requests: AtomicU64,
    tokens: (AtomicU64, AtomicU64),
    /// Cache-read input tokens (inside `tokens.0`), for the cost probe's miss/hit split.
    cached: AtomicU64,
    /// M7 (m7-design.md §3.1.3): per-run caps on actuals (0 = none), the turns admitted, and the first cap hit.
    caps: Mutex<Caps>,
    turns: AtomicU64,
    cap_hit: Mutex<Option<&'static str>>,
    inflight: AtomicU64,
    refused: AtomicU64,
    /// Request ids within the run: `<run>-<n>` (the `start` record and its `t:"sealed"` line).
    seq: AtomicU64,
    capped: AtomicU64,
    /// (finish of the last completed request, when it ended or the lane opened)
    last: Mutex<(Option<&'static str>, i64)>,
    closed: AtomicBool,
    /// Open connections (F3), counted by `Open`.
    open: AtomicU64,
}

/// One counted lane connection, uncounted on drop (also when its thread could not be spawned).
struct Open(Arc<St>);
impl Drop for Open {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, SeqCst);
    }
}

pub struct Lane {
    pub sock: PathBuf,
    st: Arc<St>,
}

/// m7-design.md §3.1.3, enforced on actuals: requests whose `x-init-purpose` is `turn` (or absent), and the
/// output and input tokens the run's responses reported (cached input included). 0 = no cap. Only the money
/// cap uses reservations, so a token cap never refuses a run's first request.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Caps {
    pub turns: u64,
    pub tokens_out: u64,
    pub tokens_in: u64,
}

/// The pool a sealed lane is charged to (egress.md §3.3): the sealed reserve, or the twin's (X9 deviation 3).
enum Pool {
    /// `sealed.reserve_usd`: Σ sealed spend since install stays within it (WP24 D6).
    Reserve(u64),
    /// The twin pool (money.rs `twin_admit`): the night's `extend.twin.usd_day`, and the zone's lifetime room
    /// (total − reserve) shared with the zone.
    Twin { night: String, day: u64, room: u64 },
}

struct Ctx {
    cfg: Cfg,
    level: i64,
    pool: Pool,
    run: String,
    usage: PathBuf,
    agent: ureq::Agent,
}

/// `egress::sealed_lane(run_id, socket, budget, level)`: binds `sock` (0600) and serves it until `close`.
pub fn sealed_lane(rh: &Path, ch: &Charter, run: &str, sock: &Path, budget_musd: u64, level: i64) -> io::Result<Lane> {
    serve(rh, ch, run, sock, budget_musd, level, Pool::Reserve(money::Limits::of(ch).reserve))
}

/// A sealed lane of the twin's nightly replay (X9): as `sealed_lane`, but charged to the twin pool of `night` (its
/// `extend.twin.usd_day`, carved from the zone's lifetime room), never to `sealed.reserve_usd`. Its ledger lines
/// carry `pool:"twin"` and `night`.
pub fn twin_lane(rh: &Path, ch: &Charter, run: &str, sock: &Path, budget_musd: u64, level: i64, night: &str) -> io::Result<Lane> {
    let day = crate::charter::musd(&ch.extend.twin.usd_day).unwrap_or(0);
    serve(rh, ch, run, sock, budget_musd, level, Pool::Twin { night: night.to_string(), day, room: money::Limits::of(ch).zone_total() })
}

fn serve(rh: &Path, ch: &Charter, run: &str, sock: &Path, budget_musd: u64, level: i64, pool: Pool) -> io::Result<Lane> {
    let (cfg, _) = Cfg::load(rh, ch);
    let agent: ureq::Agent = ureq::Agent::config_builder().proxy(None).http_status_as_error(false).max_redirects(0).max_redirects_will_error(false)
        .user_agent(format!("init-root/{}", crate::install::VERSION)).timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(600))).build().into();
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(rh.join("egress/usage"))?;
    let cx = Arc::new(Ctx { cfg, level, pool, run: run.to_string(), usage: rh.join("egress/usage"), agent });
    let l = UnixListener::bind(sock)?;
    std::fs::set_permissions(sock, std::fs::Permissions::from_mode(0o600))?;
    let st = Arc::new(St { budget: budget_musd, last: Mutex::new((None, now_ms())), ..St::default() });
    let s2 = st.clone();
    std::thread::spawn(move || {
        for c in l.incoming() {
            if s2.closed.load(SeqCst) {
                break;
            }
            let Ok(c) = c else {
                std::thread::sleep(Duration::from_millis(50)); // EMFILE and the like: no hot loop
                continue;
            };
            if s2.open.fetch_add(1, SeqCst) >= LANE_CONNS as u64 {
                s2.open.fetch_sub(1, SeqCst);
                let _ = c.set_write_timeout(Some(Duration::from_secs(1)));
                let _ = refuse(&s2, &mut &c, "root.busy", format!("more than {LANE_CONNS} connections on this sealed lane"));
                continue;
            }
            let (cx, o) = (cx.clone(), Open(s2.clone()));
            let _ = std::thread::Builder::new().spawn(move || {
                let o = o;
                conn(&cx, &o.0, c)
            });
        }
    });
    Ok(Lane { sock: sock.to_path_buf(), st })
}

impl Lane {
    /// `{spent_musd, requests, inflight, refused, capped, last_finish, idle_ms, input_tokens, output_tokens}`, plus
    /// M7's `turns`, `caps_hit` (the first cap reached, or null) and `cached_tokens`.
    pub fn status(&self) -> Value {
        status(&self.st)
    }
    pub fn set_caps(&self, c: Caps) {
        *self.st.caps.lock().unwrap_or_else(|e| e.into_inner()) = c;
    }
    /// Stops serving and removes the socket by its exact path.
    pub fn close(self) -> Value {
        self.st.closed.store(true, SeqCst);
        let _ = UnixStream::connect(&self.sock); // wakes the accept loop
        let _ = std::fs::remove_file(&self.sock);
        status(&self.st)
    }
}

fn status(st: &St) -> Value {
    let (finish, at) = *st.last.lock().unwrap_or_else(|e| e.into_inner());
    let hit = *st.cap_hit.lock().unwrap_or_else(|e| e.into_inner());
    json!({"money_enforced": false, "money_policy": "observe", "configured_budget_musd": st.budget, "reserved_musd": st.held.load(SeqCst), "cached_tokens": st.cached.load(SeqCst), "capped": st.capped.load(SeqCst), "caps_hit": hit, "idle_ms": now_ms() - at,
        "inflight": st.inflight.load(SeqCst), "input_tokens": st.tokens.0.load(SeqCst), "last_finish": finish,
        "output_tokens": st.tokens.1.load(SeqCst), "refused": st.refused.load(SeqCst), "requests": st.requests.load(SeqCst),
        "spent_musd": st.spent.load(SeqCst), "turns": st.turns.load(SeqCst)})
}

fn conn(cx: &Ctx, st: &St, s: UnixStream) {
    let s = Arc::new(s);
    let _ = s.set_write_timeout(Some(super::CONN_IDLE));
    let mut rd = BufReader::new(super::Timed { s: s.clone(), idle: super::CONN_IDLE, read: super::REQ_READ, until: None });
    let mut w = &*s;
    loop {
        rd.get_mut().until = None;
        let Ok(Some((h, body))) = http::read_request(&mut rd) else { break };
        let path = h.target.split('?').next().unwrap_or("");
        let r = match (h.method.as_str(), path) {
            ("GET", "/_init/stats") => send_json(&mut w, 200, &status(st)),
            ("GET", "/_init/health") => send_json(&mut w, 200, &json!({"lane": "sealed", "ok": true, "v": 1})),
            _ => forward(cx, st, &h, &body, &mut w),
        };
        if r.is_err() || h.close {
            break;
        }
    }
}

fn send_json(w: &mut &UnixStream, code: u16, v: &Value) -> io::Result<()> {
    http::full(w, code, vec![("content-type".into(), "application/json".into())], icj(v)?.as_bytes())
}

fn refuse(st: &St, w: &mut &UnixStream, code: &str, msg: String) -> io::Result<()> {
    st.refused.fetch_add(1, SeqCst);
    let body = json!({"error": {"code": code, "message": msg, "type": "init_egress_error"}, "init": {"code": code, "lane": "sealed", "source": "root"}});
    let hs = vec![("content-type".into(), "application/json".into()), ("x-init-egress-error".into(), code.into())];
    let status = if code.starts_with("ceiling.") { 429 } else if code == "root.busy" { 503 } else { 403 };
    http::full(w, status, hs, icj(&body)?.as_bytes())
}

/// §3.8: the `providers` class only, the audited level's models, the lane budget; then one upstream round
/// trip, buffered (a sealed run has no live reader), with the usage tap over the whole body.
fn forward(cx: &Ctx, st: &St, h: &http::Head, body: &[u8], w: &mut &UnixStream) -> io::Result<()> {
    let r = match classes::route(&cx.cfg.classes, &h.method, &h.target) {
        Ok(r) if r.class.id == "providers" && r.class.enabled => r,
        Ok(r) => return refuse(st, w, "class.disabled", format!("class {} is not available on a sealed lane", r.class.id)),
        Err(e) => return refuse(st, w, e.code, e.msg),
    };
    match &cx.cfg.benchmark {
        Err(e) => return refuse(st, w, "root.benchmark", e.clone()),
        Ok(Some(p)) => if let Err(e) = p.request(&h.method, r.provider.map(|p| p.0), &r.dest, &r.path, body) {
            return refuse(st, w, "root.benchmark", e);
        },
        Ok(None) => {}
    }
    let ins = match classes::inspect(r.family, r.class, r.provider, body) {
        Ok(i) => i,
        Err(e) => return refuse(st, w, e.code, e.msg),
    };
    let Some(md) = r.class.models.get(&ins.model).filter(|m| i64::from(m.min_level) <= cx.level) else {
        return refuse(st, w, "class.model", format!("model {} is not routable at L{}", ins.model, cx.level));
    };
    let Some((pid, p)) = r.provider else {
        return refuse(st, w, "class.disabled", "no model provider".into());
    };
    let key = cx.cfg.keys.get(pid);
    if key.is_none() && !(p.no_auth && p.local && classes::loopback(&p.host)) {
        return refuse(st, w, "class.disabled", "no usable provider key".into());
    }
    if !classes::endpoint_allowed(cx.cfg.test_mode, &r.dest, Some(p)) {
        return refuse(st, w, "class.disabled", "test mode requires loopback; production loopback requires explicit local provider".into());
    }
    let turn = h.get("x-init-purpose").is_none_or(|p| p == "turn");
    let c = *st.caps.lock().unwrap_or_else(|e| e.into_inner());
    let over = |cap: u64, used: u64| cap > 0 && used >= cap;
    let hit = [("turns", turn && over(c.turns, st.turns.load(SeqCst))), ("tokens_out", over(c.tokens_out, st.tokens.1.load(SeqCst))),
        ("tokens_in", over(c.tokens_in, st.tokens.0.load(SeqCst)))].into_iter().find(|x| x.1).map(|x| x.0);
    if let Some(cap) = hit {
        st.capped.fetch_add(1, SeqCst);
        st.cap_hit.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(cap);
        st.refused.fetch_add(1, SeqCst);
        let body = json!({"error": {"code": "cap", "message": format!("the run's {cap} cap is reached"), "type": "init_cap_exceeded"}});
        let hs = vec![("content-type".into(), "application/json".into()), ("x-init-cap".into(), cap.into())];
        return http::full(w, 429, hs, icj(&body)?.as_bytes());
    }
    if turn {
        st.turns.fetch_add(1, SeqCst);
    }
    let reserved = money::reservation(body.len() as u64, ins.max_out, md).0; // review F1, F4
    // Reservations are durable estimates, not money gates. Keep pools separated for attribution.
    let _ = st.held.fetch_update(SeqCst, SeqCst, |h| Some(h.saturating_add(reserved)));
    match &cx.pool {
        Pool::Reserve(reserve) => { let _ = money::sealed_admit(&cx.usage, *reserve, reserved); }
        Pool::Twin { night, day, room } => { let _ = money::twin_admit(&cx.usage, night, *day, *room, reserved); }
    }
    let id = format!("{}-{}", cx.run, st.seq.fetch_add(1, SeqCst));
    let mut start = json!({"at": rfc3339(now_ms()), "id": id, "lane": "sealed", "model": ins.model, "provider": pid, "reserved_musd": reserved,
        "run": cx.run, "t": "start", "v": 1});
    tag(cx, &mut start);
    if let Err(e) = (money::Ledger { dir: cx.usage.clone() }).write(&money::date(now_ms()), &start, true) {
        let _ = st.held.fetch_update(SeqCst, SeqCst, |h| Some(h.saturating_sub(reserved)));
        settle(cx, reserved, 0);
        return refuse(st, w, "root.ledger", format!("the Root cannot record usage ({e}); refusing (fail closed)"));
    }
    st.inflight.fetch_add(1, SeqCst);
    let scheme = if classes::loopback(&r.dest) { "http" } else { "https" };
    let host = r.dest.strip_suffix(":443").filter(|_| scheme == "https").unwrap_or(&r.dest);
    let mut b = ureq::http::Request::builder().method(h.method.as_str()).uri(format!("{scheme}://{host}{}", r.path));
    for (k, v) in h.headers.iter().filter(|(k, _)| KERNEL_HEADERS.contains(&k.as_str())) {
        b = b.header(k.as_str(), v.as_str());
    }
    let res = (|| -> Result<_, ureq::Error> {
        if let Some(key) = key.filter(|_| !p.no_auth) {
            let anthropic = r.path.starts_with(&format!("{}/anthropic/", p.base_path));
            let (name, mut val) = match if anthropic { Auth::XApiKey } else { p.auth } {
                Auth::Bearer => ("authorization", [&b"Bearer "[..], &key.0].concat()),
                Auth::XApiKey => ("x-api-key", key.0.clone()),
            };
            let hv = ureq::http::HeaderValue::from_bytes(&val);
            crate::sys::zero(&mut val);
            let mut hv = hv.map_err(|e| ureq::Error::Http(e.into()))?;
            hv.set_sensitive(true);
            b = b.header(name, hv);
        }
        let req = b.header("accept-encoding", "identity").body(body).map_err(ureq::Error::Http)?;
        cx.agent.run(req)
    })();
    let (code, ctype, data) = match res {
        Ok(resp) => {
            let ct = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("application/json").to_string();
            let code = resp.status().as_u16();
            let mut v = Vec::new();
            let ok = resp.into_body().into_reader().take(BODY_MAX).read_to_end(&mut v).is_ok();
            (if ok && !(300..400).contains(&code) { code } else { 502 }, ct, v)
        }
        Err(e) => (502, "application/json".into(), format!("{{\"error\":{{\"code\":\"upstream.transport\",\"message\":{:?}}}}}", e.to_string()).into_bytes()),
    };
    let data = if matches!(code, 401 | 403) { br#"{"error":{"code":"upstream.auth","message":"body withheld"}}"#.to_vec() } else { data };
    let mut t = Tap::new(r.family, ctype.contains("event-stream"));
    t.feed(&data);
    let (usage, finish) = t.finish();
    let cost = match usage {
        Some(u) => u.cost(md),
        None if (200..300).contains(&code) => reserved,
        None => 0,
    };
    let _ = st.held.fetch_update(SeqCst, SeqCst, |h| Some(h.saturating_sub(reserved)));
    let _ = st.spent.fetch_update(SeqCst, SeqCst, |s| Some(s.saturating_add(cost)));
    settle(cx, reserved, cost);
    st.requests.fetch_add(1, SeqCst);
    let u = usage.unwrap_or_default();
    st.tokens.0.fetch_add(u.input, SeqCst);
    st.tokens.1.fetch_add(u.output, SeqCst);
    st.cached.fetch_add(u.cache_read.min(u.input), SeqCst);
    *st.last.lock().unwrap_or_else(|e| e.into_inner()) = (finish.or(Some("other")), now_ms());
    st.inflight.fetch_sub(1, SeqCst);
    let now = now_ms();
    let mut line = json!({"at": rfc3339(now), "cost_musd": cost, "id": id, "lane": "sealed", "model": ins.model, "provider": pid, "run": cx.run,
        "status": code, "t": "sealed", "usage": usage.map(|u| u.json()), "v": 1});
    tag(cx, &mut line);
    let _ = (money::Ledger { dir: cx.usage.clone() }).write(&money::date(now), &line, false);
    http::full(w, code, vec![("content-type".into(), ctype)], &data)
}

/// A twin lane's ledger lines carry its pool and night (money.rs reads them to seed the pool and leave them out of
/// the reserve).
fn tag(cx: &Ctx, v: &mut Value) {
    if let Pool::Twin { night, .. } = &cx.pool {
        v["pool"] = json!(money::TWIN_POOL);
        v["night"] = json!(night);
    }
}

/// Releases a hold in the lane's pool and books its cost.
fn settle(cx: &Ctx, r: u64, cost: u64) {
    match &cx.pool {
        Pool::Reserve(_) => money::sealed_settle(&cx.usage, r, cost),
        Pool::Twin { night, .. } => money::twin_settle(&cx.usage, night, r, cost),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// Review F3 on sealed lanes: `LANE_CONNS` connections are served at once; one more gets 503 `root.busy` and is
    /// closed by the accept thread; once they close, the lane serves again.
    #[test]
    fn lane_connections_are_capped() {
        // CARGO_TARGET_DIR can point elsewhere, leaving no source-tree target.
        let fixture_parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp22");
        std::fs::create_dir_all(&fixture_parent).unwrap();
        let base = std::fs::canonicalize(fixture_parent).unwrap();
        let rh = base.join(format!("ul{}", std::process::id() % 100_000)); // short: the socket path fits sun_path
        let _ = std::fs::remove_dir_all(&rh); // exact path, created by this test
        std::fs::create_dir_all(&rh).unwrap();
        let ch = Charter { schema_version: 1, purpose: Default::default(), constitution: Default::default(), autonomy: Default::default(),
            money: Default::default(), quotas: Default::default(), actions: Default::default(), sandbox: Default::default(), panic: Default::default(),
            notify: Default::default(), mirror: Default::default(), ladder: Default::default(), sealed: Default::default(), root: Default::default(), extend: Default::default() };
        let sock = rh.join("l.sock");
        let lane = sealed_lane(&rh, &ch, "t", &sock, 0, 0).unwrap();
        let until = |n: u64| {
            let t = std::time::Instant::now();
            while lane.st.open.load(SeqCst) != n && t.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(lane.st.open.load(SeqCst), n);
        };
        let held: Vec<UnixStream> = (0..LANE_CONNS).map(|_| UnixStream::connect(&sock).unwrap()).collect();
        until(LANE_CONNS as u64);
        let mut extra = UnixStream::connect(&sock).unwrap();
        let mut text = String::new();
        extra.read_to_string(&mut text).unwrap();
        assert!(text.starts_with("HTTP/1.1 503") && text.contains("root.busy"), "{text}");
        assert_eq!(lane.st.open.load(SeqCst), LANE_CONNS as u64, "the refused one was never counted");
        drop(held);
        until(0);
        let mut c = UnixStream::connect(&sock).unwrap();
        c.write_all(b"GET /_init/health HTTP/1.1\r\nhost: l\r\nconnection: close\r\n\r\n").unwrap();
        let mut text = String::new();
        c.read_to_string(&mut text).unwrap();
        assert!(text.starts_with("HTTP/1.1 200") && text.contains("\"lane\":\"sealed\""), "{text}");
        lane.close();
        std::fs::remove_dir_all(&rh).unwrap();
    }
}
