//! RED-TEAM: the agent zone behaves adversarially against the **real** `init-root` in test mode, at $0.
//! These tests fill the gaps left by the per-WP adversarial suites (A6 in init-testkit, wp22/rsec egress,
//! wp24 gate-of-gates, wp25 ladder, rsafe sweep). They assert the Root's guarantees directly for attacks
//! the other suites did not cover on their own:
//!   - a request body that tries to forge the price / catalog (money);
//!   - requests aimed at a host or provider the charter never listed (money / egress confinement);
//!   - a hostile kernel forging Root-only lineage kinds (integrity / lineage);
//!   - a hostile kernel forging an owner signal through the lineage (control / owner impersonation);
//!   - a hostile kernel asking for the L3-gated capabilities below L3 (control / autonomy escalation);
//!   - a hostile kernel reporting its own promotions to climb the ladder (RT-GAP-1, closed: RF-25).
//!
//! Two adversaries, both as the brief asks: the egress tests speak the kernel-proxy wire to the real
//! `egress.sock`; the lineage/control tests run `init-root-fakekernel` (a hostile fake kernel binary) under
//! the real `kernel` Seatbelt profile and drive it over the real control link (`work/fk-call.json`).
//!
//! Live-system safety (CONVENTIONS §1): every process is this test's Root daemon or one the Root spawned
//! and holds (the fake kernel, its `/bin/cat` body); the test signals only its own children, by handle.
//! Keys are fake (`test-…`); upstreams are loopback; no real key is read and no real API is called. Homes
//! live under `root/target/test-homes/redteam/`, with `HOME` pointed there, removed by exact path on pass.
//! Run with `cargo test --offline -p init-root --features test-fixtures --test redteam`.
#![cfg(feature = "test-fixtures")]

use serde_json::{Value, json};
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
const FK: &str = env!("CARGO_BIN_EXE_init-root-fakekernel");
const KEY: &str = "test-key-redteam-9a9a9a9a";
/// deepseek-flash catalog prices (µUSD per 1M tokens) in the test charter below.
const IN_MUSD: u64 = 300_000;
const OUT_MUSD: u64 = 1_200_000;

fn base() -> PathBuf {
    fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/redteam")
}

fn wait(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "not within {secs} s: {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

struct Home {
    dir: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
}

/// A fresh home `root/target/test-homes/redteam/<name>` (short: ROOT_HOME stays ≤ 80 bytes).
fn home(name: &str) -> Home {
    let dir = base().join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(dir.join("rel")).unwrap();
    fs::create_dir_all(dir.join("tc/bin")).unwrap();
    fs::copy(FK, dir.join("rel/init-kernel")).unwrap();
    fs::copy(BIN, dir.join("init-root")).unwrap(); // frozen (APFS clone): a concurrent rebuild cannot swap it
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
}

/// A test-mode charter: the `providers` model class on a loopback upstream, `deepseek-flash` at L0, and
/// `$money` ceilings. `max_level = 4` so the ladder is not pinned by the charter (the build cap still holds).
fn charter(port: u16, money: (&str, &str)) -> String {
    format!(
        "schema_version = 1\n[purpose]\ntext = \"RED-TEAM test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n\
         [sealed]\nreserve_usd = \"0.00\"\naudit_usd = \"0.00\"\n[root]\ntest_mode = true\nhb_interval_s = 1\nhb_stale_s = 3\n\
         kernel_hello_timeout_s = 20\nbackoff_initial_ms = 50\nbackoff_max_ms = 200\nstop_grace_s = 5\n\
         [money]\nday_usd = \"{}\"\ntotal_usd = \"{}\"\n[actions.providers]\ndestinations = [\"127.0.0.1:{port}\"]\n\
         paths = [\"chat/completions\", \"models\", \"user/balance\"]\n\
         [actions.providers.models.\"deepseek-flash\"]\nmin_level = 0\ninput_musd_per_mtok = {IN_MUSD}\noutput_musd_per_mtok = {OUT_MUSD}\n\
         reserve_output_tokens = 65536\n[actions.providers.providers.deepseek]\nhost = \"127.0.0.1:{port}\"\n",
        money.0, money.1
    )
}

impl Home {
    fn cmd(&self) -> Command {
        let mut c = Command::new(self.dir.join("init-root"));
        c.env_clear().env("HOME", &self.dir).env("INIT_ROOT_HOME", &self.rh).env("INIT_HOME", &self.ih).env("PATH", "/usr/bin:/bin");
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).stdin(Stdio::null()).output().unwrap()
    }
    /// Install with the fake kernel as the release kernel (so `start` runs the hostile kernel).
    fn install(&self, charter: &str) {
        fs::create_dir_all(&self.rh).unwrap();
        fs::write(self.rh.join("charter.toml"), charter).unwrap();
        let (rel, tc) = (self.dir.join("rel"), self.dir.join("tc"));
        let o = self.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
        let k = self.rh.join("keys/providers/deepseek");
        fs::write(&k, KEY).unwrap();
        fs::set_permissions(&k, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn sock(&self) -> PathBuf {
        self.rh.join("run/egress.sock")
    }
    fn records(&self) -> Vec<Value> {
        fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
    fn kinds(&self, k: &str) -> Vec<Value> {
        self.records().into_iter().filter(|r| r["kind"] == k).map(|r| r["data"].clone()).collect()
    }
    fn ledger(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for e in fs::read_dir(self.rh.join("egress/usage")).into_iter().flatten().flatten() {
            out.extend(fs::read_to_string(e.path()).unwrap_or_default().lines().map(|l| serde_json::from_str::<Value>(l).unwrap()));
        }
        out
    }
    /// Start the Root with the hostile fake kernel; wait for the egress socket and the kernel's `ready`.
    fn daemon(&self) -> Daemon {
        let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
        let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
        let c = self.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .env("INIT_TEST_FK", "").env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir"))
            .env("INIT_TEST_HOME_REAL", &self.dir).spawn().unwrap();
        let d = Daemon(c, self.rh.clone());
        let (es, ow) = (self.sock(), self.rh.join("run/owner.sock"));
        wait(10, "egress and owner sockets", || UnixStream::connect(&es).is_ok() && UnixStream::connect(&ow).is_ok());
        wait(15, "the fake kernel reached ready", || self.fk("ready").iter().any(|e| e["ok"] == true));
        d
    }
    /// Every `fk.jsonl` line with the event name `ev`.
    fn fk(&self, ev: &str) -> Vec<Value> {
        fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v["ev"] == ev).collect()
    }
    /// Make the hostile kernel issue one control-link request and return its outcome
    /// (`{"ev":"call","ok":bool,"result"|"error":…}`). The fake kernel takes `work/fk-call.json` once.
    fn kernel_call(&self, cmd: &str, args: Value) -> Value {
        let before = self.fk("call").len();
        let (tmp, dst) = (self.ih.join("work/fk-call.tmp"), self.ih.join("work/fk-call.json"));
        fs::create_dir_all(self.ih.join("work")).unwrap();
        fs::write(&tmp, json!({"cmd": cmd, "args": args}).to_string()).unwrap();
        fs::rename(tmp, dst).unwrap();
        wait(15, "the kernel call outcome", || self.fk("call").len() > before);
        self.fk("call").into_iter().nth(before).unwrap()
    }
    /// One request on the egress socket, as the kernel proxy sends it.
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
}

/// The Root daemon this test spawned. On drop: an orderly `stop`, then the daemon by its own handle.
struct Daemon(Child, PathBuf);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = Command::new(BIN).env_clear().env("HOME", "/nonexistent").env("INIT_ROOT_HOME", &self.1).args(["stop"]).output();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(8) {
            let o = Command::new(BIN).env_clear().env("HOME", "/nonexistent").env("INIT_ROOT_HOME", &self.1).args(["status", "--json"]).output();
            let v: Value = o.ok().and_then(|o| serde_json::from_slice(&o.stdout).ok()).unwrap_or_default();
            if v["instances"].as_array().is_none_or(Vec::is_empty) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
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

/// A loopback upstream that records each request line and answers `reply`.
struct Upstream {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
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
    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

fn serve(c: TcpStream, seen: &Mutex<Vec<String>>, reply: &str) {
    let mut w = c.try_clone().unwrap();
    let mut r = BufReader::new(c);
    let mut first = String::new();
    if r.read_line(&mut first).unwrap_or(0) == 0 {
        return;
    }
    let mut n = 0usize;
    loop {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        if l == "\r\n" || l.is_empty() {
            break;
        }
        if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
            n = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; n];
    let _ = r.read_exact(&mut body);
    seen.lock().unwrap().push(first.split(' ').take(2).collect::<Vec<_>>().join(" "));
    let _ = w.write_all(reply.as_bytes());
}

fn http_200(body: &str) -> String {
    format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
}

/// MONEY — the zone cannot forge the price or override the catalog. A request body carrying its own
/// `input_musd_per_mtok` / `output_musd_per_mtok` / `price` fields is reserved and charged at the Root's
/// charter catalog price, not the injected one; the Root forwards the body byte-for-byte but ignores the
/// injected prices for accounting.
#[test]
fn rt_a_forged_price_in_the_request_body_does_not_override_the_catalog() {
    let up = Upstream::start(http_200("{}"));
    let h = home("price");
    h.install(&charter(up.port, ("1.00", "5.00")));
    let _d = h.daemon();
    // a body that tries to set its own (near-zero) prices and even a top-level `price`
    let body = br#"{"model":"deepseek-flash","max_tokens":1000,"input_musd_per_mtok":1,"output_musd_per_mtok":1,"cache_read_musd_per_mtok":1,"price":0}"#;
    let r = h.req("POST", "/deepseek/chat/completions", body);
    assert_eq!(r.status, 200, "forwarded: {}", String::from_utf8_lossy(&r.body));
    let start = h.ledger().into_iter().find(|v| v["t"] == "start").expect("a start line");
    // the Root's reservation from its catalog: input at 1 token/byte, output at max_tokens
    let want = (body.len() as u64 * IN_MUSD + 1000 * OUT_MUSD).div_ceil(1_000_000);
    assert_eq!(start["reserved_musd"].as_u64(), Some(want), "reserved at the catalog price, not the injected 1 µUSD");
    assert_eq!(start["model"].as_str(), Some("deepseek-flash"));
    assert!(want > 1000, "the catalog reservation dwarfs the forged near-zero price");
    // the injected fields rode along to the upstream (forwarded verbatim) but changed no accounting
    assert_eq!(up.seen(), vec!["POST /chat/completions".to_string()]);
    drop(_d);
    fs::remove_dir_all(&h.dir).unwrap();
}

/// MONEY / EGRESS CONFINEMENT — a request aimed at a provider or host the charter never listed is refused
/// with a typed `class.*` error and never reaches any upstream: an unknown provider id, the model class
/// addressed as an http `/x/` class, and (sanity) the listed provider still works.
#[test]
fn rt_requests_to_unlisted_providers_and_hosts_are_refused() {
    let up = Upstream::start(http_200("{}"));
    let h = home("host");
    h.install(&charter(up.port, ("1.00", "5.00")));
    let _d = h.daemon();
    let body = br#"{"model":"deepseek-flash","max_tokens":10}"#;
    // an unknown provider id: no model class serves it
    assert_eq!(h.req("POST", "/evilcorp/chat/completions", body).code(), "class.unknown");
    // the model class reached as an http `/x/<class>/<host>/…` to an off-charter host: a model class is not
    // an http class, so it is unknown there (and the host is not a destination regardless)
    assert_eq!(h.req("POST", "/x/providers/198.51.100.7:443/chat/completions", body).code(), "class.unknown");
    // a path grammar escape to climb out of the provider base path
    assert_eq!(h.req("POST", "/deepseek/../../secret", body).code(), "class.path");
    assert!(up.seen().is_empty(), "no refused request reached any upstream: {:?}", up.seen());
    // the control: the one listed provider/host/path is forwarded
    assert_eq!(h.req("POST", "/deepseek/chat/completions", body).status, 200);
    assert_eq!(up.seen(), vec!["POST /chat/completions".to_string()]);
    drop(_d);
    fs::remove_dir_all(&h.dir).unwrap();
}

/// INTEGRITY / LINEAGE — a hostile kernel cannot forge any Root-only lineage kind. Each append of a
/// kind reserved to the Root (`ladder`, `audit`, `mutation`, `charter`, `genesis`, `rollback`, `lkg`,
/// `audited`, `ceiling_hit`) is refused `root.lineage`; none lands in the signed chain, the chain still
/// verifies, and the folded autonomy level is untouched.
#[test]
fn rt_a_hostile_kernel_cannot_forge_root_only_lineage_kinds() {
    let h = home("forge");
    h.install(&charter(1, ("1.00", "5.00")));
    let d = h.daemon();
    let before = h.records().len();
    let forged: &[(&str, Value)] = &[
        ("ladder", json!({"cause": "unlock", "direction": "up", "from": 0, "to": 4, "trigger": {"kind": "x", "seq": 1}})),
        ("audit", json!({"tuple": "t0", "score_permille": 1000, "regression": false, "trigger": "scheduled"})),
        ("mutation", json!({"pass": true, "source": "agent", "total": 7, "rejected": 7, "controls": 3, "controls_ok": 3,
            "disagreements": 0, "failed": [], "gate": "g", "sample_n": 12, "spent_musd": 0, "tuple": null, "underpowered": false})),
        ("charter", json!({"sha256": "0".repeat(64), "ladder": {}})),
        ("genesis", json!({"v": 1})),
        ("rollback", json!({"from": "a", "to": "b", "reason": "x", "target": "x"})),
        ("lkg", json!({"tuple": "t0"})),
        ("audited", json!({"tuple": "t0"})),
        ("ceiling_hit", json!({"ceiling": "money.day", "demotes": false, "rule": "none"})),
    ];
    for (kind, data) in forged {
        let n0 = h.kinds(kind).len(); // the Root writes some of these itself (charter, genesis at install)
        let out = h.kernel_call("lineage.append", json!({"kind": kind, "data": data, "idem": format!("forge-{kind}")}));
        assert_eq!(out["ok"], false, "forging {kind:?} must be refused: {out}");
        assert_eq!(out["error"]["code"], "root.lineage", "{kind}: {out}");
        assert_eq!(h.kinds(kind).len(), n0, "a forged {kind} record landed in the chain");
    }
    // no record of a Root-only kind carries a kernel signer
    let kinds: Vec<&str> = forged.iter().map(|(k, _)| *k).collect();
    let bad: Vec<Value> = h.records().into_iter().filter(|r| kinds.contains(&r["kind"].as_str().unwrap_or(""))
        && r["by"].as_str().is_some_and(|b| b.starts_with("kernel"))).collect();
    assert!(bad.is_empty(), "kernel-signed Root-only records: {bad:?}");
    // the chain only grew with legitimate records (the Root's own), still verifies, and the level held
    assert!(h.records().len() >= before, "the chain did not shrink");
    let av = h.run(&["audit", "verify", "--full"]);
    assert!(av.status.success(), "audit verify: {}", String::from_utf8_lossy(&av.stdout));
    let lvl: Value = serde_json::from_slice(&h.run(&["ledger"]).stdout).unwrap_or(Value::Null);
    assert_eq!(lvl["level"].as_i64(), Some(0), "the forged ladder did not raise the level");
    drop(d);
    fs::remove_dir_all(&h.dir).unwrap();
}

/// CONTROL / OWNER IMPERSONATION — a hostile kernel cannot forge an owner decision through the lineage.
/// An `owner` record whose `qid` names no line the Root actually relayed is refused `root.lineage`, and no
/// owner record lands. (The genuine `owner` kind is kernel-reportable, which is why the qid-relay check,
/// not the kind registry, is the gate here.)
#[test]
fn rt_a_hostile_kernel_cannot_forge_an_owner_signal() {
    let h = home("owner");
    h.install(&charter(1, ("1.00", "5.00")));
    let d = h.daemon();
    for sig in ["bless", "accept", "reject"] {
        let out = h.kernel_call("lineage.append", json!({"kind": "owner",
            "data": {"signal": sig, "qid": "forged-qid-the-root-never-relayed", "target": "evo/x"}, "idem": format!("owner-{sig}")}));
        assert_eq!(out["ok"], false, "forged owner {sig} must be refused: {out}");
        assert_eq!(out["error"]["code"], "root.lineage", "{sig}: {out}");
    }
    assert!(h.kinds("owner").is_empty(), "a forged owner decision landed in the chain");
    drop(d);
    fs::remove_dir_all(&h.dir).unwrap();
}

/// CONTROL / AUTONOMY ESCALATION — RT-GAP-1, closed by ADR-011 §7.1 (RF-25). A hostile kernel inflates what R7 used
/// to count for the L1 unlock: `promotion{lkg:true}` is a kernel-reportable kind, and `check_promotion` only requires
/// the body blob to be in the Root's store, which the kernel can fill itself (`store.put`). With one genuine,
/// Root-written clean audit in the window, 20 self-reported promotions under distinct shas used to unlock L1 in a test
/// build (SUPPORTED_MAX_LEVEL = 4). The level now counts Root-written records only (owner turns, days, clean change
/// groups, direct harm rollbacks, quarantines, demotions, twin reports): the promotions are recorded and unlock nothing.
#[test]
fn rt_gap1_self_reported_promotions_cannot_unlock_l1() {
    let h = home("promo");
    fs::create_dir_all(&h.rh).unwrap();
    fs::write(h.rh.join("charter.toml"), charter(1, ("1.00", "5.00"))).unwrap();
    // one genuine clean audit, Root-written (the test ledger is the Root path), inside the 14-day window
    let spec = h.dir.join("spec.json");
    fs::write(&spec, json!({"start_at": "2030-01-01T00:00:00Z", "start_level": 0, "records": [{"at": "2030-01-05T00:00:00Z", "kind": "audit",
        "data": {"tuple": "t0", "score_permille": 900, "regression": false, "trigger": "scheduled"}}]}).to_string()).unwrap();
    let o = h.run(&["test", "ledger", "--spec", spec.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "test ledger: {}", String::from_utf8_lossy(&o.stderr));
    h.install(&charter(1, ("1.00", "5.00")));
    let info: Value = serde_json::from_str(&fs::read_to_string(h.rh.join("install.json")).unwrap()).unwrap();
    let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
    // `promote`: the hostile kernel store.puts a body blob of its own and reports one LKG promotion
    let c = h.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", "promote")
        .env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir")).env("INIT_TEST_HOME_REAL", &h.dir)
        .env("INIT_ROOT_TEST_NOW", "2030-01-10T00:00:00Z").spawn().unwrap();
    let d = Daemon(c, h.rh.clone());
    wait(15, "the kernel's own store.put", || h.fk("store.put").iter().any(|e| e["ok"] == true));
    let blob = h.fk("store.put")[0]["result"]["sha256"].as_str().unwrap().to_string();
    // 19 more self-reported promotions of the same self-stored blob, each under a fresh commit sha
    for i in 0..19 {
        let sha = format!("{:040x}", 0xc0ffee_u64 + i);
        let out = h.kernel_call("lineage.append", json!({"kind": "promotion", "idem": format!("inflate-{i}"), "data": {"sha": sha,
            "from_sha": "a".repeat(40), "gate_job": format!("g-{i:06}"), "scope": "body", "lkg": true, "body_binary": blob, "ext_set": null, "components": []}}));
        assert_eq!(out["ok"], true, "a promotion append was refused, which would close this gap: {out}");
    }
    std::thread::sleep(Duration::from_millis(1500)); // R7 evaluates after every record
    let lvl: Value = serde_json::from_slice(&h.run(&["ledger"]).stdout).unwrap_or(Value::Null);
    let ups = h.kinds("ladder").into_iter().filter(|l| l["direction"] == "up").count();
    drop(d);
    assert_eq!((lvl["level"].as_i64(), ups), (Some(0), 0), "self-reported promotions unlocked a level: {lvl}");
    fs::remove_dir_all(&h.dir).unwrap();
}

/// CONTROL / AUTONOMY ESCALATION — a hostile kernel cannot reach the L3-gated capabilities from below L3.
/// `system.swap` and `sealed.family_eval` are refused `root.level` at L0, and each refusal is recorded as a
/// demoting `ceiling_hit{ceiling:"level"}` (the broker's level gate; the fold then demotes, RT-L9).
#[test]
fn rt_l3_only_capabilities_are_refused_below_l3_and_demote() {
    let h = home("l3");
    h.install(&charter(1, ("1.00", "5.00")));
    let d = h.daemon();
    let hits = || h.kinds("ceiling_hit").iter().filter(|d| d["ceiling"] == "level" && d["demotes"] == true).count();
    let check = |cmd: &str, out: &Value| {
        assert_eq!(out["ok"], false, "{cmd} must be refused below L3: {out}");
        assert_eq!(out["error"]["code"], "root.level", "{cmd}: {out}");
    };
    // The first refusal demotes, and the demotion restarts the zone (rollback to the anchor): the fake kernel can be
    // stopped before it logs that call's outcome, so the Root's own record stands in for a lost outcome. The second
    // call waits for the restarted kernel; within 600 s it is the same incident, so nothing restarts again.
    let (before, ready) = (h.fk("call").len(), h.fk("ready").len());
    fs::create_dir_all(h.ih.join("work")).unwrap();
    fs::write(h.ih.join("work/fk-call.tmp"), json!({"cmd": "system.swap", "args": {}}).to_string()).unwrap();
    fs::rename(h.ih.join("work/fk-call.tmp"), h.ih.join("work/fk-call.json")).unwrap();
    wait(15, "the first refusal", || h.fk("call").len() > before || hits() >= 1);
    if let Some(out) = h.fk("call").into_iter().nth(before) {
        check("system.swap", &out);
    }
    wait(30, "the demotion's restart", || h.kinds("zone_start").iter().any(|z| z["reason"] == "demotion") && h.fk("ready").len() > ready);
    check("sealed.family_eval", &h.kernel_call("sealed.family_eval", json!({})));
    wait(10, "the demoting level ceiling_hit", || hits() >= 2);
    drop(d);
    fs::remove_dir_all(&h.dir).unwrap();
}
