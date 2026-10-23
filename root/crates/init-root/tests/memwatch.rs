//! MEM-RUNAWAY, the zone path end to end (`--features test-fixtures`, macOS, $0, no disk fill): a test Root daemon with
//! the fake kernel, whose injected `spawn` (`work/fk-call.json`) starts a body instance under the real `body` profile.
//! The body's script starts a hog (allocates and touches 400 MiB) and a sibling sleeper. With `sandbox.mem_cap_mib =
//! 256` the Root's zone watchdog kills the hog alone, by pid; the body (the leader) and the sibling keep running; the
//! kill is in `INIT_HOME/root/mem-events.jsonl` (what the body's bash tool reads) and in `alerts.jsonl`. With a fake
//! disk floor far above any real free space, a workload spawn is refused (`root.quota`, `disk_floor`).
//! Live-system safety (CONVENTIONS §1): every process here is this test's own child (the daemon) or a process its Root
//! spawned and holds; the test signals only the daemon, by its handle. Homes: `root/target/test-homes/mw/`.

#![cfg(all(feature = "test-fixtures", target_os = "macos"))]

use serde_json::{Value, json};
use std::fs;
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

fn wait(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "not within {secs} s: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `ps -o stat=`: is `pid` a live process (not a zombie)? Read-only.
fn live(pid: &str) -> bool {
    !pid.is_empty()
        && Command::new("/bin/ps").args(["-o", "stat=", "-p", pid]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .is_ok_and(|s| !s.is_empty() && !s.starts_with('Z'))
}

/// A fresh home `root/target/test-homes/mw/<name><pid>` (short: ROOT_HOME must stay ≤ 80 bytes).
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/mw");
    let dir = base.join(format!("{name}{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(dir.join("rel")).unwrap();
    fs::create_dir_all(dir.join("tc/bin")).unwrap();
    fs::copy(FK, dir.join("rel/init-kernel")).unwrap();
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
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
    fn install(&self, sandbox: &str) {
        fs::create_dir_all(&self.rh).unwrap();
        let charter = format!("schema_version = 1\n[purpose]\ntext = \"MEM-RUNAWAY test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\n\
            disk_min_free_gib = 0\n[sandbox]\n{sandbox}\n{ROOT_CFG}\n");
        fs::write(self.rh.join("charter.toml"), charter).unwrap();
        let (rel, tc) = (self.dir.join("rel"), self.dir.join("tc"));
        let o = self.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    }
    fn info(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap()
    }
    fn daemon(&self) -> Daemon {
        let info = self.info();
        let mut c = self.cmd();
        c.arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", "")
            .env("INIT_TEST_TOOLCHAIN", info["toolchain"].as_str().unwrap()).env("INIT_TEST_DEVDIR", info["devdir"].as_str().unwrap())
            .env("INIT_TEST_HOME_REAL", &self.dir);
        let d = Daemon(c.spawn().unwrap(), self.rh.clone());
        wait(10, "owner.sock", || std::os::unix::net::UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        wait(20, "the fake kernel is ready", || !self.fk("ready").is_empty());
        d
    }
    fn fk(&self, ev: &str) -> Vec<Value> {
        let s = fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["ev"] == ev).collect()
    }
    /// One request through the fake kernel (`work/fk-call.json`), and its logged outcome.
    fn call(&self, cmd: &str, args: Value) -> Value {
        let n = self.fk("call").len();
        let tmp = self.ih.join("work/.fk-call.json.tmp");
        fs::write(&tmp, json!({"cmd": cmd, "args": args}).to_string()).unwrap();
        fs::rename(&tmp, self.ih.join("work/fk-call.json")).unwrap();
        let mut v = Vec::new();
        wait(15, "the injected call's outcome", || {
            v = self.fk("call");
            v.len() > n
        });
        v.pop().unwrap()
    }
    /// The fake kernel's body spawn args (tests/support/fakekernel.rs `spawn_args`) with another program.
    fn body_args(&self, program: &str, args: &[&str]) -> Value {
        let i = self.ih.display().to_string();
        let info = self.info();
        let mut p = serde_json::Map::new();
        for (k, sub) in [("BIN", "/bin"), ("BODY", "/body"), ("CACHE", "/cache"), ("INIT_HOME", ""), ("KERNEL", "/kernel"),
            ("KERNEL_SOCK", "/run/kernel.sock"), ("LINEAGE", "/lineage"), ("LINK_BIN", "/bin"), ("LINK_KERNEL", "/kernel"),
            ("LINK_LINEAGE", "/lineage"), ("PROXY_SOCK", "/run/proxy.sock"), ("PUB", "/root"), ("RUN", "/run"), ("STATE", "/state"),
            ("STATE_KERNEL", "/state/kernel"), ("VAULT", "/vault"), ("WORK", "/work")] {
            p.insert(k.into(), json!(format!("{i}{sub}")));
        }
        p.insert("TOOLCHAIN".into(), info["toolchain"].clone());
        p.insert("DEVDIR".into(), info["devdir"].clone());
        p.insert("HOME_REAL".into(), json!(self.dir));
        json!({"args": args, "cwd": format!("{i}/work"), "env": {"PATH": "/usr/bin:/bin"}, "kind": "body", "lane": "live", "params": p,
            "profile": "body", "program": program})
    }
}

/// The Root daemon this test spawned. On drop: an orderly `stop`, then the daemon itself by its handle.
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
        let _ = self.0.kill(); // our own child, by its handle
        let _ = self.0.wait();
    }
}

#[test]
fn the_zone_watchdog_kills_only_the_workload_over_the_cap() {
    let h = home("c");
    h.install("mem_cap_mib = 256");
    let d = h.daemon();
    let w = h.ih.join("work");
    // the paths hold a space ("INIT Agent"): every one is quoted
    assert!(!w.display().to_string().contains('\''));
    let script = format!("echo $$ > '{w}/leader'; echo \"$INIT_MEM_CAP_MIB $INIT_MEM_EVENTS\" > '{w}/env'; \
        /usr/bin/perl -e '$x = \"a\" x (400 * 1024 * 1024); sleep 60' & echo $! > '{w}/hog'; /bin/sleep 60 & echo $! > '{w}/sib'; wait",
        w = w.display());
    let r = h.call("spawn", h.body_args("/bin/sh", &["-c", &script]));
    assert_eq!(r["ok"], true, "{r}");
    let read = |f: &str| fs::read_to_string(w.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
    wait(10, "the body started its workloads", || !read("sib").is_empty() && live(&read("hog")));
    let events = h.ih.join("root/mem-events.jsonl");
    assert_eq!(read("env"), format!("256 {}", events.display()), "the Root gives the body its side of the watchdog");
    let t0 = Instant::now();
    wait(15, "the hog is killed", || !live(&read("hog")));
    let took = t0.elapsed();
    assert!(live(&read("leader")) && live(&read("sib")), "the body (the leader) and its other workload keep running");
    let recs: Vec<Value> = fs::read_to_string(&events).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(recs.len(), 1, "{recs:?}");
    let rec = &recs[0];
    assert_eq!((rec["event"].as_str(), rec["scope"].as_str(), rec["kind"].as_str(), rec["cap_mib"].as_u64()),
        (Some("mem_cap"), Some("unit"), Some("body"), Some(256)), "{rec}");
    assert_eq!(rec["pgid"].as_i64().map(|p| p.to_string()), Some(read("hog")));
    assert!(rec["footprint_mib"].as_u64().unwrap() > 256);
    let alerts = fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap();
    assert!(alerts.contains("\"code\":\"mem_cap\"") && alerts.contains("a workload of body instance"), "{alerts}");
    eprintln!("MEM-RUNAWAY zone: hog killed {took:?} after it ran; record {rec}");
    drop(d);
    fs::remove_dir_all(&h.dir).unwrap(); // exact path, created by this test
}

#[test]
fn below_the_disk_floor_workloads_are_refused() {
    let h = home("f");
    h.install("disk_floor_gib = 1024");
    let d = h.daemon();
    let r = h.call("spawn", json!({"kind": "check", "profile": "build", "params": {}, "program": "/usr/bin/true", "args": [],
        "env": {}, "cwd": h.ih.join("work")}));
    assert_eq!(r["ok"], false, "{r}");
    assert_eq!((r["error"]["code"].as_str(), r["error"]["data"]["quota"].as_str()), (Some("root.quota"), Some("disk_floor")), "{r}");
    assert!(r["error"]["message"].as_str().unwrap().contains("below the workload floor sandbox.disk_floor_gib = 1024 GiB"), "{r}");
    // a body may still start: its tool processes are what the watchdog polices
    let b = h.call("spawn", h.body_args("/bin/sleep", &["30"]));
    assert_eq!(b["ok"], true, "{b}");
    wait(5, "the disk_low record", || fs::read_to_string(h.ih.join("root/mem-events.jsonl")).is_ok_and(|s| s.contains("\"disk_low\"")));
    drop(d);
    fs::remove_dir_all(&h.dir).unwrap(); // exact path, created by this test
}
