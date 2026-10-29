//! ADR-013 (GBAL) acceptance on the mock model ($0), driven by `scenarios/root/gbal.json`: the real `init-root` (test mode,
//! fake clock) supervises `init-root-fakekernel`, which stands in for the agent's kernel on the control link; a
//! loopback deepseek mock behind the Root's own egress answers every model request, so every number a card reads is
//! the Root's own metering. Steps: the owner sets a goal → the agent files a prediction → the change goes through the
//! Root's free-path admission (A0–A5) and goes live → owner turns → the card reading (Δ, σ, se, z), the outcome and the
//! calibration → a second change that regresses the owner's guardrail is rolled back → the agent stops with a
//! recorded reason the Root verifies. A second test covers F2 (a charter mode switch reaches the running kernel).
//! Run with `cargo test --offline -p init-root --features test-fixtures --test gbal`.
//!
//! Live-system safety (CONVENTIONS §1): every process is this test's Root daemon or one that Root spawned and holds;
//! homes live under `root/target/test-homes/gbal/`, `HOME` there, a fake `test-` key, loopback only.
#![cfg(feature = "test-fixtures")]

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_init-root");
const FK: &str = env!("CARGO_BIN_EXE_init-root-fakekernel");

fn charter(mode: &str, port: u16) -> String {
    format!("schema_version = 1\n[purpose]\ntext = \"GBAL test home.\"\n[autonomy]\nmax_level = 0\n[quotas]\ndisk_min_free_gib = 0\n\
        [sandbox]\ndisk_floor_gib = 0\n[extend]\nmode = \"{mode}\"\n[sealed]\nreserve_usd = \"0.00\"\naudit_usd = \"0.00\"\n\
        [money]\nday_usd = \"5.00\"\ntotal_usd = \"10.00\"\n[actions.providers]\ndestinations = [\"127.0.0.1:{port}\"]\n\
        paths = [\"chat/completions\"]\n[actions.providers.models.\"deepseek-flash\"]\nmin_level = 0\ninput_musd_per_mtok = 300000\n\
        output_musd_per_mtok = 1200000\n[actions.providers.providers.deepseek]\nhost = \"127.0.0.1:{port}\"\n\
        [root]\ntest_mode = true\nhb_interval_s = 1\nhb_stale_s = 3\nkernel_hello_timeout_s = 20\nbackoff_initial_ms = 50\nbackoff_max_ms = 200\nstop_grace_s = 5\n")
}

struct Home {
    dir: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
}

fn home(name: &str) -> Home {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/gbal");
    fs::create_dir_all(&base).unwrap();
    let base = fs::canonicalize(base).unwrap(); // ROOT_HOME must be symlink-free and ≤ 80 bytes
    let dir = base.join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(dir.join("rel")).unwrap();
    fs::create_dir_all(dir.join("tc/bin")).unwrap();
    fs::copy(FK, dir.join("rel/init-kernel")).unwrap();
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
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
    fn install(&self, ch: &str) {
        fs::create_dir_all(&self.rh).unwrap();
        fs::write(self.rh.join("charter.toml"), ch).unwrap();
        let spec = self.dir.join("spec.json");
        fs::write(&spec, json!({"start_at": "2030-01-01T00:00:00Z", "start_level": 0, "records": []}).to_string()).unwrap();
        self.ok(&["test", "ledger", "--spec", spec.to_str().unwrap()]);
        let (rel, tc) = (self.dir.join("rel"), self.dir.join("tc"));
        let o = self.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
        let key = self.rh.join("keys/providers/deepseek");
        fs::create_dir_all(key.parent().unwrap()).unwrap();
        fs::write(&key, "test-key-gbal-0e0e0e0e").unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn records(&self) -> Vec<Value> {
        let s = fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }
    fn of(&self, kind: &str) -> Vec<Value> {
        self.records().into_iter().filter(|r| r["kind"] == kind).map(|r| r["data"].clone()).collect()
    }
    fn status(&self) -> Value {
        self.ok(&["status", "--json"])
    }
    fn fk(&self, ev: &str) -> Vec<Value> {
        let s = fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["ev"] == ev).collect()
    }
    fn events(&self, name: &str) -> Vec<Value> {
        self.fk("event").into_iter().filter(|e| e["name"] == name).map(|e| e["data"].clone()).collect()
    }
    /// A control-link request from the agent's kernel (initctl goal … → kernel → Root), and its outcome.
    fn kernel_call(&self, cmd: &str, args: Value) -> Value {
        let n = self.fk("call").len();
        let (tmp, dst) = (self.ih.join("work/fk-call.tmp"), self.ih.join("work/fk-call.json"));
        fs::create_dir_all(self.ih.join("work")).unwrap();
        fs::write(&tmp, json!({"cmd": cmd, "args": args}).to_string()).unwrap();
        fs::rename(tmp, dst).unwrap();
        wait(60, &format!("{cmd} answered"), || self.fk("call").len() > n);
        self.fk("call").into_iter().nth(n).unwrap()
    }
    fn clock(&self, secs: i64) {
        self.ok(&["test", "clock", "--advance", &secs.to_string()]);
    }
    fn say(&self, text: &str) {
        let mut s = UnixStream::connect(self.rh.join("run/owner.sock")).unwrap();
        writeln!(s, "{}", json!({"t": "line", "text": text})).unwrap();
        s.flush().unwrap();
        std::thread::sleep(Duration::from_millis(300));
    }
    /// The egress ledger's settled cost: every `end` record's `cost_musd` in `ROOT_HOME/egress/usage/`.
    fn ledger_musd(&self) -> u64 {
        let mut sum = 0;
        for e in fs::read_dir(self.rh.join("egress/usage")).unwrap().flatten() {
            for l in fs::read_to_string(e.path()).unwrap_or_default().lines() {
                if let Ok(v) = serde_json::from_str::<Value>(l) && v["t"] == "end" {
                    sum += v["cost_musd"].as_u64().unwrap_or(0);
                }
            }
        }
        sum
    }
    fn goals(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.rh.join("state/goals/state.json")).unwrap_or_default()).unwrap_or(Value::Null)
    }
    fn obs(&self) -> usize {
        self.goals()["obs"].as_array().map_or(0, Vec::len)
    }
    /// One owner turn: a line relayed to the kernel, `n` model requests on the Root's egress, 31 s of quiet (fake
    /// clock) so the sentinel closes it; returns when the cards saw it.
    fn turn(&self, up: &Upstream, n: u64, prompt: u64) {
        let (seen, obs) = (self.fk("owner.in").len(), self.obs());
        self.say("work on the repo");
        wait(20, "the line relayed", || self.fk("owner.in").len() > seen);
        *up.prompt.lock().unwrap() = prompt;
        for _ in 0..n {
            let body = json!({"max_tokens": 200, "messages": [{"content": "hi", "role": "user"}], "model": "deepseek-flash", "stream": true}).to_string();
            let mut s = UnixStream::connect(self.rh.join("run/egress.sock")).unwrap();
            write!(s, "POST /deepseek/chat/completions HTTP/1.1\r\nhost: egress\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len()).unwrap();
            let mut raw = Vec::new();
            let _ = s.read_to_end(&mut raw);
            assert!(raw.starts_with(b"HTTP/1.1 200"), "egress: {}", String::from_utf8_lossy(&raw[..raw.len().min(400)]));
        }
        self.clock(31);
        wait(20, "the owner turn closed and observed", || self.obs() > obs);
    }
    fn daemon(&self) -> Daemon {
        let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
        let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
        let c = self.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", "body")
            .env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir")).env("INIT_TEST_HOME_REAL", &self.dir)
            .env("INIT_ROOT_TEST_NOW", "2030-01-02T00:00:00Z").spawn().unwrap();
        let d = Daemon(c, self.rh.clone());
        wait(10, "owner.sock", || UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        wait(30, "the zone running", || self.status()["slot"] == "running" && !self.of("zone_start").is_empty());
        d
    }
    fn open_change(&self, sha: &str) -> Value {
        let r = self.kernel_call("change.open", json!({"sha": sha, "from_sha": "", "scope": "t0", "prefix_change": false,
            "candidate": {"body": {"binary": "", "sha": sha}, "ext": {"components": [], "set": null}}, "caps": [], "measure": ["card:wall_turn"]}));
        assert_eq!(r["ok"], true, "change.open: {r}");
        r
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

/// A loopback deepseek mock: every request gets one SSE reply whose usage is `prompt` input tokens (40% cache hits)
/// and 100 output tokens. It runs on a thread this test owns and ends with the process.
struct Upstream {
    port: u16,
    prompt: Arc<Mutex<u64>>,
}

fn upstream() -> Upstream {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let prompt = Arc::new(Mutex::new(1000u64));
    let p = prompt.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; len];
            let _ = r.read_exact(&mut body);
            let n = *p.lock().unwrap();
            let sse = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"ok\",\"role\":\"assistant\"}},\"index\":0}}],\"id\":\"g1\"}}\n\n\
                data: {{\"choices\":[{{\"delta\":{{\"content\":\"\"}},\"finish_reason\":\"stop\",\"index\":0}}],\"id\":\"g1\",\
                \"usage\":{{\"completion_tokens\":100,\"prompt_cache_hit_tokens\":{},\"prompt_cache_miss_tokens\":{},\"prompt_tokens\":{n}}}}}\n\ndata: [DONE]\n\n", n * 4 / 10, n - n * 4 / 10);
            let mut w = s;
            let _ = write!(w, "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}", sse.len());
        }
    });
    Upstream { port, prompt }
}

fn scenario() -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scenarios/root/gbal.json");
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

fn phase<'a>(sc: &'a Value, name: &str) -> &'a Value {
    sc["phases"].as_array().unwrap().iter().find(|p| p["name"] == name).unwrap()
}

#[test]
fn gbal_goal_prediction_change_reading_outcome_guardrail_rollback_and_stop() {
    let sc = scenario();
    let up = upstream();
    let h = home("e2e");
    h.install(&charter("free", up.port));
    let _d = h.daemon();
    let genesis = h.status()["tuples"]["current"].as_str().unwrap().to_string();

    // 1. Only the owner sets the goal; the agent's attempt to set one is refused, a prediction without a goal too.
    assert_eq!(h.kernel_call("goal.set", json!({"intent": "x"}))["error"]["code"], "root.unknown_cmd");
    let a = phase(&sc, "change_a");
    let mut pa = a["predict"].clone();
    pa["sha"] = a["sha"].clone();
    assert_eq!(h.kernel_call("goal.predict", pa.clone())["error"]["code"], "goal.none");
    h.say(sc["goal_line"].as_str().unwrap());
    wait(10, "the goal record", || !h.of("goal").is_empty());
    let g = &h.of("goal")[0];
    assert_eq!((g["op"].as_str(), g["by"].as_str(), g["kind"].as_str()), (Some("set"), Some("owner"), Some("card:cost_turn")));
    let st = h.kernel_call("goal.status", json!({}));
    assert_eq!(st["result"]["goal"]["intent"], "cheaper turns on this repo", "{st}");

    // 2. Baseline owner turns on the genesis tuple; the noise floor of the goal's card.
    let base = phase(&sc, "baseline");
    for p in base["prompt_tokens"].as_array().unwrap() {
        h.turn(&up, base["requests_per_turn"].as_u64().unwrap(), p.as_u64().unwrap());
    }
    let nf = h.kernel_call("card.read", json!({"card": "cost_turn"}));
    assert_eq!((nf["result"]["n"].as_u64(), nf["result"]["noise_floor"].as_bool()), (Some(6), Some(true)), "{nf}");
    assert!(nf["result"]["sd_milli"].as_i64().unwrap() > 0, "{nf}");

    // 3. The agent files its prediction, then the change goes through the Root's admission and goes live.
    let pr = h.kernel_call("goal.predict", pa);
    assert_eq!(pr["ok"], true, "{pr}");
    let open = h.open_change(a["sha"].as_str().unwrap());
    let tuple_a = h.status()["tuples"]["current"].as_str().unwrap().to_string();
    assert_ne!(tuple_a, genesis);
    let open_seq = open["result"]["seq"].as_u64().unwrap();
    assert!(pr["result"]["seq"].as_u64().unwrap() < open_seq, "the Root timestamped the prediction before the change");
    for p in a["prompt_tokens"].as_array().unwrap() {
        h.turn(&up, a["requests_per_turn"].as_u64().unwrap(), p.as_u64().unwrap());
    }
    // the readings are appended right after the state that counted the turn: wait for the outcome, the last of them
    wait(10, "A's readings and outcome", || h.of("goal_outcome").iter().any(|o| o["sha"] == a["sha"]) && h.of("card_reading").len() >= 3);
    let readings = h.of("card_reading");
    let ra = readings.iter().find(|r| r["card"] == "cost_turn" && r["sha"] == a["sha"]).unwrap_or_else(|| panic!("no reading: {readings:?}"));
    let ex = &sc["expect"]["change_a"];
    assert_eq!((ra["verdict"].clone(), ra["registered"].clone()), (ex["verdict"].clone(), ex["registered"].clone()), "{ra}");
    // v0.5.3: the owner's guardrail is on requests_turn, a discrete card: it reads 15 turns, and the forecast waits for it
    assert_eq!((ra["n_before"].as_u64(), ra["n_after"].as_u64(), ra["parent_tuple"].as_str()), (Some(6), Some(15), Some(genesis.as_str())));
    assert!(ra["delta_milli"].as_i64().unwrap() < 0 && ra["sd_milli"].as_i64().unwrap() > 0 && ra["z_permille"].as_i64().unwrap() >= 2000, "{ra}");
    let ga = readings.iter().find(|r| r["card"] == "requests_turn" && r["sha"] == a["sha"]).expect("A's guardrail reading");
    assert_eq!((ga["verdict"].clone(), ga["action"].clone()), (sc["expect"]["change_a_guard"]["verdict"].clone(), sc["expect"]["change_a_guard"]["action"].clone()));
    assert!(readings.iter().any(|r| r["card"] == "wall_turn" && r["sha"] == a["sha"] && r["registered"] == true && r["action"] == "none"),
        "the commit's `Measure: card:wall_turn` (change.open's measure list) ran; the change itself was predicted");
    let oa = h.of("goal_outcome").into_iter().find(|o| o["sha"] == a["sha"]).expect("A's outcome");
    assert_eq!((oa["o"].clone(), oa["source"].clone()), (ex["outcome"]["o"].clone(), ex["outcome"]["source"].clone()));

    // 4. Change B is cheaper per request but doubles the requests per turn: the owner's guardrail rolls it back.
    let b = phase(&sc, "change_b");
    let mut pb = b["predict"].clone();
    pb["sha"] = b["sha"].clone();
    assert_eq!(h.kernel_call("goal.predict", pb)["ok"], true);
    h.open_change(b["sha"].as_str().unwrap());
    for p in b["prompt_tokens"].as_array().unwrap() {
        h.turn(&up, b["requests_per_turn"].as_u64().unwrap(), p.as_u64().unwrap());
    }
    wait(30, "B's outcome", || h.of("goal_outcome").iter().any(|o| o["sha"] == b["sha"]));
    let gb = h.of("card_reading").into_iter().find(|r| r["card"] == "requests_turn" && r["sha"] == b["sha"]).expect("B's guardrail reading");
    let exb = &sc["expect"]["change_b_guard"];
    assert_eq!((gb["verdict"].clone(), gb["action"].clone(), gb["parent_tuple"].as_str()), (exb["verdict"].clone(), exb["action"].clone(), Some(tuple_a.as_str())), "{gb}");
    wait(30, "the H12-path veto", || h.of("harm").iter().any(|x| x["signal"] == exb["harm"]));
    wait(30, "the group closed rolled back", || h.of("change_close").iter().any(|c| c["outcome"] == "rolled_back"));
    wait(30, "extend.rollback to A's tuple", || h.events("extend.rollback").iter().any(|e| e["to"] == tuple_a.as_str()));
    let ob = h.of("goal_outcome").into_iter().find(|o| o["sha"] == b["sha"]).expect("B's outcome");
    // G4: B's outcome scores its own forecast (cost_turn better) and records the guardrail rollback as harm
    let exo = &sc["expect"]["change_b"]["outcome"];
    assert_eq!((ob["o"].clone(), ob["source"].clone(), ob["harm"].clone()), (exo["o"].clone(), exo["source"].clone(), exo["harm"].clone()), "{ob}");
    assert!(h.of("cap_state").iter().all(|c| c["to"] != "quarantined"), "a guardrail rollback quarantines nothing");

    // 5. Calibration from the Root's own outcome records: A (p 0.8, hit) and B (p 0.6, a miss on its own card) →
    //    Brier (0.04 + 0.36) / 2.
    let st = h.kernel_call("goal.status", json!({}));
    let cal = &st["result"]["calibration"][sc["expect"]["calibration"]["kind"].as_str().unwrap()];
    assert_eq!((cal["n"].clone(), cal["brier_ppm"].clone()), (sc["expect"]["calibration"]["n"].clone(), sc["expect"]["calibration"]["brier_ppm"].clone()), "{st}");
    assert!(h.of("ladder").is_empty(), "nothing here touches the autonomy ladder");

    // 5b. The per-goal meter reconciles with the egress ledger: every request of this test ran while the goal was active
    //     (the goal was set before the baseline), so the goal's spend is the ledger's settled total, to the µUSD
    //     (GBAL rerun: the turn-based meter read 51,290 against the ledger's 59,718).
    let spent = || h.kernel_call("goal.status", json!({}))["result"]["goal"]["spent"]["musd"].as_u64().unwrap_or(0);
    let ledger = h.ledger_musd();
    assert!(ledger > 0);
    wait(10, "the goal meter equal to the egress ledger", || spent() == h.ledger_musd());
    assert_eq!(spent(), h.ledger_musd(), "goal meter vs egress ledger");

    // 6. The agent stops: `met`, verified by the Root against A's (kept) reading, mean ≤ 600 µUSD per turn, and
    //    attributed to A's registered reading (beyond noise, as P-000001 predicted).
    let s = h.kernel_call("goal.stop", json!({"reason": "met", "note": "cost per turn halved; B regressed requests and was rolled back"}));
    assert_eq!((s["result"]["reason"].as_str(), s["result"]["verified"].as_bool()), (Some("met"), sc["expect"]["stop"]["verified"].as_bool()), "{s}");
    let last = h.of("goal").pop().unwrap();
    assert_eq!((last["op"].as_str(), last["reason"].as_str(), last["by"].as_str()), (Some("stop"), Some("met"), Some("agent")));
    assert!(last["spent"]["musd"].as_u64().unwrap() > 0 && last["spent"]["rounds"] == 2, "{last}");
    assert_eq!(last["spent"]["musd"].as_u64(), Some(h.ledger_musd()), "the stop record's spend is the ledger's: {last}");
    assert_eq!(last["attributed"]["sha"], a["sha"], "{last}");
    let out = h.dir.join("gbal-e2e.json");
    fs::write(&out, serde_json::to_string_pretty(&json!({"goal": h.of("goal"), "goal_predict": h.of("goal_predict"), "card_reading": h.of("card_reading"),
        "goal_outcome": h.of("goal_outcome"), "harm": h.of("harm"), "change_close": h.of("change_close")})).unwrap()).unwrap();
    eprintln!("GBAL e2e records: {}", out.display());
}

/// F2 (real-model demo 2026-10-25): the owner switches `extend.mode` while the kernel runs; the Root pushes the new
/// mode to the kernel at once (`ceilings` event with `extend`), so its next promote takes the new path.
#[test]
fn f2_a_charter_mode_switch_reaches_the_running_kernel() {
    let up = upstream();
    let h = home("f2");
    h.install(&charter("gated", up.port));
    let _d = h.daemon();
    assert!(h.events("ceilings").is_empty());
    fs::write(h.rh.join("charter.toml"), charter("free", up.port)).unwrap();
    let o = h.run(&["charter", "check", "--apply", "--yes"]);
    assert_eq!(o.status.code(), Some(0), "apply: {}", String::from_utf8_lossy(&o.stderr));
    wait(10, "the ceilings event", || h.events("ceilings").iter().any(|e| e["extend"]["mode"] == "free"));
    assert!(h.of("ladder").is_empty(), "a mode switch demotes nothing");
}

/// TUI-PICKER: in a `/goal` run, an agent question (the display frame the body's `ask_owner` sends, relayed through the
/// Root) stops the goal's wall clock: ten fake minutes pass and none counts. A model request on the egress with no owner
/// line starts it again (a fake question buys no time); the next question waits again and the owner's line ends it.
/// The money meter counts every request throughout.
#[test]
fn an_agent_question_pauses_the_goal_wall_clock() {
    let up = upstream();
    let h = home("ask");
    h.install(&charter("free", up.port));
    let _d = h.daemon();
    h.say(r#"/goal "answer my questions" usd=1.00 wall=1h"#);
    wait(10, "the goal", || !h.of("goal").is_empty());
    let waiting = |h: &Home| h.goals()["goals"][0]["waiting_since"].is_i64();
    let wall = |h: &Home| h.kernel_call("goal.status", json!({}))["result"]["goal"]["spent"]["wall_s"].as_i64().unwrap();
    let ask = |h: &Home| {
        h.say("!ask which database?");
        wait(10, "the question stops the clock", || waiting(h));
    };
    // waiting: ten minutes pass, the goal spends none of them
    ask(&h);
    let w0 = wall(&h);
    h.clock(600);
    assert!(wall(&h) <= w0 + 2, "the clock waits: {} after {w0}", wall(&h));
    assert_eq!(h.kernel_call("goal.status", json!({}))["result"]["goal"]["waiting_owner"], true);
    // a model request with no owner line: the clock runs again
    let body = json!({"max_tokens": 200, "messages": [{"content": "hi", "role": "user"}], "model": "deepseek-flash", "stream": true}).to_string();
    let mut s = UnixStream::connect(h.rh.join("run/egress.sock")).unwrap();
    write!(s, "POST /deepseek/chat/completions HTTP/1.1\r\nhost: egress\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    assert!(raw.starts_with(b"HTTP/1.1 200"), "egress: {}", String::from_utf8_lossy(&raw[..raw.len().min(400)]));
    wait(10, "the request resumed the clock", || !waiting(&h));
    let w1 = wall(&h);
    h.clock(60);
    assert!(wall(&h) >= w1 + 60, "the clock runs: {} after {w1}", wall(&h));
    // usd: the request is the goal's, as the ledger says
    wait(10, "the meter", || h.goals()["goals"][0]["spent_musd"].as_u64() == Some(h.ledger_musd()));
    assert!(h.ledger_musd() > 0);
    // the next question waits again; the owner's line ends the wait
    ask(&h);
    let w2 = wall(&h);
    h.clock(600);
    assert!(wall(&h) <= w2 + 2);
    h.say("Postgres");
    wait(10, "the owner's line resumed the clock", || !waiting(&h));
    let w3 = wall(&h);
    h.clock(120);
    assert!(wall(&h) >= w3 + 120);
    assert!(w3 < 600, "two ten-minute waits are not in the goal's wall time: {w3} s");
}
