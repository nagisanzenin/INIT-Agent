//! X6/X7 integration tests ($0) against the real `init-root` in test mode, with `init-root-fakekernel` (tests/support)
//! as the kernel. Live-system safety (CONVENTIONS §1): every process is this test's Root daemon or one the Root spawned
//! and holds; the test signals only its own children, by handle. Homes live under `root/target/test-homes/x67/`, with
//! `HOME` pointed there, removed by exact path on pass.
//! Run with `cargo test --offline -p init-root --features test-fixtures --test x67`.
#![cfg(feature = "test-fixtures")]

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_init-root");
const FK: &str = env!("CARGO_BIN_EXE_init-root-fakekernel");

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

/// A fresh home `root/target/test-homes/x67/<name>-<pid>` (short: ROOT_HOME stays ≤ 80 bytes).
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/x67");
    let dir = base.join(format!("{name}{}", std::process::id() % 100_000));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(dir.join("rel")).unwrap();
    fs::create_dir_all(dir.join("tc/bin")).unwrap();
    fs::copy(FK, dir.join("rel/init-kernel")).unwrap();
    fs::copy(BIN, dir.join("init-root")).unwrap(); // frozen: a concurrent rebuild cannot swap it
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
}

fn charter(extra: &str) -> String {
    format!("schema_version = 1\n[purpose]\ntext = \"X67 test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n\
        [sandbox]\ndisk_floor_gib = 1\n[root]\ntest_mode = true\nhb_interval_s = 1\nhb_stale_s = 3\nkernel_hello_timeout_s = 20\n\
        backoff_initial_ms = 50\nbackoff_max_ms = 200\nstop_grace_s = 5\n{extra}")
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
    fn install(&self, charter: &str) {
        fs::create_dir_all(&self.rh).unwrap();
        fs::write(self.rh.join("charter.toml"), charter).unwrap();
        let (rel, tc) = (self.dir.join("rel"), self.dir.join("tc"));
        let o = self.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    }
    fn fk(&self, ev: &str) -> Vec<Value> {
        fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v["ev"] == ev).collect()
    }
    /// The fake kernel issues one control-link request and the test reads its outcome.
    fn kernel_call(&self, cmd: &str, args: Value) -> Value {
        let before = self.fk("call").len();
        let (tmp, dst) = (self.ih.join("work/fk-call.tmp"), self.ih.join("work/fk-call.json"));
        fs::create_dir_all(self.ih.join("work")).unwrap();
        fs::write(&tmp, json!({"cmd": cmd, "args": args}).to_string()).unwrap();
        fs::rename(tmp, dst).unwrap();
        wait(15, "the kernel call outcome", || self.fk("call").len() > before);
        self.fk("call").into_iter().nth(before).unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        assert_eq!(o.status.code(), Some(0), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap_or(Value::Null)
    }
    fn records(&self, kind: &str) -> Vec<Value> {
        fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|r| r["kind"] == kind).map(|r| r["data"].clone()).collect()
    }
    /// An owner line on `owner.sock`, as the TUI sends it.
    fn say(&self, text: &str) {
        let mut s = UnixStream::connect(self.rh.join("run/owner.sock")).unwrap();
        writeln!(s, "{}", json!({"t": "line", "text": text})).unwrap();
        s.flush().unwrap();
        std::thread::sleep(Duration::from_millis(300));
    }
    fn day_musd(&self) -> u64 {
        self.ok(&["status", "--json"])["ceilings"]["day_musd"].as_u64().unwrap()
    }
    fn daemon(&self, now: &str) -> Daemon {
        let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
        let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
        let c = self.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", "")
            .env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir")).env("INIT_TEST_HOME_REAL", &self.dir)
            .env("INIT_ROOT_TEST_NOW", now).spawn().unwrap();
        let d = Daemon(c, self.rh.clone());
        wait(10, "owner.sock", || UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        wait(15, "the fake kernel reached ready", || self.fk("ready").iter().any(|e| e["ok"] == true));
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

/// ADR-011 decision 11 (for K2-ADMIT): the kernel learns the effective `extend.mode` from `hello`'s boot block and from
/// `ceilings.get`: `"free"` with `[extend] mode = "free"`, `"gated"` without an `[extend]` section.
#[test]
fn the_kernel_reads_extend_mode_from_boot_and_ceilings() {
    for (name, extra, want) in [("free", "[extend]\nmode = \"free\"\n", "free"), ("gated", "", "gated")] {
        let h = home(name);
        h.install(&charter(extra));
        let d = h.daemon("2030-01-01T00:00:00Z");
        let boot = h.fk("boot").pop().unwrap();
        assert_eq!(boot["result"]["extend"]["mode"], want, "{name}: {boot}");
        let c = h.kernel_call("ceilings.get", json!({}));
        assert_eq!((c["ok"].as_bool(), c["result"]["extend"]["mode"].as_str()), (Some(true), Some(want)), "{name}: {c}");
        drop(d);
        fs::remove_dir_all(&h.dir).unwrap();
    }
}

/// RF-27 through the real wire: the kernel's `grant.request` is stored, recorded and alerted, and changes no value; a
/// Root-mechanism kind is refused; `/grant <id>` without the typed confirmation changes nothing; `/grant <id> for 2d
/// confirm` applies the value (the money ceiling here) and records it; after the fake clock passes `until` the grant
/// expires and the charter's value is back.
#[test]
fn rf27_grant_over_the_control_link_and_owner_lines() {
    let h = home("grant");
    h.install(&charter(""));
    let d = h.daemon("2030-01-01T00:00:00Z");
    let bad = h.kernel_call("grant.request", json!({"kind": "sandbox.read_deny", "value": 1, "why": "x"}));
    assert_eq!(bad["error"]["code"], "root.args", "{bad}");
    let r = h.kernel_call("grant.request", json!({"kind": "money.day_usd", "value": "8.00", "duration_h": 24, "why": "a big refactor today"}));
    assert_eq!(r["ok"], true, "{r}");
    let id = r["result"]["id"].as_str().unwrap().to_string();
    assert_eq!(h.day_musd(), 5_000_000, "a request changes no value");
    assert_eq!(h.records("grant_request")[0]["kind"], "money.day_usd");
    assert!(fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().contains(&id));
    h.say(&format!("/grant {id}"));
    assert!(h.records("grant").is_empty() && h.day_musd() == 5_000_000, "no typed confirmation, no grant");
    h.say(&format!("/grant {id} for 2d confirm"));
    wait(10, "the grant record", || !h.records("grant").is_empty());
    let g = h.records("grant")[0].clone();
    assert_eq!((g["decision"].as_str(), g["until"].as_str(), g["value"].as_str()), (Some("approve"), Some("2030-01-03T00:00:00.000Z"), Some("8.00")));
    assert_eq!(h.day_musd(), 8_000_000, "the effective charter carries the grant");
    h.ok(&["test", "clock", "--advance", &(3 * 86_400).to_string()]);
    wait(10, "the expiry record", || h.records("grant").len() == 2);
    assert_eq!(h.records("grant")[1]["decision"], "expired");
    assert_eq!(h.day_musd(), 5_000_000, "back to the charter's value");
    drop(d);
    fs::remove_dir_all(&h.dir).unwrap();
}
