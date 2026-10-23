//! WP22 integration tests: the egress gateway (root/egress.md) end to end. A test-mode Root forwards
//! to loopback upstreams only: `init-mockllm` (WP2, the binary from the kernel workspace) and a small
//! capturing upstream defined here. Keys are fake (`test-…`); no real key is read and no real API is
//! called. Homes live under `root/target/test-homes/wp22/`, with `HOME` pointed there; daemons are
//! killed by their own handles. Run with `cargo test --offline -p init-root --features test-fixtures`.
#![cfg(feature = "test-fixtures")]

use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_init-root");
const KEY: &str = "test-key-wp22-c8c8c8c8";

struct Home {
    dir: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
}

/// A fresh home `root/target/test-homes/wp22/<name>` (names ≤ 8 bytes: ROOT_HOME stays ≤ 80 bytes).
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/wp22");
    let dir = base.join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(&dir).unwrap();
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
}

fn charter(extra: &str) -> String {
    format!("schema_version = 1\n[purpose]\ntext = \"WP22 test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n\
             [sealed]\nreserve_usd = \"0.00\"\naudit_usd = \"0.00\"\n[root]\ntest_mode = true\n{extra}\n")
}

/// The `providers` class pointed at loopback upstreams, with `deepseek-flash` (L0) and `deepseek-v4-pro` (L1).
fn providers(ports: &[(&str, u16)], money: (&str, &str)) -> String {
    let dests: Vec<String> = ports.iter().map(|(_, p)| format!("\"127.0.0.1:{p}\"")).collect();
    let mut s = format!("[money]\nday_usd = \"{}\"\ntotal_usd = \"{}\"\n[actions.providers]\ndestinations = [{}]\n\
        paths = [\"chat/completions\", \"anthropic/v1/messages\", \"models\", \"user/balance\"]\n\
        [actions.providers.models.\"deepseek-flash\"]\nmin_level = 0\ninput_musd_per_mtok = 300000\noutput_musd_per_mtok = 1200000\n\
        [actions.providers.models.\"deepseek-v4-pro\"]\nmin_level = 1\ninput_musd_per_mtok = 1320000\noutput_musd_per_mtok = 3960000\n",
        money.0, money.1, dests.join(", "));
    for (id, p) in ports {
        s.push_str(&format!("[actions.providers.providers.{id}]\nhost = \"127.0.0.1:{p}\"\n"));
    }
    s
}

impl Home {
    fn cmd(&self) -> Command {
        let mut c = Command::new(BIN);
        c.env_clear().env("HOME", &self.dir).env("INIT_ROOT_HOME", &self.rh).env("INIT_HOME", &self.ih).env("PATH", "/usr/bin:/bin");
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).stdin(Stdio::null()).output().unwrap()
    }
    fn install(&self, extra: &str) {
        let f = self.dir.join("charter.toml");
        fs::write(&f, charter(extra)).unwrap();
        let o = self.run(&["install", "--charter", f.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    }
    fn apply(&self, extra: &str) {
        fs::write(self.rh.join("charter.toml"), charter(extra)).unwrap();
        let o = self.run(&["charter", "check", "--apply", "--yes"]);
        assert_eq!(o.status.code(), Some(0), "apply: {}", String::from_utf8_lossy(&o.stderr));
    }
    fn key(&self, id: &str, val: &str, mode: u32) {
        let p = self.rh.join("keys/providers").join(id);
        fs::write(&p, val).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
    }
    fn records(&self) -> Vec<Value> {
        let s = fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default();
        s.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
    fn hits(&self) -> Vec<Value> {
        self.records().into_iter().filter(|r| r["kind"] == "ceiling_hit").map(|r| r["data"].clone()).collect()
    }
    fn ledger(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for e in fs::read_dir(self.rh.join("egress/usage")).unwrap().flatten() {
            out.extend(fs::read_to_string(e.path()).unwrap().lines().map(|l| serde_json::from_str::<Value>(l).unwrap()));
        }
        out
    }
    fn ends(&self) -> Vec<Value> {
        self.ledger().into_iter().filter(|v| v["t"] == "end").collect()
    }
    fn alerts(&self) -> String {
        fs::read_to_string(self.rh.join("alerts.jsonl")).unwrap_or_default()
    }
    fn daemon(&self, env: &[(&str, &str)]) -> Daemon {
        let mut c = self.cmd();
        c.arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        for (k, v) in env {
            c.env(k, v);
        }
        let d = Daemon(c.spawn().unwrap());
        let (sock, owner) = (self.sock(), self.rh.join("run/owner.sock"));
        wait(10, || UnixStream::connect(&sock).is_ok() && UnixStream::connect(&owner).is_ok());
        d
    }
    fn sock(&self) -> PathBuf {
        self.rh.join("run/egress.sock")
    }
    /// One request on the egress socket, as the kernel proxy sends it.
    fn req(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Resp {
        request(&self.sock(), method, path, headers, body)
    }
    fn chat(&self, model: &str, max: u64, headers: &[(&str, &str)]) -> Resp {
        self.req("POST", "/deepseek/chat/completions", headers, &chat(model, max))
    }
    fn ceilings(&self) -> Value {
        self.req("GET", "/_root/ceilings", &[], b"").json()
    }
    /// A request frame on the owner socket (what `init-root keys set` sends after storing a key).
    fn owner(&self, cmd: &str) -> Value {
        let mut s = UnixStream::connect(self.rh.join("run/owner.sock")).unwrap();
        writeln!(s, "{}", json!({"args": {}, "cmd": cmd, "id": "t", "v": 1})).unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
}

/// A daemon this test spawned; killed by its own handle.
struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait(secs: u64, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "condition not met within {secs} s");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn chat(model: &str, max: u64) -> Vec<u8> {
    json!({"max_tokens": max, "messages": [{"content": "hi", "role": "user"}], "model": model, "stream": true}).to_string().into_bytes()
}

/// The Root's reservation at deepseek-flash peak prices (egress.md §3.4), computed by hand.
/// The reservation the Root holds: input at one token per request byte (review F4), output at `max` tokens.
fn reservation(body_len: usize, max: u64) -> u64 {
    (body_len as u64 * 300_000 + max * 1_200_000).div_ceil(1_000_000)
}

#[derive(Debug)]
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    complete: bool,
    raw: Vec<u8>,
}

impl Resp {
    fn h(&self, k: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&self.raw)))
    }
    fn code(&self) -> &str {
        self.h("x-init-egress-error").unwrap_or("")
    }
}

/// A blocking HTTP/1.1 client over the Unix socket (`connection: close`; chunked bodies decoded).
fn request(sock: &Path, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Resp {
    let mut s = UnixStream::connect(sock).unwrap();
    let mut h = format!("{method} {path} HTTP/1.1\r\nhost: egress\r\nconnection: close\r\ncontent-length: {}\r\n", body.len());
    for (k, v) in headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    s.write_all(format!("{h}\r\n").as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    parse_response(raw)
}

fn parse_response(raw: Vec<u8>) -> Resp {
    let at = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or_else(|| panic!("no head: {}", String::from_utf8_lossy(&raw)));
    let head = String::from_utf8_lossy(&raw[..at]).to_string();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let rest = &raw[at + 4..];
    let chunked = headers.iter().any(|(k, v)| k == "transfer-encoding" && v == "chunked");
    let (body, complete) = if chunked {
        let (mut body, mut i) = (Vec::new(), 0);
        loop {
            let Some(e) = rest[i..].windows(2).position(|w| w == b"\r\n") else { break (body, false) };
            let n = usize::from_str_radix(std::str::from_utf8(&rest[i..i + e]).unwrap(), 16).unwrap();
            if n == 0 {
                break (body, true);
            }
            let start = i + e + 2;
            if start + n > rest.len() {
                body.extend_from_slice(&rest[start..]);
                break (body, false);
            }
            body.extend_from_slice(&rest[start..start + n]);
            i = start + n + 2;
            if i > rest.len() {
                break (body, false);
            }
        }
    } else {
        (rest.to_vec(), true)
    };
    Resp { status, headers, body, complete, raw }
}

const SSE: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\",\"role\":\"assistant\"},\"index\":0}],\"id\":\"u1\"}\n\n\
    data: {\"choices\":[{\"delta\":{\"content\":\"\"},\"finish_reason\":\"stop\",\"index\":0}],\"id\":\"u1\",\
    \"usage\": {\"completion_tokens\":100,\"prompt_cache_hit_tokens\":40,\"prompt_cache_miss_tokens\":60,\"prompt_tokens\":100}}\n\n\
    data: [DONE]\n\n";
/// The cost of one `SSE` reply at deepseek-flash peak prices: 100 × 0.3 + 100 × 1.2 µUSD.
const SSE_COST: u64 = 150;

fn http_reply(status: u16, ctype: &str, body: &str, extra: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status} X\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n{extra}\r\n{body}", body.len()).into_bytes()
}

fn sse_ok() -> Vec<u8> {
    http_reply(200, "text/event-stream", SSE, "x-ds-trace-id: t1\r\nset-cookie: no\r\n")
}

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// A loopback upstream that records every request and answers from a queue (default: `sse_ok`).
struct Upstream {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
    delay_ms: Arc<Mutex<u64>>,
}

impl Upstream {
    fn start() -> Upstream {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let (seen, queue, delay_ms) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(VecDeque::new())), Arc::new(Mutex::new(0)));
        let (s, q, d) = (seen.clone(), queue.clone(), delay_ms.clone());
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                let (s, q, d) = (s.clone(), q.clone(), d.clone());
                std::thread::spawn(move || serve(c, &s, &q, &d));
            }
        });
        Upstream { port, seen, queue, delay_ms }
    }
    fn push(&self, r: Vec<u8>) {
        self.queue.lock().unwrap().push_back(r);
    }
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn serve(c: TcpStream, seen: &Mutex<Vec<Seen>>, q: &Mutex<VecDeque<Vec<u8>>>, delay: &Mutex<u64>) {
    let mut w = c.try_clone().unwrap();
    let mut r = BufReader::new(c);
    let mut first = String::new();
    if r.read_line(&mut first).unwrap_or(0) == 0 {
        return;
    }
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        if l == "\r\n" || l.is_empty() {
            break;
        }
        let (k, v) = l.split_once(':').unwrap();
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    let n: usize = headers.iter().find(|(k, _)| k == "content-length").map_or(0, |(_, v)| v.parse().unwrap());
    let mut body = vec![0; n];
    r.read_exact(&mut body).unwrap();
    let mut p = first.split(' ');
    seen.lock().unwrap().push(Seen { method: p.next().unwrap().into(), path: p.next().unwrap().into(), headers, body });
    let reply = q.lock().unwrap().pop_front().unwrap_or_else(sse_ok);
    std::thread::sleep(Duration::from_millis(*delay.lock().unwrap()));
    let _ = w.write_all(&reply);
}

/// A port nothing listens on.
fn dead_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Every file under `dir` (recursively), except paths under `skip`, with its bytes.
fn files(dir: &Path, skip: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        let ft = e.file_type().unwrap();
        if p.starts_with(skip) || ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            files(&p, skip, out);
        } else if ft.is_file() {
            out.push((p.clone(), fs::read(&p).unwrap_or_default()));
        }
    }
}

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[test]
fn key_injection_header_policy_and_metering() {
    let up = Upstream::start();
    let h = home("inject");
    h.install(&providers(&[("deepseek", up.port)], ("1.00", "5.00")));
    h.key("deepseek", &format!("{KEY}\n"), 0o600);
    let _d = h.daemon(&[]);
    // EG-01: the zone's auth header is dropped, the Root injects its key; bytes come back unchanged
    let body = chat("deepseek-flash", 1000);
    let r = h.req("POST", "/deepseek/chat/completions", &[("content-type", "application/json"), ("authorization", "Bearer zone-forged"),
        ("x-api-key", "zone-forged"), ("x-init-session", "s1"), ("cookie", "c=1"), ("x-egress-scope", "live"), ("x-egress-guard", "epoch=0")], &body);
    assert_eq!((r.status, r.complete), (200, true), "{}", String::from_utf8_lossy(&r.raw));
    assert_eq!(r.body, SSE.as_bytes(), "the stream is relayed byte for byte");
    let reserved = reservation(body.len(), 1000);
    assert_eq!(r.h("x-egress-reserved-musd"), Some(reserved.to_string().as_str()));
    assert_eq!((r.h("x-egress-retries"), r.h("x-ds-trace-id"), r.h("set-cookie")), (Some("0"), Some("t1"), None));
    let s = &up.seen()[0];
    assert_eq!((s.method.as_str(), s.path.as_str(), s.body.as_slice()), ("POST", "/chat/completions", body.as_slice()));
    let hv = |k: &str| s.headers.iter().filter(|(n, _)| n == k).map(|(_, v)| v.as_str()).collect::<Vec<_>>();
    assert_eq!(hv("authorization"), vec![format!("Bearer {KEY}").as_str()]);
    assert!(hv("x-api-key").is_empty() && hv("cookie").is_empty() && hv("x-egress-scope").is_empty());
    assert!(s.headers.iter().all(|(k, _)| !k.starts_with("x-init-") && !k.starts_with("x-egress-")), "{:?}", s.headers);
    assert_eq!((hv("accept-encoding"), hv("content-type")), (vec!["identity"], vec!["application/json"]));
    assert!(hv("user-agent")[0].starts_with("init-root/"));
    // DeepSeek's Anthropic-format route authenticates with x-api-key
    let mbody = br#"{"max_tokens":10,"messages":[],"model":"deepseek-flash"}"#;
    let m = h.req("POST", "/deepseek/anthropic/v1/messages", &[("anthropic-version", "2023-06-01")], mbody);
    assert_eq!(m.status, 200);
    let s = &up.seen()[1];
    assert_eq!(s.path, "/anthropic/v1/messages");
    assert!(s.headers.contains(&("x-api-key".into(), KEY.into())) && !s.headers.iter().any(|(k, _)| k == "authorization"));
    assert!(s.headers.contains(&("anthropic-version".into(), "2023-06-01".into())));
    // EG-07: the Root's cost is the hand computation at peak prices; an unmetered GET costs nothing
    let g = h.req("GET", "/deepseek/models", &[], b"");
    assert_eq!((g.status, up.seen()[2].path.as_str(), up.seen()[2].body.len()), (200, "/models", 0));
    let c = h.ceilings();
    // the Anthropic-format route got a chat-shaped stream: no usage the Root can read, so it is charged its reservation
    assert_eq!(c["money"]["day_spent_musd"].as_u64(), Some(SSE_COST + reservation(mbody.len(), 10)));
    let ends = h.ends();
    assert_eq!(ends.len(), 3);
    assert_eq!((ends[0]["cost_musd"].as_u64(), ends[0]["usage"].clone(), ends[0]["finish"].as_str()), (Some(SSE_COST), json!({"cache_read": 40, "input": 100, "output": 100}), Some("stop")));
    assert_eq!(c["epoch"], 2, "two metered settlements");
    assert_eq!(c["money"]["day_reserved_musd"], 0);
    assert_eq!(c["classes"][1]["requests_today"], 3);
    // RT-C8: the key reaches only the upstream hop: no response, ledger, log, lineage or INIT_HOME file holds it
    for resp in [&r, &m, &g] {
        assert!(!contains(&resp.raw, KEY));
    }
    let mut all = Vec::new();
    files(&h.dir, &h.rh.join("keys"), &mut all);
    assert!(all.len() > 10);
    for (p, b) in &all {
        assert!(!contains(b, KEY), "the key leaked into {}", p.display());
    }
    assert!(h.hits().is_empty());
}

#[test]
fn slice1_through_the_mock_model() {
    let Some(mock) = mock_bin() else {
        eprintln!("SKIP slice1_through_the_mock_model: init-mockllm is not built (set INIT_MOCKLLM_BIN)");
        return;
    };
    let h = home("slice1");
    let script = h.dir.join("s.mock.jsonl");
    let turn = r#"{"v":1,"kind":"turn","reply":{"text":"Hello from the mock.","usage":{"input":1000,"cache_read":200,"output":50}}}"#;
    fs::write(&script, format!("{turn}\n{turn}\n")).unwrap();
    let port_file = h.dir.join("mock.port");
    let rec = h.dir.join("mock");
    let m = Daemon(Command::new(&mock).args(["--mode", "upstream", "--listen", "127.0.0.1:0", "--strict", "deepseek", "--port-file"])
        .arg(&port_file).arg("--script").arg(&script).arg("--record-dir").arg(&rec).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    wait(10, || fs::read_to_string(&port_file).is_ok_and(|s| s.trim().parse::<u16>().is_ok()));
    let port: u16 = fs::read_to_string(&port_file).unwrap().trim().parse().unwrap();
    // this home prices deepseek-flash's cache hits (build-deviations §3.13); the shared helper leaves the price out
    let priced = providers(&[("deepseek", port)], ("1.00", "5.00"));
    h.install(&priced.replacen("output_musd_per_mtok = 1200000\n", "output_musd_per_mtok = 1200000\ncache_read_musd_per_mtok = 6000\n", 1));
    h.key("deepseek", KEY, 0o600);
    let _d = h.daemon(&[]);
    // RT-E1: kernel → Root egress → mock. The mock runs --strict deepseek, so an x-init-* header or a
    // malformed request reaching it would be a 400.
    let body = chat("deepseek-flash", 2000);
    let r = h.req("POST", "/deepseek/chat/completions", &[("content-type", "application/json"), ("x-init-session", "s1"), ("x-init-purpose", "turn")], &body);
    assert_eq!((r.status, r.complete), (200, true), "{}", String::from_utf8_lossy(&r.raw));
    let text = String::from_utf8_lossy(&r.body).to_string();
    assert!(text.contains("Hello") && text.trim_end().ends_with("data: [DONE]"), "{text}");
    // the same request straight to the mock gives the same bytes: the Root changed nothing
    let mut direct = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(direct, "POST /chat/completions HTTP/1.1\r\nhost: m\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n", body.len()).unwrap();
    direct.write_all(&body).unwrap();
    let mut raw = Vec::new();
    direct.read_to_end(&mut raw).unwrap();
    assert_eq!(parse_response(raw).body, r.body);
    // EG-07 on the mock's DeepSeek usage (1000 prompt tokens, 200 cached, 50 completion): cache reads at the cache-read
    // price, the rest at the input price, all at peak (provisional owner decision 2026-10-20, build-deviations §3.13)
    let cost = (800 * 300_000u64 + 200 * 6_000 + 50 * 1_200_000).div_ceil(1_000_000);
    let c = h.ceilings();
    assert_eq!((c["money"]["day_spent_musd"].as_u64(), c["money"]["total_spent_musd"].as_u64(), c["epoch"].as_u64()), (Some(cost), Some(cost), Some(1)));
    assert_eq!(h.ends()[0]["usage"], json!({"cache_read": 200, "input": 1000, "output": 50}));
    // RT-C8 on this path too
    let mut all = Vec::new();
    files(&h.dir, &h.rh.join("keys"), &mut all);
    assert!(all.iter().all(|(_, b)| !contains(b, KEY)));
    drop(m);
}

/// `INIT_MOCKLLM_BIN`, or the newest build in the kernel workspace's target dirs.
fn mock_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("INIT_MOCKLLM_BIN") {
        let p = PathBuf::from(p);
        assert!(p.is_file(), "INIT_MOCKLLM_BIN={} does not exist", p.display());
        return Some(p);
    }
    let k = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../kernel/target");
    let mut c: Vec<PathBuf> = vec![k.join("debug/init-mockllm")];
    c.extend(fs::read_dir(&k).into_iter().flatten().flatten().map(|e| e.path().join("debug/init-mockllm")));
    c.into_iter().filter(|p| p.is_file()).max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
}

#[test]
fn action_class_refusals_and_lineage() {
    let up = Upstream::start();
    let h = home("refuse");
    h.install(&format!("{}[actions.crates]\ndestinations = [\"127.0.0.1:{}\"]\n", providers(&[("deepseek", up.port)], ("1.00", "5.00")), up.port));
    h.key("deepseek", KEY, 0o600);
    let _d = h.daemon(&[]);
    let crates_post = format!("/x/crates/127.0.0.1:{}/se/rd/serde", up.port);
    let cases: Vec<(&str, &str, Vec<u8>, &str, &str)> = vec![
        // R3-01 / RT-E2: path, method, destination, unknown class
        ("POST", "/deepseek/fine_tuning/jobs", b"{}".to_vec(), "class.path", "a"),
        ("POST", "/deepseek/../admin", b"{}".to_vec(), "class.path", "a"),
        ("DELETE", "/deepseek/models", vec![], "class.method", "a"),
        ("GET", "/x/crates/example.com/se/rd/serde", vec![], "class.destination", "a"),
        ("POST", "/nope/chat/completions", b"{}".to_vec(), "class.unknown", "a"),
        // R3-02: a model above the level; an unlisted model
        ("POST", "/deepseek/chat/completions", chat("deepseek-v4-pro", 10), "class.level", "b"),
        ("POST", "/deepseek/chat/completions", chat("gpt-x", 10), "class.model", "b"),
        // R3-03: provider-executed tools
        ("POST", "/deepseek/chat/completions", br#"{"model":"deepseek-flash","tools":[{"type":"web_search"}]}"#.to_vec(), "class.provider_tool", "a"),
        ("POST", "/deepseek/anthropic/v1/messages", br#"{"model":"deepseek-flash","mcp_servers":[{"url":"https://x"}]}"#.to_vec(), "class.provider_tool", "a"),
        // R3-04: crates refuses writes
        ("POST", &crates_post, vec![], "class.method", "a"),
    ];
    for (m, p, b, code, rule) in &cases {
        let r = h.req(m, p, &[], b);
        assert_eq!((r.status, r.code()), (403, *code), "{m} {p}: {}", String::from_utf8_lossy(&r.raw));
        let v = r.json();
        assert_eq!((v["init"]["code"].as_str(), v["init"]["demotes"].as_bool(), v["init"]["retryable"].as_bool()), (Some(*code), Some(true), Some(false)));
        let shape = if p.contains("anthropic") { v["type"] == "error" } else { v["error"]["code"] == *code };
        assert!(shape, "{v}");
        let hit = h.hits().pop().unwrap();
        assert_eq!((hit["code"].as_str(), hit["demotes"].as_bool(), hit["rule"].as_str(), hit["source"].as_str()), (Some(*code), Some(true), Some(*rule), Some("egress")));
    }
    assert_eq!(h.hits().len(), cases.len(), "one ceiling_hit per demoting refusal");
    // a raw CONNECT names a destination outside every class
    let mut s = UnixStream::connect(h.sock()).unwrap();
    s.write_all(b"CONNECT example.com:443 HTTP/1.1\r\nhost: example.com:443\r\nconnection: close\r\n\r\n").unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    assert_eq!(parse_response(raw).code(), "class.destination");
    // R3-04: crates reads go through, with no credential
    let r = h.req("GET", &format!("/x/crates/127.0.0.1:{}/se/rd/serde", up.port), &[], b"");
    assert_eq!(r.status, 200);
    let s = up.seen().pop().unwrap();
    assert_eq!((s.method.as_str(), s.path.as_str()), ("GET", "/se/rd/serde"));
    assert!(!s.headers.iter().any(|(k, _)| k == "authorization" || k == "x-api-key"));
    assert_eq!(up.seen().len(), 1, "no refused request reached the upstream");
    // nothing in the lineage carries a path, query or key
    let lin = fs::read_to_string(h.rh.join("lineage/lineage.jsonl")).unwrap();
    assert!(!lin.contains("fine_tuning") && !lin.contains("example.com") && !lin.contains(KEY));
    // helper verbs are not in this release, and are not a hit
    assert_eq!(h.req("POST", "/_root/action", &[], b"{}").code(), "root.unsupported");
    assert_eq!(h.hits().len(), cases.len() + 1);
}

#[test]
fn money_is_observed_without_automatic_refusal_or_demotion() {
    let up = Upstream::start();
    let h = home("mobs");
    h.install(&providers(&[("deepseek", up.port)], ("0.0001", "0.0001")));
    h.key("deepseek", KEY, 0o600);
    let _d = h.daemon(&[]);
    for _ in 0..5 {
        assert_eq!(h.chat("deepseek-flash", 2000, &[]).status, 200);
    }
    let c = h.ceilings();
    assert_eq!(c["money"]["enforced"], false);
    assert_eq!(c["money"]["policy"], "observe");
    assert_eq!(c["money"]["day_spent_musd"], 5 * SSE_COST);
    assert_eq!(c["money"]["day_reserved_musd"], 0);
    assert!(h.hits().is_empty(), "money never demotes");
    let ends = h.ends().len();
    let invalid = h.chat("deepseek-flash", u64::MAX, &[]);
    assert_eq!(invalid.code(), "request.output_limit");
    assert_eq!(h.ends().len(), ends, "request validation remains enforced");
}

#[test]
fn observed_money_survives_day_roll_reload_and_restart() {
    let up = Upstream::start();
    let h = home("tobs");
    let ch = |day: &str, total: &str| providers(&[("deepseek", up.port)], (day, total));
    h.install(&ch("0.0001", "0.0001"));
    h.key("deepseek", KEY, 0o600);
    let d = h.daemon(&[("INIT_ROOT_TEST_NOW", "2026-10-20T10:00:00Z")]);
    for _ in 0..3 { assert_eq!(h.chat("deepseek-flash", 1000, &[]).status, 200); }
    assert_eq!(h.run(&["test", "clock", "--advance", "86400"]).status.code(), Some(0));
    let c = h.ceilings();
    assert_eq!(c["money"]["day_spent_musd"], 0);
    assert_eq!(c["money"]["total_spent_musd"], 3 * SSE_COST);
    h.apply(&ch("0.000001", "0.000001"));
    assert_eq!(h.chat("deepseek-flash", 1000, &[]).status, 200);
    let before = h.ceilings();
    drop(d);
    let _d = h.daemon(&[("INIT_ROOT_TEST_NOW", "2026-10-21T10:00:00Z")]);
    let after = h.ceilings();
    for k in ["enforced", "policy", "day_spent_musd", "total_spent_musd", "day_limit_musd"] {
        assert_eq!(before["money"][k], after["money"][k], "{k}");
    }
    assert_eq!(h.chat("deepseek-flash", 1000, &[]).status, 200);
    assert!(h.hits().is_empty());
}

#[test]
fn upstream_faults_retries_and_estimates() {
    let up = Upstream::start();
    let dead = dead_port();
    let h = home("faults");
    h.install(&providers(&[("deepseek", up.port), ("dead", dead)], ("1.00", "5.00")));
    h.key("deepseek", KEY, 0o600);
    h.key("dead", KEY, 0o600);
    let _d = h.daemon(&[]);
    // EG-09: a 401 whose body echoes a key fragment is withheld
    up.push(http_reply(401, "application/json", &format!("{{\"error\":\"bad key {}\"}}", &KEY[..12]), ""));
    let r = h.chat("deepseek-flash", 100, &[]);
    assert_eq!((r.status, r.code()), (401, "upstream.auth"));
    assert!(!contains(&r.raw, &KEY[..12]));
    assert!(h.alerts().contains("upstream_auth") && h.alerts().contains("\"source\":\"egress\""));
    // EG-08: a 503 is passed through, never retried by the Root
    up.push(http_reply(503, "application/json", "{\"error\":\"overloaded\"}", ""));
    let n = up.seen().len();
    let r = h.chat("deepseek-flash", 100, &[]);
    assert_eq!((r.status, r.code(), r.h("x-egress-retries"), r.body.as_slice()), (503, "", Some("0"), &b"{\"error\":\"overloaded\"}"[..]));
    assert_eq!(up.seen().len(), n + 1);
    // connect failures: at most 2 Root retries, then 502
    let t = Instant::now();
    let r = h.req("POST", "/dead/chat/completions", &[], &chat("deepseek-flash", 100));
    assert_eq!((r.status, r.code(), r.h("x-egress-retries")), (502, "upstream.transport", Some("2")));
    assert!(t.elapsed() >= Duration::from_millis(1400));
    // a redirect is never followed
    up.push(http_reply(302, "text/plain", "", "location: https://elsewhere.example/\r\n"));
    assert_eq!(h.chat("deepseek-flash", 100, &[]).code(), "upstream.redirect");
    // RT-C7: a stream cut before its usage is charged the full reservation, never zero
    let cut = SSE[..SSE.find("data: {\"choices\":[{\"delta\":{\"content\":\"\"}").unwrap()].to_string();
    up.push(format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 5000\r\nconnection: close\r\n\r\n{cut}").into_bytes());
    let body = chat("deepseek-flash", 100);
    let r = h.req("POST", "/deepseek/chat/completions", &[("x-usage", "0"), ("x-egress-cost", "0")], &body);
    assert_eq!((r.status, r.complete), (200, false), "the kernel sees a cut stream");
    let end = h.ends().into_iter().last().unwrap();
    assert_eq!(end["cost_musd"].as_u64(), Some(reservation(body.len(), 100)));
    assert!(end["flags"].as_array().unwrap().iter().any(|f| f == "usage_estimated"));
    // refused and failed requests released their reservations; only provider refusals cost nothing
    let c = h.ceilings();
    assert_eq!((c["money"]["day_reserved_musd"].as_u64(), c["money"]["day_spent_musd"].as_u64()), (Some(0), Some(reservation(body.len(), 100))));
    assert!(h.hits().is_empty(), "upstream failures are not ceiling hits");
}

#[test]
fn keys_panic_rate_and_fail_closed() {
    let up = Upstream::start();
    let h = home("keys");
    h.install(&format!("{}[actions.crates]\ndestinations = [\"127.0.0.1:{}\"]\nrate_per_min = 2\n",
        providers(&[("deepseek", up.port)], ("1.00", "5.00")), up.port));
    let _d = h.daemon(&[("INIT_ROOT_TEST_NOW", "2026-10-20T10:00:00Z")]);
    // no key: the provider is disabled; not a hit
    let r = h.chat("deepseek-flash", 10, &[]);
    assert_eq!((r.status, r.code()), (503, "class.disabled"));
    // test mode refuses a key that is not marked as a test key, and an insecure one
    h.key("deepseek", "sk-live-0123456789", 0o600);
    assert_eq!(h.owner("keys.reload")["result"]["keys"]["deepseek"], "refused");
    assert!(h.alerts().contains("key_file_real") && !h.alerts().contains("sk-live"));
    h.key("deepseek", KEY, 0o644);
    assert_eq!(h.owner("keys.reload")["result"]["keys"]["deepseek"], "insecure");
    assert!(h.alerts().contains("key_file_insecure"));
    assert_eq!(h.chat("deepseek-flash", 10, &[]).code(), "class.disabled");
    h.key("deepseek", KEY, 0o600);
    assert_eq!(h.owner("keys.reload")["result"]["keys"]["deepseek"], "present");
    assert_eq!(h.chat("deepseek-flash", 10, &[]).status, 200);
    assert!(h.hits().is_empty());
    // R3-05 / RT-E5: crates at 2 per minute; the third is 429 class.rate, not demoting; allowed after the minute
    let crates = format!("/x/crates/127.0.0.1:{}/se/rd/serde", up.port);
    assert!(h.req("GET", &crates, &[], b"").status == 200 && h.req("GET", &crates, &[], b"").status == 200);
    let r = h.req("GET", &crates, &[], b"");
    assert_eq!((r.status, r.code(), r.h("retry-after")), (429, "class.rate", Some("30")));
    assert_eq!((h.hits()[0]["demotes"].as_bool(), h.hits()[0]["code"].as_str()), (Some(false), Some("class.rate")));
    assert_eq!(h.run(&["test", "clock", "--advance", "60"]).status.code(), Some(0));
    assert_eq!(h.req("GET", &crates, &[], b"").status, 200);
    // the ledger cannot be written: fail closed, alert, recover
    let usage = h.rh.join("egress/usage");
    fs::set_permissions(&usage, fs::Permissions::from_mode(0o500)).unwrap();
    let day = fs::read_dir(&usage).unwrap().flatten().next().unwrap().path();
    fs::set_permissions(&day, fs::Permissions::from_mode(0o400)).unwrap();
    let r = h.chat("deepseek-flash", 10, &[]);
    fs::set_permissions(&day, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&usage, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!((r.status, r.code()), (503, "root.ledger"));
    assert!(h.alerts().contains("egress_ledger"));
    assert_eq!(h.ceilings()["money"]["day_reserved_musd"], 0);
    assert_eq!(h.chat("deepseek-flash", 10, &[]).status, 200);
    // panic closes the egress; resume reopens it
    assert_eq!(h.run(&["panic"]).status.code(), Some(0));
    let hl = h.req("GET", "/_root/health", &[], b"").json();
    assert_eq!((hl["panicked"].as_bool(), hl["ok"].as_bool()), (Some(true), Some(false)));
    assert_eq!(h.chat("deepseek-flash", 10, &[]).code(), "root.panicked");
    assert_eq!(h.run(&["resume"]).status.code(), Some(0));
    assert_eq!(h.chat("deepseek-flash", 10, &[]).status, 200);
    let stats = h.run(&["egress", "stats"]);
    let v: Value = serde_json::from_slice(&stats.stdout).unwrap();
    assert_eq!(v["classes"][0]["id"], "crates");
    assert!(!contains(&stats.stdout, KEY));
}

#[test]
fn level_gates_models_from_the_ledger() {
    let up = Upstream::start();
    let h = home("level");
    fs::create_dir_all(&h.rh).unwrap();
    fs::write(h.rh.join("charter.toml"), charter(&providers(&[("deepseek", up.port)], ("1.00", "5.00")))).unwrap();
    let spec = h.dir.join("spec.json");
    fs::write(&spec, r#"{"start_at":"2026-09-01T00:00:00Z","start_level":1,"records":[]}"#).unwrap();
    assert_eq!(h.run(&["test", "ledger", "--spec", spec.to_str().unwrap()]).status.code(), Some(0));
    assert_eq!(h.run(&["install"]).status.code(), Some(0));
    h.key("deepseek", KEY, 0o600);
    let _d = h.daemon(&[("INIT_ROOT_TEST_NOW", "2026-10-20T10:00:00Z")]);
    let c = h.ceilings();
    assert_eq!(c["level"], 1);
    let routable: Vec<(String, bool)> = c["models"].as_array().unwrap().iter().map(|m| (m["model"].as_str().unwrap().into(), m["routable"] == true)).collect();
    assert_eq!(routable, vec![("deepseek-flash".into(), true), ("deepseek-v4-pro".into(), true)]);
    assert_eq!(h.chat("deepseek-v4-pro", 10, &[]).status, 200, "L1 may route to the L1 model");
    // a panic demotes L1 → L0 (CH-2), and the egress gates the model at once
    assert_eq!(h.run(&["panic"]).status.code(), Some(0));
    wait(10, || h.ceilings()["level"] == 0); // the CLI confirms within 2 s; the records may land just after
    assert_eq!(h.run(&["resume"]).status.code(), Some(0));
    assert_eq!(h.ceilings()["level"], 0);
    assert_eq!(h.chat("deepseek-v4-pro", 10, &[]).code(), "class.level");
    assert_eq!(h.chat("deepseek-flash", 10, &[]).status, 200);
}
