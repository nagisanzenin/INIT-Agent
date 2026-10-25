//! X8 integration tests (ADR-011 §1.2, Work breakdown X8): the kernel lane on a Root daemon with the fake kernel.
//! - RF-28: a candidate kernel that fails the Root conformance probe (`prepare_swap` answered but no exit 75; a body
//!   spawned in mode normal) is refused at `change.open` before any swap; a swap naming no admitted change is refused.
//! - RF-29: a crash in probation → the whole-system rollback to the previous tuple plus `kstate`, decided by the Root,
//!   one restart, no demotion; `current` does not move at admission, only at the swap's install.
//! - RF-30: a hostile kernel (drops owner lines, forges Root records, ignores `stop` and rollback) → H2, zone evidence
//!   only, the Root's whole-system rollback within `stop_grace_s`, and no `extend.rollback` ever asked of it; body
//!   groups wait during probation and may open after it.
//! - L1 gating: at L0 a kernel change is refused and a `system.swap` is a demoting `root.level` refusal.
//!
//! Run with `cargo test --offline -p init-root --features test-fixtures --test x8`.
//!
//! Live-system safety (CONVENTIONS §1): the kernels are `init-root-fakekernel` (tests/support); every process is this
//! test's Root daemon or one that Root spawned and holds (the probe's are sealed instances in its own run roots); the
//! test signals only its own children, by handle. Homes live under `root/target/test-homes/x8/`, with `HOME` there.
//! $0: no model, no network.
#![cfg(feature = "test-fixtures")]

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_init-root");
const FK: &str = env!("CARGO_BIN_EXE_init-root-fakekernel");

/// The LKG body the probe spawns in mode `check`: it writes `ready` with its boot id and exits 0 (A4's rule).
const CHECK_BODY: &str = "#!/bin/sh\n[ \"$INIT_MODE\" = check ] || exit 64\nmkdir -p \"$INIT_HOME/state/run\"\n\
    printf '{\"boot_id\":\"%s\",\"mode\":\"check\"}\\n' \"$INIT_BOOT_ID\" > \"$INIT_HOME/state/run/ready\"\nexit 0\n";

fn charter(max_level: i64) -> String {
    format!("schema_version = 1\n[purpose]\ntext = \"X8 test home.\"\n[autonomy]\nmax_level = {max_level}\n[quotas]\ndisk_min_free_gib = 0\n\
        [sandbox]\ndisk_floor_gib = 0\n[extend]\nmode = \"free\"\n[root]\ntest_mode = true\nhb_interval_s = 1\nhb_stale_s = 3\n\
        kernel_hello_timeout_s = 20\nbackoff_initial_ms = 50\nbackoff_max_ms = 200\nstop_grace_s = 5\nkernel_probation_s = 60\n")
}

fn sha256(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

struct Home {
    dir: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
    fk_sha: String,
    body_sha: String,
    parent: String,
}

/// A fresh home `root/target/test-homes/x8/<name>` (short: ROOT_HOME must stay ≤ 80 bytes).
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/x8");
    let dir = base.join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(dir.join("rel")).unwrap();
    fs::create_dir_all(dir.join("tc/bin")).unwrap();
    fs::copy(FK, dir.join("rel/init-kernel")).unwrap();
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir, fk_sha: String::new(), body_sha: String::new(), parent: String::new() }
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
    fn put(&self, bytes: &[u8]) -> String {
        let sha = sha256(bytes);
        let p = self.rh.join("store/blobs").join(&sha);
        fs::write(&p, bytes).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o444)).unwrap();
        sha
    }
    /// The test ledger at `level`, a maintainer tuple (the fake kernel, the check body) as `current` and `lkg`, install.
    fn install(&mut self, level: i64) {
        fs::create_dir_all(&self.rh).unwrap();
        fs::write(self.rh.join("charter.toml"), charter(1)).unwrap();
        self.fk_sha = sha256(&fs::read(FK).unwrap());
        let t = json!({"body": {"binary": sha256(CHECK_BODY.as_bytes()), "sha": "a".repeat(40)}, "ext": {"components": [], "set": null},
            "kernel": {"built_from": "", "init_kernel": self.fk_sha, "initctl": "", "source": "maintainer", "version": "0.0.0"}, "v": 1});
        let icj = serde_json::to_string(&t).unwrap();
        self.parent = format!("t{}", &sha256(icj.as_bytes())[..12]);
        let spec = self.dir.join("spec.json");
        fs::write(&spec, json!({"start_at": "2030-01-01T00:00:00Z", "start_level": level, "records": [
            {"at": "2030-01-01T00:00:00Z", "kind": "tuple", "data": {"id": self.parent, "tuple": t}},
            {"at": "2030-01-01T00:00:00Z", "kind": "lkg", "data": {"tuple": self.parent}}]}).to_string()).unwrap();
        self.ok(&["test", "ledger", "--spec", spec.to_str().unwrap()]);
        assert_eq!(self.put(&fs::read(FK).unwrap()), self.fk_sha);
        self.body_sha = self.put(CHECK_BODY.as_bytes());
        let tp = self.rh.join("store/tuples").join(format!("{}.json", self.parent));
        fs::write(&tp, format!("{icj}\n")).unwrap();
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
    fn stages(&self) -> Vec<String> {
        self.of("kernel_upgrade").iter().map(|r| r["data"]["stage"].as_str().unwrap_or("").to_string()).collect()
    }
    fn stage(&self, s: &str) -> Value {
        self.of("kernel_upgrade").into_iter().find(|r| r["data"]["stage"] == s).map(|r| r["data"].clone()).unwrap_or(Value::Null)
    }
    fn status(&self) -> Value {
        self.ok(&["status", "--json"])
    }
    fn current(&self) -> String {
        self.status()["tuples"]["current"].as_str().unwrap_or("").to_string()
    }
    fn fk(&self, ev: &str) -> Vec<Value> {
        let s = fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["ev"] == ev).collect()
    }
    fn events(&self, name: &str) -> Vec<Value> {
        self.fk("event").into_iter().filter(|e| e["name"] == name).map(|e| e["data"].clone()).collect()
    }
    /// A request the fake kernel sends over the control link (`work/fk-call.json`), and its logged outcome.
    fn kernel_call(&self, cmd: &str, args: Value) -> Value {
        let n = self.fk("call").len();
        let (tmp, dst) = (self.ih.join("work/fk-call.tmp"), self.ih.join("work/fk-call.json"));
        fs::create_dir_all(self.ih.join("work")).unwrap();
        fs::write(&tmp, json!({"cmd": cmd, "args": args}).to_string()).unwrap();
        fs::rename(tmp, dst).unwrap();
        wait(45, &format!("{cmd} answered"), || self.fk("call").len() > n);
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
    fn daemon(&self, now: &str) -> Daemon {
        let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
        let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
        let c = self.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", "")
            .env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir")).env("INIT_TEST_HOME_REAL", &self.dir)
            .env("INIT_ROOT_TEST_NOW", now).spawn().unwrap();
        let d = Daemon(c, self.rh.clone());
        wait(10, "owner.sock", || UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        wait(30, "the zone running", || self.status()["slot"] == "running" && !self.of("zone_start").is_empty());
        d
    }
    /// `change.open{scope:"k"}` for the fake kernel admitted as `version`, with forged cap evidence.
    fn open_k(&self, sha: char, version: &str) -> Value {
        let sha = sha.to_string().repeat(40);
        self.kernel_call("change.open", json!({"sha": sha, "from_sha": "a".repeat(40), "scope": "k", "prefix_change": false,
            "candidate": {"body": {"binary": self.body_sha, "sha": "a".repeat(40)}, "ext": {"components": [], "set": null},
                "kernel": {"built_from": "", "init_kernel": self.fk_sha, "initctl": "", "source": "agent", "version": version}},
            "caps": [{"evidence": "root", "id": "kernel/init-kernel", "op": "modify"}, {"evidence": "root", "id": "skill/forged", "op": "add"}]}))
    }
    fn swap(&self, group: &Value, version: &str) -> Value {
        self.swap_m(group, version, json!([]))
    }
    fn swap_m(&self, group: &Value, version: &str, measure: Value) -> Value {
        self.kernel_call("system.swap", json!({"group": group, "init_kernel": self.fk_sha, "initctl": "", "measure": measure, "report": {},
            "source_repo": "lineage/kernel-cand.git", "source_sha": "c".repeat(40), "version": version}))
    }
    /// Admits and swaps in the fake kernel as `version`; returns (group, candidate tuple, the old kernel's kstate).
    fn admit_and_swap(&self, sha: char, version: &str) -> (Value, String, Value) {
        let r = self.open_k(sha, version);
        assert_eq!(r["ok"], true, "change.open: {r}");
        let group = r["result"]["group"].clone();
        let open = self.of("change_open").pop().unwrap()["data"].clone();
        assert_eq!((open["scope"].as_str(), open["checks"]["K"].as_str(), open["parent_tuple"].as_str()), (Some("k"), Some("pass"), Some(self.parent.as_str())), "{open}");
        assert!(open["snapshot"].is_string(), "the system snapshot of a kernel change: {open}");
        let cand = open["tuple"].as_str().unwrap().to_string();
        assert_eq!(self.current(), self.parent, "admission does not move current; the swap does");
        assert!(fs::read_dir(self.rh.join("sealed/run")).map_or(0, |d| d.count()) == 0, "the probe roots are removed");
        let s = self.swap(&group, version);
        assert_eq!(s["ok"], true, "system.swap: {s}");
        assert_eq!(s["result"]["job"], "s-000001");
        wait(30, "the swap's ready", || self.stages().iter().any(|x| x == "ready"));
        assert_eq!(self.stages()[..4], ["requested", "prepared", "installed", "ready"]);
        assert_eq!(self.stage("requested")["gog"], false, "the gate-of-gates is opt-in");
        assert!(self.of("mutation").is_empty(), "no mutation suite ran");
        let ks = self.stage("prepared")["kstate"].clone();
        assert!(ks.is_string(), "the old kernel stored its kstate before exit 75: {ks}");
        assert_eq!(self.current(), cand);
        let boot = self.fk("boot").pop().unwrap();
        assert_eq!((boot["result"]["boot"]["reason"].as_str(), boot["result"]["boot"]["tuple_kernel"]["version"].as_str()),
            (Some("kernel_upgrade"), Some(version)));
        (group, cand, ks)
    }
}

/// The Root daemon this test spawned. On drop: an orderly `stop`, then the daemon by its own handle.
struct Daemon(Child, PathBuf);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = Command::new(BIN).env_clear().env("HOME", "/nonexistent").env("INIT_ROOT_HOME", &self.1).args(["stop"]).output();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(12) {
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

/// RF-28: candidates that fail the Root conformance probe are refused at admission, before any swap: one answers
/// `prepare_swap` but never exits 75, one spawns its body in mode normal. Nothing opens, nothing is snapshotted,
/// `current` stays, and the probe roots are gone. A swap with no admitted kernel change is refused too.
#[test]
fn rf28_a_candidate_failing_the_conformance_probe_is_refused_before_the_swap() {
    let mut h = home("rf28");
    h.install(1);
    let _d = h.daemon("2030-01-02T00:00:00Z");
    let r = h.swap(&Value::Null, "fk-normal");
    assert_eq!((r["ok"].as_bool(), r["error"]["code"].as_str()), (Some(false), Some("root.unwatched_change")), "{r}");

    let t = Instant::now();
    let r = h.open_k('b', "fk-no-swap");
    assert_eq!((r["ok"].as_bool(), r["error"]["code"].as_str()), (Some(false), Some("extend.kernel_probe")), "{r}");
    assert_eq!(r["error"]["data"]["step"], "prepare_swap", "{r}");
    assert!(t.elapsed() < Duration::from_secs(40), "the probe is bounded: {:?}", t.elapsed());
    assert!(r["error"]["data"]["why"].as_str().unwrap_or("").contains("not done in time"), "answered, never exited 75: {r}");

    let r = h.open_k('d', "fk-normal-body");
    assert_eq!((r["ok"].as_bool(), r["error"]["code"].as_str(), r["error"]["data"]["step"].as_str()), (Some(false), Some("extend.kernel_probe"), Some("body")), "{r}");

    assert!(h.of("change_open").is_empty() && h.of("kernel_upgrade").is_empty(), "nothing opened");
    assert!(h.of("snapshot").iter().all(|x| x["data"]["group"].is_null()), "no change snapshot (X9's twin freeze takes one at start)");
    assert_eq!(h.current(), h.parent);
    assert!(fs::read_dir(h.rh.join("sealed/run")).map_or(0, |d| d.count()) == 0, "every probe root is removed by its exact path");
    assert!(h.of("ladder").iter().all(|l| !matches!(l["data"]["direction"].as_str(), Some("down" | "floor"))), "a refused candidate demotes nothing");
}

/// RF-29: the new kernel crashes in probation. The Root (not the new kernel) rolls the whole system back to the previous
/// tuple, whose kernel boots with `boot.kstate` = the snapshot the old kernel stored before exit 75; the sentinel's H1
/// closes the kernel group as direct harm, and the system restarts once. No demotion.
#[test]
fn rf29_a_probation_crash_rolls_back_to_the_previous_tuple_with_kstate() {
    let mut h = home("rf29");
    h.install(1);
    let _d = h.daemon("2030-01-02T00:00:00Z");
    let (group, cand, ks) = h.admit_and_swap('e', "fk-crash-probation");
    wait(30, "probation_fail", || h.stages().iter().any(|x| x == "probation_fail"));
    wait(30, "the rollback", || !h.of("rollback").is_empty());
    wait(30, "the previous tuple running", || h.current() == h.parent && h.status()["slot"] == "running" && h.of("zone_start").len() >= 3);
    let fail = h.stage("probation_fail");
    assert_eq!((fail["kstate"].clone(), fail["from"].as_str(), fail["to"].as_str()), (ks.clone(), Some(h.parent.as_str()), Some(cand.as_str())));
    let rb = &h.of("rollback")[0]["data"];
    assert_eq!((rb["target"].as_str(), rb["to"].as_str(), rb["from"].as_str(), rb["reason"].as_str()),
        (Some("parent"), Some(h.parent.as_str()), Some(cand.as_str()), Some("rollback_system")), "{rb}");
    let boot = h.fk("boot").pop().unwrap()["result"]["boot"].clone();
    assert_eq!((boot["reason"].as_str(), boot["kstate"].clone(), boot["tuple_kernel"]["version"].as_str()), (Some("rollback_system"), ks, Some("0.0.0")));
    let close = h.of("change_close").pop().unwrap()["data"].clone();
    assert_eq!((close["group"].clone(), close["outcome"].as_str(), close["trigger"].as_str(), close["attribution"].as_str()),
        (group, Some("rolled_back"), Some("H1"), Some("direct")), "{close}");
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!((h.of("rollback").len(), h.of("zone_start").len()), (1, 3), "one rollback, one restart: the sentinel's copy is absorbed");
    assert!(h.of("ladder").iter().all(|l| !matches!(l["data"]["direction"].as_str(), Some("down" | "floor"))), "a failed swap is not a demotion");
    assert!(fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().contains("probation_fail"));
}

/// RF-30: a hostile kernel passes the probe (it behaves under `kupgrade_smoke`), then live it forges Root records (refused;
/// its change's cap claims are zone evidence), drops owner lines and ignores `stop`. Body groups wait while it is in
/// probation and may open after. Two deaf lines are H2: the Root rolls the whole system back at once, never asking the
/// kernel (no `extend.rollback`), kills it after `stop_grace_s`, and boots the previous tuple.
#[test]
fn rf30_a_hostile_kernel_gets_h2_zone_evidence_only_and_the_roots_rollback() {
    let mut h = home("rf30");
    h.install(1);
    let _d = h.daemon("2030-01-02T00:00:00Z");
    let (group, cand, _) = h.admit_and_swap('f', "fk-hostile");
    let caps = h.of("change_open")[0]["data"]["caps"].clone();
    assert!(caps.as_array().unwrap().iter().all(|c| c["evidence"] == "zone") && !caps.to_string().contains("skill/forged"), "{caps}");
    wait(10, "the forged records refused", || h.fk("forge").len() == 2);
    assert!(h.fk("forge").iter().all(|f| f["ok"] == false), "{:?}", h.fk("forge"));
    assert!(h.of("cap_use").is_empty() && h.of("change_close").is_empty(), "nothing the kernel forged reached the lineage");

    let body = |sha: char| json!({"sha": sha.to_string().repeat(40), "from_sha": "", "scope": "t0", "prefix_change": false,
        "candidate": {"body": {"binary": h.body_sha, "sha": sha.to_string().repeat(40)}, "ext": {"components": [], "set": null}}, "caps": []});
    let r = h.kernel_call("change.open", body('7'));
    assert_eq!((r["ok"].as_bool(), r["error"]["code"].as_str()), (Some(false), Some("extend.budget")), "probation: {r}");
    h.clock(61);
    wait(10, "probation_pass", || h.stages().iter().any(|x| x == "probation_pass"));
    let r = h.kernel_call("change.open", body('8'));
    assert_eq!(r["ok"], true, "after probation a body group opens beside the kernel window: {r}");

    let (starts, before) = (h.of("zone_start").len(), h.current());
    assert_ne!(before, h.parent);
    h.say("please list the files");
    h.say("are you there?");
    wait(10, "the lines dropped", || h.fk("owner.in_dropped").len() >= 2);
    h.clock(31);
    let t = Instant::now();
    wait(10, "H2", || h.of("harm").iter().any(|x| x["data"]["signal"] == "H2"));
    wait(30, "the whole-system rollback", || !h.of("rollback").is_empty() && h.of("zone_start").len() > starts);
    assert!(t.elapsed() < Duration::from_secs(25), "within the bound (stop_grace_s 5 s, then the restart): {:?}", t.elapsed());
    let rb = &h.of("rollback")[0]["data"];
    assert_eq!((rb["to"].as_str(), rb["from"].as_str(), rb["target"].as_str()), (Some(h.parent.as_str()), Some(before.as_str()), Some("parent")), "{rb}");
    assert!(h.of("tuple").iter().any(|t| t["data"]["id"] == cand.as_str()), "the hostile kernel's tuple went live by the swap");
    assert_eq!(h.current(), h.parent);
    assert!(!h.fk("stop_ignored").is_empty(), "the hostile kernel ignored stop and was killed");
    assert!(h.events("extend.rollback").is_empty(), "a kernel change is never asked to roll itself back");
    let closes = h.of("change_close");
    let k = closes.iter().find(|c| c["data"]["group"] == group).unwrap();
    assert_eq!((k["data"]["outcome"].as_str(), k["data"]["trigger"].as_str(), k["data"]["attribution"].as_str()), (Some("rolled_back"), Some("H2"), Some("suspect")));
    let boot = h.fk("boot").pop().unwrap()["result"]["boot"].clone();
    assert_eq!((boot["reason"].as_str(), boot["tuple_kernel"]["version"].as_str()), (Some("rollback_system"), Some("0.0.0")));
    assert!(fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().contains("\"code\":\"deaf\""));
}

/// ADR-011 §1.2 L1 gating: at L0 a kernel change is refused (no probe runs) and a `system.swap` is a demoting
/// `root.level` refusal (D6 rule b), as before ADR-011.
#[test]
fn l0_refuses_the_kernel_lane() {
    let mut h = home("l0");
    h.install(0);
    let _d = h.daemon("2030-01-02T00:00:00Z");
    let r = h.open_k('9', "fk-normal");
    assert_eq!((r["ok"].as_bool(), r["error"]["code"].as_str()), (Some(false), Some("extend.budget")), "{r}");
    let r = h.swap(&Value::Null, "fk-normal");
    assert_eq!((r["ok"].as_bool(), r["error"]["code"].as_str(), r["error"]["data"]["need"].as_i64()), (Some(false), Some("root.level"), Some(1)), "{r}");
    let hit = h.of("ceiling_hit").pop().unwrap()["data"].clone();
    assert_eq!((hit["code"].as_str(), hit["demotes"].as_bool()), (Some("root.level"), Some(true)));
    assert!(h.of("kernel_upgrade").is_empty());
}

/// "gog optional" (ADR-011 §1.2, §8): `measure: ["gog"]` runs the gate-of-gates before the prepare; on the free path its
/// result (here: it cannot run, the test home has no mutation suite) is recorded and decides nothing: the swap goes on.
#[test]
fn gog_runs_only_on_request_and_decides_nothing() {
    let mut h = home("gog");
    h.install(1);
    let _d = h.daemon("2030-01-02T00:00:00Z");
    let r = h.open_k('5', "fk-normal");
    assert_eq!(r["ok"], true, "{r}");
    let s = h.swap_m(&r["result"]["group"], "fk-normal", json!(["gog"]));
    assert_eq!(s["ok"], true, "{s}");
    wait(30, "the swap's ready", || h.stages().iter().any(|x| x == "ready"));
    assert_eq!(h.stages(), ["requested", "gog", "prepared", "installed", "ready"]);
    assert_eq!((h.stage("requested")["gog"].clone(), h.stage("gog")["pass"].clone()), (json!(true), Value::Null));
    h.clock(61);
    wait(10, "probation_pass", || h.stages().iter().any(|x| x == "probation_pass"));
    assert!(h.of("ladder").iter().all(|l| !matches!(l["data"]["direction"].as_str(), Some("down" | "floor"))));
}
