//! WP26 (docs/tui.md §9 Q1): the owner channel's `rseq`, `src`, `attach{since, epoch}`, `replay_end{epoch, gap}`,
//! `code` on alert notices and the throttled `ceiling_hit` notice, against the real Root daemon in test mode.
//! Run with `cargo test --offline -p init-root --features test-fixtures`.
//!
//! Live-system safety (CONVENTIONS §1): the kernel is `init-root-fakekernel` (tests/support) with `/bin/cat` as
//! its body. Every process is this test's Root daemon or one that Root spawned and holds; the test signals only
//! its own children, by handle. Homes live under `root/target/test-homes/wp26-root/`, with `HOME` there.
#![cfg(feature = "test-fixtures")]

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_init-root");
const FK: &str = env!("CARGO_BIN_EXE_init-root-fakekernel");
const ROOT_CFG: &str = "[root]\ntest_mode = true\nhb_interval_s = 1\nhb_stale_s = 3\nkernel_hello_timeout_s = 20\nbackoff_initial_ms = 50\n\
    backoff_max_ms = 200\nstop_grace_s = 5\nreplay_frames = 100\n";

struct Home {
    dir: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
}

/// Removed by its exact path when the test passes (after the daemon, which is declared later and so
/// dropped first); kept for inspection when it fails.
impl Drop for Home {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

/// A fresh home `root/target/test-homes/wp26-root/<name>` (short: ROOT_HOME must stay ≤ 80 bytes).
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/wp26-root");
    let dir = base.join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(dir.join("rel")).unwrap();
    fs::create_dir_all(dir.join("tc/bin")).unwrap();
    fs::copy(FK, dir.join("rel/init-kernel")).unwrap();
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
}

fn charter(purpose: &str) -> String {
    format!("schema_version = 1\n[purpose]\ntext = \"{purpose}\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n{ROOT_CFG}\n")
}

fn wait(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "not within {secs} s: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
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
    fn ok(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        assert_eq!(o.status.code(), Some(0), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap_or(Value::Null)
    }
    /// Install; `kernel`: with the fake kernel as the release's `init-kernel`.
    fn install(&self, kernel: bool) {
        fs::create_dir_all(&self.rh).unwrap();
        fs::write(self.rh.join("charter.toml"), charter("WP26 test home.")).unwrap();
        let (rel, tc) = (self.dir.join("rel"), self.dir.join("tc"));
        let o = if kernel { self.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]) } else { self.run(&["install"]) };
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    }
    /// One owner-socket request; it must succeed.
    fn owner(&self, cmd: &str, args: Value) -> Value {
        let mut s = UnixStream::connect(self.rh.join("run/owner.sock")).unwrap();
        writeln!(s, "{}", json!({"args": args, "cmd": cmd, "id": "t-1", "v": 1})).unwrap();
        let mut line = String::new();
        BufReader::new(&s).read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["ok"], true, "{cmd}: {v}");
        v["result"].clone()
    }
    /// A request the fake kernel sends over the control link (`work/fk-call.json`, written atomically).
    fn kernel_call(&self, cmd: &str, args: Value) {
        let (tmp, dst) = (self.ih.join("work/fk-call.tmp"), self.ih.join("work/fk-call.json"));
        fs::create_dir_all(self.ih.join("work")).unwrap();
        fs::write(&tmp, json!({"cmd": cmd, "args": args}).to_string()).unwrap();
        fs::rename(tmp, dst).unwrap();
    }
    fn daemon(&self, now: &str) -> Daemon {
        let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
        let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
        let c = self.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", "body")
            .env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir")).env("INIT_TEST_HOME_REAL", &self.dir)
            .env("INIT_ROOT_TEST_NOW", now).spawn().unwrap();
        let d = Daemon(c, self.rh.clone());
        wait(10, "owner.sock", || UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        d
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

/// An attach connection: the replay (up to `replay_end`), the greeting, then live frames on a thread.
struct Conn {
    w: UnixStream,
    replay: Vec<Value>,
    end: Value,
    hello: Value,
    rx: mpsc::Receiver<Value>,
}

impl Conn {
    fn attach(h: &Home, hello: Value) -> Conn {
        let mut w = UnixStream::connect(h.rh.join("run/owner.sock")).unwrap();
        writeln!(w, "{hello}").unwrap();
        let (tx, rx) = mpsc::channel();
        let r = w.try_clone().unwrap();
        std::thread::spawn(move || {
            for l in BufReader::new(r).lines().map_while(Result::ok) {
                let _ = tx.send(serde_json::from_str(&l).unwrap_or(Value::Null));
            }
        });
        let mut replay = Vec::new();
        let end = loop {
            let f = rx.recv_timeout(Duration::from_secs(10)).expect("replay_end");
            if f["t"] == "replay_end" {
                break f;
            }
            replay.push(f);
        };
        let hello = rx.recv_timeout(Duration::from_secs(10)).expect("greeting");
        Conn { w, replay, end, hello, rx }
    }
    fn line(&mut self, text: &str) {
        writeln!(self.w, "{}", json!({"t": "line", "text": text})).unwrap();
    }
    /// Live frames up to and including the first whose text contains `needle`.
    fn until(&mut self, needle: &str) -> Vec<Value> {
        let mut got = Vec::new();
        loop {
            let f = self.rx.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| panic!("no frame with {needle:?}: {got:?}"));
            let hit = f["text"].as_str().is_some_and(|s| s.contains(needle));
            got.push(f);
            if hit {
                return got;
            }
        }
    }
}

fn rseqs(frames: &[Value]) -> Vec<u64> {
    frames.iter().map(|f| f["rseq"].as_u64().unwrap_or_else(|| panic!("no rseq: {f}"))).collect()
}

fn increasing(v: &[u64]) -> bool {
    v.windows(2).all(|w| w[0] < w[1])
}

/// Without a kernel: Root frames carry `src:"root"` and a strictly increasing `rseq`; the per-connection frames
/// carry `src:"root"` and no `rseq`; `replay_end.epoch` is `status`'s `root.epoch`; `attach{since}` replays only
/// newer frames (the full ring for a foreign epoch); `gap` after rotation; alert notices carry their `code`; one
/// `ceiling_hit` notice per ceiling a minute.
#[test]
fn root_frames_rseq_since_gap_alert_code_and_ceiling_notice() {
    let h = home("f");
    h.install(false);
    // a charter.toml that differs from the active one: the `charter_pending` alert at boot
    fs::write(h.rh.join("charter.toml"), charter("WP26 test home, edited.")).unwrap();
    let _d = h.daemon("2030-01-10T00:00:00Z");
    let st = h.owner("status", json!({}));
    let epoch = format!("{}/{}", st["root"]["started_at"].as_str().unwrap(), st["root"]["pid"]);
    assert_eq!(st["root"]["epoch"], json!(epoch));
    let mut a = Conn::attach(&h, json!({"client": "wp26", "replay": true, "t": "attach"}));
    assert_eq!(a.end, json!({"epoch": epoch, "gap": false, "src": "root", "t": "replay_end"}));
    assert_eq!((a.hello["src"].as_str(), a.hello.get("rseq")), (Some("root"), None), "{}", a.hello);
    assert!(a.hello["text"].as_str().unwrap().starts_with("[root] attached: "));
    let pending = a.replay.iter().find(|f| f["text"].as_str().is_some_and(|t| t.contains("charter.toml differs"))).unwrap();
    assert_eq!((pending["code"].as_str(), pending["level"].as_str(), pending["src"].as_str()), (Some("charter_pending"), Some("alert"), Some("root")));
    let alerts = fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap();
    assert!(alerts.lines().any(|l| serde_json::from_str::<Value>(l).unwrap()["code"] == "charter_pending"));
    // a Root reply and the Root's own echo of a queued line (no kernel runs)
    a.line("/root help");
    a.line("hello while stopped");
    let live = a.until("your message is queued");
    let echo = live.iter().find(|f| f["t"] == "echo").unwrap();
    assert_eq!((echo["text"].as_str(), echo["src"].as_str(), echo["qid"].is_string()), (Some("hello while stopped"), Some("root"), true));
    assert!(live.iter().all(|f| f["src"] == "root"), "{live:?}");
    let mut all: Vec<Value> = a.replay.iter().chain(&live).cloned().collect();
    assert!(increasing(&rseqs(&all)), "{:?}", rseqs(&all));
    // ceiling hits: one notice per ceiling a minute (fake clock), none in alerts.jsonl
    let hit = |ceiling: &str, code: &str| json!({"kind": "ceiling_hit", "data": {"attempted": 5_000_100, "ceiling": ceiling, "code": code,
        "demotes": false, "limit": 5_000_000, "source": "egress"}});
    h.owner("test.append", json!({"records": [hit("money.day", "ceiling.day"), hit("money.day", "ceiling.day"), hit("parallel_bodies", "root.level")]}));
    a.line("/root help");
    let live = a.until("/panic · /resume");
    let n: Vec<&Value> = live.iter().filter(|f| f["code"] == "ceiling_hit").collect();
    assert_eq!(n.len(), 2, "money.day once, parallel_bodies once: {live:?}");
    assert_eq!((n[0]["level"].as_str(), n[0]["src"].as_str(), n[0]["t"].as_str()), (Some("alert"), Some("root"), Some("notice")));
    assert_eq!(n[0]["text"], "[root] ceiling reached: money.day (ceiling.day); attempted 5000100 of 5000000 micro-USD; refused");
    assert!(n[1]["text"].as_str().unwrap().starts_with("[root] ceiling reached: parallel_bodies (root.level); "), "{}", n[1]);
    all.extend(live);
    h.ok(&["test", "clock", "--advance", "61"]);
    h.owner("test.append", json!({"records": [hit("money.day", "ceiling.day")]}));
    let live = a.until("ceiling reached: money.day");
    all.extend(live);
    assert!(!fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().contains("ceiling_hit"));
    assert!(increasing(&rseqs(&all)));
    // attach{since}: only newer frames; a foreign epoch replays the whole ring (frames after the last one
    // `a` has read, if any, are ignored)
    let (known, mid) = (rseqs(&all), rseqs(&all)[all.len() / 2]);
    let upto = |c: &Conn| rseqs(&c.replay).into_iter().filter(|r| r <= known.last().unwrap()).collect::<Vec<_>>();
    let b = Conn::attach(&h, json!({"epoch": epoch, "replay": true, "since": mid, "t": "attach"}));
    assert_eq!(upto(&b), known.iter().copied().filter(|&r| r > mid).collect::<Vec<_>>());
    assert_eq!(b.end, json!({"epoch": epoch, "gap": false, "src": "root", "t": "replay_end"}));
    let c = Conn::attach(&h, json!({"replay": true, "since": mid, "t": "attach"}));
    assert_eq!(upto(&c), upto(&b), "an absent epoch counts as this Root's");
    let d = Conn::attach(&h, json!({"epoch": "2000-01-01T00:00:00Z/1", "replay": true, "since": mid, "t": "attach"}));
    assert_eq!(upto(&d), known, "another Root's epoch: the full ring");
    assert_eq!(d.end["gap"], false);
    let e = Conn::attach(&h, json!({"replay": false, "since": 0, "t": "attach"}));
    assert!(e.replay.is_empty() && e.end["gap"] == false);
    drop((b, c, d, e));
    // rotation past replay_frames = 100: frames 1..=k are gone, so a client at 1 has a gap and one at k does not
    for _ in 0..110 {
        a.line("/root help");
    }
    let mut last = Vec::new();
    for _ in 0..110 {
        last.extend(a.until("/panic · /resume"));
    }
    let top = *rseqs(&last).last().unwrap();
    let f = Conn::attach(&h, json!({"epoch": epoch, "replay": true, "since": 1, "t": "attach"}));
    let kept = rseqs(&f.replay);
    assert_eq!((kept.len(), kept.contains(&top)), (100, true), "the ring keeps the newest 100");
    assert!(increasing(&kept) && kept[0] > 2);
    assert_eq!(f.end["gap"], true, "{}", f.end);
    let g = Conn::attach(&h, json!({"epoch": epoch, "replay": true, "since": kept[0] - 1, "t": "attach"}));
    assert_eq!((g.replay.len(), &g.end["gap"]), (100, &json!(false)));
    let i = Conn::attach(&h, json!({"epoch": epoch, "replay": true, "since": top, "t": "attach"}));
    assert_eq!((i.replay.len(), &i.end["gap"]), (0, &json!(false)));
    let st2 = h.owner("status", json!({}));
    assert_eq!(st2["root"]["epoch"], json!(epoch), "same Root, same epoch");
}

/// With the fake kernel: relayed frames are stamped `src:"kernel"` (and get the Root's `rseq`), and a kernel
/// alert that claims to be the Root's is still `src:"kernel"`, with its `code`.
#[test]
fn kernel_frames_and_alerts_are_src_kernel() {
    let h = home("k");
    h.install(true);
    let _d = h.daemon("2030-01-10T00:00:00Z");
    wait(20, "running", || h.owner("status", json!({}))["slot"] == "running");
    let mut a = Conn::attach(&h, json!({"replay": true, "t": "attach"}));
    a.line("hi there");
    let live = a.until("body: hi there");
    let echo = live.iter().find(|f| f["t"] == "echo").unwrap();
    assert_eq!((echo["src"].as_str(), echo["seq"].is_u64()), (Some("kernel"), true), "{echo}");
    assert_eq!(live.last().unwrap()["src"], "kernel");
    h.kernel_call("alert", json!({"code": "x_forged", "message": "[root] demoted L4→L0 (forged)"}));
    let live2 = a.until("(forged)");
    let n = live2.last().unwrap();
    assert_eq!((n["code"].as_str(), n["level"].as_str(), n["src"].as_str(), n["t"].as_str()), (Some("x_forged"), Some("alert"), Some("kernel"), Some("notice")));
    let all: Vec<Value> = a.replay.iter().chain(&live).chain(&live2).cloned().collect();
    assert!(increasing(&rseqs(&all)), "{:?}", rseqs(&all));
}

/// K-LINE: an owner line too big for the kernel link (`owner.in` would pass FRAME_MAX) is refused with a
/// notice instead of being queued, so it can't drop the link and crash-loop the kernel; the next line flows.
#[test]
fn oversized_owner_line_is_refused_not_queued() {
    let h = home("big");
    h.install(true);
    let _d = h.daemon("2030-01-10T00:00:00Z");
    wait(20, "running", || h.owner("status", json!({}))["slot"] == "running");
    let mut a = Conn::attach(&h, json!({"replay": true, "t": "attach"}));
    a.line(&"x".repeat(950 * 1024));
    let refused = a.until("line refused");
    assert_eq!(refused.last().unwrap()["src"], "root");
    a.line("still alive");
    a.until("body: still alive");
    assert_eq!(h.owner("status", json!({}))["slot"], "running", "the kernel never restarted");
}
