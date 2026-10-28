//! TUI-5…TUI-10 against the std fixture owner socket (docs/tui.md §8): the protocol, reconnects with
//! the same and a new epoch, the echo merge, the `/panic` paths, and line mode with its exit codes.
//! Every process here is a child of this test, stopped by its handle; homes are scratch dirs.

mod common;

use common::*;
use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::time::{Duration, Instant};

#[test]
fn tui5_attach_first_requests_elsewhere_only_client_frames() {
    let fx = Fixture::start("t5");
    fx.emit(json!({"boot_seq": 1, "kind": "text", "t": "display", "text": "replayed\n", "zzz": 1}));
    fx.emit(json!({"t": "zzz_unknown", "text": "ignored"}));
    let o = run(&fx.rh, &["--plain", "--wait-idle", "--timeout", "20"], "hello\n", 30);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let out = text(&o);
    assert!(out.starts_with("replayed\n"), "replay first: {out}");
    assert!(out.contains("[root] attached:"), "{out}");
    assert!(out.contains("> hello\nfx: hello\n"), "{out}");
    assert!(!out.contains("ignored"));
    let rec = fx.received();
    // the attach connection: attach first, then only the five client frames
    let attach_conn = rec.iter().find(|r| r.frame["t"] == "attach").unwrap().conn;
    let on_attach: Vec<_> = rec.iter().filter(|r| r.conn == attach_conn).collect();
    assert_eq!(on_attach[0].frame["t"], "attach");
    assert_eq!(on_attach[0].frame["client"], format!("init-tui/{}", env!("CARGO_PKG_VERSION")));
    for r in &on_attach {
        assert!(["attach", "line", "interrupt", "panic", "detach"].contains(&r.frame["t"].as_str().unwrap_or("")), "{:?}", r.frame);
        assert!(r.frame.get("cmd").is_none(), "no request on the attach connection");
    }
    assert_eq!(on_attach.last().unwrap().frame["t"], "detach");
    // requests only on other connections
    let reqs: Vec<_> = rec.iter().filter(|r| r.frame.get("cmd").is_some()).collect();
    assert!(reqs.iter().any(|r| r.frame["cmd"] == "status"));
    assert!(reqs.iter().all(|r| r.conn != attach_conn && r.frame["id"].as_str().unwrap().starts_with("t-")));
    cleanup(&fx.dir);
}

/// A long-running line-mode TUI with piped stdin and stdout lines collected on a thread.
struct Live {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    rx: std::sync::mpsc::Receiver<String>,
    pub lines: Vec<String>,
}

impl Live {
    fn start(rh: &std::path::Path, args: &[&str]) -> Live {
        let mut child = tui(rh, args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let stdin = child.stdin.take();
        let out = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send(l);
            }
        });
        Live { child, stdin, rx, lines: Vec::new() }
    }
    fn send(&mut self, s: &str) {
        writeln!(self.stdin.as_mut().unwrap(), "{s}").unwrap();
    }
    fn until(&mut self, secs: u64, f: impl Fn(&[String]) -> bool) {
        let t = Instant::now();
        while !f(&self.lines) {
            let left = Duration::from_secs(secs).checked_sub(t.elapsed()).unwrap_or_else(|| panic!("timeout; lines: {:#?}", self.lines));
            if let Ok(l) = self.rx.recv_timeout(left.min(Duration::from_millis(200))) {
                self.lines.push(l);
            }
        }
    }
    fn finish(mut self, secs: u64) -> (Option<i32>, Vec<String>) {
        drop(self.stdin.take());
        let t = Instant::now();
        let code = loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                break s.code();
            }
            if t.elapsed() > Duration::from_secs(secs) {
                let _ = self.child.kill(); // our own child
                let _ = self.child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        while let Ok(l) = self.rx.recv_timeout(Duration::from_millis(300)) {
            self.lines.push(l);
        }
        (code, std::mem::take(&mut self.lines))
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.child.kill(); // our own child, if still running
        let _ = self.child.wait();
    }
}

#[test]
fn tui6_same_epoch_reconnect_never_shows_a_frame_twice() {
    for (case, numbered) in [("t6a", false), ("t6b", true)] {
        let fx = Fixture::start(case);
        fx.state().numbered = numbered;
        let mut live = Live::start(&fx.rh, &["--raw", "--wait"]);
        fx.wait_for(10, "attached", |s| !s.clients.is_empty());
        let mut n = 0;
        // drop the connection after various numbers of frames
        for drops in [0usize, 1, 3, 7, 20] {
            for _ in 0..drops {
                n += 1;
                fx.emit(json!({"boot_seq": 1, "kind": "text", "t": "display", "text": format!("f{n}")}));
            }
            let want = format!("f{n}");
            if n > 0 {
                live.until(10, |ls| ls.iter().any(|l| l.contains(&format!("\"text\":\"{want}\""))));
            }
            fx.kick();
            fx.wait_for(10, "re-attached", |s| !s.clients.is_empty());
            std::thread::sleep(Duration::from_millis(100));
        }
        n += 1;
        fx.emit(json!({"boot_seq": 1, "kind": "text", "t": "display", "text": format!("f{n}")}));
        let last = format!("\"text\":\"f{n}\"");
        live.until(10, |ls| ls.iter().any(|l| l.contains(&last)));
        let (_, lines) = live.finish(5);
        let shown: Vec<String> = lines.iter().filter(|l| l.contains("\"kind\":\"text\"")).cloned().collect();
        let texts: Vec<String> = shown.iter().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["text"].as_str().unwrap().to_string()).collect();
        let want: Vec<String> = (1..=n).map(|i| format!("f{i}")).collect();
        assert_eq!(texts, want, "{case}: each frame exactly once, in order");
        if numbered {
            // later attaches carried `since`
            let attaches = fx.attach_frames("attach");
            assert!(attaches.iter().skip(1).any(|a| a["since"].is_u64()), "{attaches:?}");
        }
        cleanup(&fx.dir);
    }
}

#[test]
fn tui6_rotated_ring_prints_the_missing_divider() {
    let fx = Fixture::start("t6r");
    fx.state().ring_max = 3;
    let mut live = Live::start(&fx.rh, &["--plain", "--wait"]);
    fx.wait_for(10, "attached", |s| !s.clients.is_empty());
    fx.emit(json!({"boot_seq": 1, "kind": "text", "t": "display", "text": "a\n"}));
    live.until(10, |ls| ls.iter().any(|l| l == "a"));
    // the client is gone while the ring rotates past everything it saw
    fx.state().refuse_attach = true;
    fx.kick();
    std::thread::sleep(Duration::from_millis(300));
    for i in 0..5 {
        fx.emit(json!({"boot_seq": 1, "kind": "text", "t": "display", "text": format!("r{i}\n")}));
    }
    fx.state().refuse_attach = false;
    live.until(10, |ls| ls.iter().any(|l| l == "r4"));
    let (_, lines) = live.finish(5);
    assert!(lines.iter().any(|l| l == "── reconnected; some output may be missing ──"), "{lines:#?}");
    assert_eq!(lines.iter().filter(|l| *l == "a").count(), 1);
    cleanup(&fx.dir);
}

#[test]
fn tui7_new_epoch_with_pending_lines_sends_each_line_once() {
    let fx = Fixture::start("t7");
    fx.state().auto_echo = false;
    let mut live = Live::start(&fx.rh, &["--plain", "--wait"]);
    fx.wait_for(10, "attached", |s| !s.clients.is_empty());
    for l in ["one", "two", "three"] {
        live.send(l);
    }
    fx.wait_for(10, "three lines", |s| s.received.iter().filter(|r| r.frame["t"] == "line").count() == 3);
    fx.restart("2026-10-20T22:00:00Z", 2002);
    fx.wait_for(10, "re-attached", |s| !s.clients.is_empty());
    // the restarted Root redelivers; the kernel echoes each line once
    for (i, l) in ["one", "two", "three"].iter().enumerate() {
        fx.emit(json!({"seq": i + 1, "t": "echo", "text": l}));
    }
    live.until(10, |ls| ls.iter().any(|l| l == "> three"));
    let (_, lines) = live.finish(5);
    assert!(lines.iter().any(|l| l.starts_with("── Root restarted (22:00:00Z)")), "{lines:#?}");
    for l in ["> one", "> two", "> three"] {
        assert_eq!(lines.iter().filter(|x| *x == l).count(), 1, "{l}: {lines:#?}");
    }
    let sent = fx.received().into_iter().filter(|r| r.frame["t"] == "line").count();
    assert_eq!(sent, 3, "exactly one line frame per line typed; never resent");
    cleanup(&fx.dir);
}

#[test]
fn tui8_echo_merge() {
    let fx = Fixture::start("t8");
    fx.state().auto_echo = false;
    let mut live = Live::start(&fx.rh, &["--plain", "--wait"]);
    fx.wait_for(10, "attached", |s| !s.clients.is_empty());
    live.send("queued line");
    live.send("/later follow up");
    live.send("/new");
    fx.wait_for(10, "lines", |s| s.received.iter().filter(|r| r.frame["t"] == "line").count() == 3);
    fx.emit(json!({"qid": "q1", "t": "echo", "text": "queued line"}));
    fx.emit(json!({"seq": 1, "t": "echo", "text": "queued line"}));
    fx.emit(json!({"seq": 2, "t": "echo", "text": "follow up"}));
    fx.emit(json!({"seq": 3, "t": "echo", "text": "/new"}));
    fx.emit(json!({"seq": 4, "t": "echo", "text": "from another client"}));
    live.until(10, |ls| ls.iter().any(|l| l == "> from another client"));
    let (_, lines) = live.finish(5);
    let owner: Vec<&String> = lines.iter().filter(|l| l.starts_with("> ")).collect();
    assert_eq!(owner, ["> queued line · queued", "> follow up", "> /new", "> from another client"], "{lines:#?}");
    assert!(lines.iter().any(|l| l == "── new session ──"));
    cleanup(&fx.dir);
}

fn stub_init_root(rh: &std::path::Path) -> std::path::PathBuf {
    let bin = rh.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let rec = rh.join("stub-argv.txt");
    let stub = bin.join("init-root");
    fs::write(&stub, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\necho latched\n", rec.display())).unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    rec
}

#[test]
fn tui9_panic_paths() {
    // 1. attach up: one panic frame on the attach connection
    let fx = Fixture::start("t9a");
    let mut live = Live::start(&fx.rh, &["--plain", "--wait"]);
    fx.wait_for(10, "attached", |s| !s.clients.is_empty());
    std::thread::sleep(Duration::from_millis(200)); // past the replay and the greeting
    live.send("/panic");
    fx.wait_for(10, "the panic frame", |s| s.received.iter().any(|r| r.frame["t"] == "panic"));
    let (code, _) = live.finish(10);
    assert_eq!(code, Some(0));
    assert_eq!(fx.attach_frames("panic").len(), 1);
    assert!(fx.received().iter().all(|r| r.frame["cmd"] != "panic"), "no request while the attach is up");
    assert!(fx.received().iter().all(|r| r.frame["t"] != "line"), "/panic is never a line");
    cleanup(&fx.dir);
    // 2. attach down, the socket up: one panic request on a fresh connection
    let fx = Fixture::start("t9b");
    fx.state().refuse_attach = true;
    let rec = stub_init_root(&fx.rh);
    let o = run(&fx.rh, &["--plain", "--wait", "--timeout", "10"], "/panic\n", 20);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let reqs: Vec<_> = fx.received().into_iter().filter(|r| r.frame["cmd"] == "panic").collect();
    assert_eq!(reqs.len(), 1);
    assert!(text(&o).contains("[tui] /panic: panicked (fixture)"), "{}", text(&o));
    assert!(!rec.exists(), "the stub is not run while the socket answers");
    cleanup(&fx.dir);
    // 3. the socket absent: the Root's own fallback, `ROOT_HOME/bin/init-root --root-home <rh> panic`
    let dir = home("t9c");
    let rh = dir.join("rh");
    let rec = stub_init_root(&rh);
    let o = run(&rh, &["--plain", "--wait", "--timeout", "10"], "/panic\n", 20);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let argv = fs::read_to_string(&rec).unwrap();
    assert_eq!(argv, format!("--root-home\n{}\npanic\n", rh.display()));
    assert!(text(&o).contains("panic latched by"), "{}", text(&o));
    cleanup(&dir);
}

#[test]
fn tui10_line_mode_golden_raw_and_exit_codes() {
    let fx = Fixture::start("t10");
    fx.emit(json!({"boot_seq": 1, "kind": "thinking", "t": "display", "text": "hmm"}));
    fx.emit(json!({"boot_seq": 1, "kind": "thinking", "t": "display", "text": " more"}));
    fx.emit(json!({"boot_seq": 1, "kind": "text", "t": "display", "text": "Reading\tfile \u{1b}[31mred\u{1b}[0m"}));
    fx.emit(json!({"boot_seq": 1, "call_id": "c1", "kind": "tool_start", "name": "bash", "t": "display", "text": "cat SELF.md"}));
    fx.emit(json!({"boot_seq": 1, "call_id": "c1", "kind": "tool_end", "name": "bash", "ok": true, "t": "display", "text": "exit 0, 96 lines"}));
    fx.emit(json!({"boot_seq": 1, "call_id": "c2", "kind": "tool_end", "name": "edit", "ok": false, "t": "display", "text": "error: no_match"}));
    fx.emit(json!({"boot_seq": 1, "kind": "info", "t": "display", "text": "retrying (1/3)"}));
    fx.emit(json!({"boot_seq": 1, "kind": "error", "t": "display", "text": "provider: 500"}));
    fx.emit(json!({"boot_seq": 2, "kind": "turn_end", "t": "display", "text": "tokens: in 5, out 6"}));
    fx.emit(json!({"level": "warn", "t": "notice", "text": "[kernel] promoted", "ts": "x"}));
    let o = run(&fx.rh, &["--plain", "--wait-idle", "--timeout", "20"], "hello\n/status\n", 30);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    golden("tui10_plain.txt", &text(&o));
    // --raw: the frames as received, C1 and bidi re-escaped
    fx.emit(json!({"boot_seq": 2, "kind": "text", "t": "display", "text": "c1:\u{9b}31m bidi:\u{202e}x del:\u{7f}"}));
    let o = run(&fx.rh, &["--raw", "--no-replay", "--wait-idle", "--timeout", "20"], "", 30);
    assert_eq!(o.status.code(), Some(0));
    let raw = text(&o);
    assert!(raw.is_ascii(), "{raw}");
    for l in raw.lines() {
        serde_json::from_str::<serde_json::Value>(l).unwrap_or_else(|_| panic!("not JSON: {l}"));
    }
    let o = run(&fx.rh, &["--raw", "--wait-idle", "--timeout", "20"], "", 30);
    let raw = text(&o);
    assert!(raw.contains("\"text\":\"c1:\\u009b31m bidi:\\u202ex del:\\u007f\""), "{raw}");
    assert!(raw.contains(r#"{"t":"replay_end"}"#));
    // exit codes: 64 usage, 69 unreachable, 76 not a Root, 2 timeout
    assert_eq!(run(&fx.rh, &["--bogus"], "", 10).status.code(), Some(64));
    let nowhere = home("t10-none").join("rh");
    let o = run(&nowhere, &["--plain"], "", 10);
    assert_eq!(o.status.code(), Some(69));
    let g = Fixture::start("t10-garbage");
    g.state().garbage = true;
    assert_eq!(run(&g.rh, &["--plain"], "hi\n", 10).status.code(), Some(76));
    let q = Fixture::start("t10-quiet");
    q.state().auto_echo = false;
    let t = Instant::now();
    assert_eq!(run(&q.rh, &["--plain", "--wait-idle", "--timeout", "1"], "never echoed\n", 10).status.code(), Some(2));
    assert!(t.elapsed() < Duration::from_secs(5));
    // --version, --help
    let v = run(&fx.rh, &["--version"], "", 5);
    assert!(text(&v).starts_with("init-tui (INIT Agent TUI) "));
    for d in [&fx.dir, &g.dir, &q.dir, &nowhere.parent().unwrap().to_path_buf()] {
        cleanup(d);
    }
}

#[test]
fn tui10_interrupt_and_double_sigint_detach() {
    let fx = Fixture::start("t10s");
    let mut live = Live::start(&fx.rh, &["--plain", "--wait"]);
    fx.wait_for(10, "attached", |s| !s.clients.is_empty());
    let pid = live.child.id() as i32;
    // our own child, by its PID (line mode: SIGINT sends interrupt; twice within 2 s detaches)
    let sig = |p: i32| assert!(std::process::Command::new("/bin/kill").args(["-INT", &p.to_string()]).status().unwrap().success());
    sig(pid);
    fx.wait_for(10, "interrupt", |s| s.received.iter().any(|r| r.frame["t"] == "interrupt"));
    std::thread::sleep(Duration::from_millis(2200));
    sig(pid);
    fx.wait_for(10, "second interrupt", |s| s.received.iter().filter(|r| r.frame["t"] == "interrupt").count() == 2);
    sig(pid);
    let t = Instant::now();
    while live.child.try_wait().unwrap().is_none() {
        assert!(t.elapsed() < Duration::from_secs(5), "the second SIGINT within 2 s detaches");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(live.child.try_wait().unwrap().unwrap().code(), Some(0));
    assert_eq!(fx.attach_frames("detach").len(), 1);
    assert_eq!(fx.attach_frames("interrupt").len(), 2, "after 2.1 s the next Ctrl-C was another interrupt");
    cleanup(&fx.dir);
}

/// TUI-14: the TUI under `sandbox-exec` with a profile that denies every file write, every read under the
/// scratch ROOT_HOME except the socket, and all network except a connection to the test socket; the
/// TUI-5 session and a line-mode session pass unchanged (T-1, T-2, "writes no files").
#[test]
fn tui14_confined_session_passes_unchanged() {
    let fx = Fixture::start("t14");
    let sock = fx.rh.join("run/owner.sock");
    let profile = format!(
        "(version 1)\n(allow default)\n(deny network*)\n(allow network-outbound (remote unix-socket (path-literal \"{s}\")))\n\
         (deny file-write*)\n(allow file-write-data (literal \"/dev/null\") (regex #\"^/dev/tty\"))\n\
         (deny file-read* (subpath \"{rh}\"))\n(allow file-read* (literal \"{s}\"))\n",
        s = sock.display(),
        rh = fx.rh.display()
    );
    let canary = fx.rh.join("canary");
    fs::write(&canary, "secret").unwrap();
    // the profile works: a confined `cat` cannot read the canary nor write a file
    let o = std::process::Command::new("/usr/bin/sandbox-exec").args(["-p", &profile, "/bin/cat"]).arg(&canary).output().unwrap();
    assert!(!o.status.success(), "the profile denies reads under ROOT_HOME");
    let o = std::process::Command::new("/usr/bin/sandbox-exec").args(["-p", &profile, "/usr/bin/touch"]).arg(fx.dir.join("written")).output().unwrap();
    assert!(!o.status.success() && !fx.dir.join("written").exists(), "the profile denies file writes");
    let mut c = std::process::Command::new("/usr/bin/sandbox-exec");
    c.env_clear().env("HOME", &fx.dir).env("PATH", "/usr/bin:/bin").env("TERM", "dumb").args(["-p", &profile, BIN, "--plain", "--wait-idle", "--timeout", "20", "--root-home"]).arg(&fx.rh);
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"hello\n/status\n").unwrap();
    let o = wait_output(child, 30);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let out = text(&o);
    assert!(out.contains("> hello\nfx: hello\n") && out.contains("slot running · mode normal"), "{out}");
    // the /panic fallback path is denied nothing it needs: a request on a fresh connection
    fx.state().refuse_attach = true;
    let mut c = std::process::Command::new("/usr/bin/sandbox-exec");
    c.env_clear().env("HOME", &fx.dir).env("PATH", "/usr/bin:/bin").env("TERM", "dumb").args(["-p", &profile, BIN, "--plain", "--wait", "--timeout", "10", "--root-home"]).arg(&fx.rh);
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"/panic\n").unwrap();
    let o = wait_output(child, 20);
    assert_eq!(o.status.code(), Some(0));
    assert!(text(&o).contains("[tui] /panic: panicked (fixture)"));
    cleanup(&fx.dir);
}

/// WIRE-009 (ADR-009; root SBX-009 §A): the cwd is sent as `workspace.set` right after the first
/// attach and before any owner line; `/undo` asks for the Root's typed confirmation, then restores;
/// a wrong confirmation cancels `/full on`; none of these lines reaches the attach channel.
#[test]
fn wire9_workspace_first_undo_and_full_confirmations() {
    let fx = Fixture::start("w9-undo");
    fx.state().access = Some(access_view(None, false));
    let proj = fx.dir.join("proj");
    fs::create_dir_all(&proj).unwrap();
    let mut c = tui(&fx.rh, &["--plain", "--wait-idle", "--timeout", "20"]);
    c.current_dir(&proj).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut i = child.stdin.take().unwrap();
    std::thread::spawn(move || {
        let _ = i.write_all(b"hello\n/undo\nconfirm\n/full on\nnope\nbye\n");
    });
    let o = wait_output(child, 30);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let out = text(&o);
    let rec = fx.received();
    let ws_at = rec.iter().position(|r| r.frame["cmd"] == "workspace.set").expect("workspace.set sent");
    let ws = &rec[ws_at];
    assert!(!ws.attach, "a request connection, not the attach one");
    assert_eq!(ws.frame["args"]["cwd"], json!(proj.to_string_lossy()), "the TUI's cwd");
    let first_line = rec.iter().position(|r| r.attach && r.frame["t"] == "line").expect("a line");
    assert!(ws_at < first_line, "the workspace before any owner line");
    assert_eq!(rec.iter().filter(|r| r.frame["cmd"] == "workspace.set").count(), 1);
    let undos: Vec<_> = rec.iter().filter(|r| r.frame["cmd"] == "undo").map(|r| r.frame["args"].clone()).collect();
    assert_eq!(undos, [json!({}), json!({"confirm": "confirm"})]);
    let fulls: Vec<_> = rec.iter().filter(|r| r.frame["cmd"] == "full.set").map(|r| r.frame["args"].clone()).collect();
    assert_eq!(fulls, [json!({"on": true})], "a wrong phrase is never sent");
    let lines: Vec<_> = fx.attach_frames("line").into_iter().map(|f| f["text"].as_str().unwrap_or("").to_string()).collect();
    assert_eq!(lines, ["hello", "bye"], "access commands and answers stay off the attach channel");
    assert!(out.contains(&format!("[root] workspace: {}", proj.display())), "{out}");
    assert!(out.contains("[tui] undo restores the snapshot s1 of the workspace; type confirm"), "{out}");
    assert!(out.contains("[tui] type confirm and press Enter to go ahead with /undo; anything else cancels"), "{out}");
    assert!(out.contains("[tui] /undo: restored snapshot s1 (2026-10-20T21:00:00Z) of "), "{out}");
    assert!(out.contains("[tui] /full on: not confirmed; nothing changed"), "{out}");
    assert!(!out.contains("no workspace"), "{out}");
    let (h, b) = (out.find("> hello").unwrap(), out.find("> bye").unwrap());
    let u = out.find("restored snapshot").unwrap();
    assert!(h < u && u < b, "in order: {out}");
    cleanup(&fx.dir);
}

/// ADR-012: under a full-default charter `init-agent` gets full access with no confirmation;
/// `--workspace` confines the session in `workspace.set{cwd, mode:"workspace"}`, and `/full on`
/// then lifts it at once (the Root asks for no phrase).
#[test]
fn fullaccess_default_and_workspace_flag() {
    let fx = Fixture::start("fa-default");
    fx.state().access = Some(access_view(None, false));
    fx.state().charter_full = true;
    let proj = fx.dir.join("proj");
    fs::create_dir_all(&proj).unwrap();
    let o = {
        let mut c = tui(&fx.rh, &["--plain", "--wait-idle", "--timeout", "20"]);
        c.current_dir(&proj).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        let mut i = child.stdin.take().unwrap();
        std::thread::spawn(move || {
            let _ = i.write_all(b"hi\n");
        });
        wait_output(child, 30)
    };
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let ws: Vec<_> = fx.received().iter().filter(|r| r.frame["cmd"] == "workspace.set").map(|r| r.frame["args"].clone()).collect();
    assert_eq!(ws, [json!({"cwd": proj.to_string_lossy()})], "the default launch sends no mode");
    assert_eq!(fx.state().access.as_ref().unwrap()["full"], true, "full is the charter default");
    assert_eq!(fx.received().iter().filter(|r| r.frame["cmd"] == "full.set").count(), 0);

    let mut c = tui(&fx.rh, &["--plain", "--workspace", "--wait-idle", "--timeout", "20"]);
    c.current_dir(&proj).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut i = child.stdin.take().unwrap();
    std::thread::spawn(move || {
        let _ = i.write_all(b"one\n/full on\ntwo\n");
    });
    let o = wait_output(child, 30);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let out = text(&o);
    let rec = fx.received();
    let ws: Vec<_> = rec.iter().filter(|r| r.frame["cmd"] == "workspace.set").map(|r| r.frame["args"].clone()).collect();
    assert_eq!(ws.last(), Some(&json!({"cwd": proj.to_string_lossy(), "mode": "workspace"})));
    let fulls: Vec<_> = rec.iter().filter(|r| r.frame["cmd"] == "full.set").map(|r| r.frame["args"].clone()).collect();
    assert_eq!(fulls, [json!({"on": true})], "no confirmation round under a full-default charter");
    assert!(!out.contains("type confirm"), "{out}");
    assert!(out.contains("[root] full mode on"), "{out}");
    assert_eq!(fx.state().access.as_ref().unwrap()["full"], true);
    let lines: Vec<_> = fx.attach_frames("line").into_iter().map(|f| f["text"].as_str().unwrap_or("").to_string()).collect();
    assert_eq!(lines, ["hi", "one", "two"]);
    cleanup(&fx.dir);
}

/// Launched in `~`: no workspace, and the TUI says so when the Root does not (unchanged);
/// `--full` asks for full mode once the workspace is set, and the next line confirms it.
#[test]
fn wire9_home_has_no_workspace_and_full_flag_confirms() {
    let fx = Fixture::start("w9-home");
    fx.state().access = Some(access_view(None, false));
    let mut c = tui(&fx.rh, &["--plain", "--full", "--wait-idle", "--timeout", "20"]);
    c.current_dir(&fx.dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut i = child.stdin.take().unwrap();
    std::thread::spawn(move || {
        let _ = i.write_all(b"confirm\nhi\n");
    });
    let o = wait_output(child, 30);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let out = text(&o);
    assert!(out.contains("[tui] no workspace: cd into a project and run `init-agent` to let me edit it (the home itself)"), "{out}");
    let fulls: Vec<_> = fx.received().iter().filter(|r| r.frame["cmd"] == "full.set").map(|r| r.frame["args"].clone()).collect();
    assert_eq!(fulls, [json!({"on": true}), json!({"confirm": "confirm", "on": true})]);
    assert!(out.contains("[root] full mode on"), "{out}");
    assert_eq!(fx.state().access.as_ref().unwrap()["full"], true);
    let lines: Vec<_> = fx.attach_frames("line").into_iter().map(|f| f["text"].as_str().unwrap_or("").to_string()).collect();
    assert_eq!(lines, ["hi"]);
    cleanup(&fx.dir);
}

/// WIRE-009: the launcher `scripts/init-agent` passes every flag (`--full` included) through to the TUI
/// and keeps the cwd, which the TUI sends as the workspace. (v0.4.1's macOS `init` shim, `scripts/init-shim`, is gone
/// since v0.5.0; nagisanzenin/INIT-Agent#1.)
#[test]
fn wire9_launcher_passes_flags_and_cwd_through() {
    let dir = home("w9-launcher");
    let stub = dir.join("bin/init-tui");
    fs::create_dir_all(stub.parent().unwrap()).unwrap();
    let out = dir.join("args.txt");
    fs::write(&stub, format!("#!/bin/sh\nprintf '%s|%s\\n' \"$(pwd -P)\" \"$*\" > '{}'\n", out.display())).unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    let proj = dir.join("proj");
    fs::create_dir_all(&proj).unwrap();
    let launcher = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/init-agent");
    let o = std::process::Command::new("/bin/bash")
        .arg(&launcher)
        .args(["--full", "--plain"])
        .current_dir(&proj)
        .env_clear()
        .env("HOME", &dir)
        .env("PATH", "/usr/bin:/bin")
        .env("INIT_ROOT_HOME", dir.join("rh"))
        .env("INIT_HOME", dir.join("ih"))
        .env("INIT_TUI", &stub)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let got = fs::read_to_string(&out).unwrap();
    assert_eq!(got.trim(), format!("{}|--full --plain", fs::canonicalize(&proj).unwrap().display()));
    let o = std::process::Command::new("/bin/bash")
        .arg(&launcher)
        .args(["--workspace"])
        .current_dir(&proj)
        .env_clear()
        .env("HOME", &dir)
        .env("PATH", "/usr/bin:/bin")
        .env("INIT_ROOT_HOME", dir.join("rh"))
        .env("INIT_HOME", dir.join("ih"))
        .env("INIT_TUI", &stub)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let got = fs::read_to_string(&out).unwrap();
    assert_eq!(got.trim(), format!("{}|--workspace", fs::canonicalize(&proj).unwrap().display()), "ADR-012's --workspace too");
    cleanup(&dir);
}

/// /usage is local display only: it never becomes a paid model input or a Root command.
#[test]
fn usage_command_is_local_and_keeps_stop_guidance() {
    let fx = Fixture::start("usage");
    let o = run(&fx.rh, &["--plain", "--wait-idle", "--timeout", "10"], "/usage\n", 15);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let out = text(&o);
    assert!(out.contains("[usage] (USD estimated") && out.contains("  Root ledger  "), "{out}");
    assert!(out.contains("/stop stops the current run"), "{out}");
    assert!(!fx.received().iter().any(|r| r.frame["t"] == "line" && r.frame["text"] == "/usage"));
    cleanup(&fx.dir);
}

/// TUI-UX line mode (§3.12): no animation; the tool calls of a turn end in one summary line, the
/// run's end prints the turn's tokens, cache rate and cost, and per-request token lines stay out
/// unless --verbose.
#[test]
fn tuiux_line_mode_activity_and_turn_summaries() {
    let fx = Fixture::start("tux");
    let d = |kind: &str, text: &str| json!({"boot_seq": 1, "kind": kind, "t": "display", "text": text});
    let call = |kind: &str, id: &str, name: &str, text: &str, ok: Option<bool>| {
        let mut v = json!({"boot_seq": 1, "call_id": id, "kind": kind, "name": name, "t": "display", "text": text});
        if let Some(ok) = ok {
            v["ok"] = json!(ok);
        }
        v
    };
    fx.emit(json!({"busy": true, "t": "state"}));
    fx.emit(d("turn_end", "tokens: in 1000 (cached 900), out 10"));
    fx.emit(call("tool_start", "c1", "bash", "cargo build", None));
    fx.emit(call("tool_progress", "c1", "bash", "Compiling x", None));
    fx.emit(call("tool_end", "c1", "bash", "exit 0", Some(true)));
    fx.emit(call("tool_start", "c2", "read", "read x", None));
    fx.emit(call("tool_end", "c2", "read", "error: not_found", Some(false)));
    fx.emit(call("tool_start", "c3", "bash", "ls", None));
    fx.emit(call("tool_end", "c3", "bash", "exit 0", Some(true)));
    fx.emit(d("turn_end", "tokens: in 3000 (cached 2900), out 20"));
    fx.emit(d("text", "Done."));
    fx.emit(d("turn_end", "tokens: in 4000, out 5"));
    fx.emit(json!({"busy": false, "t": "state"}));
    let o = run(&fx.rh, &["--plain", "--wait-idle", "--timeout", "20"], "", 30);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let out = text(&o);
    assert!(out.contains("  « error error: not_found\n» bash: ls\n  « ok exit 0\n[tools] ran 3 calls (2 bash, 1 read), 1 failed\nDone.\n"), "{out}");
    // 3 requests; the last reported no cache field, so the rate is partial (~), never 0%
    let turn = out.lines().find(|l| l.starts_with("[turn] ")).unwrap_or_else(|| panic!("no turn line: {out}"));
    assert!(turn.starts_with("[turn] 3 requests: in 8.0k out 35 · cache ~95.0% · $") && turn.ends_with(" (session cache ~95.0%)"), "{turn}");
    assert!(!out.contains("tokens: in"), "per-request lines only with --verbose: {out}");
    assert!(!out.contains("Compiling"), "line mode prints no progress: {out}");
    let o = run(&fx.rh, &["--plain", "--verbose", "--wait-idle", "--timeout", "20"], "", 30);
    assert!(text(&o).contains("tokens: in 3000 (cached 2900), out 20"), "{}", text(&o));
    cleanup(&fx.dir);
}

/// TUI-PICKER line mode (docs/tui.md §3.7.2): an agent question prints as a numbered prompt under the
/// TUI's own `[agent asks]` label (the body's fallback text is not printed), a reply of a number goes
/// out as that option's label, and any other line is the owner's own words, sent as typed.
#[test]
fn picker_line_mode_numbered_prompt() {
    let fx = Fixture::start("pick");
    fx.emit(json!({"boot_seq": 1, "kind": "info", "t": "display",
        "text": "The agent asks: Which database?\n  1. Postgres\n  2. SQLite — one file\nReply with a number, the option, or your own words.",
        "ask": {"header": "Database", "multi": false, "question": "Which database?",
            "options": [{"label": "Postgres", "recommended": true}, {"description": "one file", "label": "SQLite"}]}}));
    let o = run(&fx.rh, &["--plain", "--wait-idle", "--timeout", "10"], "2\nand 2 more things\n", 15);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let out = text(&o);
    assert!(out.contains("[agent asks] Database: Which database?\n  1. Postgres (recommended)\n  2. SQLite — one file\n[agent asks] reply with a number from 1 to 2, or type your own answer\n"), "{out}");
    assert!(!out.contains("The agent asks:"), "{out}");
    let lines: Vec<String> = fx.received().iter().filter(|r| r.frame["t"] == "line").map(|r| r.frame["text"].as_str().unwrap_or("").to_string()).collect();
    assert_eq!(lines, ["SQLite", "and 2 more things"]);
    cleanup(&fx.dir);
}
