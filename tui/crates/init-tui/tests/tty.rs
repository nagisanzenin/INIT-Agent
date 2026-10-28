//! TUI-12 and TUI-13 through a pseudo-terminal (tests/pty_drive.py, stdlib only, `python3 -I`):
//! TTY mode end to end against the fixture owner socket, and the latency and load figures of
//! docs/tui.md §6. The pty driver's child (init-tui) is the only process it signals, by its pid.

mod common;

use common::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

struct Drive {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    pub pid: u32,
}

impl Drive {
    fn start(rh: &std::path::Path, args: &[&str]) -> Drive {
        Drive::start_in(rh, args, None)
    }
    /// The same with the TUI's cwd (the driver's) set to `cwd` (WIRE-009: the workspace).
    fn start_in(rh: &std::path::Path, args: &[&str], cwd: Option<&std::path::Path>) -> Drive {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pty_drive.py");
        let mut cmd = Command::new("/usr/bin/python3");
        if let Some(c) = cwd {
            cmd.current_dir(c);
        }
        let mut child = cmd
            .arg("-I")
            .arg(script)
            .arg(std::env::var("TUI13_BIN").unwrap_or_else(|_| BIN.to_string()))
            .arg(rh)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut l = String::new();
        out.read_line(&mut l).unwrap();
        let pid = l.trim().strip_prefix("pid ").unwrap().parse().unwrap();
        Drive { child, stdin, out, pid }
    }
    fn cmd(&mut self, c: Value) -> String {
        writeln!(self.stdin, "{c}").unwrap();
        let mut l = String::new();
        self.out.read_line(&mut l).unwrap();
        l.trim().to_string()
    }
    fn ok(&mut self, c: Value) {
        let r = self.cmd(c.clone());
        assert_eq!(r, "ok", "{c}: {r}");
    }
    fn exit(&mut self, secs: u64) -> Value {
        let r = self.cmd(json!(["exit", secs]));
        assert_eq!(r, "ok", "exit: {r}");
        let mut l = String::new();
        self.out.read_line(&mut l).unwrap();
        let _ = self.child.wait();
        serde_json::from_str(&l).unwrap()
    }
}

impl Drop for Drive {
    fn drop(&mut self) {
        let _ = self.child.kill(); // our own child (the driver), by its handle
        let _ = self.child.wait();
    }
}

/// The `--latency-report` lines from the TUI's output.
fn report(out: &str) -> Vec<String> {
    out.split(['\r', '\n']).filter(|l| l.starts_with("init-tui ")).map(str::to_string).collect()
}

fn field(line: &str, k: &str) -> u64 {
    line.split_whitespace().find_map(|w| w.strip_prefix(&format!("{k}="))).map(|v| v.trim_end_matches("us").parse::<i64>().unwrap_or(-1).max(0) as u64).unwrap_or(0)
}

#[test]
fn tui12_tty_session_through_a_pty() {
    let fx = Fixture::start("t12");
    fx.emit(json!({"boot_seq": 1, "kind": "text", "t": "display", "text": "earlier reply\n"}));
    let mut d = Drive::start(&fx.rh, &["--latency-report"]);
    d.ok(json!(["wait", "root ok", 10]));
    d.ok(json!(["wait", "earlier reply", 5]));
    // type and send; the echo and the reply stream in
    d.ok(json!(["mark"]));
    d.ok(json!(["typing", "hello", 0.02]));
    d.ok(json!(["type", "\r"]));
    d.ok(json!(["wait", "fx: hello", 10]));
    // a tool call, then the transcript view: expand it, leave
    fx.emit(json!({"boot_seq": 1, "call_id": "c1", "kind": "tool_start", "name": "bash", "t": "display", "text": "cat SELF.md"}));
    fx.emit(json!({"boot_seq": 1, "call_id": "c1", "detail": "line 1 of the output", "kind": "tool_end", "name": "bash", "ok": true, "t": "display", "text": "exit 0, 96 lines"}));
    d.ok(json!(["wait", "exit 0, 96 lines", 5]));
    d.ok(json!(["mark"]));
    d.ok(json!(["type", "\u{f}"]));
    d.ok(json!(["wait", "\u{1b}[?1049h", 5]));
    d.ok(json!(["wait", "transcript", 5]));
    d.ok(json!(["type", "\r"]));
    d.ok(json!(["wait", "line 1 of the output", 5]));
    d.ok(json!(["type", "q"]));
    d.ok(json!(["wait", "\u{1b}[?1049l", 5]));
    // resize 80 → 120 → 60
    d.ok(json!(["resize", 40, 120]));
    d.ok(json!(["sleep", 0.3]));
    d.ok(json!(["resize", 24, 60]));
    d.ok(json!(["sleep", 0.3]));
    d.ok(json!(["mark"]));
    // Ctrl-C: interrupt; again within 2 s: quit, the terminal restored
    d.ok(json!(["type", "\u{3}"]));
    d.ok(json!(["wait", "interrupt sent", 5]));
    fx.wait_for(5, "interrupt", |s| s.received.iter().any(|r| r.frame["t"] == "interrupt"));
    d.ok(json!(["type", "\u{3}"]));
    let s = d.exit(10);
    assert_eq!(s["exit"], 0, "{}", s["output"]);
    assert_eq!(s["termios_same"], true, "the terminal modes are restored");
    let out = s["output"].as_str().unwrap();
    assert!(out.contains("\u{1b}[?2004h") && out.contains("\u{1b}[?2004l"), "bracketed paste on, then off");
    assert_eq!(fx.attach_frames("detach").len(), 1);
    assert_eq!(fx.attach_frames("interrupt").len(), 1);
    assert_eq!(fx.received().iter().filter(|r| r.frame["t"] == "line").count(), 1);
    for l in report(out) {
        eprintln!("TUI-12 {l}");
    }
    cleanup(&fx.dir);
}

/// STEER: the kernel's `state` frame with the steer lists (supervise.md §4.6).
fn steer_state(busy: bool, pending: &[u64], seen: &[u64], retracted: &[u64]) -> Value {
    json!({"busy": busy, "level": 0, "members": 0, "mode": "normal", "queued": pending.len(), "sha12": "3f9c0a1b2c4d", "slot": "running",
        "steer_pending": pending, "steer_retracted": retracted, "steer_seen": seen, "t": "state"})
}

/// Waits until the fixture received a frame matching `f`.
fn got(fx: &Fixture, what: &str, f: impl Fn(&Value) -> bool) {
    fx.wait_for(10, what, |s| s.received.iter().any(|r| f(&r.frame)));
}

/// STEER (tui.md §3.7.1) end to end through a pty, the test playing the kernel behind the fixture
/// owner socket: a line typed during a long bash call waits in the tray with "after bash: …" and the
/// running call is NOT marked `? no result`; the body's ack moves it into the transcript as `· seen`;
/// Tab queues the box as `/later` (the next task); Up takes back a line not yet read (its text returns
/// to the box); Esc with a steer pending sends `interrupt{then:"continue"}`, Esc with none a plain one.
#[test]
fn steer_tray_seen_tab_retract_and_esc_through_a_pty() {
    let fx = Fixture::start("steer");
    fx.state().auto_echo = false;
    let mut d = Drive::start(&fx.rh, &[]);
    d.ok(json!(["wait", "root ok", 10]));
    fx.emit(steer_state(false, &[], &[], &[]));

    // the run starts with a long bash call
    d.ok(json!(["typing", "fix the flaky test", 0.005]));
    d.ok(json!(["type", "\r"]));
    got(&fx, "line 1", |f| f["t"] == "line" && f["text"] == "fix the flaky test");
    fx.emit(json!({"seq": 1, "t": "echo", "text": "fix the flaky test"}));
    fx.emit(steer_state(true, &[1], &[], &[]));
    fx.emit(steer_state(true, &[], &[1], &[]));
    fx.emit(json!({"boot_seq": 1, "call_id": "c1", "kind": "tool_start", "name": "bash", "t": "display", "text": "cargo test -p init-root"}));
    d.ok(json!(["wait", "cargo test -p init-root", 5]));

    // a steer typed while it runs: the tray says when it will be read; the call keeps running
    d.ok(json!(["mark"]));
    d.ok(json!(["typing", "rename the test too", 0.005]));
    d.ok(json!(["type", "\r"]));
    got(&fx, "the steer", |f| f["t"] == "line" && f["text"] == "rename the test too");
    fx.emit(json!({"seq": 2, "t": "echo", "text": "rename the test too"}));
    fx.emit(steer_state(true, &[2], &[1], &[]));
    d.ok(json!(["wait", "after bash: cargo test", 5]));
    d.ok(json!(["sleep", 0.3]));
    // the batch ends, the body acks seq 2: the steer is in the transcript, marked seen
    d.ok(json!(["mark"]));
    fx.emit(json!({"boot_seq": 1, "call_id": "c1", "kind": "tool_end", "name": "bash", "ok": true, "t": "display", "text": "exit 0, 214 passed"}));
    fx.emit(steer_state(true, &[], &[1, 2], &[]));
    d.ok(json!(["wait", "· seen", 5])); // `rename the test too` then the dim suffix

    // Tab: the box goes out as `/later`, shown as the next task
    d.ok(json!(["mark"]));
    d.ok(json!(["typing", "then update the changelog", 0.005]));
    d.ok(json!(["type", "\t"]));
    got(&fx, "the follow-up", |f| f["t"] == "line" && f["text"] == "/later then update the changelog");
    fx.emit(json!({"seq": 3, "t": "echo", "text": "then update the changelog"}));
    fx.emit(steer_state(true, &[3], &[1, 2], &[]));
    d.ok(json!(["wait", "next task", 5]));

    // Up on the empty box: take it back (the kernel still holds it); its text returns to the box
    d.ok(json!(["mark"]));
    d.ok(json!(["type", "\u{1b}[A"]));
    got(&fx, "the retract", |f| f["t"] == "retract" && f["seq"] == 3);
    fx.emit(steer_state(true, &[], &[1, 2], &[3]));
    fx.emit(json!({"level": "info", "t": "notice", "text": "[kernel] taken back: message 3 will not be read", "ts": "x"}));
    d.ok(json!(["wait", "taken back: it was not read", 5]));
    d.ok(json!(["type", "\u{15}"])); // Ctrl-U clears the restored text

    // Esc with a steer pending: interrupt and steer
    d.ok(json!(["mark"]));
    fx.emit(json!({"boot_seq": 1, "call_id": "c2", "kind": "tool_start", "name": "bash", "t": "display", "text": "sleep 600"}));
    d.ok(json!(["typing", "stop, use nextest instead", 0.005]));
    d.ok(json!(["type", "\r"]));
    got(&fx, "the urgent steer", |f| f["t"] == "line" && f["text"] == "stop, use nextest instead");
    fx.emit(json!({"seq": 4, "t": "echo", "text": "stop, use nextest instead"}));
    fx.emit(steer_state(true, &[4], &[1, 2], &[3]));
    d.ok(json!(["wait", "after bash: sleep 600", 5]));
    d.ok(json!(["type", "\u{1b}"]));
    got(&fx, "interrupt then continue", |f| f["t"] == "interrupt" && f["then"] == "continue");
    d.ok(json!(["wait", "your note goes in now", 5]));
    // the body aborts the call, then reads the steer and continues
    fx.emit(json!({"boot_seq": 1, "call_id": "c2", "kind": "tool_end", "name": "bash", "ok": false, "t": "display", "text": "aborted after 3 s; process group killed"}));
    fx.emit(json!({"boot_seq": 1, "kind": "info", "t": "display", "text": "interrupted · continuing with your note"}));
    fx.emit(steer_state(true, &[], &[1, 2, 4], &[3]));
    d.ok(json!(["wait", "· seen", 5]));
    // Esc with nothing pending: a plain interrupt
    d.ok(json!(["sleep", 1.1]));
    d.ok(json!(["type", "\u{1b}"]));
    fx.wait_for(10, "a plain interrupt", |s| s.received.iter().filter(|r| r.frame["t"] == "interrupt").count() == 2);
    fx.emit(steer_state(false, &[], &[1, 2, 4], &[3]));

    d.ok(json!(["type", "\u{3}"]));
    d.ok(json!(["type", "\u{3}"]));
    let s = d.exit(10);
    assert_eq!(s["exit"], 0, "{}", s["output"]);
    let out = s["output"].as_str().unwrap();
    assert!(!out.contains("no result"), "a steer never marks a running call `? no result`");
    let ints: Vec<Value> = fx.received().into_iter().filter(|r| r.frame["t"] == "interrupt").map(|r| r.frame).collect();
    assert_eq!(ints[..2], [json!({"t": "interrupt", "then": "continue"}), json!({"t": "interrupt"})], "Esc with a steer, then Esc with none (then the Ctrl-C that quits)");
    let lines: Vec<String> = fx.received().iter().filter(|r| r.frame["t"] == "line").map(|r| r.frame["text"].as_str().unwrap().to_string()).collect();
    assert_eq!(lines, ["fix the flaky test", "rename the test too", "/later then update the changelog", "stop, use nextest instead"]);
    cleanup(&fx.dir);
}

/// STEER in line mode (`--plain`, no animation): a line echoed while the agent works prints
/// `[steer queued] …`, then `[steer seen] …` once the kernel reports the body's ack; a taken-back
/// line prints `[steer retracted] …`.
#[test]
fn steer_lines_in_line_mode() {
    let fx = Fixture::start("steer-line");
    fx.state().auto_echo = false;
    let st = fx.st.clone();
    let kernel = std::thread::spawn(move || {
        let wait = |text: &str| {
            let t = Instant::now();
            while !lock(&st).received.iter().any(|r| r.frame["t"] == "line" && r.frame["text"] == text) {
                assert!(t.elapsed() < Duration::from_secs(10), "no line {text}");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait("first");
        emit(&mut lock(&st), json!({"seq": 1, "t": "echo", "text": "first"}));
        emit(&mut lock(&st), steer_state(true, &[], &[1], &[]));
        wait("second");
        emit(&mut lock(&st), json!({"seq": 2, "t": "echo", "text": "second"}));
        emit(&mut lock(&st), json!({"seq": 3, "t": "echo", "text": "third"}));
        emit(&mut lock(&st), steer_state(true, &[2, 3], &[1], &[]));
        emit(&mut lock(&st), steer_state(true, &[2], &[1], &[3]));
        emit(&mut lock(&st), steer_state(true, &[], &[1, 2], &[3]));
        emit(&mut lock(&st), json!({"boot_seq": 1, "kind": "text", "t": "display", "text": "ok, both\n"}));
        emit(&mut lock(&st), steer_state(false, &[], &[1, 2], &[3]));
    });
    let o = run(&fx.rh, &["--plain", "--wait-idle", "--timeout", "20"], "first\nsecond\n/later third\n", 30);
    kernel.join().unwrap();
    let out = text(&o);
    let lines: Vec<&str> = out.lines().collect();
    let at = |s: &str| lines.iter().position(|l| *l == s).unwrap_or_else(|| panic!("no line {s:?} in:\n{out}"));
    assert!(at("> first") < at("[steer queued] second"), "{out}");
    assert!(at("[next task queued] third") < at("[steer retracted] third"), "{out}");
    assert!(at("[steer retracted] third") < at("[steer seen] second"), "{out}");
    assert!(at("[steer seen] second") < at("ok, both"), "{out}");
    cleanup(&fx.dir);
}

/// WIRE-009 in TTY mode: the workspace in the status bar, `/undo` and `/full on` with the Root's
/// typed confirmation (the hint names the phrase, the next Enter answers it), then `FULL`.
#[test]
fn wire9_tty_undo_and_full_with_confirmations() {
    let fx = Fixture::start("w9-tty");
    fx.state().access = Some(access_view(None, false));
    let proj = fx.dir.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let mut d = Drive::start_in(&fx.rh, &[], Some(&proj));
    d.ok(json!(["wait", "root ok", 10]));
    fx.wait_for(10, "workspace.set", |s| s.access.as_ref().is_some_and(|a| a["workspace"].is_string()));
    d.ok(json!(["wait", "/proj", 10])); // the workspace in the status bar
    d.ok(json!(["mark"]));
    d.ok(json!(["typing", "/undo", 0.01]));
    d.ok(json!(["type", "\r"]));
    d.ok(json!(["wait", "type confirm and press Enter", 10]));
    d.ok(json!(["typing", "confirm", 0.01]));
    d.ok(json!(["type", "\r"]));
    d.ok(json!(["wait", "/undo: restored snapshot s1", 10]));
    d.ok(json!(["mark"]));
    d.ok(json!(["typing", "/full on", 0.01]));
    d.ok(json!(["type", "\r"]));
    d.ok(json!(["wait", "type confirm and press Enter", 10]));
    d.ok(json!(["typing", "confirm", 0.01]));
    d.ok(json!(["type", "\r"]));
    d.ok(json!(["wait", "FULL", 10]));
    d.ok(json!(["type", "\u{3}"]));
    d.ok(json!(["type", "\u{3}"]));
    let s = d.exit(10);
    assert_eq!(s["exit"], 0, "{}", s["output"]);
    let undos: Vec<_> = fx.received().iter().filter(|r| r.frame["cmd"] == "undo").map(|r| r.frame["args"].clone()).collect();
    assert_eq!(undos, [json!({}), json!({"confirm": "confirm"})]);
    assert_eq!(fx.state().access.as_ref().unwrap()["full"], true);
    assert_eq!(fx.received().iter().filter(|r| r.frame["t"] == "line").count(), 0, "no access line reached the attach channel");
    cleanup(&fx.dir);
}

fn flood(fx: &Fixture, per_sec: u64, secs: f64, label: &str) -> u64 {
    let n = (per_sec as f64 * secs) as u64;
    let start = Instant::now();
    for i in 0..n {
        let due = start + Duration::from_secs_f64(i as f64 / per_sec as f64);
        if let Some(w) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(w);
        }
        let text = if i % 20 == 19 { format!("{label}{i}\n") } else { format!("{label}{i} ") };
        fx.emit(json!({"boot_seq": 1, "kind": "text", "t": "display", "text": text}));
    }
    n
}

/// The TUI under the driver's shell (`pgrep -P`, read-only).
fn tui_pid(shell: u32) -> u32 {
    let t = Instant::now();
    loop {
        let o = Command::new("/usr/bin/pgrep").args(["-P", &shell.to_string()]).output().unwrap();
        if let Some(p) = String::from_utf8_lossy(&o.stdout).split_whitespace().next().and_then(|p| p.parse().ok()) {
            return p;
        }
        assert!(t.elapsed() < Duration::from_secs(5), "no init-tui under the shell {shell}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn cpu_secs(pid: u32) -> f64 {
    // read-only: `ps` sends no signal
    let o = Command::new("/bin/ps").args(["-o", "time=", "-p", &pid.to_string()]).output().unwrap();
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    let (m, sec) = s.split_once(':').unwrap_or(("0", "0"));
    m.parse::<f64>().unwrap_or(0.0) * 60.0 + sec.parse::<f64>().unwrap_or(0.0)
}

fn rss_kib(pid: u32) -> u64 {
    let o = Command::new("/bin/ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0)
}

/// One TUI session against a fresh fixture: `body` drives it; returns the `--latency-report` lines.
fn session(name: &str, prefill: usize, body: impl FnOnce(&Fixture, &mut Drive)) -> Vec<String> {
    let fx = Fixture::start(name);
    if prefill > 0 {
        let mut st = fx.state();
        let pad = "x".repeat(100);
        while st.ring.iter().map(String::len).sum::<usize>() < prefill && st.ring.len() < 2000 {
            let i = st.ring.len();
            emit(&mut st, json!({"boot_seq": 1, "kind": "text", "t": "display", "text": format!("replay {i} {pad}\n")}));
        }
    }
    let mut d = Drive::start(&fx.rh, &["--latency-report"]);
    d.ok(json!(["wait", "root ok", 10]));
    body(&fx, &mut d);
    d.ok(json!(["sleep", 0.5]));
    d.ok(json!(["type", "\u{3}"]));
    d.ok(json!(["type", "\u{3}"]));
    let s = d.exit(20);
    assert_eq!(s["exit"], 0);
    let r = report(s["output"].as_str().unwrap());
    let attaches = fx.attach_frames("attach").len();
    assert_eq!(attaches, 1, "{name}: one attach, no reconnect");
    cleanup(&fx.dir);
    r
}

fn line_of<'a>(r: &'a [String], k: &str) -> &'a str {
    r.iter().find(|l| l.contains(k)).map(String::as_str).unwrap_or("")
}

/// TUI-13 (full tier): `cargo test --offline --test tty -- --ignored`. Prints the measurements and
/// checks them against docs/tui.md §6, times `TUI13_SLACK` (default 1.0; a debug build or a loaded
/// machine may need more). `TUI13_BIN` picks the binary (e.g. target/release/init-tui).
#[test]
#[ignore]
fn tui13_latency_and_load() {
    let slack: f64 = std::env::var("TUI13_SLACK").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let ms = |us: u64| us as f64 / 1000.0;
    let mut fails = Vec::new();
    let mut check = |ok: bool, what: String| {
        eprintln!("TUI-13 {} {what}", if ok { "PASS" } else { "MISS" });
        if !ok {
            fails.push(what);
        }
    };
    // A: keystrokes every 40 ms while a reply streams at 200 frames/s
    let a = session("t13a", 0, |fx, d| {
        std::thread::scope(|s| {
            let h = s.spawn(|| flood(fx, 200, 3.0, "a"));
            d.ok(json!(["typing", "the quick brown fox jumps over the lazy dog", 0.04]));
            h.join().unwrap();
        });
    });
    let (k, f) = (line_of(&a, "keystroke->flush"), line_of(&a, "frame->flush"));
    check(ms(field(k, "p50")) <= 4.0 * slack && ms(field(k, "p99")) <= 16.0 * slack, format!("keystroke p50 ≤ 4 ms, p99 ≤ 16 ms: {k}"));
    check(ms(field(f, "p99")) <= 16.0 * slack, format!("frame p99 ≤ 16 ms at 200 frames/s: {f}"));
    // B: 2,000 frames/s
    let b = session("t13b", 0, |fx, _| {
        flood(fx, 2000, 3.0, "b");
    });
    let f = line_of(&b, "frame->flush");
    check(ms(field(f, "p99")) <= 33.0 * slack, format!("frame p99 ≤ 33 ms at 2,000 frames/s: {f}"));
    // C: first paint with a full 256 KiB replay; idle CPU; 50,000 blocks
    let mut idle = 0.0;
    let mut rss = 0;
    let c = session("t13c", 256 * 1024 - 200, |fx, d| {
        d.ok(json!(["sleep", 1.0]));
        let pid = tui_pid(d.pid);
        let c0 = cpu_secs(pid);
        d.ok(json!(["sleep", 5.0]));
        idle = (cpu_secs(pid) - c0) / 5.0 * 100.0;
        for i in 0..50_000u64 {
            let mut st = fx.state();
            emit(&mut st, json!({"boot_seq": 1, "kind": "info", "t": "display", "text": format!("block {i}")}));
        }
        d.ok(json!(["sleep", 3.0]));
        rss = rss_kib(pid);
    });
    let sum = line_of(&c, "first_paint_ms");
    check(field(sum, "first_paint_ms") as f64 <= 150.0 * slack, format!("first paint ≤ 150 ms with a full replay: {sum}"));
    check(idle < 0.5, format!("idle CPU < 0.5% of a core: {idle:.2}%"));
    check(rss / 1024 <= 64, format!("RSS ≤ 64 MiB with 50,000 blocks: {} MiB", rss / 1024));
    // D: 5,000 frames/s for 10 s (the doc's 60 s run is the same loop, longer)
    let secs: f64 = std::env::var("TUI13_FLOOD_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let dd = session("t13d", 0, |fx, _| {
        flood(fx, 5000, secs, "d");
    });
    let sum = line_of(&dd, "first_paint_ms");
    check(field(sum, "disconnects") == 0, format!("0 disconnects at 5,000 frames/s for {secs} s: {sum}; {}", line_of(&dd, "frame->flush")));
    assert!(fails.is_empty(), "TUI-13 misses: {fails:#?}");
}
