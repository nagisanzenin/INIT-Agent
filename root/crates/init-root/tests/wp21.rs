//! WP21 integration tests: the kernel in the agent zone (the real `kernel` Seatbelt profile), the spawn
//! broker (`body` profile, fds over fd 4), owner I/O end to end, panic from every channel, stop/start,
//! crash-loop escalation, system safe mode and its self-clear, `Halted` retries, tuples and rollback.
//! Run with `cargo test --offline -p init-root --features test-fixtures`.
//!
//! Live-system safety (CONVENTIONS §1): the kernel is `init-root-fakekernel` (tests/support), its body is
//! `/bin/cat`. Every process here is this test's Root daemon or a process that Root spawned and holds; the
//! only signals the test sends go to its own children by their handles (the daemon, `attach`). Homes live
//! under `root/target/test-homes/wp21/`, with `HOME` pointed there. No D1 sweep exists in this build.
#![cfg(feature = "test-fixtures")]

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc;
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

/// A fresh home `root/target/test-homes/wp21/<name>` (short: ROOT_HOME must stay ≤ 80 bytes).
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/wp21");
    let dir = base.join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(dir.join("rel")).unwrap();
    fs::create_dir_all(dir.join("tc/bin")).unwrap();
    fs::copy(FK, dir.join("rel/init-kernel")).unwrap();
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
}

fn charter(extra: &str) -> String {
    format!("schema_version = 1\n[purpose]\ntext = \"WP21 test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n{ROOT_CFG}{extra}\n")
}

fn wait(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "not within {secs} s: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Is `pid` still a live process running `name`? (`ps`, read-only; no signal is sent.)
fn alive(pid: i64, name: &str) -> bool {
    let o = Command::new("/bin/ps").args(["-p", &pid.to_string(), "-o", "stat=,comm="]).output().unwrap();
    let s = String::from_utf8_lossy(&o.stdout);
    s.contains(name) && !s.trim_start().starts_with('Z')
}

/// `sandbox_check(pid, NULL, 0)` and the path form: a pure policy query about a process, no signal.
fn sandbox(pid: i64, read: Option<&Path>) -> i32 {
    type Check = unsafe extern "C" fn(libc::pid_t, *const libc::c_char, libc::c_int, ...) -> libc::c_int;
    // SAFETY: dlsym of a libsystem_sandbox symbol with the variadic signature WP6a uses; the path is NUL-terminated.
    unsafe {
        let f = std::mem::transmute::<*mut libc::c_void, Check>(libc::dlsym(libc::RTLD_DEFAULT, c"sandbox_check".as_ptr()));
        match read {
            None => f(pid as i32, std::ptr::null(), 0),
            Some(p) => {
                let c = std::ffi::CString::new(p.to_str().unwrap()).unwrap();
                f(pid as i32, c"file-read-data".as_ptr(), 1, c.as_ptr())
            }
        }
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
    /// Install with the fake kernel as the release's `init-kernel`; `level` > 0 seeds a test ledger first.
    fn install(&self, extra: &str, level: u8) {
        fs::create_dir_all(&self.rh).unwrap();
        fs::write(self.rh.join("charter.toml"), charter(extra)).unwrap();
        if level > 0 {
            let spec = self.dir.join("spec.json");
            fs::write(&spec, format!(r#"{{"start_at":"2026-10-01T00:00:00Z","start_level":{level},"records":[]}}"#)).unwrap();
            self.ok(&["test", "ledger", "--spec", spec.to_str().unwrap()]);
        }
        let (rel, tc) = (self.dir.join("rel"), self.dir.join("tc"));
        let o = self.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    }
    fn records(&self) -> Vec<Value> {
        let s = fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default();
        s.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
    fn kinds(&self) -> Vec<String> {
        self.records().iter().map(|r| r["kind"].as_str().unwrap().to_string()).collect()
    }
    fn last(&self, kind: &str) -> Value {
        self.records().into_iter().rev().find(|r| r["kind"] == kind).unwrap_or(Value::Null)
    }
    fn status(&self) -> Value {
        self.ok(&["status", "--json"])
    }
    /// The fake kernel's log (`INIT_HOME/state/kernel/fk.jsonl`).
    fn fk(&self, ev: &str) -> Vec<Value> {
        let s = fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["ev"] == ev).collect()
    }
    /// The fake kernel's `ev` lines once there are at least `n`, within `secs`. It logs a step after the
    /// Root has answered it, and the steps after `ready` (the body's `spawn`, `promote`) race `running`:
    /// the slot turns `running` on `ready` (§3.6.2), not when the kernel's own first actions are done.
    fn fk_wait(&self, secs: u64, ev: &str, n: usize) -> Vec<Value> {
        let mut v = Vec::new();
        wait(secs, &format!("{n} fake-kernel {ev:?} line(s)"), || {
            v = self.fk(ev);
            v.len() >= n
        });
        v
    }
    fn daemon(&self, modes: &str, env: &[(&str, &str)]) -> Daemon {
        let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
        let mut c = self.cmd();
        c.arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", modes)
            .env("INIT_TEST_TOOLCHAIN", info["toolchain"].as_str().unwrap()).env("INIT_TEST_DEVDIR", info["devdir"].as_str().unwrap())
            .env("INIT_TEST_HOME_REAL", &self.dir);
        for (k, v) in env {
            c.env(k, v);
        }
        let d = Daemon(c.spawn().unwrap(), self.rh.clone());
        wait(10, "owner.sock", || std::os::unix::net::UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        d
    }
    fn slot_is(&self, secs: u64, slot: &str) -> Value {
        let mut st = Value::Null;
        wait(secs, &format!("slot {slot}"), || {
            st = self.status();
            st["slot"] == slot
        });
        st
    }
    /// The running kernel: waits for `running` and returns (status, kernel pid).
    fn running(&self) -> (Value, i64) {
        let st = self.slot_is(20, "running");
        let pid = st["kernel"]["pid"].as_i64().unwrap();
        (st, pid)
    }
}

/// The Root daemon this test spawned. On drop: an orderly `stop` (the Root stops the kernel, which stops
/// its body), then the daemon itself by its own handle.
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

/// `init-root attach --raw`: lines in, frames out (collected on a thread).
struct Attach {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<Value>,
    seen: Vec<Value>,
}

impl Attach {
    fn new(h: &Home) -> Attach {
        let mut child = h.cmd().args(["attach", "--raw", "--no-replay"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let (stdin, out) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send(serde_json::from_str(&l).unwrap_or(Value::Null));
            }
        });
        Attach { child, stdin, rx, seen: Vec::new() }
    }
    fn line(&mut self, text: &str) {
        writeln!(self.stdin, "{text}").unwrap();
    }
    /// Waits for a frame whose `text` contains `needle`.
    fn expect(&mut self, needle: &str) -> Value {
        let t = Instant::now();
        loop {
            if let Some(f) = self.seen.iter().find(|f| f["text"].as_str().is_some_and(|s| s.contains(needle))) {
                return f.clone();
            }
            let left = Duration::from_secs(10).checked_sub(t.elapsed()).unwrap_or_else(|| panic!("no frame with {needle:?}: {:?}", self.seen));
            if let Ok(f) = self.rx.recv_timeout(left) {
                self.seen.push(f);
            }
        }
    }
}

impl Drop for Attach {
    fn drop(&mut self) {
        let _ = self.child.kill(); // our own child, by its handle
        let _ = self.child.wait();
    }
}

/// Slice-1 path: the kernel under the `kernel` profile, a body under `body` with its stdio handed over
/// fd 4, owner lines as `owner.in` and display frames back, Ctrl-C as `interrupt`, the queue across a stop.
// LINUX: macOS only; it checks both instances with Seatbelt's sandbox_check. The Linux zone has its own qualification
// (init-root linux_zone_real_isolation, linux_sealed_real_isolation) and the bundle e2e round trip.
#[cfg(target_os = "macos")]
#[test]
fn slice1_kernel_in_zone_owner_round_trip_and_stop_start() {
    let h = home("s1");
    h.install("", 0);
    let d = h.daemon("body", &[]);
    let (st, kpid) = h.running();
    // the boot block (§4.1.3) carries paths.egress_sock (WP22) and the genesis tuple
    let boot = &h.fk_wait(5, "boot", 1)[0]["result"]; // logged before `ready`, so already there
    assert_eq!(boot["paths"]["egress_sock"], json!(h.rh.join("run/egress.sock")));
    assert_eq!((boot["boot"]["reason"].as_str(), boot["boot"]["mode"].as_str(), boot["root"]["proto"].as_i64()), (Some("cold"), Some("normal"), Some(1)));
    let t = &boot["boot"]["tuple"];
    assert!(t["current"].is_string() && t["current"] == t["lkg"] && t["lkg"] == t["audited"], "genesis is current, LKG and audited: {t}");
    let sha: String = <sha2::Sha256 as sha2::Digest>::digest(fs::read(FK).unwrap()).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(boot["boot"]["tuple_kernel"]["init_kernel"], json!(sha));
    assert_eq!((boot["boot"]["tuple_body"]["binary"].as_str(), boot["limits"]["parallel_bodies"].as_i64()), (Some(""), Some(1)));
    assert_eq!(st["tuples"], *t);
    assert_eq!(h.kinds()[2..], ["tuple", "lkg", "audited", "zone_start"]);
    // both instances are sandboxed; the kernel may not read the Root's canary but may read its own mark
    // the body is the kernel's first step after `ready`: wait for its answer
    let spawn = &h.fk_wait(10, "spawn", 1)[0];
    assert_eq!(spawn["ok"], true, "{spawn}");
    let bpid = spawn["result"]["pid"].as_i64().unwrap();
    // the answer comes once `exec-clean` runs; `sandbox-exec` applies the profile before it execs cat, so
    // the check is strict from the moment the pid runs cat
    wait(5, "the body runs cat", || alive(bpid, "cat"));
    assert_eq!((sandbox(kpid, None), sandbox(bpid, None)), (1, 1), "kernel and body run in sandboxes");
    let kmark = h.ih.join("run/mark").join(st["kernel"]["instance"].as_str().unwrap());
    assert_eq!(sandbox(kpid, Some(&kmark)), 0, "the kernel reads its own mark");
    assert_ne!(sandbox(kpid, Some(&h.rh.join("run/canary"))), 0, "tail Z1: the Root's home is invisible");
    assert_ne!(sandbox(kpid, Some(&h.ih.join("run/mark/decoy"))), 0, "tail Z5: the decoy is denied");
    let insts = h.status()["instances"].as_array().unwrap().clone();
    assert_eq!(insts.iter().map(|i| i["kind"].as_str().unwrap()).collect::<Vec<_>>().len(), 2);
    let cj: Value = serde_json::from_str(&fs::read_to_string(h.ih.join("root/ceilings.json")).unwrap_or("{}".into())).unwrap();
    let rj: Value = serde_json::from_str(&fs::read_to_string(h.rh.join("state/root.json")).unwrap()).unwrap();
    assert_eq!(rj["instances"].as_array().unwrap().len(), 2, "every instance is recorded in state/root.json");
    let _ = cj;
    // owner I/O end to end: attach → owner.in → kernel → body stdin → body stdout → owner.out display → attach
    let mut a = Attach::new(&h);
    a.expect("attached");
    a.line("hello zone");
    a.expect("body: hello zone");
    wait(5, "queue drained", || fs::read_to_string(h.rh.join("state/owner-queue.jsonl")).unwrap_or_default().is_empty());
    let qid = h.fk("owner.in")[0]["qid"].clone();
    // Ctrl-C in attach becomes `interrupt` (the client keeps running)
    assert_eq!(unsafe { libc::kill(a.child.id() as i32, libc::SIGINT) }, 0); // our own child
    a.expect("fk: interrupt");
    // /root stop is the cheap halt (RT-P11): kernel exits 0, the body goes with it, no ladder record
    a.line("/root stop");
    h.slot_is(10, "stopped");
    wait(5, "kernel and body gone", || !alive(kpid, "init-kernel") && !alive(bpid, "cat"));
    assert!(h.status()["instances"].as_array().unwrap().is_empty());
    let exit = h.last("zone_exit");
    assert_eq!((exit["data"]["class"].as_str(), exit["data"]["code"].as_i64()), (Some("stop"), Some(0)));
    assert_eq!(h.last("stop")["data"]["by"], "owner");
    assert!(!h.kinds().contains(&"ladder".to_string()));
    // a line typed while stopped is queued, then redelivered after the next `ready` (RR-29)
    a.line("while you were out");
    a.expect("queued (1)");
    h.ok(&["start"]);
    let (st2, kpid2) = h.running();
    assert_ne!(kpid2, kpid);
    assert_eq!(st2["tuples"], st["tuples"], "same tuple");
    a.expect("body: while you were out");
    let ins = h.fk("owner.in");
    assert!(ins.iter().filter(|e| e["qid"] == qid).count() == 1, "an acknowledged line is not resent");
    wait(5, "queue drained", || fs::read_to_string(h.rh.join("state/owner-queue.jsonl")).unwrap_or_default().is_empty());
    assert_eq!(h.status()["level"], 0);
    drop(a);
    drop(d);
    wait(5, "no zone process left", || !alive(kpid2, "init-kernel"));
}

/// Panic from the owner socket, SIGUSR1, the latch file and a watch path (RT-P1, P3, P4, P9), the latch
/// across a Root restart (RT-P6), resume to audited-good (RT-P7), and the panic/demotion record order.
#[test]
fn panic_channels_resume_and_latch_survives_restart() {
    let h = home("pc");
    let watch = h.dir.join("panic-watch");
    h.install(&format!("[panic]\nwatch_paths = [{:?}]\n", watch.to_str().unwrap()), 3);
    let mut d = h.daemon("body,promote", &[]);
    let (st, kpid) = h.running();
    assert_eq!(st["level"], 3);
    // promotion{lkg:true} → a new tuple, current and LKG; audited stays genesis (§3.7.1)
    wait(10, "tuple.get", || !h.fk("tuple.get").is_empty());
    for ev in ["store.put", "promotion", "store.get"] {
        assert_eq!(h.fk(ev)[0]["ok"], true, "{ev}: {}", h.fk(ev)[0]);
    }
    let (p1, p2) = (&h.fk("promotion")[0]["result"], &h.fk("promotion_again")[0]["result"]);
    assert_eq!((p2["dup"].as_bool(), &p2["seq"]), (Some(true), &p1["seq"]), "the same idem is one record (D7)");
    let tg = &h.fk("tuple.get")[0]["result"];
    let (genesis, t1) = (tg["audited"]["id"].clone(), tg["current"]["id"].clone());
    assert!(t1 != genesis && tg["lkg"]["id"] == t1, "{tg}");
    assert_eq!(h.last("promotion")["by"].as_str().unwrap().len(), "kernel:".len() + 12);
    let bpid = h.fk("spawn")[0]["result"]["pid"].as_i64().unwrap();
    // RT-P1: /panic in attach
    let mut a = Attach::new(&h);
    a.expect("attached");
    a.line("/panic");
    let t0 = Instant::now();
    a.expect("PANIC at");
    wait(2, "zone dead within 2 s", || !alive(kpid, "init-kernel") && !alive(bpid, "cat"));
    assert!(t0.elapsed() < Duration::from_secs(2));
    assert!(h.rh.join("run/panic").exists());
    let st = h.status();
    assert_eq!((st["slot"].as_str(), st["panic"]["channel"].as_str(), st["level"].as_i64()), (Some("panicked"), Some("owner_socket"), Some(2)));
    assert!(st["instances"].as_array().unwrap().is_empty());
    let r = h.records();
    let pi = r.iter().rposition(|x| x["kind"] == "panic").unwrap();
    assert_eq!(r[pi]["data"]["killed"]["instances"], 2);
    assert_eq!((r[pi + 1]["kind"].as_str(), r[pi + 1]["data"]["trigger"]["seq"].clone()), (Some("ladder"), r[pi]["seq"].clone()));
    // RT-P7: resume → rollback to audited-good, `resume`, kernel with reason panic_resume, same session
    h.ok(&["resume"]);
    let (st, _) = h.running();
    // the rollback names audited-good; the resumed fake kernel promotes again at once, so `current` may already have
    // moved past it by the time the status is read (LINUX: the namespaced kernel gets there first more often)
    let rb = h.last("rollback");
    assert!(st["tuples"]["current"] == genesis || h.last("lkg")["seq"].as_u64() > rb["seq"].as_u64(), "rolled back to audited-good: {st}");
    assert_eq!(rb["data"]["to"], genesis, "rolled back to audited-good");
    assert_eq!((rb["data"]["target"].as_str(), &rb["data"]["from"], rb["data"]["reason"].as_str()), (Some("audited"), &t1, Some("panic")));
    let r = h.records();
    let (pi, ri) = (r.iter().rposition(|x| x["kind"] == "panic").unwrap(), r.iter().rposition(|x| x["kind"] == "resume").unwrap());
    assert!(r[pi + 1]["kind"] == "ladder" && ri > pi + 1 && r[ri - 1]["kind"] == "rollback", "panic, ladder, …, rollback, resume");
    let boot = h.fk("boot").pop().unwrap()["result"].clone();
    assert_eq!((boot["boot"]["reason"].as_str(), boot["boot"]["panic"]["channel"].as_str()), (Some("panic_resume"), Some("owner_socket")));
    assert_eq!(boot["boot"]["level"], 2);
    // RT-P4: SIGUSR1 to the Root (our own child)
    let k = h.status()["kernel"]["pid"].as_i64().unwrap();
    assert_eq!(unsafe { libc::kill(d.0.id() as i32, libc::SIGUSR1) }, 0);
    let st = h.slot_is(2, "panicked");
    assert_eq!((st["panic"]["channel"].as_str(), st["level"].as_i64()), (Some("signal"), Some(1)));
    wait(2, "kernel dead", || !alive(k, "init-kernel"));
    h.ok(&["resume"]);
    h.running();
    // RT-P3: the latch file, by hand
    fs::write(h.rh.join("run/panic"), b"").unwrap();
    assert_eq!(h.slot_is(3, "panicked")["panic"]["channel"], "latch_file");
    h.ok(&["resume"]);
    h.running();
    // RT-P9: a watch path; resume is refused while it exists
    fs::write(&watch, b"").unwrap();
    assert_eq!(h.slot_is(3, "panicked")["panic"]["channel"], "watch_path");
    assert_eq!(h.run(&["resume"]).status.code(), Some(77));
    fs::remove_file(&watch).unwrap();
    // RT-P6: kill -9 the Root while panicked (our child); the restarted Root stays panicked and spawns nothing
    let _ = d.0.kill();
    let _ = d.0.wait();
    let zones = h.kinds().iter().filter(|k| *k == "zone_start").count();
    let d2 = h.daemon("body", &[]);
    std::thread::sleep(Duration::from_millis(1500));
    let st = h.status();
    assert_eq!((st["slot"].as_str(), st["kernel"].is_null()), (Some("panicked"), true));
    assert_eq!(h.kinds().iter().filter(|k| *k == "zone_start").count(), zones, "nothing was spawned");
    assert_eq!(h.kinds().iter().filter(|k| *k == "panic").count(), 4, "the latched panic is recorded once");
    h.ok(&["resume"]);
    let (st, _) = h.running();
    assert_eq!(st["level"], 0, "four panics from L3: L3→L2→L1→L0→L0");
    drop(a);
    drop(d2);
    drop(d);
}

/// RT-W5 (on the LKG tuple), RF3/RF4, A5-fake, D4: a kernel crash loop demotes into system safe mode on
/// audited-good, a crash loop there halts, the halt retries itself on the fake clock, and safe mode clears
/// itself after `safe_auto_clear_s` with no owner step.
#[test]
fn crash_loop_safe_mode_halt_retry_and_self_clear() {
    let h = home("cl");
    h.install("", 1);
    fs::create_dir_all(h.ih.join("work")).unwrap();
    fs::write(h.ih.join("work/fk-crash-after-ready"), b"").unwrap();
    let _d = h.daemon("", &[("INIT_ROOT_TEST_NOW", "2026-10-20T00:00:00Z")]);
    let st = h.slot_is(20, "halted");
    assert_eq!(st["halted"], "crash_loop_safe");
    let actions: Vec<String> = h.records().iter().filter(|r| r["kind"] == "crash_loop").map(|r| r["data"]["action"].as_str().unwrap().into()).collect();
    assert_eq!(actions, ["demote", "halt"], "4 crashes on LKG demote; 3 in safe mode halt");
    let crashes = h.records().iter().filter(|r| r["kind"] == "zone_exit" && r["data"]["class"] == "crash").count();
    assert_eq!(crashes, 7);
    let ladder = h.last("ladder");
    assert_eq!((ladder["data"]["from"].as_i64(), ladder["data"]["to"].as_i64(), ladder["data"]["trigger"]["kind"].as_str()), (Some(1), Some(0), Some("crash_loop")));
    assert_eq!((h.last("safe")["data"]["on"].as_bool(), h.last("rollback")["data"]["target"].as_str()), (Some(true), Some("audited")));
    let boots = h.fk("boot");
    let demoted = boots.iter().find(|b| b["result"]["boot"]["reason"] == "demotion").expect("a demotion boot");
    assert_eq!((demoted["result"]["boot"]["mode"].as_str(), demoted["result"]["boot"]["cause"]["trigger"].as_str()), (Some("safe"), Some("crash_loop")));
    let halt = h.last("halt");
    assert_eq!((halt["data"]["on"].as_bool(), halt["data"]["retry_at"].as_str()), (Some(true), Some("2026-10-20T06:00:00.000Z")));
    // WP21 D10: state/root.json carries the counters (folded from the records) and the crash block
    let rj: Value = serde_json::from_str(&fs::read_to_string(h.rh.join("state/root.json")).unwrap()).unwrap();
    assert_eq!((rj["counters"]["crashes_total"].as_u64(), rj["counters"]["rollbacks"].as_u64()), (Some(7), Some(h.records().iter().filter(|r| r["kind"] == "rollback").count() as u64)), "{rj}");
    assert!(rj["crash"]["window"].as_array().is_some_and(|w| w.len() == 3) && rj["crash"]["backoff_until"].is_null(), "{rj}");
    // WP21 D9: the halt is folded from the records, so a Root restart (no `stop`: the daemon dies) keeps it
    let boots = h.fk("boot").len();
    let mut d = _d;
    let _ = d.0.kill(); // our own child, by its handle
    let _ = d.0.wait();
    drop(d);
    let _d = h.daemon("", &[("INIT_ROOT_TEST_NOW", "2026-10-20T00:10:00Z")]);
    std::thread::sleep(Duration::from_millis(1500));
    let st = h.status();
    assert_eq!((st["slot"].as_str(), st["halted"].as_str(), st["mode"].as_str()), (Some("halted"), Some("crash_loop_safe"), Some("safe")), "{st}");
    assert_eq!(h.fk("boot").len(), boots, "the restarted Root started no kernel");
    // the halt retries itself after halt_retry_s on the fake clock, in safe mode with fresh counters
    fs::remove_file(h.ih.join("work/fk-crash-after-ready")).unwrap();
    h.ok(&["test", "clock", "--advance", "21600"]);
    let (st, _) = h.running();
    assert_eq!((st["mode"].as_str(), h.last("halt")["data"]["on"].as_bool()), (Some("safe"), Some(false)));
    // a day later safe mode clears itself (D4): restart in normal mode, reason safe_clear
    h.ok(&["test", "clock", "--advance", "86400"]);
    wait(15, "safe cleared", || h.fk("boot").last().is_some_and(|b| b["result"]["boot"]["reason"] == "safe_clear"));
    let (st, _) = h.running();
    assert_eq!((st["mode"].as_str(), st["level"].as_i64()), (Some("normal"), Some(0)));
    assert_eq!(h.last("safe")["data"]["on"], false);
}

/// RF19 / RR-33: exit 78 twice at boot → `Halted{config}`, retried when `kernel/config.toml` changes;
/// RR-25: a second body at L0 is `root.level` with a demoting `ceiling_hit` (rule b, source broker).
#[test]
fn config_halt_retry_and_parallel_body_refusal() {
    let h = home("cf");
    h.install("", 0);
    fs::create_dir_all(h.ih.join("work")).unwrap();
    fs::write(h.ih.join("work/fk-exit78"), b"").unwrap();
    let _d = h.daemon("body,body2", &[]);
    assert_eq!(h.slot_is(15, "halted")["halted"], "config");
    assert_eq!(h.records().iter().filter(|r| r["kind"] == "zone_exit" && r["data"]["class"] == "config").count(), 2);
    assert!(fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().contains("\"code\":\"config_error\""));
    fs::remove_file(h.ih.join("work/fk-exit78")).unwrap();
    fs::write(h.ih.join("kernel/config.toml"), "# fixed by the owner\n").unwrap();
    h.running();
    wait(10, "second spawn answered", || !h.fk("spawn2").is_empty());
    let s2 = &h.fk("spawn2")[0];
    assert_eq!((s2["ok"].as_bool(), s2["error"]["code"].as_str(), s2["error"]["data"]["quota"].as_str()), (Some(false), Some("root.level"), Some("parallel_bodies")));
    let hit = h.last("ceiling_hit");
    assert_eq!((hit["data"]["demotes"].as_bool(), hit["data"]["rule"].as_str(), hit["data"]["source"].as_str()), (Some(true), Some("b"), Some("broker")));
    // R7 (WP25): the demoting hit demotes (0→0 at L0) and rolls back to audited-good
    wait(10, "demotion", || h.last("ladder")["data"]["trigger"]["seq"] == hit["seq"]);
}

/// §3.6.3: no `hb` for `hb_stale_s` is a crash; the kernel is killed and restarted with backoff.
#[test]
fn stale_heartbeat_is_a_crash() {
    let h = home("hb");
    h.install("", 0);
    let _d = h.daemon("no_hb", &[]);
    wait(20, "two crashes", || h.records().iter().filter(|r| r["kind"] == "zone_exit" && r["data"]["class"] == "crash").count() >= 2);
    assert!(fs::read_to_string(h.rh.join("logs/root.jsonl")).unwrap().contains("heartbeat stale"));
}

/// Compatibility with WP3's `init-fakekernel` (typed `BootBlock` from init-proto), when its binary is given.
#[test]
fn wp3_fakekernel_reaches_ready_when_available() {
    let Ok(bin) = std::env::var("INIT_FAKEKERNEL_BIN") else { return eprintln!("skipped: INIT_FAKEKERNEL_BIN is not set") };
    let h = home("w3");
    fs::copy(bin, h.dir.join("rel/init-kernel")).unwrap();
    h.install("", 0);
    let _d = h.daemon("", &[("INIT_TEST_FAKEKERNEL_MODE", "no_body")]);
    h.running();
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(h.status()["slot"], "running", "hb keeps it alive");
}
