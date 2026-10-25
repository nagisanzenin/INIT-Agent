//! WP25 integration tests: R7, the autonomy ledger (root/root.md §3.13; testing.md RT-L1…RT-L7, RT-L9), on
//! test ledgers and the fake clock. X7 (ADR-011 §7) rewrote the climbing tests for the record-based levels; the
//! demotion tests run on the `gated` path, whose triggers are unchanged. Run with `cargo test --offline -p init-root --features test-fixtures`.
//!
//! Live-system safety (CONVENTIONS §1): the kernel is `init-root-fakekernel` (tests/support) with `/bin/cat`
//! as its body. Every process is this test's Root daemon or one that Root spawned and holds; the test signals
//! only its own children, by handle. Homes live under `root/target/test-homes/wp25/`, with `HOME` there.
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
const ROOT_CFG: &str = "[root]\ntest_mode = true\nhb_interval_s = 1\nhb_stale_s = 3\nkernel_hello_timeout_s = 20\nbackoff_initial_ms = 50\n\
    backoff_max_ms = 200\nstop_grace_s = 5\n";

struct Home {
    dir: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
}

/// A fresh home `root/target/test-homes/wp25/<name>` (short: ROOT_HOME must stay ≤ 80 bytes).
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/wp25");
    let dir = base.join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(dir.join("rel")).unwrap();
    fs::create_dir_all(dir.join("tc/bin")).unwrap();
    fs::copy(FK, dir.join("rel/init-kernel")).unwrap();
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
}

fn charter(max_level: u8) -> String {
    format!("schema_version = 1\n[purpose]\ntext = \"WP25 test home.\"\n[autonomy]\nmax_level = {max_level}\n[quotas]\ndisk_min_free_gib = 0\n{ROOT_CFG}\n")
}

fn wait(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "not within {secs} s: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `n` Root `turns` records (ADR-011 §7.1), one per owner day from `first` (a date), each with `owner` owner turns;
/// each is appended the next day, as selection appends it.
fn turn_days(first: &str, n: i64, owner: u64) -> Vec<Value> {
    let d0: i64 = first[8..10].parse().unwrap();
    (0..n).map(|i| {
        let day = format!("{}{:02}", &first[..8], d0 + i);
        json!({"at": format!("{}{:02}T00:00:00Z", &first[..8], d0 + i + 1), "kind": "turns", "data": {"background": 0, "background_harmed": 0,
            "day": day, "owner": owner, "owner_harmed": 0, "start": format!("{day}T00:00:00.000Z")}})
    }).collect()
}

/// `n` change groups (groups `from`…) opened and closed `clean` at `at`, each with 5 window turns and a changed tuple.
fn clean_groups(at: &str, from: u64, n: u64) -> Vec<Value> {
    (from..from + n).flat_map(|g| [
        json!({"at": at, "kind": "change_open", "data": {"caps": [], "group": g, "parent_tuple": format!("tp{g}"), "scope": "t0", "sha": format!("{g:040x}"), "tuple": format!("t{g}")}}),
        json!({"at": at, "kind": "change_close", "data": {"attribution": null, "group": g, "outcome": "clean", "trigger": null, "turns": 5}}),
    ]).collect()
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
    /// A test ledger (`start_level`, `records`) from 2030-01-01, then install; `kernel`: with the fake kernel.
    fn install(&self, max_level: u8, level: u8, records: Value, kernel: bool) {
        fs::create_dir_all(&self.rh).unwrap();
        fs::write(self.rh.join("charter.toml"), charter(max_level)).unwrap();
        let spec = self.dir.join("spec.json");
        fs::write(&spec, json!({"start_at": "2030-01-01T00:00:00Z", "start_level": level, "records": records}).to_string()).unwrap();
        self.ok(&["test", "ledger", "--spec", spec.to_str().unwrap()]);
        let (rel, tc) = (self.dir.join("rel"), self.dir.join("tc"));
        let o = if kernel { self.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]) } else { self.run(&["install"]) };
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    }
    fn records(&self) -> Vec<Value> {
        let s = fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default();
        s.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
    fn count(&self, kind: &str) -> usize {
        self.records().iter().filter(|r| r["kind"] == kind).count()
    }
    fn last(&self, kind: &str) -> Value {
        self.records().into_iter().rev().find(|r| r["kind"] == kind).unwrap_or(Value::Null)
    }
    fn status(&self) -> Value {
        self.ok(&["status", "--json"])
    }
    fn ledger(&self) -> Value {
        self.ok(&["ledger"])
    }
    fn criterion(&self, id: &str) -> Value {
        let l = self.ledger();
        l["next"]["criteria"].as_array().into_iter().flatten().find(|c| c["id"] == id).cloned().unwrap_or_default()
    }
    fn fk(&self, ev: &str) -> Vec<Value> {
        let s = fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["ev"] == ev).collect()
    }
    /// One owner-socket request (`test.append`, …); it must succeed.
    fn owner(&self, cmd: &str, args: Value) -> Value {
        let mut s = UnixStream::connect(self.rh.join("run/owner.sock")).unwrap();
        writeln!(s, "{}", json!({"args": args, "cmd": cmd, "id": "t-1", "v": 1})).unwrap();
        let mut line = String::new();
        BufReader::new(&s).read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["ok"], true, "{cmd}: {v}");
        v["result"].clone()
    }
    /// Appends Root-path records (test.append) and returns the first one's seq.
    fn append(&self, records: Value) -> u64 {
        self.owner("test.append", json!({"records": records}))["seqs"][0].as_u64().unwrap()
    }
    /// A request the fake kernel sends over the control link (`work/fk-call.json`, written atomically).
    fn kernel_call(&self, cmd: &str, args: Value) {
        let (tmp, dst) = (self.ih.join("work/fk-call.tmp"), self.ih.join("work/fk-call.json"));
        fs::create_dir_all(self.ih.join("work")).unwrap();
        fs::write(&tmp, json!({"cmd": cmd, "args": args}).to_string()).unwrap();
        fs::rename(tmp, dst).unwrap();
    }
    fn clock(&self, secs: i64) {
        self.ok(&["test", "clock", "--advance", &secs.to_string()]);
    }
    fn daemon(&self, now: &str) -> Daemon {
        let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
        let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
        let c = self.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", "body,promote")
            .env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir")).env("INIT_TEST_HOME_REAL", &self.dir)
            .env("INIT_ROOT_TEST_NOW", now).spawn().unwrap();
        let d = Daemon(c, self.rh.clone());
        wait(10, "owner.sock", || UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        d
    }
    /// The next `ladder` record after `n` of them.
    fn ladder_after(&self, n: usize) -> Value {
        wait(15, &format!("ladder record #{}", n + 1), || self.count("ladder") > n);
        self.records().into_iter().filter(|r| r["kind"] == "ladder").nth(n).unwrap()
    }
    /// The zone runs again after `n` zone_start records: the incident is over (§3.13.3 step 1).
    fn restarted(&self, n: usize) -> Value {
        wait(20, "a new zone_start", || self.count("zone_start") > n);
        let mut st = Value::Null;
        wait(10, "running", || {
            st = self.status();
            st["slot"] == "running"
        });
        st
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

fn down(l: &Value, from: i64, to: i64, kind: &str, seq: u64) {
    let d = &l["data"];
    assert_eq!((d["direction"].as_str(), d["cause"].as_str(), d["from"].as_i64(), d["to"].as_i64()), (Some(if from == 0 { "floor" } else { "down" }), Some("demotion"), Some(from), Some(to)), "{l}");
    assert_eq!((d["trigger"]["kind"].as_str(), d["trigger"]["seq"].as_u64()), (Some(kind), Some(seq)), "{l}");
}

/// RF-26 / RT-L1, RT-L2 (ADR-011 §7, lane X7): the L1 record from Root-written records only: 50 owner turns in
/// `turns` days, 9 clean groups (each with 5 window turns and a changed tuple), 9 days at L0. `next` shows
/// `clean_groups` at 9 of 10. The 10th clean group close unlocks L1 at once: a `ladder up` with its criteria, the
/// `level` event, egress and the view at L1, the `level_up` alert, and no process restarted.
#[test]
fn rt_l1_l2_the_record_unlocks_l1_without_a_restart() {
    let h = home("l12");
    let mut recs = turn_days("2030-01-01", 5, 10);
    recs.extend(clean_groups("2030-01-07T00:00:00Z", 0, 9));
    h.install(4, 0, json!(recs), true);
    let l = h.ledger(); // offline: folded from the files
    assert_eq!((l["level"].as_i64(), l["next"]["level"].as_i64()), (Some(0), Some(1)));
    assert_eq!(h.criterion("clean_groups"), json!({"have": 9, "id": "clean_groups", "met": false, "need": 10}));
    assert_eq!(h.criterion("owner_turns")["have"], 50);
    let _d = h.daemon("2030-01-10T00:00:00Z");
    wait(15, "the fake kernel reached ready", || h.fk("ready").iter().any(|e| e["ok"] == true));
    h.clock(0);
    assert_eq!((h.status()["level"].as_i64(), h.count("ladder")), (Some(0), 0));
    h.append(json!(clean_groups("2030-01-10T00:00:00Z", 9, 1).into_iter().map(|mut r| { r.as_object_mut().unwrap().remove("at"); r }).collect::<Vec<_>>()));
    // the fake kernel may report a promotion between the two appends: find the close by its group
    let close = h.records().into_iter().find(|r| r["kind"] == "change_close" && r["data"]["group"] == 9).unwrap()["seq"].as_u64().unwrap();
    let up = h.ladder_after(0);
    let d = &up["data"];
    assert_eq!((d["direction"].as_str(), d["cause"].as_str(), d["from"].as_i64(), d["to"].as_i64()), (Some("up"), Some("unlock"), Some(0), Some(1)));
    let crit = d["criteria"].as_array().unwrap();
    assert!(crit.iter().all(|c| c["met"] == true) && crit.iter().any(|c| c["id"] == "clean_groups" && c["have"] == 10), "{d}");
    assert!(up["seq"].as_u64() > Some(close), "the clean close came first");
    wait(10, "the kernel's level event", || h.fk("event").iter().any(|e| e["name"] == "level" && e["data"]["to"] == 1));
    let ev = h.fk("event").into_iter().find(|e| e["name"] == "level").unwrap();
    assert_eq!((ev["data"]["cause"].as_str(), ev["data"]["r4_seq"].as_u64()), (Some("unlock"), up["seq"].as_u64()));
    let st = h.status();
    assert_eq!((st["level"].as_i64(), st["slot"].as_str()), (Some(1), Some("running")));
    assert_eq!(h.fk("start").len(), 1, "no kernel restart");
    // LINUX: the Root holds the outer bwrap pid; the kernel sees itself by its pid-namespace pid
    #[cfg(target_os = "macos")]
    assert_eq!(st["kernel"]["pid"], h.fk("start")[0]["pid"]);
    assert_eq!(h.count("zone_start"), 1);
    let view: Value = serde_json::from_str(&fs::read_to_string(h.ih.join("root/ledger.json")).unwrap()).unwrap();
    assert_eq!((view["level"].as_i64(), view["entered_seq"].as_u64(), view["next"]["level"].as_i64()), (Some(1), up["seq"].as_u64(), Some(2)));
    let ceil: Value = serde_json::from_str(&fs::read_to_string(h.ih.join("root/ceilings.json")).unwrap()).unwrap();
    assert_eq!(ceil["level"], 1, "the egress gates at L1");
    assert!(fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().contains("\"code\":\"level_up\""));
}

/// RT-L3 (a demotion holds L1 for 7 days), RT-L4 (the charter's max), RT-L6 (L1 held 20 then 21 days with 150 owner
/// turns, 40 clean groups and two clean weekly twin reports) and RT-L5 (a lowered max caps, with no rollback), on the
/// ADR-011 §7 record. No kernel is installed: R7 runs on the lineage and the fake clock alone.
#[test]
fn rt_l3_l4_l5_l6_criteria_on_the_fake_clock() {
    // RT-L3: a 0→0 demotion on 2030-01-02 restarts the record; the L1 record is complete by 2030-01-07
    let h = home("l3");
    let mut recs = vec![json!({"at": "2030-01-02T00:00:00Z", "kind": "ladder", "data": {"cause": "demotion", "direction": "down", "from": 0, "to": 0,
        "trigger": {"kind": "ceiling_hit", "seq": 1}}})];
    recs.extend(turn_days("2030-01-02", 5, 10));
    recs.extend(clean_groups("2030-01-07T00:00:00Z", 0, 10));
    h.install(4, 0, json!(recs), false);
    let d = h.daemon("2030-01-08T12:00:00Z");
    h.clock(0); // one evaluation, synchronously
    assert_eq!((h.status()["level"].as_i64(), h.count("ladder")), (Some(0), 1));
    assert_eq!(h.criterion("days_at_level")["have"], 6);
    assert_eq!(h.criterion("demotions_last_7_days")["have"], 1);
    assert_eq!(h.criterion("clean_groups")["met"], true);
    h.clock(12 * 3600); // 2030-01-09T00:00: seven days since the demotion
    let up = h.ladder_after(1);
    assert_eq!((up["data"]["from"].as_i64(), up["data"]["to"].as_i64()), (Some(0), Some(1)));
    drop(d);

    // RT-L4: the record holds, but the charter's max is L0; raising it unlocks at the next evaluation
    let h = home("l4");
    let mut recs = turn_days("2030-01-01", 5, 10);
    recs.extend(clean_groups("2030-01-06T00:00:00Z", 0, 10));
    h.install(0, 0, json!(recs), false);
    let _d = h.daemon("2030-01-09T00:00:00Z");
    h.clock(0);
    let l = h.ledger();
    assert_eq!((l["level"].as_i64(), l["cap"].as_i64(), l["next"].is_null(), h.count("ladder")), (Some(0), Some(0), true, 0));
    fs::write(h.rh.join("charter.toml"), charter(4)).unwrap();
    h.ok(&["charter", "check", "--apply", "--yes"]);
    let up = h.ladder_after(0);
    assert_eq!((up["data"]["to"].as_i64(), up["seq"].as_u64()), (Some(1), h.last("charter")["seq"].as_u64().map(|s| s + 1)));

    // RT-L6 and RT-L5: L1 since 2030-01-01, 152 owner turns, 40 clean groups, two clean twin reports
    let h = home("l6");
    let mut recs = turn_days("2030-01-01", 19, 8);
    recs.extend(clean_groups("2030-01-03T00:00:00Z", 0, 40));
    for at in ["2030-01-10T00:00:00Z", "2030-01-17T00:00:00Z"] {
        recs.push(json!({"at": at, "kind": "twin_report", "data": {"pairs": 42, "regression": false, "week": at}}));
    }
    h.install(4, 1, json!(recs), false);
    let _d = h.daemon("2030-01-21T00:00:00Z");
    h.clock(0);
    assert_eq!((h.status()["level"].as_i64(), h.criterion("days_at_level")["have"].as_i64()), (Some(1), Some(20)));
    assert_eq!(h.criterion("twin_reports_no_regression"), json!({"have": 2, "id": "twin_reports_no_regression", "met": true, "need": 2}));
    h.clock(86_400);
    let up = h.ladder_after(1);
    assert_eq!((up["data"]["from"].as_i64(), up["data"]["to"].as_i64(), h.status()["level"].as_i64()), (Some(1), Some(2), Some(2)));
    fs::write(h.rh.join("charter.toml"), charter(1)).unwrap();
    h.ok(&["charter", "check", "--apply", "--yes"]);
    let cap = h.ladder_after(2);
    assert_eq!((cap["data"]["direction"].as_str(), cap["data"]["to"].as_i64(), cap["data"]["trigger"]["kind"].as_str()), (Some("cap"), Some(1), Some("charter")));
    h.clock(90 * 86_400);
    let l = h.ledger();
    assert_eq!((l["level"].as_i64(), l["next"].is_null(), h.count("ladder"), h.count("rollback")), (Some(1), true, 3, 0), "capped: no climb, no rollback");
    assert_eq!(h.ok(&["ledger", "--verify"])["ok"], true, "every level change verifies against the record");
}

/// RT-L7 and RT-L9 on a test ledger at L4 with the kernel running: each trigger costs one level in its own
/// incident, restarts the hold time and rolls back to audited-good (not a mutation-suite failure: nothing was
/// swapped); a burst of five demoting hits costs one level; `system.swap` below L3 is refused `root.level`
/// with a demoting `ceiling_hit{ceiling:"level"}`; a trigger at L0 still rolls back. Crash loops and panics
/// demote through the same path (tests/wp21.rs, tests/wp20.rs).
#[test]
fn rt_l7_l9_each_trigger_costs_one_level_and_rolls_back() {
    let h = home("l7");
    h.install(4, 4, json!([]), true);
    let d = h.daemon("2030-02-01T00:00:00Z");
    h.restarted(0);
    wait(10, "the kernel's promotion forms t1", || h.count("lkg") >= 2);
    let genesis = h.last("audited")["data"]["tuple"].clone();
    assert_ne!(h.status()["tuples"]["current"], genesis);
    let check = |ladders: usize, from: i64, kind: &str, seq: u64, starts: usize| {
        let l = h.ladder_after(ladders);
        down(&l, from, (from - 1).max(0), kind, seq);
        let st = h.restarted(starts);
        let rb = h.last("rollback");
        assert_eq!((st["level"].as_i64(), rb["data"]["to"].clone(), rb["data"]["target"].as_str()), (Some((from - 1).max(0)), genesis.clone(), Some("audited")));
        assert!(rb["seq"].as_u64() > l["seq"].as_u64(), "{rb}");
        assert_eq!(h.ledger()["entered_seq"].as_u64(), l["seq"].as_u64(), "the hold time restarts");
        let boot = h.fk("boot").pop().unwrap()["result"]["boot"].clone();
        assert_eq!((boot["reason"].as_str(), boot["cause"]["trigger"].as_str()), (Some("demotion"), Some(kind)));
        wait(10, "the restarted kernel's promotion", || h.last("lkg")["seq"].as_u64() > rb["seq"].as_u64());
    };
    // a sealed-audit regression (SA-1)
    let s = h.append(json!([{"kind": "audit", "data": {"tuple": "t", "score_permille": 600, "regression": true, "trigger": "scheduled"}}]));
    check(4, 4, "audit", s, 1);
    // a burst of five demoting ceiling hits (D6 rule c) inside one incident: one level
    let hit = json!({"kind": "ceiling_hit", "data": {"attempted": 9, "ceiling": "money.day", "class": "providers", "code": "ceiling.day",
        "demotes": true, "level": 3, "limit": 5, "rule": "c", "source": "egress"}});
    let s = h.append(json!([hit, hit, hit, hit, hit]));
    check(5, 3, "ceiling_hit", s, 2);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(h.count("ladder"), 6, "the burst cost one level");
    // RT-L9: system.swap from the kernel at L2
    h.kernel_call("system.swap", json!({"version": "0.0.1", "source_repo": "x", "source_sha": "a", "init_kernel": "b", "initctl": "c", "report": {}}));
    let l = h.ladder_after(6);
    let hit = h.last("ceiling_hit");
    assert_eq!((hit["data"]["ceiling"].as_str(), hit["data"]["code"].as_str(), hit["data"]["demotes"].as_bool(), hit["data"]["rule"].as_str()),
        (Some("level"), Some("root.level"), Some(true), Some("b")));
    down(&l, 2, 1, "ceiling_hit", hit["seq"].as_u64().unwrap());
    h.restarted(3);
    // a canary trip reported by the kernel's gate
    h.kernel_call("lineage.append", json!({"kind": "gate", "idem": "canary-1",
        "data": {"gate_id": "g-1", "gate_kind": "G4", "sha": "c", "verdict": "fail", "reason": "canary"}}));
    let l = h.ladder_after(7);
    assert_eq!(h.last("gate")["seq"], l["data"]["trigger"]["seq"]);
    down(&l, 1, 0, "gate", h.last("gate")["seq"].as_u64().unwrap());
    h.restarted(4);
    wait(10, "the restarted kernel's promotion", || h.last("lkg")["seq"].as_u64() > l["seq"].as_u64());
    // a mutation-suite failure at L0: 0→0, no rollback, no restart
    let (pid, rollbacks, starts) = (h.status()["kernel"]["pid"].clone(), h.count("rollback"), h.count("zone_start"));
    let s = h.append(json!([{"kind": "mutation", "data": {"gate": "g", "tuple": "t", "total": 9, "rejected": 8, "controls_ok": true, "pass": false, "source": "agent"}}]));
    down(&h.ladder_after(8), 0, 0, "mutation", s);
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!((h.status()["kernel"]["pid"].clone(), h.count("rollback"), h.count("zone_start")), (pid, rollbacks, starts));
    assert!(h.ledger()["incident"].is_object(), "the incident stays open: no kernel restart closes it");
    h.clock(601); // the incident ends after 600 s
    assert!(h.ledger()["incident"].is_null());
    // a body crash loop on body LKG at L0: 0→0, rollback, system safe mode
    h.kernel_call("lineage.append", json!({"kind": "body_safe", "idem": "bs-1", "data": {"on": true, "sha": "d", "crashes": 3}}));
    let l = h.ladder_after(9);
    down(&l, 0, 0, "body_safe", h.last("body_safe")["seq"].as_u64().unwrap());
    let st = h.restarted(starts);
    assert_eq!((st["mode"].as_str(), h.last("safe")["data"]["on"].as_bool()), (Some("safe"), Some(true)));
    assert!(h.last("rollback")["seq"].as_u64() > l["seq"].as_u64());
    // F2: an incident at L0 alerts `incident` (no level lost), a demotion above L0 alerts `demoted`
    let alerts = fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap();
    assert!(alerts.matches("\"code\":\"demoted\"").count() + alerts.matches("\"code\":\"incident\"").count() >= 6);
    assert!(alerts.contains("\"code\":\"incident\""), "the L0 incidents say so");
    h.ok(&["stop"]);
    wait(15, "stopped", || h.status()["slot"] == "stopped");
    drop(d); // the chain verifies once the Root has stopped appending
    let ok = h.run(&["audit", "verify", "--full"]);
    assert_eq!(ok.status.code(), Some(0), "{}", String::from_utf8_lossy(&ok.stdout));
}

/// build-deviations §3.9 (provisional owner decision 2026-10-20): a kernel that repeats a violation after every
/// restart at L0 is demoted 0→0 each time; the third demotion at L0 within 24 h enters system safe mode on the
/// crash-loop path (`safe{on:true}`, the `demotion_loop` alert, a safe-mode boot), instead of a rollback loop.
#[test]
fn rt_l10_three_demotions_at_l0_in_24h_enter_safe_mode() {
    let h = home(&format!("r{:05}", std::process::id() % 100_000)); // per process: lanes may run this test at once
    h.install(4, 0, json!([]), true);
    let d = h.daemon("2030-02-01T00:00:00Z");
    h.restarted(0);
    let hit = json!({"kind": "ceiling_hit", "data": {"attempted": 9, "ceiling": "money.day", "class": "providers", "code": "ceiling.day",
        "demotes": true, "level": 0, "limit": 5, "rule": "c", "source": "egress"}});
    for n in 0..3 {
        let s = h.append(json!([hit]));
        down(&h.ladder_after(n), 0, 0, "ceiling_hit", s);
        let st = h.restarted(n + 1);
        let boot = h.fk("boot").pop().unwrap()["result"]["boot"].clone();
        let want = if n < 2 { "normal" } else { "safe" };
        assert_eq!((st["mode"].as_str(), boot["mode"].as_str(), boot["reason"].as_str()), (Some(want), Some(want), Some("demotion")), "demotion {}", n + 1);
        h.clock(60); // the next violation, a minute later
    }
    assert_eq!((h.count("safe"), h.last("safe")["data"]["on"].as_bool()), (1, Some(true)));
    let alerts = fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap();
    assert_eq!(alerts.matches("\"code\":\"demotion_loop\"").count(), 1, "{alerts}");
    // a fourth in safe mode demotes once more and stays in safe mode; no second alert
    let s = h.append(json!([hit]));
    down(&h.ladder_after(3), 0, 0, "ceiling_hit", s);
    let st = h.restarted(4);
    assert_eq!(st["mode"], "safe");
    assert_eq!(fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().matches("\"code\":\"demotion_loop\"").count(), 1);
    drop(d);
    fs::remove_dir_all(&h.dir).unwrap(); // exact path, this test's own home
}
