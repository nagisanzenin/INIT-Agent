//! TUI-R1…TUI-R3, owner-machine variant (docs/tui.md §8, RELEASE_PLAN.md): `init-tui` against the REAL
//! `init-root` in test mode (a test build: cargo feature `test-fixtures`, no `live-sweep`), with the Root's
//! own fake kernel (`init-root-fakekernel`, body `/bin/cat`, under the real `kernel` and `body` profiles)
//! and `init-mockllm` as the loopback model upstream behind the Root's egress. $0: a `test-` key, no network.
//!
//! One scripted session: TTY mode through a pty (status bar L1, a line round trip, spend past the legacy
//! day value with no refusal (observe-only money policy), a scripted demotion L1→L0, `/panic` showing
//! PANICKED within 1 s, `/resume`), then line mode (line→screen latency through Root → kernel → body,
//! and a Root restart with three lines queued: each reaches the kernel exactly once).
//!
//! Binaries are frozen copies (other lanes rebuild the shared target dirs): `INIT_ROOT_BIN`,
//! `INIT_ROOT_FAKEKERNEL_BIN`, `INIT_MOCKLLM_BIN`, defaulting to root/target/debug and kernel/target/debug.
//! Missing binaries skip the test unless `INIT_TUI_E2E=1` (ci/local.sh sets it), which makes them a failure.
//!
//! Live-system safety (CONVENTIONS §1): every process is this test's child (the Root daemon, the mock, the
//! pty driver and its TUI) or one that test Root spawned and holds; the test signals only its own children,
//! by handle. The homes are `tui/target/test-homes/tui/r*`, with `HOME` pointed there. Never ~/.init*.

mod common;

use common::{BIN, cleanup, home};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const KEY: &str = "test-key-wp26-e2e-5d5d5d5d";
const ROOT_CFG: &str = "[root]\ntest_mode = true\nhb_interval_s = 1\nhb_stale_s = 3\nkernel_hello_timeout_s = 20\nbackoff_initial_ms = 50\n\
    backoff_max_ms = 200\nstop_grace_s = 5\n";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap()
}

/// A binary from the env or its default build, or None (skip) unless INIT_TUI_E2E=1.
fn bin(env: &str, default: &str) -> Option<PathBuf> {
    let p = std::env::var_os(env).map(PathBuf::from).unwrap_or_else(|| repo().join(default));
    if p.is_file() {
        return Some(p);
    }
    assert!(std::env::var_os("INIT_TUI_E2E").is_none(), "INIT_TUI_E2E=1 but {} is missing (build it or set {env})", p.display());
    eprintln!("SKIP e2e_root: {} is missing (set {env})", p.display());
    None
}

fn wait(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "not within {secs} s: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct Home {
    dir: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
    root: PathBuf,
}

impl Home {
    fn cmd(&self) -> Command {
        let mut c = Command::new(&self.root);
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
    fn status(&self) -> Value {
        self.ok(&["status", "--json"])
    }
    fn owner(&self, cmd: &str, args: Value) -> Value {
        let mut s = UnixStream::connect(self.rh.join("run/owner.sock")).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        writeln!(s, "{}", json!({"args": args, "cmd": cmd, "id": "e-1", "v": 1})).unwrap();
        let mut line = String::new();
        BufReader::new(&s).read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["ok"], true, "{cmd}: {v}");
        v["result"].clone()
    }
    fn daemon(&self) -> Daemon {
        let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
        let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
        let c = self.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", "body")
            .env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir")).env("INIT_TEST_HOME_REAL", &self.dir).spawn().unwrap();
        let d = Daemon { child: Some(c), root: self.root.clone(), rh: self.rh.clone() };
        wait(15, "owner.sock", || UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        d
    }
    fn slot_is(&self, secs: u64, slot: &str) {
        wait(secs, &format!("slot {slot}"), || self.status()["slot"] == slot);
    }
    /// The fake kernel's `owner.in` lines (`INIT_HOME/state/kernel/fk.jsonl`).
    fn owner_in(&self) -> Vec<Value> {
        let s = fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["ev"] == "owner.in").collect()
    }
    fn chat(&self, max: u64) -> String {
        let body = json!({"max_tokens": max, "messages": [{"content": "hi", "role": "user"}], "model": "deepseek-flash", "stream": true}).to_string();
        let mut s = UnixStream::connect(self.rh.join("run/egress.sock")).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        write!(s, "POST /deepseek/chat/completions HTTP/1.1\r\nhost: egress\r\nconnection: close\r\ncontent-type: application/json\r\nx-init-session: s1\r\nx-init-purpose: turn\r\ncontent-length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut raw = Vec::new();
        let _ = s.read_to_end(&mut raw);
        String::from_utf8_lossy(&raw).into_owned()
    }
}

/// The test's Root daemon. On drop: an orderly `stop`, then the daemon itself by its own handle.
struct Daemon {
    child: Option<Child>,
    root: PathBuf,
    rh: PathBuf,
}

impl Daemon {
    fn root_cmd(&self, args: &[&str]) -> Option<Output> {
        Command::new(&self.root).env_clear().env("HOME", "/nonexistent").env("INIT_ROOT_HOME", &self.rh).args(args).stdin(Stdio::null()).output().ok()
    }
    /// Stops the zone, waits until the Root holds no instance, then kills the daemon (our own child).
    fn kill(&mut self) {
        let _ = self.root_cmd(&["stop"]);
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            let v: Value = self.root_cmd(&["status", "--json"]).and_then(|o| serde_json::from_slice(&o.stdout).ok()).unwrap_or_default();
            if v["instances"].as_array().is_none_or(Vec::is_empty) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Our own child process, killed by its handle on drop.
struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The pty driver (tests/pty_drive.py) around `init-tui` in TTY mode.
struct Drive {
    child: Owned,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    raw_log: Option<PathBuf>,
}

impl Drive {
    fn start(rh: &Path) -> Drive {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pty_drive.py");
        // cwd: the test's HOME, so no workspace (ADR-009) and nothing of the source tree is snapshotted
        let mut c = Command::new("/usr/bin/python3").arg("-I").arg(script).arg(BIN).arg(rh).arg("--latency-report")
            .current_dir(rh.parent().unwrap()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
        let stdin = c.stdin.take().unwrap();
        let mut out = BufReader::new(c.stdout.take().unwrap());
        let mut l = String::new();
        out.read_line(&mut l).unwrap();
        Drive { child: Owned(c), stdin, out, raw_log: None }
    }
    fn cmd(&mut self, c: Value) -> String {
        writeln!(self.stdin, "{c}").unwrap();
        let mut l = String::new();
        self.out.read_line(&mut l).unwrap();
        l.trim().to_string()
    }
    fn ok(&mut self, c: Value) {
        let r = self.cmd(c.clone());
        if r != "ok" {
            let log = self.raw_log.as_ref().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
            let tail: Vec<&str> = log.lines().rev().take(15).collect::<Vec<_>>().into_iter().rev().collect();
            panic!("{c}: {}\nthe frames an observer client saw last:\n{}", &r[..r.len().min(600)], tail.join("\n"));
        }
    }
    fn exit(&mut self, secs: u64) -> Value {
        assert_eq!(self.cmd(json!(["exit", secs])), "ok");
        let mut l = String::new();
        self.out.read_line(&mut l).unwrap();
        let _ = self.child.0.wait();
        serde_json::from_str(&l).unwrap()
    }
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

#[test]
fn tui_r_real_root_session() {
    let (Some(root), Some(fk), Some(mock)) = (
        bin("INIT_ROOT_BIN", "root/target/debug/init-root"),
        bin("INIT_ROOT_FAKEKERNEL_BIN", "root/target/debug/init-root-fakekernel"),
        bin("INIT_MOCKLLM_BIN", "kernel/target/debug/init-mockllm"),
    ) else {
        return;
    };
    let dir = home("r1");
    fs::remove_dir_all(dir.join("rh")).unwrap(); // the Root's install creates its home
    for d in ["bin", "rel", "tc/bin"] {
        fs::create_dir_all(dir.join(d)).unwrap();
    }
    // frozen copies
    let root_bin = dir.join("bin/init-root");
    fs::copy(&root, &root_bin).unwrap();
    fs::copy(&fk, dir.join("rel/init-kernel")).unwrap();
    let mock_bin = dir.join("bin/init-mockllm");
    fs::copy(&mock, &mock_bin).unwrap();
    let h = Home { rh: dir.join("rh"), ih: dir.join("ih"), root: root_bin, dir: dir.clone() };
    assert!(h.rh.to_string_lossy().len() <= 80, "ROOT_HOME must stay ≤ 80 bytes: {}", h.rh.display());
    // a test build answers `test ledger` without --spec with its usage (64); a production build with 2
    let probe = h.run(&["test", "ledger"]);
    if probe.status.code() != Some(64) {
        assert!(std::env::var_os("INIT_TUI_E2E").is_none(), "{} is not a test-fixtures build", root.display());
        eprintln!("SKIP e2e_root: {} is not a test-fixtures build (set INIT_ROOT_BIN)", root.display());
        return cleanup(&dir);
    }

    // the mock model: one expensive turn, then cheap ones
    let script = dir.join("mock.jsonl");
    let big = r#"{"v":1,"kind":"turn","reply":{"text":"Hello from the mock.","usage":{"input":30000,"cache_read":0,"output":2000}}}"#;
    fs::write(&script, format!("{big}\n{big}\n{big}\n")).unwrap();
    let port_file = dir.join("mock.port");
    let _m = Owned(
        Command::new(&mock_bin).args(["--mode", "upstream", "--listen", "127.0.0.1:0", "--strict", "deepseek", "--port-file"]).arg(&port_file)
            .arg("--script").arg(&script).arg("--record-dir").arg(dir.join("mock-rec")).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap(),
    );
    wait(10, "mock port", || fs::read_to_string(&port_file).is_ok_and(|s| s.trim().parse::<u16>().is_ok()));
    let port: u16 = fs::read_to_string(&port_file).unwrap().trim().parse().unwrap();

    // a test ledger at L1, then the install with the fake kernel as the release's init-kernel
    fs::create_dir_all(&h.rh).unwrap();
    let charter = format!(
        "schema_version = 1\n[purpose]\ntext = \"WP26 TUI e2e.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n\
         [sealed]\nreserve_usd = \"0.00\"\naudit_usd = \"0.00\"\n{ROOT_CFG}[money]\nday_usd = \"0.01\"\ntotal_usd = \"5.00\"\n\
         [actions.providers]\ndestinations = [\"127.0.0.1:{port}\"]\npaths = [\"chat/completions\", \"models\"]\n\
         [actions.providers.models.\"deepseek-flash\"]\nmin_level = 0\ninput_musd_per_mtok = 300000\noutput_musd_per_mtok = 1200000\n\
         [actions.providers.providers.deepseek]\nhost = \"127.0.0.1:{port}\"\n"
    );
    fs::write(h.rh.join("charter.toml"), charter).unwrap();
    let spec = dir.join("spec.json");
    fs::write(&spec, r#"{"start_at":"2026-10-01T00:00:00Z","start_level":1,"records":[]}"#).unwrap();
    h.ok(&["test", "ledger", "--spec", spec.to_str().unwrap()]);
    let (rel, tc) = (dir.join("rel"), dir.join("tc"));
    let o = h.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    let kp = h.rh.join("keys/providers/deepseek");
    fs::create_dir_all(kp.parent().unwrap()).unwrap();
    fs::write(&kp, KEY).unwrap();
    fs::set_permissions(&kp, fs::Permissions::from_mode(0o600)).unwrap();
    let mut d = h.daemon();
    h.slot_is(30, "running");
    assert_eq!(h.status()["level"], 1);

    // an observer: a second client in --raw mode logs every frame (shown when a step fails)
    let raw_log = dir.join("raw.log");
    let _obs = Owned(
        Command::new(BIN).env_clear().env("HOME", &dir).env("PATH", "/usr/bin:/bin").args(["--raw", "--wait", "--root-home"]).arg(&h.rh)
            .current_dir(&dir).stdin(Stdio::piped()).stdout(fs::File::create(&raw_log).unwrap()).stderr(Stdio::null()).spawn().unwrap(),
    );
    // ---- TTY mode through a pty: TUI-R1, TUI-R3 ----
    let mut t = Drive::start(&h.rh);
    t.raw_log = Some(raw_log.clone());
    t.ok(json!(["wait", "running", 15]));
    t.ok(json!(["wait", "L1", 5]));
    t.ok(json!(["mark"]));
    t.ok(json!(["typing", "hello root", 0.02]));
    t.ok(json!(["type", "\r"]));
    t.ok(json!(["wait", "body: hello root", 15]));
    // TUI-R3 (2026-10-23 owner money policy: observe-only): an expensive call spends past the legacy $0.01 day
    // value, the status bar shows the live estimate, and the next call still goes through: no ceiling notice.
    t.ok(json!(["mark"]));
    let r = h.chat(2000);
    assert!(r.starts_with("HTTP/1.1 200"), "{}", &r[..r.len().min(300)]);
    t.ok(json!(["wait", "live total est", 15]));
    let r = h.chat(2000);
    assert!(r.starts_with("HTTP/1.1 200"), "observe-only: the second call is not refused: {}", &r[..r.len().min(300)]);
    // TUI-R1: the demotion L1→L0. The refused call may demote by itself (egress.md D6 rule c); if it did not,
    // a scripted one through the test ledger (root.md §3.16)
    if h.status()["level"] == 1 {
        let hit = json!({"kind": "ceiling_hit", "data": {"attempted": 1, "ceiling": "level", "code": "root.level", "demotes": true, "level": 1}});
        h.owner("test.append", json!({"records": [hit.clone(), hit.clone(), hit]}));
    }
    wait(15, "the demotion", || h.status()["level"] == 0);
    t.ok(json!(["wait", "▼ [root] demoted L1→L0", 10]));
    t.ok(json!(["wait", "L0\u{1b}[", 10])); // the status bar's level (the notice text has no escape after it)
    // /panic: PANICKED in the status bar within 1 s
    t.ok(json!(["mark"]));
    let p0 = Instant::now();
    t.ok(json!(["type", "/panic\r"]));
    t.ok(json!(["wait", "PANICKED", 5]));
    let panic_ms = p0.elapsed().as_millis();
    assert_eq!(h.status()["slot"], "panicked");
    t.ok(json!(["mark"]));
    t.ok(json!(["type", "/resume\r"]));
    h.slot_is(30, "running");
    t.ok(json!(["wait", "running", 10]));
    t.ok(json!(["type", "\u{3}"]));
    t.ok(json!(["type", "\u{3}"]));
    let s = t.exit(10);
    assert_eq!(s["exit"], 0, "{}", s["output"]);
    assert_eq!(s["termios_same"], true);
    let tty_report: Vec<String> = s["output"].as_str().unwrap().split(['\r', '\n']).filter(|l| l.starts_with("init-tui ")).map(str::to_string).collect();
    let tty_out = s["output"].as_str().unwrap().to_string();
    // (observe-only money policy: the TUI raises no ceiling notice)
    assert!(!tty_out.contains("daily ceiling reached"), "observe-only: no ceiling notice");
    assert!(panic_ms <= 1000, "PANICKED shown {panic_ms} ms after /panic");

    // ---- line mode: latency through Root → kernel → body, then TUI-R2 ----
    let mut c = Command::new(BIN);
    c.env_clear().env("HOME", &dir).env("PATH", "/usr/bin:/bin").args(["--plain", "--wait", "--root-home"]).arg(&h.rh).current_dir(&dir);
    let mut tui = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut tin = tui.stdin.take().unwrap();
    let tout = tui.stdout.take().unwrap();
    let tui = Owned(tui);
    let (tx, rx) = mpsc::channel::<(Instant, String)>();
    std::thread::spawn(move || {
        for l in BufReader::new(tout).lines().map_while(Result::ok) {
            let _ = tx.send((Instant::now(), l));
        }
    });
    let mut seen: Vec<String> = Vec::new();
    let until = |secs: u64, f: &dyn Fn(&str) -> bool, seen: &mut Vec<String>| -> Instant {
        let t = Instant::now();
        loop {
            let left = Duration::from_secs(secs).checked_sub(t.elapsed()).unwrap_or_else(|| panic!("timeout; lines: {seen:#?}"));
            if let Ok((at, l)) = rx.recv_timeout(left) {
                let hit = f(&l);
                seen.push(l);
                if hit {
                    return at;
                }
            }
        }
    };
    until(15, &|l| l.starts_with("[root] attached:"), &mut seen);
    let mut lat = Vec::new();
    for i in 0..20 {
        let want = format!("body: ping {i}");
        let t0 = Instant::now();
        writeln!(tin, "ping {i}").unwrap();
        let at = until(10, &|l| l == want, &mut seen);
        lat.push(at.duration_since(t0).as_secs_f64() * 1000.0);
    }
    let (p50, p99) = (pct(&mut lat.clone(), 0.5), pct(&mut lat, 0.99));
    // TUI-R2: the zone stopped, three lines queued by the Root, the Root killed (our child) and restarted
    writeln!(tin, "/root stop").unwrap();
    h.slot_is(20, "stopped");
    for q in ["q1", "q2", "q3"] {
        writeln!(tin, "{q}").unwrap();
        let want = format!("> {q} · queued");
        until(10, &|l| l == want, &mut seen);
    }
    d.kill();
    let mut d2 = h.daemon();
    if h.status()["slot"] == "stopped" {
        h.owner("start", json!({}));
    }
    h.slot_is(30, "running");
    for q in ["q1", "q2", "q3"] {
        let want = format!("body: {q}");
        until(20, &|l| l == want, &mut seen);
    }
    drop(tin);
    let mut tui = tui;
    wait(10, "the TUI exits at EOF", || tui.0.try_wait().ok().flatten().is_some());
    while let Ok((_, l)) = rx.recv_timeout(Duration::from_millis(300)) {
        seen.push(l);
    }
    assert!(seen.iter().any(|l| l.starts_with("── Root restarted (")), "{seen:#?}");
    for q in ["q1", "q2", "q3"] {
        let lines = seen.iter().filter(|l| l.starts_with(&format!("> {q}"))).count();
        assert_eq!(lines, 1, "{q}: one transcript line (the queued echo and the kernel echo merged): {seen:#?}");
        let delivered = h.owner_in().iter().filter(|v| v["frame"]["text"] == q).count();
        assert_eq!(delivered, 1, "{q} reached the kernel exactly once");
    }
    for i in 0..20 {
        assert_eq!(h.owner_in().iter().filter(|v| v["frame"]["text"] == format!("ping {i}")).count(), 1);
    }
    eprintln!("TUI-R: /panic → PANICKED in the status bar: {panic_ms} ms");
    eprintln!("TUI-R: line → screen through Root → fake kernel → /bin/cat body → Root → TUI: p50 {p50:.1} ms, p99 {p99:.1} ms (n=20)");
    for l in tty_report {
        eprintln!("TUI-R TTY {l}");
    }
    d2.kill();
    drop(_m);
    cleanup(&dir);
}

/// WIRE-009 (ADR-009) end to end, $0: the REAL `init-root` (test mode) with a Python fake kernel
/// (tests/support/w9_kernel.py) whose body is `/bin/sh` under the real `body` profile, and
/// `init-mockllm` as the model. `init-tui` (line mode, `--workspace`) is launched in a fixture project dir: its cwd
/// becomes the workspace (the Root snapshots it and restarts the body with the grant), the mock's
/// `write` tool call is carried out by the sandboxed body inside the workspace, and `/undo` (typed
/// confirmation) restores the file.
#[test]
fn wire9_workspace_tool_write_and_undo() {
    let (Some(root), Some(mock)) = (bin("INIT_ROOT_BIN", "root/target/debug/init-root"), bin("INIT_MOCKLLM_BIN", "kernel/target/debug/init-mockllm")) else {
        return;
    };
    let dir = home("w9");
    fs::remove_dir_all(dir.join("rh")).unwrap();
    for d in ["bin", "rel", "tc/bin"] {
        fs::create_dir_all(dir.join(d)).unwrap();
    }
    let root_bin = dir.join("bin/init-root");
    fs::copy(&root, &root_bin).unwrap();
    let kernel = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/w9_kernel.py");
    fs::copy(&kernel, dir.join("rel/init-kernel")).unwrap();
    fs::set_permissions(dir.join("rel/init-kernel"), fs::Permissions::from_mode(0o755)).unwrap();
    let mock_bin = dir.join("bin/init-mockllm");
    fs::copy(&mock, &mock_bin).unwrap();
    let h = Home { rh: dir.join("rh"), ih: dir.join("ih"), root: root_bin, dir: dir.clone() };
    let probe = h.run(&["test", "ledger"]);
    if probe.status.code() != Some(64) {
        assert!(std::env::var_os("INIT_TUI_E2E").is_none(), "{} is not a test-fixtures build", root.display());
        eprintln!("SKIP wire9 e2e: {} is not a test-fixtures build (set INIT_ROOT_BIN)", root.display());
        return cleanup(&dir);
    }
    // the project: one file the agent will change
    let proj = dir.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("notes.txt"), "original\n").unwrap();
    let proj = fs::canonicalize(&proj).unwrap();

    // the mock model: one turn that calls `write`
    let script = dir.join("mock.jsonl");
    let call = r#"{"v":1,"kind":"turn","reply":{"think":"Change the notes.","calls":[{"name":"write","input":{"path":"notes.txt","content":"changed by the agent\n"}}],"stop":"tool_use","usage":{"input":100,"cache_read":0,"output":20}}}"#;
    fs::write(&script, format!("{call}\n")).unwrap();
    let port_file = dir.join("mock.port");
    let _m = Owned(
        Command::new(&mock_bin).args(["--mode", "upstream", "--listen", "127.0.0.1:0", "--strict", "deepseek", "--port-file"]).arg(&port_file)
            .arg("--script").arg(&script).arg("--record-dir").arg(dir.join("mock-rec")).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap(),
    );
    wait(10, "mock port", || fs::read_to_string(&port_file).is_ok_and(|s| s.trim().parse::<u16>().is_ok()));
    let port: u16 = fs::read_to_string(&port_file).unwrap().trim().parse().unwrap();

    fs::create_dir_all(&h.rh).unwrap();
    let charter = format!(
        "schema_version = 1\n[purpose]\ntext = \"WIRE-009 e2e.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n\
         [sealed]\nreserve_usd = \"0.00\"\naudit_usd = \"0.00\"\n{ROOT_CFG}[money]\nday_usd = \"1.00\"\ntotal_usd = \"5.00\"\n\
         [actions.providers]\ndestinations = [\"127.0.0.1:{port}\"]\npaths = [\"chat/completions\", \"models\"]\n\
         [actions.providers.models.\"deepseek-flash\"]\nmin_level = 0\ninput_musd_per_mtok = 300000\noutput_musd_per_mtok = 1200000\n\
         [actions.providers.providers.deepseek]\nhost = \"127.0.0.1:{port}\"\n"
    );
    fs::write(h.rh.join("charter.toml"), charter).unwrap();
    let spec = dir.join("spec.json");
    fs::write(&spec, r#"{"start_at":"2026-10-01T00:00:00Z","start_level":1,"records":[]}"#).unwrap();
    h.ok(&["test", "ledger", "--spec", spec.to_str().unwrap()]);
    let (rel, tc) = (dir.join("rel"), dir.join("tc"));
    let o = h.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    let kp = h.rh.join("keys/providers/deepseek");
    fs::create_dir_all(kp.parent().unwrap()).unwrap();
    fs::write(&kp, KEY).unwrap();
    fs::set_permissions(&kp, fs::Permissions::from_mode(0o600)).unwrap();
    let mut d = h.daemon();
    h.slot_is(30, "running");
    let klog = || fs::read_to_string(h.ih.join("state/kernel/w9k.jsonl")).unwrap_or_default();
    wait(20, "the fake kernel's first body", || klog().contains("\"ev\":\"body\""));

    // init-tui in line mode, launched in the project dir; ADR-012: full access is the default, so
    // `--workspace` asks for ADR-009's workspace mode, which this test is about
    let mut c = Command::new(BIN);
    c.env_clear().env("HOME", &dir).env("PATH", "/usr/bin:/bin").args(["--plain", "--workspace", "--wait", "--root-home"]).arg(&h.rh).current_dir(&proj);
    let mut tui = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut tin = tui.stdin.take().unwrap();
    let tout = tui.stdout.take().unwrap();
    let tui = Owned(tui);
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for l in BufReader::new(tout).lines().map_while(Result::ok) {
            let _ = tx.send(l);
        }
    });
    let mut seen: Vec<String> = Vec::new();
    let mut until = |secs: u64, what: &str, f: &dyn Fn(&str) -> bool| -> String {
        let t = Instant::now();
        loop {
            let left = Duration::from_secs(secs).checked_sub(t.elapsed()).unwrap_or_else(|| panic!("timeout waiting for {what}; lines: {seen:#?}\nkernel log:\n{}", klog()));
            if let Ok(l) = rx.recv_timeout(left) {
                let hit = f(&l);
                seen.push(l.clone());
                if hit {
                    return l;
                }
            }
        }
    };
    until(15, "attach", &|l| l.starts_with("[root] attached:"));
    // the workspace is the TUI's cwd; the Root says so, and the body restarts with the grant
    let ws_line = format!("[root] workspace: {}", proj.display());
    until(20, "the workspace notice", &|l| l.starts_with(&ws_line));
    let grant = format!("body: ws={} access=workspace", proj.display());
    until(30, "the restarted body with the grant", &|l| l == grant);
    let st = h.status();
    assert_eq!(st["access"]["workspace"], json!(proj.to_string_lossy()), "{st}");
    // the mock's tool call, carried out by the sandboxed body in the workspace
    writeln!(tin, "!tool please change the notes").unwrap();
    until(30, "the write", &|l| l == "body: wrote notes.txt");
    assert_eq!(fs::read_to_string(proj.join("notes.txt")).unwrap(), "changed by the agent\n");
    // /undo: the Root asks for its phrase, the TUI shows it, the next line confirms
    writeln!(tin, "/undo").unwrap();
    let ask = until(20, "the confirmation request", &|l| l.starts_with("[tui] type "));
    let phrase = ask.trim_start_matches("[tui] type ").split(" and press Enter").next().unwrap().to_string();
    assert!(!phrase.is_empty(), "{ask}");
    writeln!(tin, "{phrase}").unwrap();
    until(60, "the restore", &|l| l.starts_with("[tui] /undo: restored snapshot "));
    assert_eq!(fs::read_to_string(proj.join("notes.txt")).unwrap(), "original\n", "/undo restored the file");
    // nothing of the access flow reached the kernel as an owner line
    let k = klog();
    for l in ["/undo", phrase.as_str()] {
        assert!(!k.contains(&format!("\"text\":\"{l}\"")), "{l} reached the kernel: {k}");
    }
    assert!(k.contains("\"text\":\"!tool please change the notes\""));
    let snaps: Vec<_> = fs::read_dir(h.rh.join("snapshots")).map(|r| r.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
    assert!(snaps.len() >= 2, "the session snapshot and the pre-undo one: {snaps:?}");
    drop(tin);
    let mut tui = tui;
    wait(10, "the TUI exits at EOF", || tui.0.try_wait().ok().flatten().is_some());
    eprintln!("WIRE-009 e2e: workspace {} · write by the sandboxed body · /undo restored · snapshots {snaps:?}", proj.display());
    d.kill();
    drop(_m);
    cleanup(&dir);
}

/// UX-KEYMODEL, $0: the first-run setup in TTY mode (through the pty) against the REAL `init-root` (test mode) and
/// its fake kernel (`/bin/cat` body). No provider key is set, so `init-agent` guides provider → key (hidden) → model; the
/// Root stores the key (0600), adds deepseek-v4-pro to the charter, selects it in kernel/config.toml and restarts
/// the kernel; then a chat line round-trips, `/model` switches back, and `/clear` reaches the kernel as `/new`. The
/// key never appears on the screen, in a log, or in any file but its own.
#[test]
fn uxk_first_run_setup_key_model_then_chat() {
    let (Some(root), Some(fk)) = (bin("INIT_ROOT_BIN", "root/target/debug/init-root"), bin("INIT_ROOT_FAKEKERNEL_BIN", "root/target/debug/init-root-fakekernel")) else {
        return;
    };
    let dir = home("uxk");
    fs::remove_dir_all(dir.join("rh")).unwrap();
    for d in ["bin", "rel", "tc/bin"] {
        fs::create_dir_all(dir.join(d)).unwrap();
    }
    let root_bin = dir.join("bin/init-root");
    fs::copy(&root, &root_bin).unwrap();
    fs::copy(&fk, dir.join("rel/init-kernel")).unwrap();
    let h = Home { rh: dir.join("rh"), ih: dir.join("ih"), root: root_bin, dir: dir.clone() };
    let probe = h.run(&["test", "ledger"]);
    if probe.status.code() != Some(64) {
        assert!(std::env::var_os("INIT_TUI_E2E").is_none(), "{} is not a test-fixtures build", root.display());
        eprintln!("SKIP uxk e2e: {} is not a test-fixtures build (set INIT_ROOT_BIN)", root.display());
        return cleanup(&dir);
    }
    // a test-mode charter whose DeepSeek egress is a closed loopback port: nothing leaves this machine
    fs::create_dir_all(&h.rh).unwrap();
    let charter = format!(
        "schema_version = 1\n[purpose]\ntext = \"UX-KEYMODEL e2e.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n\
         [sealed]\nreserve_usd = \"0.00\"\naudit_usd = \"0.00\"\n{ROOT_CFG}\
         [actions.providers]\ndestinations = [\"127.0.0.1:9\"]\npaths = [\"chat/completions\", \"models\"]\n\
         [actions.providers.models.\"deepseek-flash\"]\nmin_level = 0\ninput_musd_per_mtok = 300000\noutput_musd_per_mtok = 1200000\n\
         [actions.providers.providers.deepseek]\nhost = \"127.0.0.1:9\"\n"
    );
    fs::write(h.rh.join("charter.toml"), charter).unwrap();
    let (rel, tc) = (dir.join("rel"), dir.join("tc"));
    let o = h.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    assert!(!h.rh.join("keys/providers/deepseek").exists(), "no key: the first run starts");
    let mut d = h.daemon();
    h.slot_is(30, "running");
    let list = h.owner("providers.list", json!({}));
    assert_eq!(list["ready"], false, "{list}");

    const KEY: &str = "test-uxk-first-run-7f7f7f7f";
    let mut t = Drive::start(&h.rh);
    t.ok(json!(["wait", "Choose a model provider", 15]));
    t.ok(json!(["wait", "experimental (not live-tested)", 5]));
    t.ok(json!(["mark"]));
    t.ok(json!(["type", "1\r"]));
    t.ok(json!(["wait", "Paste the DeepSeek API key", 10]));
    t.ok(json!(["typing", KEY, 0.005]));
    t.ok(json!(["wait", "••••", 5])); // the box shows a mask
    t.ok(json!(["mark"]));
    t.ok(json!(["type", "\r"]));
    t.ok(json!(["wait", "key stored for deepseek", 10]));
    t.ok(json!(["wait", "Choose a model (Enter", 5]));
    t.ok(json!(["mark"]));
    t.ok(json!(["type", "2\r"]));
    t.ok(json!(["wait", "model deepseek/deepseek-v4-pro", 20]));
    let kp = h.rh.join("keys/providers/deepseek");
    assert_eq!(fs::read_to_string(&kp).unwrap(), KEY);
    assert_eq!(fs::metadata(&kp).unwrap().permissions().mode() & 0o777, 0o600);
    let kcfg = fs::read_to_string(h.ih.join("kernel/config.toml")).unwrap();
    assert!(kcfg.contains("[model]\nprovider = \"deepseek\"\nid = \"deepseek-v4-pro\""), "{kcfg}");
    let ch = fs::read_to_string(h.rh.join("charter.toml")).unwrap();
    assert!(ch.contains("[actions.providers.models.deepseek-v4-pro]") || ch.contains("[actions.providers.models.\"deepseek-v4-pro\"]"), "{ch}");
    assert_eq!(ch, fs::read_to_string(h.rh.join("state/charter.active.toml")).unwrap(), "applied through charter.apply");
    assert_eq!(h.owner("providers.list", json!({}))["ready"], true);
    // the kernel restarted with the model: chat goes through Root → kernel → body
    h.slot_is(30, "running");
    t.ok(json!(["mark"]));
    t.ok(json!(["typing", "hello after setup", 0.01]));
    t.ok(json!(["type", "\r"]));
    t.ok(json!(["wait", "body: hello after setup", 20]));
    // /model: the list, then a pick by number
    t.ok(json!(["mark"]));
    t.ok(json!(["type", "/model\r"]));
    t.ok(json!(["wait", "deepseek/deepseek-flash", 10]));
    t.ok(json!(["mark"]));
    t.ok(json!(["type", "1\r"]));
    t.ok(json!(["wait", "model deepseek/deepseek-flash", 10]));
    h.slot_is(30, "running");
    // /clear is /new: the kernel receives "/new"
    t.ok(json!(["mark"]));
    t.ok(json!(["type", "/clear\r"]));
    t.ok(json!(["wait", "body: /new", 20]));
    t.ok(json!(["type", "\u{3}"]));
    t.ok(json!(["type", "\u{3}"]));
    let s = t.exit(10);
    assert_eq!(s["exit"], 0, "{}", s["output"]);
    assert_eq!(s["termios_same"], true);
    let out = s["output"].as_str().unwrap();
    assert!(!out.contains(KEY), "the key was shown on the screen");
    let fk_lines: Vec<String> = h.owner_in().iter().filter_map(|v| v["frame"]["text"].as_str().map(String::from)).collect();
    assert!(fk_lines.iter().any(|l| l == "/new") && !fk_lines.iter().any(|l| l == "/clear" || l.contains(KEY)), "{fk_lines:?}");
    d.kill();
    // the key is in exactly one file under the test homes (logs, lineage, ring, queue, kernel state included)
    fn walk(p: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(p).into_iter().flatten().flatten() {
            let ty = e.file_type().unwrap();
            if ty.is_dir() { walk(&e.path(), out) } else if ty.is_file() { out.push(e.path()) }
        }
    }
    let mut files = Vec::new();
    walk(&h.rh, &mut files);
    walk(&h.ih, &mut files);
    let holders: Vec<&PathBuf> = files.iter().filter(|f| fs::read(f).is_ok_and(|b| b.windows(KEY.len()).any(|w| w == KEY.as_bytes()))).collect();
    assert_eq!(holders, vec![&kp], "only the 0600 key file holds the key");
    assert!(files.iter().any(|f| f.starts_with(h.rh.join("logs"))), "the Root's logs were searched");
    cleanup(&dir);
}
