//! A std-only fixture owner socket (docs/tui.md §8): enough of the Root's owner channel (root/root.md
//! §4.2, link.rs as built) to drive `init-tui` in tests: attach with replay, `replay_end`, the greeting,
//! live frames, the request frames on other connections, and records of everything received.
//! Homes live under `tui/target/test-homes/tui/<name>/`; every process a test starts is its own child.
#![allow(dead_code)]

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const BIN: &str = env!("CARGO_BIN_EXE_init-tui");

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `tui/target/test-homes/tui/<name>`, fresh. Asserted to lie under the scratch tree.
pub fn home(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/tui");
    fs::create_dir_all(&base).unwrap();
    let base = fs::canonicalize(base).unwrap();
    let d = base.join(name);
    if d.exists() {
        fs::remove_dir_all(&d).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(d.join("rh/run")).unwrap();
    assert!(d.starts_with(&base) && d.to_string_lossy().contains("/target/test-homes/tui/"));
    d
}

/// Removes a test home by its exact path.
pub fn cleanup(d: &Path) {
    assert!(d.to_string_lossy().contains("/target/test-homes/tui/"));
    let _ = fs::remove_dir_all(d);
}

pub fn icj(v: &Value) -> String {
    serde_json::to_string(v).unwrap()
}

#[derive(Clone, Debug)]
pub struct Rec {
    /// Connection number (1, 2, …) and whether it attached.
    pub conn: u64,
    pub attach: bool,
    pub frame: Value,
}

pub struct State {
    /// The HOME the TUI runs with (the fixture refuses it as a workspace).
    pub home: String,
    pub ring: Vec<String>,
    pub clients: Vec<(u64, UnixStream)>,
    pub received: Vec<Rec>,
    pub status: Value,
    pub egress: Option<Value>,
    /// Echo every line (kernel echo, `seq`) and answer with a display frame.
    pub auto_echo: bool,
    pub seq: u64,
    /// Stamp `rseq`/`src` and honour `attach{since, epoch}` (root.md §9 Q1 as WP26 built it).
    pub numbered: bool,
    pub rseq: u64,
    pub ring_max: usize,
    /// Close attach connections at once (the socket is up, attach is down).
    pub refuse_attach: bool,
    /// Answer everything with garbage.
    pub garbage: bool,
    pub conns: u64,
    /// ADR-009 (root SBX-009 §A): answer `workspace.set`, `full.set`, `undo` and `access.get`
    /// with this `AccessView`, asking for the phrase `confirm`; None: unknown commands, as v0.1.
    pub access: Option<Value>,
}

/// The fixture's `AccessView` (SBX-009 §A).
pub fn access_view(ws: Option<&str>, full: bool) -> Value {
    json!({"access": if full { "full" } else if ws.is_some() { "workspace" } else { "read_home" }, "allow_full": true, "full": full,
           "read_home": true, "restart_pending": false, "snapshot": null, "workspace": ws})
}

fn confirm_err(v: &Value, message: &str, extra: Value) -> Value {
    let mut data = json!({"phrase": "confirm"});
    if let Some(o) = extra.as_object() {
        for (k, x) in o {
            data[k] = x.clone();
        }
    }
    json!({"error": {"code": "root.confirm", "data": data, "details": [], "message": message}, "id": v["id"], "ok": false, "v": 1})
}

/// The ADR-009 requests, as the Root answers them (SBX-009 §A); None for other commands.
fn access_answer(g: &mut State, v: &Value) -> Option<Value> {
    let cmd = v["cmd"].as_str()?;
    let cur = g.access.clone()?;
    let ok = |r: Value| json!({"id": v["id"], "ok": true, "result": r, "v": 1});
    let a = &v["args"];
    Some(match cmd {
        "access.get" => ok(cur),
        "workspace.set" => {
            let cwd = a["cwd"].as_str().unwrap_or("").to_string();
            let home = g.home.clone();
            let (ws, refused) = if cwd == home { (None, json!({"code": "home", "message": "the home itself"})) } else { (Some(cwd.clone()), Value::Null) };
            let new = access_view(ws.as_deref(), false);
            let changed = new["workspace"] != cur["workspace"] || cur["full"] == true;
            g.access = Some(new.clone());
            g.status["access"] = new.clone();
            if changed {
                let text = match &ws {
                    Some(w) => format!("[root] workspace: {w}"),
                    None => "[root] no workspace: cd into a project and run init to let me edit it".into(),
                };
                emit(g, json!({"code": "access", "level": "info", "src": "root", "t": "notice", "text": text, "ts": "x"}));
            }
            ok(json!({"access": new, "changed": changed, "refused": refused}))
        }
        "full.set" => {
            let on = a["on"] == true;
            if on && a["confirm"] != "confirm" {
                return Some(confirm_err(v, "full mode lets the agent write your whole home except secrets and the Root; type confirm", Value::Null));
            }
            let new = access_view(cur["workspace"].as_str(), on);
            g.access = Some(new.clone());
            g.status["access"] = new.clone();
            let text = if on { "[root] full mode on: the agent may write your home except secrets and the Root" } else { "[root] full mode off" };
            emit(g, json!({"code": "access", "level": "warn", "src": "root", "t": "notice", "text": text, "ts": "x"}));
            ok(json!({"access": new, "changed": true, "snapshot": null}))
        }
        "undo" => {
            let ws = cur["workspace"].as_str().unwrap_or("").to_string();
            if ws.is_empty() {
                return Some(json!({"error": {"code": "root.mode", "data": {"reason": "no_workspace"}, "details": [], "message": "no workspace"}, "id": v["id"], "ok": false, "v": 1}));
            }
            let snap = json!({"at": "2026-10-20T21:00:00Z", "id": "s1", "reason": "session", "workspace": ws});
            if a["confirm"] != "confirm" {
                return Some(confirm_err(v, "undo restores the snapshot s1 of the workspace; type confirm", json!({"snapshot": snap})));
            }
            ok(json!({"restored": snap, "saved": {"at": "2026-10-20T21:05:00Z", "id": "s2", "reason": "pre_undo", "workspace": ws}}))
        }
        _ => return None,
    })
}

pub struct Fixture {
    pub dir: PathBuf,
    pub rh: PathBuf,
    pub st: Arc<Mutex<State>>,
}

pub fn status_for(started: &str, pid: u64) -> Value {
    json!({"v": 1, "slot": "running", "mode": "normal", "halted": null, "panic": null, "level": 0, "cap": 1, "max_level": 4,
        "tuples": {"current": "t1", "lkg": "t1", "audited": "t1"}, "kernel": {"ready": true, "boot_seq": 1, "pid": 7},
        "queued": 0, "root": {"started_at": started, "pid": pid, "version": "0.1.0", "test_mode": true}})
}

impl Fixture {
    pub fn start(name: &str) -> Fixture {
        let dir = home(name);
        let rh = dir.join("rh");
        let l = UnixListener::bind(rh.join("run/owner.sock")).unwrap();
        let st = Arc::new(Mutex::new(State {
            home: dir.to_string_lossy().into_owned(),
            ring: Vec::new(),
            clients: Vec::new(),
            received: Vec::new(),
            status: status_for("2026-10-20T21:00:00Z", 1001),
            egress: Some(json!({"money": {"day_spent_musd": 40_000, "day_reserved_musd": 0, "day_limit_musd": 5_000_000,
                "day_resets_at": "2026-10-21T00:00:00Z", "total_spent_musd": 40_000, "total_limit_musd": 100_000_000},
                "models": [{"model": "deepseek-flash", "routable": true}]})),
            auto_echo: true,
            seq: 0,
            numbered: false,
            rseq: 0,
            ring_max: 2000,
            refuse_attach: false,
            garbage: false,
            conns: 0,
            access: None,
        }));
        let s2 = st.clone();
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                let st = s2.clone();
                std::thread::spawn(move || serve(st, c));
            }
        });
        Fixture { dir, rh, st }
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.st)
    }

    /// A frame to every attached client and into the ring (the Root's `emit`).
    pub fn emit(&self, frame: Value) {
        emit(&mut lock(&self.st), frame);
    }

    /// Drops every attach connection (a dropped client, same epoch).
    pub fn kick(&self) {
        let mut s = lock(&self.st);
        for (_, c) in s.clients.drain(..) {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
    }

    /// A Root restart: a new epoch, an empty ring, every client dropped.
    pub fn restart(&self, started: &str, pid: u64) {
        let mut s = lock(&self.st);
        s.status = status_for(started, pid);
        s.ring.clear();
        s.rseq = 0;
        for (_, c) in s.clients.drain(..) {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
    }

    pub fn received(&self) -> Vec<Rec> {
        lock(&self.st).received.clone()
    }

    pub fn attach_frames(&self, t: &str) -> Vec<Value> {
        self.received().into_iter().filter(|r| r.attach && r.frame["t"] == t).map(|r| r.frame).collect()
    }

    pub fn wait_for(&self, secs: u64, what: &str, mut f: impl FnMut(&State) -> bool) {
        let t = Instant::now();
        loop {
            if f(&lock(&self.st)) {
                return;
            }
            assert!(t.elapsed() < Duration::from_secs(secs), "not within {secs} s: {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn attached(&self) -> usize {
        lock(&self.st).clients.len()
    }
}

pub fn emit(s: &mut State, mut frame: Value) {
    if s.numbered {
        s.rseq += 1;
        frame["rseq"] = json!(s.rseq);
        if frame.get("src").is_none() {
            frame["src"] = json!("kernel");
        }
    }
    let line = icj(&frame) + "\n";
    s.ring.push(line.clone());
    let max = s.ring_max;
    if s.ring.len() > max {
        let n = s.ring.len() - max;
        s.ring.drain(..n);
    }
    s.clients.retain_mut(|(_, c)| {
        let _ = c.set_write_timeout(Some(Duration::from_secs(1)));
        c.write_all(line.as_bytes()).is_ok()
    });
}

fn epoch(s: &State) -> String {
    format!("{}/{}", s.status["root"]["started_at"].as_str().unwrap_or(""), s.status["root"]["pid"])
}

fn serve(st: Arc<Mutex<State>>, s: UnixStream) {
    let n = {
        let mut g = lock(&st);
        g.conns += 1;
        g.conns
    };
    let Ok(mut w) = s.try_clone() else { return };
    let mut rd = BufReader::new(s);
    let mut attached = false;
    loop {
        let mut buf = String::new();
        if !matches!((&mut rd).take((1 << 20) + 1).read_line(&mut buf), Ok(k) if k > 0) || !buf.ends_with('\n') {
            break;
        }
        if lock(&st).garbage {
            let _ = w.write_all(b"this is not json\n");
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&buf) else {
            let _ = w.write_all(b"{\"error\":{\"code\":\"root.proto\",\"details\":[],\"message\":\"malformed frame\"},\"id\":null,\"ok\":false,\"v\":1}\n");
            continue;
        };
        lock(&st).received.push(Rec { conn: n, attach: attached || v["t"] == "attach", frame: v.clone() });
        if let Some(cmd) = v.get("cmd").and_then(Value::as_str) {
            let mut g = lock(&st);
            if let Some(r) = access_answer(&mut g, &v) {
                drop(g);
                if w.write_all((icj(&r) + "\n").as_bytes()).is_err() {
                    break;
                }
                continue;
            }
            let r = match cmd {
                "status" => json!({"id": v["id"], "ok": true, "result": g.status, "v": 1}),
                "egress.stats" => match &g.egress {
                    Some(e) => json!({"id": v["id"], "ok": true, "result": e, "v": 1}),
                    None => json!({"error": {"code": "root.unknown_cmd", "details": [], "message": "x"}, "id": v["id"], "ok": false, "v": 1}),
                },
                "panic" => json!({"id": v["id"], "ok": true, "result": {"message": "panicked (fixture)"}, "v": 1}),
                _ => json!({"error": {"code": "root.unknown_cmd", "details": [], "message": "x"}, "id": v["id"], "ok": false, "v": 1}),
            };
            drop(g);
            if w.write_all((icj(&r) + "\n").as_bytes()).is_err() {
                break;
            }
            continue;
        }
        match v["t"].as_str() {
            Some("attach") => {
                if lock(&st).refuse_attach {
                    break;
                }
                attached = true;
                let mut g = lock(&st);
                let ep = epoch(&g);
                let since = v["since"].as_u64().filter(|_| g.numbered && v["epoch"] == ep.as_str());
                let mut c = w.try_clone().unwrap();
                let _ = c.set_write_timeout(Some(Duration::from_secs(1)));
                let mut out = String::new();
                if v["replay"] != false {
                    for l in &g.ring {
                        let keep = since.is_none_or(|s| serde_json::from_str::<Value>(l).ok().and_then(|f| f["rseq"].as_u64()).is_some_and(|r| r > s));
                        if keep {
                            out.push_str(l);
                        }
                    }
                }
                let mut end = json!({"t": "replay_end"});
                if g.numbered {
                    end = json!({"epoch": ep, "gap": false, "src": "root", "t": "replay_end"});
                }
                out.push_str(&(icj(&end) + "\n"));
                let mut hello = json!({"level": "info", "t": "notice", "text": "[root] attached: slot running, level L0, 0 queued. Type /panic to stop everything, /root help for more.", "ts": "2026-10-20T21:00:00Z"});
                if g.numbered {
                    hello["src"] = json!("root");
                }
                out.push_str(&(icj(&hello) + "\n"));
                if c.write_all(out.as_bytes()).is_err() {
                    break;
                }
                g.clients.push((n, c));
            }
            Some("line") => {
                let mut g = lock(&st);
                if g.auto_echo {
                    let text = v["text"].as_str().unwrap_or("").to_string();
                    if text.trim() == "/status" {
                        emit(&mut g, json!({"level": "info", "t": "notice", "text": "slot running · mode normal", "ts": "x"}));
                        continue;
                    }
                    g.seq += 1;
                    let seq = g.seq;
                    let echo = text.trim().strip_prefix("/later ").map(str::to_string).unwrap_or(text.clone());
                    emit(&mut g, json!({"seq": seq, "t": "echo", "text": echo}));
                    emit(&mut g, json!({"boot_seq": 1, "kind": "text", "t": "display", "text": format!("fx: {echo}\n")}));
                    emit(&mut g, json!({"boot_seq": 1, "kind": "turn_end", "t": "display", "text": "tokens: in 1, out 1"}));
                    emit(&mut g, json!({"busy": false, "level": 0, "members": 0, "mode": "normal", "queued": 0, "sha12": "3f9c0a1b2c4d", "slot": "running", "t": "state"}));
                }
            }
            Some("interrupt") | Some("panic") => {}
            _ => break, // detach, or an unknown frame
        }
    }
    let _ = w.shutdown(std::net::Shutdown::Both);
    lock(&st).clients.retain(|(c, _)| *c != n);
}

/// `init-tui` with a clean environment, pointed at `rh`.
pub fn tui(rh: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(BIN);
    c.env_clear().env("HOME", rh.parent().unwrap()).env("PATH", "/usr/bin:/bin").env("TERM", "dumb").arg("--root-home").arg(rh).args(args);
    c
}

/// Runs the TUI in line mode with `input` on stdin; returns its output. Bounded by `secs`.
pub fn run(rh: &Path, args: &[&str], input: &str, secs: u64) -> Output {
    let mut c = tui(rh, args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut i = c.stdin.take().unwrap();
    let input = input.to_string();
    std::thread::spawn(move || {
        let _ = i.write_all(input.as_bytes());
    });
    wait_output(c, secs)
}

/// Waits for our own child, killing it by its handle after `secs`.
pub fn wait_output(mut c: Child, secs: u64) -> Output {
    let t = Instant::now();
    loop {
        if c.try_wait().unwrap().is_some() {
            return c.wait_with_output().unwrap();
        }
        if t.elapsed() > Duration::from_secs(secs) {
            let _ = c.kill(); // our own child
            let o = c.wait_with_output().unwrap();
            panic!("init-tui did not exit within {secs} s; stdout: {}\nstderr: {}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn text(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Compares with a golden file under tests/golden (UPDATE_GOLDEN=1 rewrites it).
pub fn golden(name: &str, got: &str) {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::write(&p, got).unwrap();
        return;
    }
    let want = fs::read_to_string(&p).unwrap_or_else(|_| panic!("missing golden {} (UPDATE_GOLDEN=1)", p.display()));
    assert_eq!(want, got, "golden {name}");
}
