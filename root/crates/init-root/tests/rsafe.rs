//! R-SAFE (ADR-008, owner-machine mode): the Root signals only what it holds, `live-sweep` stays compiled
//! out, and the sweep dry-run over the real process table selects exactly INIT's own instances.
//!
//! Source audits (every build, no processes): `live-sweep` is never a default, implied or scripted feature;
//! the only `kill`/`killpg` is `sys::signal` (held pids; `signal_member` exists only under `live-sweep`);
//! every caller of `sys::signal` is a reviewed held-pid site; the dry-run module holds no signalling call.
//!
//! Live tests (`--features test-fixtures`, macOS): owner-machine variants of SW-1…SW-5, RT-P1, RT-P7 and
//! RT-P11, plus cold start. Live-system safety (CONVENTIONS §1): every process here is this test's own
//! child (the Root daemon, the decoys) or a process that test Root spawned and holds; the only signals the
//! test sends go to its own children by their handles. The dry-run only queries (`proc_listpids`,
//! `sandbox_check`). The one process that leaves its group (the SW-1 escapee) is a bounded `sleep` that
//! ends on its own; the test only watches it with `ps`. Homes live under `root/target/test-homes/rsafe/`.

use std::fs;
use std::path::{Path, PathBuf};

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo() -> PathBuf {
    crate_dir().join("../../..").canonicalize().unwrap()
}

/// Every non-test source file of the crate: (path relative to src/, text up to its `#[cfg(test)]` module).
fn sources() -> Vec<(String, String)> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let src = crate_dir().join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
    files.sort();
    files.into_iter().filter(|p| p.file_name().is_some_and(|n| n != "tests.rs")).map(|p| {
        let text = fs::read_to_string(&p).unwrap();
        let cut = text.find("#[cfg(test)]\nmod tests").unwrap_or(text.len());
        (p.strip_prefix(&src).unwrap().display().to_string(), text[..cut].to_string())
    }).collect()
}

/// The name of the `fn` each line belongs to (the last `fn <name>` header above it).
fn enclosing(text: &str) -> Vec<(usize, String, String)> {
    let mut cur = String::new();
    text.lines().enumerate().map(|(i, l)| {
        if let Some(rest) = l.split("fn ").nth(1).filter(|_| l.trim_start().starts_with("fn ") || l.contains(" fn "))
            && let Some(name) = rest.split(['(', '<']).next().filter(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
        {
            cur = name.to_string();
        }
        (i + 1, cur.clone(), l.to_string())
    }).collect()
}

/// Task 1: `live-sweep` is opt-in only. Not in `default`, not implied by another feature, not passed by
/// any script or workflow in the repo, and not compiled into this test build.
#[test]
fn live_sweep_is_never_a_default_implied_or_scripted_feature() {
    let manifest: toml::Table = fs::read_to_string(crate_dir().join("Cargo.toml")).unwrap().parse().unwrap();
    let features = manifest["features"].as_table().unwrap();
    assert!(features.contains_key("live-sweep"), "the feature exists (disposable environments only)");
    for (name, list) in features {
        let list: Vec<&str> = list.as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        if name != "live-sweep" {
            assert!(!list.iter().any(|f| f.contains("live-sweep")), "feature {name} must not enable live-sweep: {list:?}");
        }
    }
    let default: Vec<&str> = features.get("default").and_then(|d| d.as_array()).map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
    assert!(!default.contains(&"live-sweep"), "live-sweep in default features");
    // the build of this very test: the default and test-fixtures builds both leave it out
    const { assert!(!cfg!(feature = "live-sweep"), "this build carries live-sweep: run it only in a disposable environment (ADR-008)") };
    // no build script, CI step or workflow turns it on (or turns on every feature)
    let r = repo();
    let mut scripts: Vec<PathBuf> = Vec::new();
    for d in ["ci", "scripts", "scripts/slice1", ".github/workflows"] {
        scripts.extend(fs::read_dir(r.join(d)).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_file()));
    }
    for p in scripts.iter().filter(|p| p.extension().is_some_and(|x| ["sh", "yml", "yaml", "py", "toml"].contains(&x.to_str().unwrap_or("")))) {
        let s = fs::read_to_string(p).unwrap_or_default();
        for (n, l) in s.lines().enumerate() {
            let code = l.split('#').next().unwrap_or("");
            assert!(!(code.contains("live-sweep") || code.contains("live_sweep")), "{}:{}: enables live-sweep: {l}", p.display(), n + 1);
            assert!(!(code.contains("--all-features") && code.contains("init-root")), "{}:{}: --all-features would enable live-sweep: {l}", p.display(), n + 1);
        }
    }
    // a release build cannot carry it: sweep.rs stops such a build at compile time
    let sweep = fs::read_to_string(crate_dir().join("src/sweep.rs")).unwrap();
    assert!(sweep.contains("#[cfg(all(feature = \"live-sweep\", not(debug_assertions)))]\ncompile_error!"), "the release guard is gone");
}

/// Task 1: no code path of a default build calls `kill`/`killpg` on a pid the Root does not hold.
/// (1) the only `libc::kill`/`libc::killpg` are in `sys::signal`, which checks `held(pid)` (an unreaped child)
/// first, in `sys::signal_member`, which is compiled only under `live-sweep`, and in `sys::signal_tracked` (the
/// MEM-RUNAWAY amendment to ADR-008), which re-checks a watched workload's identity and sandbox first; (2) no other
/// way to send a signal or kill by name exists; (3) every `sys::signal` caller is one of the reviewed held-pid sites,
/// so a new one fails here until it is reviewed; (4) `signal_member` is reached only from the live-sweep scan;
/// (5) `signal_tracked` is reached only from the memory watchdog's `kill_members`.
#[test]
fn only_held_pids_can_be_signalled_in_a_default_build() {
    let srcs = sources();
    let mut kill_sites = Vec::new();
    let mut signal_callers = Vec::new();
    let mut member_callers = Vec::new();
    let mut tracked_callers = Vec::new();
    for (file, text) in &srcs {
        let lines = enclosing(text);
        for (n, func, l) in &lines {
            let code = l.split("//").next().unwrap_or("");
            if code.contains("libc::kill(") || code.contains("libc::killpg(") {
                kill_sites.push((file.clone(), func.clone()));
            }
            for bad in ["libc::raise(", "pthread_kill(", "sigqueue(", "proc_terminate(", "\"pkill\"", "\"killall\"", "\"/bin/kill\"",
                "Command::new(\"kill\")", "libc::killpg(-", "libc::kill(-", "libc::kill(0", "libc::kill(-1"] {
                assert!(!code.contains(bad), "{file}:{n}: {bad} is not allowed (ADR-008): {l}");
            }
            if code.contains("sys::signal(") {
                signal_callers.push((file.clone(), func.clone()));
            }
            if code.contains("signal_member(") && !code.contains("pub fn signal_member") {
                member_callers.push((file.clone(), func.clone(), *n));
            }
            if code.contains("signal_tracked(") && !code.contains("pub fn signal_tracked") {
                tracked_callers.push((file.clone(), func.clone()));
            }
        }
    }
    kill_sites.sort();
    kill_sites.dedup();
    assert_eq!(kill_sites, [("sys.rs".to_string(), "signal".to_string()), ("sys.rs".to_string(), "signal_member".to_string()),
        ("sys.rs".to_string(), "signal_tracked".to_string())]);
    let sys = &srcs.iter().find(|(f, _)| f == "sys.rs").unwrap().1;
    let body = |name: &str| sys.split(&format!("pub fn {name}(")).nth(1).and_then(|s| s.split("\n}\n").next()).unwrap().to_string();
    assert!(body("signal").contains("held(pid) && unsafe"), "sys::signal must check held(pid) before kill/killpg");
    assert!(body("held").contains("libc::WNOHANG | libc::WNOWAIT"), "held() must only look (WNOHANG | WNOWAIT), never reap");
    // MEM-RUNAWAY: signal_tracked checks the identity (start time, uid) and the sandbox right before its kill, only
    // SIGSTOP/SIGKILL, never pid ≤ 1 or our own; its one caller is the memory watchdog's kill_members
    let tracked = sys.split("#[cfg(target_os = \"macos\")]\npub fn signal_tracked(").nth(1).and_then(|s| s.split("\n}\n").next()).unwrap();
    for must in ["q.start_us == p.start_us && q.uid == p.uid && q.uid == euid()", "sandboxed(p.pid) == Some(true) && unsafe { libc::kill(p.pid, sig) }",
        "p.pid <= 1 || p.pid == std::process::id() as i32", "!(sig == libc::SIGKILL || sig == libc::SIGSTOP)"] {
        assert!(tracked.contains(must), "sys::signal_tracked lost its check {must:?}");
    }
    tracked_callers.sort();
    tracked_callers.dedup();
    assert_eq!(tracked_callers, [("memwatch.rs".to_string(), "kill_members".to_string())], "a new signal_tracked caller needs review");
    let before_member = sys.split("pub fn signal_member(").next().unwrap();
    assert!(before_member.trim_end().lines().rev().take(4).any(|l| l.contains("#[cfg(all(feature = \"live-sweep\"")),
        "signal_member must be compiled only with live-sweep");
    // the reviewed held-pid sites: the broker's instance kills (leader + own group, unreaped under the table
    // lock), the reaper (a zombie leader keeps its group ours), the sealed runs this process spawned, and (X8) the
    // conformance probe's check body: `kprobe::spawn_body`'s waiter thread owns the body's `Child`, signals its pid
    // (leader of its own process group) only before it reaps it, on the 90 s bound or the run's end; and (v0.5.0,
    // UPGRADE, reviewed at integration REL-050) `upgrade::Daemon::drop`: `upgrade --probe` owns the probe Root's
    // `Child`, sends it SIGTERM (not to a group) before it reaps it, then reaps it through the same handle
    signal_callers.sort();
    signal_callers.dedup();
    let reviewed: Vec<(String, String)> = [("sealed/run.rs", "cancel_all"), ("sealed/run.rs", "supervise"), ("sup/kprobe.rs", "spawn_body"),
        ("upgrade.rs", "drop"), ("zone.rs", "kill"), ("zone.rs", "kill_all"), ("zone.rs", "reaper")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    assert_eq!(signal_callers, reviewed, "a new sys::signal caller needs review: it must pass a pid the Root holds");
    // signal_member: only zone::scan, inside its live-sweep block
    assert_eq!(member_callers.len(), 1, "{member_callers:?}");
    let (file, func, n) = &member_callers[0];
    assert_eq!((file.as_str(), func.as_str()), ("zone.rs", "scan"));
    let zone = &srcs.iter().find(|(f, _)| f == "zone.rs").unwrap().1;
    let head: Vec<&str> = zone.lines().take(*n).collect();
    let gate = head.iter().rposition(|l| l.contains("#[cfg(all(feature = \"live-sweep\", target_os = \"macos\"))]")).unwrap();
    assert!(head[gate..].iter().any(|l| l.contains("sweep::disposable()")), "the live scan checks the disposable sentinel first");
    // the cold start never signals a recorded pid
    let cold = zone.split("pub fn cold_start(").nth(1).unwrap().split("\n}\n").next().unwrap();
    assert!(!cold.contains("signal(") && !cold.contains("kill("), "cold_start must not signal a previous Root's pids");
}

/// Task 2: the dry-run module, and the live table it reads, hold no call that could send a signal.
#[test]
fn the_dry_run_cannot_signal() {
    let doctor = fs::read_to_string(crate_dir().join("src/doctor.rs")).unwrap();
    for bad in ["kill(", "killpg", "kill (", "signal(", "signal_member", "SIGKILL", "SIGTERM", "SIGINT", "SIGUSR", "SIGSTOP", "libc::",
        "unsafe", "Command", "sweep::sweep", " sweep(", "zone::", "sys::signal", ".kill()"] {
        assert!(!doctor.contains(bad), "src/doctor.rs contains {bad:?}: the dry-run must never signal (ADR-008)");
    }
    assert!(doctor.contains("\"signals_sent\": 0"));
    let sys = fs::read_to_string(crate_dir().join("src/sys.rs")).unwrap();
    let table = sys.split("pub struct LiveTable;").nth(1).unwrap().split("/// Overwrites a buffer").next().unwrap();
    for bad in ["kill", "signal("] {
        assert!(!table.contains(bad), "the live table must only read: found {bad:?}");
    }
    let sweep = fs::read_to_string(crate_dir().join("src/sweep.rs")).unwrap();
    let classify = sweep.split("pub fn classify(").nth(1).unwrap().split("\n}\n").next().unwrap();
    assert!(!classify.contains("kill") && classify.contains("member(t, pid"), "classify applies `member`, the live sweep's own rule");
}

#[cfg(all(feature = "test-fixtures", target_os = "macos"))]
mod live {
    use serde_json::{Value, json};
    use std::fs;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
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

    /// A fresh home `root/target/test-homes/rsafe/<name>` (short: ROOT_HOME must stay ≤ 80 bytes).
    fn home(name: &str) -> Home {
        let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/rsafe");
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

    /// A process this test spawns and holds: `/bin/sleep 120`, bare or under its own Seatbelt profile, in
    /// its own process group. Dropped: killed by its handle (our own child) and reaped.
    struct Decoy(Child);
    impl Decoy {
        fn spawn(profile: Option<&str>) -> Decoy {
            let mut c = match profile {
                Some(p) => {
                    let mut c = Command::new("/usr/bin/sandbox-exec");
                    c.args(["-p", p, "/bin/sleep", "120"]);
                    c
                }
                None => {
                    let mut c = Command::new("/bin/sleep");
                    c.arg("120");
                    c
                }
            };
            let d = Decoy(c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn().unwrap());
            wait(5, "the decoy runs sleep", || alive(d.pid(), "sleep"));
            d
        }
        fn pid(&self) -> i64 {
            i64::from(self.0.id())
        }
        fn running(&mut self) -> bool {
            self.0.try_wait().unwrap().is_none()
        }
    }
    impl Drop for Decoy {
        fn drop(&mut self) {
            let _ = self.0.kill(); // our own child, by its handle
            let _ = self.0.wait();
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
            fs::create_dir_all(&self.rh).unwrap();
            let charter = format!("schema_version = 1\n[purpose]\ntext = \"R-SAFE test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n{ROOT_CFG}\n");
            fs::write(self.rh.join("charter.toml"), charter).unwrap();
            let (rel, tc) = (self.dir.join("rel"), self.dir.join("tc"));
            let o = self.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]);
            assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
        }
        fn records(&self) -> Vec<Value> {
            let s = fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default();
            s.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
        }
        fn of(&self, kind: &str) -> Vec<Value> {
            self.records().into_iter().filter(|r| r["kind"] == kind).collect()
        }
        fn last(&self, kind: &str) -> Value {
            self.of(kind).pop().unwrap_or(Value::Null)
        }
        fn status(&self) -> Value {
            self.ok(&["status", "--json"])
        }
        fn fk(&self, ev: &str) -> Vec<Value> {
            let s = fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default();
            s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["ev"] == ev).collect()
        }
        fn fk_wait(&self, secs: u64, ev: &str, n: usize) -> Vec<Value> {
            let mut v = Vec::new();
            wait(secs, &format!("{n} fake-kernel {ev:?} line(s)"), || {
                v = self.fk(ev);
                v.len() >= n
            });
            v
        }
        fn info(&self) -> Value {
            serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap()
        }
        fn daemon(&self, modes: &str) -> Daemon {
            let info = self.info();
            let mut c = self.cmd();
            c.arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", modes)
                .env("INIT_TEST_TOOLCHAIN", info["toolchain"].as_str().unwrap()).env("INIT_TEST_DEVDIR", info["devdir"].as_str().unwrap())
                .env("INIT_TEST_HOME_REAL", &self.dir);
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
        fn running(&self) -> (Value, i64) {
            let st = self.slot_is(20, "running");
            let pid = st["kernel"]["pid"].as_i64().unwrap();
            (st, pid)
        }
        /// `init-root doctor --sweep-dry-run` over the live process table.
        fn dry_run(&self) -> Value {
            let v = self.ok(&["doctor", "--sweep-dry-run"]);
            assert_eq!((v["dry_run"].as_bool(), v["signals_sent"].as_i64()), (Some(true), Some(0)), "{v}");
            assert_eq!(v["build"]["sweep"], "held_only", "owner-machine build");
            v
        }
        fn mark(&self, inst: &str) -> PathBuf {
            self.ih.join("run/mark").join(inst)
        }
        fn decoy(&self) -> PathBuf {
            self.ih.join("run/mark/decoy")
        }
        fn canary(&self) -> PathBuf {
            self.rh.join("run/canary")
        }
    }

    fn pids(v: &Value) -> Vec<i64> {
        let mut p: Vec<i64> = v["would_kill"].as_array().unwrap().iter().map(|w| w["pid"].as_i64().unwrap()).collect();
        p.sort();
        p
    }

    fn sorted(mut v: Vec<i64>) -> Vec<i64> {
        v.sort();
        v
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

    /// The summary line kept for build/status/R-SAFE.md (how much of the live table was evaluated).
    fn record_summary(name: &str, v: &Value) {
        let line = json!({"test": name, "at": v["at"], "evaluated": v["evaluated"], "would_kill": pids(v), "rejected": v["rejected"],
            "unrecorded_members": v["unrecorded_members"]});
        eprintln!("R-SAFE dry-run {line}");
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/rsafe/dry-run-summary.jsonl");
        let mut f = fs::OpenOptions::new().create(true).append(true).open(p).unwrap();
        std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes()).unwrap();
    }

    /// Task 2 and the owner-machine SW-2…SW-5, RT-P1/P7/P11: with this test's two instances running (the
    /// kernel and its body, both under the real zone profiles) and three decoys shaped like the owner's
    /// applications, the dry-run over the real process table selects exactly the two instances. Panic, the
    /// resume rollback and stop then take only held processes; every decoy is still running.
    #[test]
    fn dry_run_selects_exactly_our_instances_and_panic_spares_decoys() {
        let h = home("dry");
        h.install();
        let d = h.daemon("body");
        let (st, kpid) = h.running();
        assert_eq!(st["sweep"], "held_only");
        let spawn = &h.fk_wait(10, "spawn", 1)[0];
        assert_eq!(spawn["ok"], true, "{spawn}");
        let bpid = spawn["result"]["pid"].as_i64().unwrap();
        wait(5, "the body runs cat", || alive(bpid, "cat"));
        let (kinst, binst) = (st["kernel"]["instance"].as_str().unwrap().to_string(), spawn["result"]["instance"].as_str().unwrap().to_string());
        let (rh, ih) = (h.rh.display().to_string(), h.ih.display().to_string());
        // SW-2: unsandboxed, reads everything (fails a). SW-3: its own permissive sandbox, reads the canary and
        // the decoy too (fails c). SW-4: a sandbox that denies the canary and cannot read the marks (fails b).
        let mut sw2 = Decoy::spawn(None);
        let mut sw3 = Decoy::spawn(Some("(version 1)(allow default)"));
        let sw4p = format!("(version 1)(allow default)(deny file-read* (subpath \"{rh}\"))(deny file-read* (subpath \"{ih}/run/mark\"))\
            (deny file-read* (literal \"{ih}/run/zone-mark\"))");
        let mut sw4 = Decoy::spawn(Some(&sw4p));
        let km = h.mark(&kinst);
        assert_eq!((sandbox(sw2.pid(), None), sandbox(sw2.pid(), Some(&km))), (0, 0), "SW-2: not sandboxed, reads the mark");
        assert_eq!((sandbox(sw3.pid(), None), sandbox(sw3.pid(), Some(&km)), sandbox(sw3.pid(), Some(&h.canary())), sandbox(sw3.pid(), Some(&h.decoy()))),
            (1, 0, 0, 0), "SW-3: sandboxed, reads the mark, the canary and the decoy");
        assert_eq!(sandbox(sw4.pid(), None), 1);
        assert!(sandbox(sw4.pid(), Some(&km)) != 0 && sandbox(sw4.pid(), Some(&h.canary())) != 0, "SW-4: denied the mark and the canary");
        // the dry-run: exactly the two instances, both recorded as held, and no other process of this uid
        let v = h.dry_run();
        record_summary("dry_run_selects_exactly_our_instances", &v);
        assert_eq!(pids(&v), sorted(vec![kpid, bpid]), "the would-kill set is exactly this test's two instances: {v}");
        assert!(v["would_kill"].as_array().unwrap().iter().all(|w| w["recorded"] == true), "{v}");
        assert_eq!(v["unrecorded_members"], 0);
        let members = |name: &str| v["marks"].as_array().unwrap().iter().find(|m| m["name"] == name).map(|m| m["members"].clone()).unwrap();
        assert_eq!((members(&kinst), members(&binst)), (json!([kpid]), json!([bpid])), "each instance reads only its own mark (Z5)");
        let zm: Vec<i64> = members("zone-mark").as_array().unwrap().iter().filter_map(Value::as_i64).collect();
        assert_eq!(sorted(zm), sorted(vec![kpid, bpid]), "the zone mark: both instances");
        let r = &v["rejected"];
        assert!(r["not_sandboxed"].as_i64() >= Some(1) && r["reads_decoy_or_canary"].as_i64() >= Some(1) && r["mark_unreadable"].as_i64() >= Some(1), "{r}");
        assert_eq!(v["evaluated"].as_i64(), r["total"].as_i64().map(|t| t + 2), "every other process evaluated is rejected");
        // SW-5 / RT-P1 (owner-machine): panic takes the held instances only
        let o = h.run(&["panic"]);
        assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
        h.slot_is(5, "panicked");
        wait(5, "kernel and body gone", || !alive(kpid, "init-kernel") && !alive(bpid, "cat"));
        let p = h.last("panic");
        assert_eq!(p["data"]["killed"], json!({"instances": 2, "processes": 2}), "{p}");
        assert!(sw2.running() && sw3.running() && sw4.running(), "SW-5: every decoy still runs (checked by its held handle)");
        // escapee detection runs when the zone empties: nothing of the zone is left, so nothing is reported
        std::thread::sleep(Duration::from_millis(800));
        assert!(h.of("escapee").is_empty(), "no decoy is mistaken for an escapee");
        assert!(pids(&h.dry_run()).is_empty(), "after the panic nothing meets D1");
        // RT-P7 (owner-machine): resume rolls back to audited-good and restarts; RT-P11: stop
        h.ok(&["resume"]);
        let (_, k2) = h.running();
        assert_ne!(k2, kpid);
        assert!(h.last("rollback")["data"].is_object(), "the resume rollback is recorded");
        let o = h.run(&["stop"]);
        assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
        h.slot_is(10, "stopped");
        wait(5, "the restarted kernel is gone", || !alive(k2, "init-kernel"));
        std::thread::sleep(Duration::from_millis(800));
        assert!(sw2.running() && sw3.running() && sw4.running(), "resume and stop spare every decoy");
        assert!(h.of("escapee").is_empty());
        drop(d);
    }

    /// Cold start never signals a previous Root's instance, even one that meets D1 exactly: a stand-in the
    /// test spawns under a profile that reads the mark and is denied the decoy and the canary, recorded in
    /// `state/root.json` as a previous Root's body. The new Root removes the mark, signals nothing, and
    /// reports the process (review F5: an `escapee` record and an alert). A held-only panic leaves it
    /// running too, and reports it again: ADR-008's residual risk, made visible.
    #[test]
    fn cold_start_never_signals_a_recorded_pid_and_reports_it() {
        let h = home("cold");
        h.install();
        let id = "0123456789ab";
        let mark = h.mark(id);
        fs::OpenOptions::new().write(true).create_new(true).mode(0o400).open(&mark).unwrap();
        let prof = format!("(version 1)(allow default)(deny file-read* (literal \"{}\"))(deny file-read* (subpath \"{}\"))",
            h.decoy().display(), h.rh.display());
        let mut orphan = Decoy::spawn(Some(&prof));
        let op = orphan.pid();
        assert_eq!((sandbox(op, None), sandbox(op, Some(&mark))), (1, 0));
        assert!(sandbox(op, Some(&h.decoy())) != 0 && sandbox(op, Some(&h.canary())) != 0, "the stand-in meets D1");
        let rj = h.rh.join("state/root.json");
        let mut prev: Value = fs::read_to_string(&rj).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"v": 1}));
        prev["instances"] = json!([{"id": id, "kind": "body", "lane": "live", "mark": format!("run/mark/{id}"), "pgid": op, "pid": op,
            "pidns_ino": null, "started_at": "2026-10-20T00:00:00.000Z"}]);
        fs::write(&rj, format!("{prev}\n")).unwrap();
        let v = h.dry_run();
        record_summary("cold_start_stand_in", &v);
        assert_eq!(pids(&v), [op], "the dry-run sees the stand-in, and only it");
        let d = h.daemon("");
        h.running();
        assert!(orphan.running(), "cold start never signals a recorded pid");
        assert!(!mark.exists(), "the orphan's mark is removed by exact path");
        let log = fs::read_to_string(h.rh.join("logs/root.jsonl")).unwrap_or_default();
        assert!(log.contains(&format!("orphan instance {id}")) && log.contains("never signalled by pid"), "{log}");
        // R-SEC: the daemon raised its soft NOFILE limit to min(hard, 4096) and logged what it read back
        let nofile = log.lines().find_map(|l| serde_json::from_str::<Value>(l).ok().and_then(|v| v["msg"].as_str().map(String::from))
            .filter(|m| m.starts_with("RLIMIT_NOFILE soft"))).expect("the daemon logs its fd limit");
        let num = |after: &str| after.trim_start().split(|c: char| !c.is_ascii_digit()).next().unwrap().parse::<u64>().unwrap();
        let (soft, hard) = (num(nofile.split("-> ").nth(1).unwrap()), num(nofile.split("(hard ").nth(1).unwrap()));
        assert!(soft >= 4096.min(hard), "{nofile}");
        wait(5, "the cold-start escapee record", || !h.of("escapee").is_empty());
        let e = h.last("escapee");
        assert_eq!((e["data"]["after"].as_str(), e["data"]["pids"].clone(), e["data"]["signalled"].as_bool()),
            (Some("cold_start"), json!([op]), Some(false)), "{e}");
        let alerts = fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap_or_default();
        assert!(alerts.lines().any(|l| l.contains("\"code\":\"escapee\"") && l.contains(&op.to_string())), "{alerts}");
        // a held-only panic takes the kernel it holds; the stand-in survives and is reported again
        let o = h.run(&["panic"]);
        assert_eq!(o.status.code(), Some(0));
        h.slot_is(5, "panicked");
        wait(5, "the panic escapee record", || h.of("escapee").len() >= 2);
        let e = h.last("escapee");
        assert_eq!((e["data"]["after"].as_str(), e["data"]["pids"].clone()), (Some("panic"), json!([op])), "{e}");
        assert!(orphan.running(), "the held-only panic signals no process it does not hold");
        drop(d);
        drop(orphan);
    }

    /// SW-1 (owner-machine): a real zone escapee. The body forks a child that calls `setsid` (a new session
    /// and process group), both bounded `sleep 20`s. The dry-run finds kernel, leader and escapee, and only the
    /// escapee is unrecorded. A held-only panic takes kernel and leader; the escapee survives (ADR-008's
    /// residual risk), is reported in an `escapee` record and an alert, and ends on its own; no one signals it.
    #[test]
    fn setsid_escapee_is_found_and_reported_never_signalled() {
        let h = home("esc");
        h.install();
        let d = h.daemon("");
        let (_, kpid) = h.running();
        let info = h.info();
        let ih = h.ih.display().to_string();
        let mut p = serde_json::Map::new();
        for (k, sub) in [("BIN", "/bin"), ("BODY", "/body"), ("CACHE", "/cache"), ("INIT_HOME", ""), ("KERNEL", "/kernel"),
            ("KERNEL_SOCK", "/run/kernel.sock"), ("LINEAGE", "/lineage"), ("LINK_BIN", "/bin"), ("LINK_KERNEL", "/kernel"),
            ("LINK_LINEAGE", "/lineage"), ("PROXY_SOCK", "/run/proxy.sock"), ("PUB", "/root"), ("RUN", "/run"), ("STATE", "/state"),
            ("STATE_KERNEL", "/state/kernel"), ("VAULT", "/vault"), ("WORK", "/work")] {
            p.insert(k.into(), json!(format!("{ih}{sub}")));
        }
        p.insert("TOOLCHAIN".into(), info["toolchain"].clone());
        p.insert("DEVDIR".into(), info["devdir"].clone());
        p.insert("HOME_REAL".into(), json!(h.dir));
        let script = "if (!fork) { eval { require POSIX; POSIX::setsid(); 1 } or setpgrp(0, 0); exec '/bin/sleep', '20' } exec '/bin/sleep', '20'";
        let args = json!({"args": ["-e", script], "cwd": format!("{ih}/work"), "env": {"PATH": "/usr/bin:/bin"}, "kind": "body",
            "lane": "live", "params": p, "profile": "body", "program": "/usr/bin/perl"});
        fs::write(h.ih.join("work/fk-call.json"), json!({"cmd": "spawn", "args": args}).to_string()).unwrap();
        let call = &h.fk_wait(10, "call", 1)[0];
        assert_eq!(call["ok"], true, "{call}");
        let lpid = call["result"]["pid"].as_i64().unwrap();
        let mut v = Value::Null;
        wait(10, "the dry-run finds the escapee", || {
            v = h.dry_run();
            v["unrecorded_members"] == 1 && pids(&v).len() == 3
        });
        record_summary("setsid_escapee", &v);
        let esc = v["would_kill"].as_array().unwrap().iter().find(|w| w["recorded"] == false).unwrap()["pid"].as_i64().unwrap();
        assert_eq!(pids(&v), sorted(vec![kpid, lpid, esc]), "{v}");
        assert!(alive(esc, "sleep") && alive(lpid, "sleep"));
        let o = h.run(&["panic"]);
        assert_eq!(o.status.code(), Some(0));
        h.slot_is(5, "panicked");
        wait(5, "kernel and leader gone", || !alive(kpid, "init-kernel") && !alive(lpid, "sleep"));
        let p = h.last("panic");
        assert_eq!(p["data"]["killed"], json!({"instances": 2, "processes": 2}), "{p}");
        wait(5, "the escapee record", || !h.of("escapee").is_empty());
        let e = h.last("escapee");
        assert_eq!((e["data"]["after"].as_str(), e["data"]["pids"].clone(), e["data"]["signalled"].as_bool()), (Some("panic"), json!([esc]), Some(false)));
        assert!(alive(esc, "sleep"), "the held-only panic does not reach a process that left its group (ADR-008)");
        assert_eq!(pids(&h.dry_run()), [esc], "the dry-run still sees it");
        drop(d);
        wait(30, "the escapee (a bounded sleep) ends on its own", || !alive(esc, "sleep"));
    }
}
