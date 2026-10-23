//! R-SEC integration tests for the egress findings of build/review/root-security-2026-10-20.md: F2 (every
//! model-class route needs a metering rule, default-deny) and F3/F7 (a bounded number of kernel connections,
//! with the Root still serving `owner.sock` and panic under a flood). A test-mode Root forwards only to a
//! loopback upstream defined here; keys are fake (`test-…`); no real key is read and no real API is called.
//! Homes live under `root/target/test-homes/rsec/`, with `HOME` pointed there; the daemon is killed by its
//! own handle. Run with `cargo test --offline -p init-root --features test-fixtures`.
#![cfg(feature = "test-fixtures")]

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_init-root");
const KEY: &str = "test-key-rsec-5e5e5e5e";
/// egress.rs `MAX_CONNS`.
const MAX_CONNS: usize = 96;

struct Home {
    dir: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
}

/// A fresh home `root/target/test-homes/rsec/<name>` (short names: ROOT_HOME stays ≤ 80 bytes).
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/rsec");
    let dir = base.join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(&dir).unwrap();
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
}

/// A test-mode charter whose `providers` class points at a loopback upstream and lists `paths`.
fn charter(port: u16, paths: &str) -> String {
    format!("schema_version = 1\n[purpose]\ntext = \"R-SEC test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n\
        [sealed]\nreserve_usd = \"0.00\"\naudit_usd = \"0.00\"\n[root]\ntest_mode = true\n[money]\nday_usd = \"1.00\"\ntotal_usd = \"5.00\"\n\
        [actions.providers]\ndestinations = [\"127.0.0.1:{port}\"]\npaths = [{paths}]\n\
        [actions.providers.models.\"deepseek-flash\"]\nmin_level = 0\ninput_musd_per_mtok = 300000\noutput_musd_per_mtok = 1200000\n\
        [actions.providers.providers.deepseek]\nhost = \"127.0.0.1:{port}\"\n")
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
    fn install(&self, charter: &str) -> Output {
        let f = self.dir.join("charter.toml");
        fs::write(&f, charter).unwrap();
        let o = self.run(&["install", "--charter", f.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
        o
    }
    fn key(&self) {
        let p = self.rh.join("keys/providers/deepseek");
        fs::write(&p, KEY).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn daemon(&self) -> Daemon {
        let d = Daemon(self.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        let (sock, owner) = (self.sock(), self.rh.join("run/owner.sock"));
        wait(10, || UnixStream::connect(&sock).is_ok() && UnixStream::connect(&owner).is_ok());
        d
    }
    fn sock(&self) -> PathBuf {
        self.rh.join("run/egress.sock")
    }
    fn req(&self, method: &str, path: &str, body: &[u8]) -> Resp {
        let mut s = UnixStream::connect(self.sock()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let head = format!("{method} {path} HTTP/1.1\r\nhost: egress\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n", body.len());
        s.write_all(head.as_bytes()).unwrap();
        s.write_all(body).unwrap();
        let mut raw = Vec::new();
        let _ = s.read_to_end(&mut raw);
        Resp::parse(&raw)
    }
    /// One request frame on the owner socket.
    fn owner(&self, cmd: &str) -> Value {
        let mut s = UnixStream::connect(self.rh.join("run/owner.sock")).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        writeln!(s, "{}", json!({"args": {}, "cmd": cmd, "id": "t", "v": 1})).unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
    }
    fn ledger(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for e in fs::read_dir(self.rh.join("egress/usage")).unwrap().flatten() {
            out.extend(fs::read_to_string(e.path()).unwrap().lines().map(|l| serde_json::from_str::<Value>(l).unwrap()));
        }
        out
    }
    fn hits(&self) -> usize {
        fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default().lines().filter(|l| l.contains("\"ceiling_hit\"")).count()
    }
}

/// The daemon this test spawned; killed by its own handle.
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

struct Resp {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Resp {
    fn parse(raw: &[u8]) -> Resp {
        let at = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or_else(|| panic!("no head: {}", String::from_utf8_lossy(raw)));
        let head = String::from_utf8_lossy(&raw[..at]).to_ascii_lowercase();
        let status = head.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        Resp { status, head, body: raw[at + 4..].to_vec() }
    }
    fn code(&self) -> &str {
        self.head.split("\r\n").find_map(|l| l.strip_prefix("x-init-egress-error: ")).unwrap_or("")
    }
}

/// A loopback upstream that records each request line and its auth header, and answers `reply`.
struct Upstream {
    port: u16,
    seen: Arc<Mutex<Vec<(String, bool)>>>,
}

impl Upstream {
    fn start(reply: String) -> Upstream {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (s, reply) = (seen.clone(), Arc::new(reply));
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                let (s, reply) = (s.clone(), reply.clone());
                std::thread::spawn(move || serve(c, &s, &reply));
            }
        });
        Upstream { port, seen }
    }
    fn seen(&self) -> Vec<(String, bool)> {
        self.seen.lock().unwrap().clone()
    }
}

fn serve(c: TcpStream, seen: &Mutex<Vec<(String, bool)>>, reply: &str) {
    let mut w = c.try_clone().unwrap();
    let mut r = BufReader::new(c);
    let mut first = String::new();
    if r.read_line(&mut first).unwrap_or(0) == 0 {
        return;
    }
    let (mut n, mut auth) = (0usize, false);
    loop {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        if l == "\r\n" || l.is_empty() {
            break;
        }
        let l = l.to_ascii_lowercase();
        auth |= l == format!("authorization: bearer {}\r\n", KEY.to_ascii_lowercase());
        if let Some(v) = l.strip_prefix("content-length:") {
            n = v.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; n];
    r.read_exact(&mut body).unwrap();
    let line = first.split(' ').take(2).collect::<Vec<_>>().join(" ");
    seen.lock().unwrap().push((line, auth));
    let _ = w.write_all(reply.as_bytes());
}

const BALANCE: &str = r#"{"balance_infos":[],"is_available":true}"#;

/// F2: a billable endpoint the owner lists but the Root has no metering rule for is refused with a typed
/// error and never reaches the provider; the free balance read S1 uses is forwarded with the key at $0; a
/// completion is still reserved and metered.
#[test]
fn unmeterable_paths_are_refused_and_free_ones_cost_nothing() {
    let up = Upstream::start(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{BALANCE}", BALANCE.len()));
    let h = home("meter");
    let paths = "\"chat/completions\", \"completions\", \"embeddings\", \"models\", \"user/balance\"";
    let o = h.install(&charter(up.port, paths));
    let warned = String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
    assert!(warned.contains("class.unmetered"), "C11 warns at install: {warned}");
    h.key();
    let _d = h.daemon();
    let body = br#"{"model":"deepseek-flash","input":"x","prompt":"x","max_tokens":10}"#;
    for (m, p) in [("POST", "/deepseek/embeddings"), ("POST", "/deepseek/completions"), ("GET", "/deepseek/chat/completions"), ("POST", "/deepseek/user/balance")] {
        let r = h.req(m, p, body);
        assert_eq!((r.status, r.code()), (403, "class.unmetered"), "{m} {p}: {}", String::from_utf8_lossy(&r.body));
    }
    assert!(up.seen().is_empty(), "no unmetered request reached the provider: {:?}", up.seen());
    assert_eq!(h.hits(), 0, "an owner-listed path the Root cannot meter is not a demoting hit");
    let refused = h.ledger().into_iter().filter(|v| v["t"] == "refuse" && v["code"] == "class.unmetered").count();
    assert_eq!(refused, 4, "each refusal is in the usage ledger");
    // the balance read: forwarded with the Root's key, reserved and charged nothing
    let r = h.req("GET", "/deepseek/user/balance", b"");
    assert_eq!((r.status, String::from_utf8_lossy(&r.body).contains(BALANCE)), (200, true));
    assert_eq!(up.seen(), vec![("GET /user/balance".to_string(), true)]);
    // a completion is reserved before it is forwarded; this upstream's 200 carries no usage, so the
    // reservation is charged (RT-C7)
    let r = h.req("POST", "/deepseek/chat/completions", br#"{"model":"deepseek-flash","max_tokens":10}"#);
    assert_eq!(r.status, 200);
    let ledger = h.ledger();
    let starts: Vec<&Value> = ledger.iter().filter(|v| v["t"] == "start").collect();
    let ends: Vec<&Value> = ledger.iter().filter(|v| v["t"] == "end").collect();
    assert_eq!((starts.len(), ends.len()), (2, 2));
    assert_eq!((starts[0]["reserved_musd"].as_u64(), ends[0]["cost_musd"].as_u64(), starts[0]["model"].is_null()), (Some(0), Some(0), true));
    assert!(starts[1]["reserved_musd"].as_u64().unwrap() > 0 && starts[1]["model"] == "deepseek-flash");
    assert_eq!(ends[1]["cost_musd"], starts[1]["reserved_musd"], "a 2xx without usage is charged its reservation");
}

/// The Root's thread count, read for the daemon this test spawned (read-only).
fn threads(pid: u32) -> usize {
    if cfg!(target_os = "linux") {
        return fs::read_dir(format!("/proc/{pid}/task")).map(|d| d.count()).unwrap_or(0);
    }
    let o = Command::new("/bin/ps").args(["-M", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).lines().count().saturating_sub(1)
}

/// F3/F7: connections held open with a partial request head stay bounded at `MAX_CONNS` (each one more is
/// answered 503 `root.busy` at once and spawns no thread), the local GETs are refused the same way, and the
/// Root still serves `owner.sock` and panics, which closes every held connection.
#[test]
fn a_connection_flood_stays_bounded_and_owner_and_panic_still_work() {
    let h = home("flood");
    h.install(&charter(1, "\"chat/completions\", \"models\", \"user/balance\""));
    let d = h.daemon();
    std::thread::sleep(Duration::from_millis(300)); // the readiness probes' connections have ended
    let base = threads(d.0.id());
    let (mut held, mut busy) = (Vec::new(), 0);
    for _ in 0..MAX_CONNS + 40 {
        let mut s = UnixStream::connect(h.sock()).unwrap();
        let _ = s.set_read_timeout(Some(Duration::from_millis(100))); // EINVAL on macOS once the Root closed it
        let _ = s.write_all(b"POST /deepseek/chat/completions HTTP/1.1\r\nhost: egress\r\n");
        let mut raw = Vec::new();
        match s.read_to_end(&mut raw) {
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) && raw.is_empty() => held.push(s),
            _ => {
                let r = Resp::parse(&raw);
                assert_eq!((r.status, r.code()), (503, "root.busy"), "{}", String::from_utf8_lossy(&raw));
                busy += 1;
            }
        }
    }
    assert_eq!((held.len(), busy), (MAX_CONNS, 40), "at most MAX_CONNS connections are served at once");
    let flooded = threads(d.0.id());
    assert!(base > 0 && flooded >= base + MAX_CONNS / 2 && flooded <= base + MAX_CONNS + 2, "threads stay bounded: {base} → {flooded}");
    // the local routes get the same typed back-pressure
    let r = h.req("GET", "/_root/health", b"");
    assert_eq!((r.status, r.code()), (503, "root.busy"));
    assert!(threads(d.0.id()) <= flooded + 1, "a refused connection spawns no thread");
    // the owner is still served, and panic still works and frees every slot
    assert_eq!(h.owner("status")["ok"], true, "owner.sock answers under the flood");
    assert_eq!(h.owner("panic")["ok"], true);
    for s in &mut held {
        let _ = s.set_read_timeout(Some(Duration::from_secs(5))); // EINVAL once closed, as above
        let mut raw = Vec::new();
        assert!(s.read_to_end(&mut raw).is_ok() && raw.is_empty(), "panic closed the held connection");
    }
    drop(held);
    wait(5, || h.req("GET", "/_root/health", b"").status == 200);
    let r = h.req("POST", "/deepseek/chat/completions", b"{}");
    assert_eq!((r.status, r.code()), (503, "root.panicked"));
}
