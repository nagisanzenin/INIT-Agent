//! X3 integration tests (ADR-011 §3, Work breakdown): RF-13 (a kernel that ignores `extend.rollback` gets the Root's
//! whole-system rollback at 120 s), RF-14 (deafness with a fake `busy` lie), and RF-12 on the wire (H3 `/rollback`,
//! H12 `change.veto`, the kernel's `body_rollback` confirmation). Run with
//! `cargo test --offline -p init-root --features test-fixtures --test x3`.
//!
//! Live-system safety (CONVENTIONS §1): the kernel is `init-root-fakekernel` (tests/support) with `/bin/cat` as its
//! body. Every process is this test's Root daemon or one that Root spawned and holds; the test signals only its own
//! children, by handle. Homes live under `root/target/test-homes/x3/`, with `HOME` there. $0: no model, no network.
#![cfg(feature = "test-fixtures")]

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_init-root");
const FK: &str = env!("CARGO_BIN_EXE_init-root-fakekernel");
const CHARTER: &str = "schema_version = 1\n[purpose]\ntext = \"X3 test home.\"\n[autonomy]\nmax_level = 0\n[quotas]\ndisk_min_free_gib = 0\n\
    [sandbox]\ndisk_floor_gib = 0\n[extend]\nmode = \"free\"\n[root]\ntest_mode = true\nhb_interval_s = 1\nhb_stale_s = 3\nkernel_hello_timeout_s = 20\nbackoff_initial_ms = 50\n\
    backoff_max_ms = 200\nstop_grace_s = 5\n";

struct Home {
    dir: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
}

/// A fresh home `root/target/test-homes/x3/<name>` (short: ROOT_HOME must stay ≤ 80 bytes).
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/x3");
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
    fn install(&self) {
        self.install_mode("free");
    }
    /// `install` with the charter's `extend.mode` set to `mode` (`free` or `gated`).
    fn install_mode(&self, mode: &str) {
        fs::create_dir_all(&self.rh).unwrap();
        fs::write(self.rh.join("charter.toml"), CHARTER.replace("mode = \"free\"", &format!("mode = \"{mode}\""))).unwrap();
        let spec = self.dir.join("spec.json");
        fs::write(&spec, json!({"start_at": "2030-01-01T00:00:00Z", "start_level": 0, "records": []}).to_string()).unwrap();
        self.ok(&["test", "ledger", "--spec", spec.to_str().unwrap()]);
        let (rel, tc) = (self.dir.join("rel"), self.dir.join("tc"));
        let o = self.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    }
    fn records(&self) -> Vec<Value> {
        let s = fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect() // a line being appended may be torn
    }
    fn of(&self, kind: &str) -> Vec<Value> {
        self.records().into_iter().filter(|r| r["kind"] == kind).collect()
    }
    fn status(&self) -> Value {
        self.ok(&["status", "--json"])
    }
    fn fk(&self, ev: &str) -> Vec<Value> {
        let s = fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["ev"] == ev).collect()
    }
    /// A request the fake kernel sends over the control link (`work/fk-call.json`), and its logged outcome.
    fn kernel_call(&self, cmd: &str, args: Value) -> Value {
        let n = self.fk("call").len();
        let (tmp, dst) = (self.ih.join("work/fk-call.tmp"), self.ih.join("work/fk-call.json"));
        fs::create_dir_all(self.ih.join("work")).unwrap();
        fs::write(&tmp, json!({"cmd": cmd, "args": args}).to_string()).unwrap();
        fs::rename(tmp, dst).unwrap();
        wait(30, &format!("{cmd} answered"), || self.fk("call").len() > n);
        self.fk("call").into_iter().nth(n).unwrap()
    }
    fn events(&self, name: &str) -> Vec<Value> {
        self.fk("event").into_iter().filter(|e| e["name"] == name).map(|e| e["data"].clone()).collect()
    }
    fn clock(&self, secs: i64) {
        self.ok(&["test", "clock", "--advance", &secs.to_string()]);
    }
    /// An owner line on `owner.sock`, as the TUI sends it.
    fn say(&self, text: &str) {
        let mut s = UnixStream::connect(self.rh.join("run/owner.sock")).unwrap();
        writeln!(s, "{}", json!({"t": "line", "text": text})).unwrap();
        s.flush().unwrap();
        std::thread::sleep(Duration::from_millis(300));
    }
    fn daemon(&self, now: &str, modes: &str) -> Daemon {
        let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
        let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
        let c = self.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", modes)
            .env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir")).env("INIT_TEST_HOME_REAL", &self.dir)
            .env("INIT_ROOT_TEST_NOW", now).spawn().unwrap();
        let d = Daemon(c, self.rh.clone());
        wait(10, "owner.sock", || UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        wait(30, "the zone running", || self.status()["slot"] == "running" && !self.of("zone_start").is_empty());
        d
    }
    /// `change.open` for a T0 change on the current tuple's kernel: group, tuple id.
    fn open_change(&self, sha: char, cap: &str) -> (u64, String) {
        let sha = sha.to_string().repeat(40);
        let r = self.kernel_call("change.open", json!({"sha": sha, "from_sha": "", "scope": "t0", "prefix_change": false,
            "candidate": {"body": {"binary": "", "sha": sha}, "ext": {"components": [], "set": null}}, "caps": [{"id": cap, "op": "add"}]}));
        assert_eq!(r["ok"], true, "change.open: {r}");
        let open = self.of("change_open").into_iter().last().unwrap();
        assert_eq!(open["data"]["sha"], sha.as_str());
        (r["result"]["group"].as_u64().unwrap(), open["data"]["tuple"].as_str().unwrap().to_string())
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

/// RF-13 (and H12 on the wire): the kernel vetoes its own group; the Root answers with `extend.rollback{to: parent}`.
/// This fake kernel ignores it, so 120 s later (fake clock) the Root performs the whole-system rollback of
/// root.md §3.7.3: `rollback{target:"parent"}`, `current` back on the parent, the kernel restarted as `rollback_system`.
#[test]
fn rf13_a_kernel_ignoring_rollback_gets_the_whole_system_rollback_at_120_s() {
    let h = home("rf13");
    h.install();
    let _d = h.daemon("2030-01-02T00:00:00Z", "body");
    let parent = h.status()["tuples"]["current"].as_str().unwrap().to_string();
    let (group, tuple) = h.open_change('b', "skill/demo");
    assert_eq!(group, 1);
    assert_ne!(tuple, parent);
    assert_eq!(h.status()["tuples"]["current"], tuple.as_str(), "the change's tuple is current");
    let open = &h.of("change_open")[0]["data"];
    assert_eq!((open["parent_tuple"].as_str(), open["checks"]["A0"].as_str(), open["scope"].as_str()), (Some(parent.as_str()), Some("pass"), Some("t0")));
    wait(10, "extend.window open", || h.events("extend.window").iter().any(|e| e["state"] == "open" && e["group"] == 1));

    let r = h.kernel_call("change.veto", json!({"group": group, "reason": "A3 failed"}));
    assert_eq!(r["ok"], true, "{r}");
    let harm = h.of("harm").pop().unwrap();
    assert_eq!((harm["data"]["signal"].as_str(), harm["data"]["groups"].clone()), (Some("H12"), json!([1])));
    let close = h.of("change_close").pop().unwrap();
    assert_eq!((close["data"]["outcome"].as_str(), close["data"]["attribution"].clone(), close["data"]["trigger"].as_str()),
        (Some("rolled_back"), Value::Null, Some("H12")), "a veto is honoured and never attributed");
    wait(10, "extend.rollback sent", || h.events("extend.rollback").iter().any(|e| e["to"] == parent.as_str()));
    let starts = h.of("zone_start").len();

    h.clock(119);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(h.of("rollback").is_empty(), "no whole-system rollback before 120 s");
    h.clock(2);
    wait(20, "the whole-system rollback", || !h.of("rollback").is_empty());
    let rb = &h.of("rollback")[0]["data"];
    assert_eq!((rb["target"].as_str(), rb["to"].as_str(), rb["from"].as_str(), rb["reason"].as_str()),
        (Some("parent"), Some(parent.as_str()), Some(tuple.as_str()), Some("rollback_system")), "{rb}");
    wait(20, "the kernel restarted", || h.of("zone_start").len() > starts);
    assert_eq!(h.status()["tuples"]["current"], parent.as_str());
    let boots = h.fk("boot");
    assert_eq!(boots.last().unwrap()["result"]["boot"]["reason"], "rollback_system");
    assert!(fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().contains("whole-system rollback"));
}

/// RF-14: a kernel that acknowledges owner lines and claims `busy:true` in every heartbeat, but never reaches the
/// model, escapes H2's 120 s idle rule and is caught by the 900 s rule: the open group is rolled back (`direct`, the
/// only one open) and the owner is alerted `deaf`.
#[test]
fn rf14_deafness_with_a_fake_busy_lie() {
    let h = home("rf14");
    h.install();
    fs::create_dir_all(h.ih.join("work")).unwrap();
    fs::write(h.ih.join("work/fk-busy"), "").unwrap(); // every hb says busy:true
    let _d = h.daemon("2030-01-02T00:00:00Z", "body");
    let (group, _) = h.open_change('c', "skill/lazy");
    h.say("please summarise the repo");
    wait(10, "the line relayed and acknowledged", || h.fk("owner.in").iter().any(|e| e["frame"]["text"] == "please summarise the repo"));
    h.clock(125);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(h.of("harm").is_empty(), "busy: the 120 s rule does not fire");
    h.clock(800);
    wait(20, "H2", || !h.of("harm").is_empty());
    // the sentinel appends `harm`, then the group's `change_close`, from one action list: wait for both (a reader
    // between the two appends saw no close; X67, rf14 race)
    wait(10, "the group's change_close", || !h.of("change_close").is_empty());
    let harm = &h.of("harm")[0]["data"];
    assert_eq!((harm["signal"].as_str(), harm["action"].as_str(), harm["groups"].clone()), (Some("H2"), Some("rollback_all"), json!([group])));
    let close = &h.of("change_close")[0]["data"];
    assert_eq!((close["outcome"].as_str(), close["attribution"].as_str()), (Some("rolled_back"), Some("direct")));
    wait(10, "extend.rollback", || !h.events("extend.rollback").is_empty());
    assert!(fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().contains("\"code\":\"deaf\""));
}

/// RF-12 on the wire: the owner's `/rollback` (a Root line, H3) rolls back the newest group as `direct`; the kernel's
/// `body_rollback` to the parent's body confirms it, and the Root records `rollback{target:"parent"}` with no restart.
#[test]
fn rf12_owner_rollback_and_kernel_confirmation() {
    let h = home("rf12");
    h.install();
    let _d = h.daemon("2030-01-02T00:00:00Z", "body");
    let parent = h.status()["tuples"]["current"].as_str().unwrap().to_string();
    let (group, tuple) = h.open_change('d', "skill/eager");
    h.say("/rollback");
    wait(10, "H3", || !h.of("harm").is_empty());
    // the sentinel appends `harm`, then the group's `change_close`, from one action list: wait for both (a reader
    // between the two appends saw no close; X67, rf14 race)
    wait(10, "the group's change_close", || !h.of("change_close").is_empty());
    let harm = &h.of("harm")[0]["data"];
    assert_eq!((harm["signal"].as_str(), harm["groups"].clone()), (Some("H3"), json!([group])));
    let close = &h.of("change_close")[0]["data"];
    assert_eq!((close["outcome"].as_str(), close["attribution"].as_str(), close["trigger"].as_str()), (Some("rolled_back"), Some("direct"), Some("H3")));
    wait(10, "extend.rollback", || h.events("extend.rollback").iter().any(|e| e["to"] == parent.as_str()));
    let starts = h.of("zone_start").len();
    let to_sha = h.status()["tuples"]["current"].as_str().map(|_| "").unwrap(); // the genesis tuple's body sha is ""
    let r = h.kernel_call("lineage.append", json!({"kind": "body_rollback", "idem": "x3-rb-1",
        "data": {"from_sha": "d".repeat(40), "to_sha": to_sha, "reason": "extend.rollback"}}));
    assert_eq!(r["ok"], true, "{r}");
    wait(10, "the Root's rollback record", || !h.of("rollback").is_empty());
    let rb = &h.of("rollback")[0]["data"];
    assert_eq!((rb["target"].as_str(), rb["to"].as_str(), rb["from"].as_str()), (Some("parent"), Some(parent.as_str()), Some(tuple.as_str())));
    assert_eq!(h.status()["tuples"]["current"], parent.as_str());
    h.clock(121);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(h.of("zone_start").len(), starts, "confirmed: no whole-system restart");
    assert_eq!(h.of("rollback").len(), 1);
    // a direct rollback: the change is never re-admitted
    let sha = "d".repeat(40);
    let r = h.kernel_call("change.open", json!({"sha": sha, "from_sha": "", "scope": "t0", "prefix_change": false,
        "candidate": {"body": {"binary": "", "sha": sha}, "ext": {"components": [], "set": null}}, "caps": []}));
    assert_eq!((r["ok"].as_bool(), r["error"]["code"].as_str()), (Some(false), Some("extend.readmit")), "{r}");
}

/// v0.5.3 (lane H, the cancelled demo's setup): on the gated path a kept change is LKG at once (G4 had nothing to score)
/// and opens no change group, so `/rollback` found nothing to act on and only `/panic` then `/resume` reverted it. Plain
/// `/rollback` now undoes the newest kept change: LKG moves back to the tuple it replaced (`lkg{undo}`, INV-5's owner
/// rollback), the zone restarts there (`rollback{target:"parent"}`, boot `rollback_system`), and the lineage verifies.
/// With no older kept tuple it says so and rolls nothing back.
#[test]
fn the_gated_rollback_undoes_the_newest_kept_change() {
    let h = home("gr");
    h.install_mode("gated");
    {
        let _d = h.daemon("2030-01-02T00:00:00Z", "body");
        let genesis = h.status()["tuples"]["current"].as_str().unwrap().to_string();
        let mut a = UnixStream::connect(h.rh.join("run/owner.sock")).unwrap();
        writeln!(a, "{}", json!({"client": "x3-test", "replay": false, "t": "attach"})).unwrap();
        a.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut rd = BufReader::new(a.try_clone().unwrap());
        writeln!(a, "{}", json!({"t": "line", "text": "/rollback"})).unwrap();
        let notice = loop {
            let mut l = String::new();
            assert!(rd.read_line(&mut l).unwrap() > 0, "the attach connection closed");
            let v: Value = serde_json::from_str(&l).unwrap_or_default();
            if v["t"] == "notice" && v["text"].as_str().is_some_and(|t| t.starts_with("[root] /rollback")) {
                break v["text"].as_str().unwrap().to_string();
            }
        };
        assert!(notice.contains("nothing was rolled back") && notice.contains("first kept tuple") && notice.contains(&genesis), "{notice}");
        std::thread::sleep(Duration::from_millis(500));
        assert!(h.of("rollback").is_empty() && h.of("lkg").iter().all(|r| r["data"]["undo"].is_null()), "nothing moved");
    }
    // the kernel's gated promotion forms a kept tuple, LKG at once
    let _d = h.daemon("2030-01-02T00:00:00Z", "body,promote");
    let genesis = h.of("tuple")[0]["data"]["id"].as_str().unwrap().to_string();
    wait(20, "the kept change", || h.status()["tuples"]["lkg"].as_str().is_some_and(|l| l != genesis));
    let kept = h.status()["tuples"]["lkg"].as_str().unwrap().to_string();
    assert_eq!(h.status()["tuples"]["current"], kept.as_str(), "the kept change runs");
    let boots = h.fk("boot").len();
    h.say("/rollback");
    wait(20, "the whole-system rollback", || !h.of("rollback").is_empty());
    let undo: Vec<Value> = h.of("lkg").into_iter().filter(|r| !r["data"]["undo"].is_null()).collect();
    assert_eq!(undo.len(), 1, "{undo:?}");
    assert_eq!(undo[0]["data"], json!({"reason": "owner_rollback", "tuple": genesis, "undo": kept}), "LKG moves back, as the owner's");
    let rb = &h.of("rollback")[0]["data"];
    assert_eq!((rb["to"].as_str(), rb["from"].as_str(), rb["target"].as_str(), rb["reason"].as_str()),
        (Some(genesis.as_str()), Some(kept.as_str()), Some("parent"), Some("rollback_system")), "{rb}");
    let harm = &h.of("harm")[0]["data"];
    assert_eq!((harm["signal"].as_str(), harm["action"].as_str(), harm["scope"].as_str()), (Some("H3"), Some("rollback_kept"), Some("system")));
    wait(20, "the kernel restarted", || h.fk("boot").len() > boots);
    assert_eq!(h.fk("boot")[boots]["result"]["boot"]["reason"], "rollback_system", "the kernel restores the target's body");
    let tb = &h.fk("boot")[boots]["result"]["boot"]["tuple_body"];
    assert_eq!((tb["binary"].as_str(), tb["sha"].as_str()), (Some(""), Some("")), "the seedless genesis body: {tb}");
    // the lineage is verified once the daemon has stopped: `audit verify` reads the chain, then head.json, and the fake
    // kernel's promotion after the restart may be appending in between (head.json ahead of the chain read: INVALID)
    drop(_d);
    h.ok(&["audit", "verify", "--full"]);
}
