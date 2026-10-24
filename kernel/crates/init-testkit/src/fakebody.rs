//! `init-fakebody` (testing.md §3.5): a std-only body that implements just the kernel-facing
//! contract of kernel/supervise.md: the env vars of §4.1, `state/run/ready` and
//! `state/run/heartbeat` with `boot_id` (§4.4), a `boot` event in the session log, the stdio-jsonl/1
//! frames of §4.5 (typed by `init_proto::stdio`), and exit codes 0, 75 and others.
//!
//! **Modes** (comma-separated) come from, in order of precedence: a sidecar file `<exe>.mode` next
//! to the executable (so a gate candidate and the live body can differ), `FAKEBODY_MODE`, then
//! `INIT_TEST_FAKEBODY_MODE` (the name the kernel passes through in test mode, supervise.md §4.1).
//!
//! | Mode | Behaviour |
//! |---|---|
//! | `ok` | boot, write `ready`, heartbeat, obey pause, exit 75 on `exit{75}`, 0 on `stop` |
//! | `crash_before_ready` | exit 101 before `ready` |
//! | `hang_before_ready` | hang before `ready`, holding a child `sleep 3600` in its own process group |
//! | `crash_after_ready:<ms>` | exit 101 `<ms>` after `ready` |
//! | `hang_after_ready` | stop heartbeats after `ready` |
//! | `ignore_pause` | never answer `pause` |
//! | `slow_ready:<ms>` | write `ready` after `<ms>` |
//! | `exit_code:<n>` | exit `<n>` after `ready` |
//! | `exit_before_ready:<n>` | exit `<n>` before `ready` (78 = configuration) |
//! | `escape_setsid` | spawn a `setsid` child that outlives the body; **disposable environment only** |
//! | `crash_first:<n>` | crash after `ready` on the first `<n>` boots (count in `state/fakebody.count`), then `ok` |
//!
//! Owner inputs are answered `fake reply: <text>`. Test hooks in the text: `/fake crash`,
//! `/fake exit <n>`, `/fake busy <ms>` (a long turn: a pause waits for its end, the step
//! boundary), `/fake hang` (stop reading), `/fake display <text>`.
//!
//! Safety valve: the hang modes still exit on stdin EOF (the kernel is gone), and kill the
//! `sleep` child they hold by pid, so a fake body never outlives its kernel on the live machine.
//! Heartbeat interval: 5 s, or `INIT_TEST_FAKEBODY_HB_MS`.

use crate::util::{self, ts};
use crate::{fault, guard};
use init_proto::files::{HeartbeatFile, ReadyFile};
use init_proto::icj::{self, V1};
use init_proto::stdio::{self, Display, DisplayKind, FromBody, ToBody};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Exit code of a deliberate crash.
pub const CRASH: i32 = 101;

/// One fake-body mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Ok,
    CrashBeforeReady,
    HangBeforeReady,
    CrashAfterReady(u64),
    HangAfterReady,
    IgnorePause,
    SlowReady(u64),
    ExitCode(i32),
    ExitBeforeReady(i32),
    EscapeSetsid,
    CrashFirst(u64),
}

/// Parse a comma-separated mode list (empty = `ok`).
pub fn parse_modes(s: &str) -> Result<Vec<Mode>, String> {
    let mut out = Vec::new();
    for m in s.split(',').map(str::trim).filter(|m| !m.is_empty()) {
        let (k, v) = m.split_once(':').map_or((m, None), |(k, v)| (k, Some(v)));
        let num = |v: Option<&str>| -> Result<u64, String> {
            v.and_then(|x| x.parse().ok())
                .ok_or_else(|| format!("`{m}` needs a number"))
        };
        let code = |v: Option<&str>| -> Result<i32, String> {
            v.and_then(|x| x.parse().ok())
                .filter(|c| (0..=255).contains(c))
                .ok_or_else(|| format!("`{m}` needs an exit code"))
        };
        out.push(match k {
            "ok" => Mode::Ok,
            "crash_before_ready" => Mode::CrashBeforeReady,
            "hang_before_ready" => Mode::HangBeforeReady,
            "crash_after_ready" => Mode::CrashAfterReady(num(v)?),
            "hang_after_ready" => Mode::HangAfterReady,
            "ignore_pause" => Mode::IgnorePause,
            "slow_ready" => Mode::SlowReady(num(v)?),
            "exit_code" => Mode::ExitCode(code(v)?),
            "exit_before_ready" => Mode::ExitBeforeReady(code(v)?),
            "escape_setsid" => Mode::EscapeSetsid,
            "crash_first" => Mode::CrashFirst(num(v)?),
            _ => return Err(format!("unknown FAKEBODY_MODE `{m}`")),
        });
    }
    if out.is_empty() {
        out.push(Mode::Ok);
    }
    Ok(out)
}

/// The modes for this process (see the module docs for the precedence).
pub fn modes_from_env() -> Result<Vec<Mode>, String> {
    if let Ok(exe) = std::env::current_exe() {
        let mut side = exe.into_os_string();
        side.push(".mode");
        if let Ok(s) = fs::read_to_string(PathBuf::from(side)) {
            return parse_modes(s.trim());
        }
    }
    let v = std::env::var("FAKEBODY_MODE")
        .or_else(|_| std::env::var("INIT_TEST_FAKEBODY_MODE"))
        .unwrap_or_default();
    parse_modes(&v)
}

/// The supervise.md §4.1 environment the fake body reads.
#[derive(Clone, Debug)]
pub struct Env {
    pub home: PathBuf,
    pub session: String,
    pub sha: String,
    pub reason: String,
    pub boot_id: String,
    pub mode: String,
    pub body_id: String,
}

impl Env {
    /// Read the environment; `Err` is a configuration error (exit 78).
    pub fn from_env() -> Result<Env, String> {
        let home = std::env::var("INIT_HOME").map_err(|_| "INIT_HOME is not set")?;
        let ui = std::env::var("INIT_UI").unwrap_or_else(|_| init_proto::UI_PROTO.to_string());
        if ui != init_proto::UI_PROTO {
            return Err(format!(
                "INIT_UI={ui}: this body speaks {}",
                init_proto::UI_PROTO
            ));
        }
        let get = |k: &str, d: &str| {
            std::env::var(k)
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| d.to_string())
        };
        let session = std::env::var("INIT_SESSION")
            .ok()
            .filter(|s| util::is_session_id(s))
            .unwrap_or_else(|| util::new_session_id(util::now_ms()));
        let boot_id = std::env::var("INIT_BOOT_ID")
            .ok()
            .filter(|b| b.len() == 16 && b.bytes().all(|c| c.is_ascii_hexdigit()))
            .unwrap_or_else(|| util::random_hex(8));
        Ok(Env {
            home: PathBuf::from(home),
            session,
            sha: get("INIT_SHA", &"0".repeat(40)),
            reason: get("INIT_BOOT_REASON", "cold"),
            boot_id,
            mode: get("INIT_MODE", "normal"),
            body_id: get("INIT_BODY_ID", "b0"),
        })
    }
}

/// The session log writer: ICJ lines with `id`, `parent_id`, `ts`, `type`.
pub struct Log {
    path: PathBuf,
    file: File,
    pub leaf: Option<String>,
    input_ids: HashSet<String>,
    notice_ids: HashSet<String>,
}

impl Log {
    /// Open (or create) `state/sessions/<sid>.jsonl`, scanning it for ids and the leaf. A torn
    /// last line is left in place and the next append starts on a new line.
    pub fn open(home: &Path, session: &str) -> io::Result<Log> {
        let dir = home.join("state/sessions");
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{session}.jsonl"));
        let mut existing = Vec::new();
        if let Ok(mut f) = File::open(&path) {
            f.read_to_end(&mut existing)?;
        }
        let mut log = Log {
            path: path.clone(),
            file: OpenOptions::new().create(true).append(true).open(&path)?,
            leaf: None,
            input_ids: HashSet::new(),
            notice_ids: HashSet::new(),
        };
        for l in existing.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
            if let Ok(v) = serde_json::from_slice::<Value>(l) {
                if let Some(id) = v["id"].as_str() {
                    log.leaf = Some(id.to_string());
                }
                if let Some(i) = v["input_id"].as_str() {
                    log.input_ids.insert(i.to_string());
                }
                if let Some(n) = v["notice_id"].as_str() {
                    log.notice_ids.insert(n.to_string());
                }
            }
        }
        if !existing.is_empty() && existing.last() != Some(&b'\n') {
            log.file.write_all(b"\n")?;
        }
        if existing.is_empty() {
            log.append(json!({"type": "session", "format": 1, "origin": "init-fakebody", "session": session}))?;
        }
        Ok(log)
    }

    /// Append an event (adds `id`, `parent_id`, `ts`), fsync; returns its id.
    pub fn append(&mut self, mut ev: Value) -> io::Result<String> {
        let id = util::random_hex(8);
        ev["id"] = Value::String(id.clone());
        ev["parent_id"] = self.leaf.clone().map_or(Value::Null, Value::String);
        ev["ts"] = Value::String(ts());
        let line = icj::to_line(&ev)
            .or_else(|_| {
                serde_json::to_vec(&ev).map(|mut b| {
                    b.push(b'\n');
                    b
                })
            })
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.file.write_all(&line)?;
        self.file.sync_data()?;
        self.leaf = Some(id.clone());
        Ok(id)
    }

    pub fn len(&self) -> u64 {
        fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

struct Out(Mutex<io::Stdout>);

impl Out {
    fn send(&self, f: &FromBody) {
        if let Ok(line) = stdio::encode_from_body(f) {
            let mut o = self.0.lock().unwrap_or_else(|e| e.into_inner());
            let _ = o.write_all(&line);
            let _ = o.flush();
        }
    }
    fn display(&self, kind: DisplayKind, text: &str) {
        self.send(&FromBody::Display(Display {
            kind,
            text: text.to_string(),
            call_id: None,
            name: None,
            ok: None,
        }));
    }
}

enum In {
    Frame(Box<ToBody>),
    Eof,
}

fn stdin_reader() -> Receiver<In> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut lock = stdin.lock();
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match (&mut lock)
                .take(stdio::MAX_LINE as u64 + 2)
                .read_until(b'\n', &mut buf)
            {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            match stdio::decode_to_body(&buf) {
                Ok(f) => {
                    if tx.send(In::Frame(Box::new(f))).is_err() {
                        return;
                    }
                }
                Err(e) => eprintln!("init-fakebody: ignoring a bad stdin line: {e}"),
            }
        }
        let _ = tx.send(In::Eof);
    });
    rx
}

fn bump_count(home: &Path) -> u64 {
    let p = home.join("state/fakebody.count");
    let n = fs::read_to_string(&p)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
        + 1;
    let _ = util::write_atomic(&p, format!("{n}\n").as_bytes(), true);
    n
}

fn write_heartbeat(home: &Path, boot_id: &str, busy: bool) {
    let hb = HeartbeatFile {
        v: V1,
        boot_id: boot_id.to_string(),
        busy,
        ts: ts(),
    };
    if let Ok(b) = init_proto::files::encode(&hb) {
        let _ = util::write_atomic(&home.join(init_proto::files::paths::HEARTBEAT), &b, false);
    }
}

/// The prefix hash the fake reports: constant for a given `sha` (so G2-style comparisons are deterministic).
pub fn fake_prefix_hash(sha: &str) -> String {
    format!(
        "sha256:{}",
        util::sha256_hex(format!("init-fakebody:{sha}").as_bytes())
    )
}

fn exit_now(code: i32, held: &mut Option<Child>) -> ! {
    if let Some(c) = held.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
    let _ = io::stdout().flush();
    std::process::exit(code)
}

fn hang_until_eof(rx: &Receiver<In>, held: &mut Option<Child>) -> ! {
    loop {
        match rx.recv() {
            Ok(In::Eof) | Err(_) => exit_now(0, held),
            Ok(In::Frame(_)) => {}
        }
    }
}

/// The `init-fakebody` entry point; returns the exit code.
pub fn main() -> i32 {
    let env = match Env::from_env() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("init-fakebody: {e}");
            return 78;
        }
    };
    let modes = match modes_from_env() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("init-fakebody: {e}");
            return 64;
        }
    };
    let has = |m: &Mode| modes.contains(m);
    let num = |f: fn(&Mode) -> Option<u64>| modes.iter().find_map(f);
    let home = env.home.clone();
    let _ = fs::create_dir_all(home.join("state/run"));
    let boots = bump_count(&home);
    let rx = stdin_reader();
    let mut held: Option<Child> = None;
    let _ = fault::point("fakebody.start");

    if has(&Mode::CrashBeforeReady) {
        eprintln!("init-fakebody: crash_before_ready");
        return CRASH;
    }
    if let Some(c) = modes.iter().find_map(|m| {
        if let Mode::ExitBeforeReady(c) = m {
            Some(*c)
        } else {
            None
        }
    }) {
        eprintln!("init-fakebody: exit_before_ready:{c}");
        return c;
    }
    if has(&Mode::EscapeSetsid) {
        guard::require_disposable_or_exit("init-fakebody escape_setsid");
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("3600")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::sys::new_session(&mut cmd);
        if let Ok(c) = cmd.spawn() {
            eprintln!("init-fakebody: escape_setsid child pid {}", c.id());
        }
    }
    if has(&Mode::HangBeforeReady) {
        // The child stays in this body's process group, so a group kill by the kernel or Root
        // (held pgid) also removes it; the sweep test is disposable-only (testing.md §3.1a).
        held = crate::watchdog::spawn(
            Command::new("/bin/sleep")
                .arg("3600")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .ok();
        eprintln!(
            "init-fakebody: hang_before_ready (child {:?})",
            held.as_ref().map(Child::id)
        );
        hang_until_eof(&rx, &mut held);
    }
    if let Some(ms) = num(|m| {
        if let Mode::SlowReady(ms) = m {
            Some(*ms)
        } else {
            None
        }
    }) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    let mut log = match Log::open(&home, &env.session) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("init-fakebody: log: {e}");
            return 1;
        }
    };
    let boot = json!({"type": "boot", "boot_id": env.boot_id, "reason": env.reason, "sha": env.sha, "mode": env.mode,
        "body_id": env.body_id, "pid": std::process::id(), "boot_count": boots, "text": format!("[boot] fake body {} ({}, {})", &env.sha[..env.sha.len().min(12)], env.reason, env.mode)});
    if let Err(e) = log.append(boot) {
        eprintln!("init-fakebody: boot event: {e}");
        return 1;
    }
    let _ = util::write_atomic(
        &home.join("state/session"),
        format!("{}\n", env.session).as_bytes(),
        true,
    );
    write_heartbeat(&home, &env.boot_id, false);
    let ready = ReadyFile {
        v: V1,
        boot_id: env.boot_id.clone(),
        pid: std::process::id() as i32,
        prefix_hash: fake_prefix_hash(&env.sha),
        sha: env.sha.clone(),
        ts: ts(),
    };
    if let Err(e) = fault::point("fakebody.before_ready") {
        eprintln!("init-fakebody: {e}");
        return 1;
    }
    match init_proto::files::encode(&ready) {
        Ok(b) => {
            if let Err(e) =
                util::write_atomic(&home.join(init_proto::files::paths::READY), &b, true)
            {
                eprintln!("init-fakebody: ready: {e}");
                return 1;
            }
        }
        Err(e) => {
            eprintln!("init-fakebody: ready: {e}");
            return 1;
        }
    }
    let _ = fault::point("fakebody.after_ready");

    let busy = Arc::new(AtomicBool::new(false));
    if !has(&Mode::HangAfterReady) {
        let (h, b, id) = (home.clone(), busy.clone(), env.boot_id.clone());
        let every = std::env::var("INIT_TEST_FAKEBODY_HB_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5000u64)
            .max(10);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(every));
                write_heartbeat(&h, &id, b.load(Ordering::SeqCst));
            }
        });
    }
    if let Some(ms) = num(|m| {
        if let Mode::CrashAfterReady(ms) = m {
            Some(*ms)
        } else {
            None
        }
    }) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            eprintln!("init-fakebody: crash_after_ready");
            std::process::exit(CRASH);
        });
    }
    if let Some(c) = modes.iter().find_map(|m| {
        if let Mode::ExitCode(c) = m {
            Some(*c)
        } else {
            None
        }
    }) {
        eprintln!("init-fakebody: exit_code:{c}");
        return c;
    }
    if let Some(n) = num(|m| {
        if let Mode::CrashFirst(n) = m {
            Some(*n)
        } else {
            None
        }
    }) && boots <= n
    {
        eprintln!("init-fakebody: crash_first:{n} (boot {boots})");
        return CRASH;
    }

    let out = Out(Mutex::new(io::stdout()));
    let ignore_pause = has(&Mode::IgnorePause);
    let mut paused: Option<String> = None;
    let mut deferred: Vec<ToBody> = Vec::new();
    loop {
        let frame = if paused.is_none() && !deferred.is_empty() {
            deferred.remove(0)
        } else {
            match rx.recv() {
                Ok(In::Frame(f)) => *f,
                Ok(In::Eof) | Err(_) => exit_now(0, &mut held),
            }
        };
        if let Some(tok) = paused.clone() {
            match frame {
                ToBody::Resume {
                    token,
                    update_failed,
                } if token == tok => {
                    paused = None;
                    if let Some(u) = update_failed {
                        let mut ev = icj::to_value(&u).unwrap_or(json!({}));
                        ev["type"] = json!("update_failed");
                        let _ = log.append(ev);
                    }
                }
                ToBody::Exit { token, code } if token == tok => exit_now(code, &mut held),
                ToBody::Stop { .. } => exit_now(0, &mut held), // D11: stop while paused exits at once
                ToBody::Resume { .. } | ToBody::Exit { .. } => {}
                other => deferred.push(other),
            }
            continue;
        }
        match frame {
            ToBody::Input { id, seq, text, .. } => {
                if log.input_ids.contains(&id) {
                    out.send(&FromBody::Ack { seq });
                    continue;
                }
                busy.store(true, Ordering::SeqCst);
                out.send(&FromBody::State { busy: true });
                if log.append(json!({"type": "message", "role": "user", "source": "owner", "input_id": id, "content": text})).is_err() {
                    exit_now(1, &mut held);
                }
                log.input_ids.insert(id);
                let _ = fault::point("fakebody.input.before_ack");
                out.send(&FromBody::Ack { seq });
                let t = text.trim();
                let reply = if t == "/fake crash" {
                    exit_now(CRASH, &mut held)
                } else if let Some(c) = t.strip_prefix("/fake exit ") {
                    exit_now(c.trim().parse().unwrap_or(1), &mut held)
                } else if t == "/fake hang" {
                    hang_until_eof(&rx, &mut held)
                } else if let Some(ms) = t.strip_prefix("/fake busy ") {
                    std::thread::sleep(Duration::from_millis(ms.trim().parse().unwrap_or(0)));
                    format!("fake reply: busy {}", ms.trim())
                } else if let Some(d) = t.strip_prefix("/fake display ") {
                    d.to_string()
                } else {
                    format!("fake reply: {text}")
                };
                out.display(DisplayKind::Text, &reply);
                let _ = log.append(json!({"type": "message", "role": "assistant", "content": reply, "final": true, "model": "fake"}));
                out.display(DisplayKind::TurnEnd, "");
                busy.store(false, Ordering::SeqCst);
                out.send(&FromBody::State { busy: false });
            }
            ToBody::Interrupt { .. } => out.display(
                DisplayKind::Info,
                "interrupted (fake body: no turn running)",
            ),
            ToBody::Pause { token, reason } => {
                if ignore_pause {
                    eprintln!("init-fakebody: ignoring pause ({reason:?})");
                    continue;
                }
                let _ = log.file.sync_all();
                out.send(&FromBody::Paused {
                    token: token.clone(),
                    log_len: log.len(),
                    leaf: log.leaf.clone().unwrap_or_default(),
                });
                paused = Some(token);
            }
            ToBody::Resume { .. } => {}
            ToBody::Exit { code, .. } => exit_now(code, &mut held),
            ToBody::Stop { .. } => exit_now(0, &mut held),
            ToBody::Reload {
                job, components, ..
            } => {
                let _ = log.append(json!({"type": "reload", "job": job, "text": "[reload] fake body: nothing to reload"}));
                out.send(&FromBody::ReloadDone {
                    job,
                    ok: true,
                    prefix_changed: false,
                    components: components.unwrap_or_default(),
                    error: None,
                });
            }
            ToBody::Notice { id, kind, data } => {
                if !log.notice_ids.contains(&id) {
                    let ty = match kind.as_str() {
                        "gate" | "update_failed" | "thinning_done" => kind.clone(),
                        _ => "notice".to_string(),
                    };
                    if log
                        .append(json!({"type": ty, "kind": kind, "notice_id": id, "data": data}))
                        .is_err()
                    {
                        exit_now(1, &mut held);
                    }
                    log.notice_ids.insert(id.clone());
                }
                out.send(&FromBody::NoticeAck { id });
            }
            ToBody::Retract { .. } | ToBody::Unknown => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert_eq!(parse_modes("").unwrap(), vec![Mode::Ok]);
        assert_eq!(
            parse_modes("crash_after_ready:250, ignore_pause,exit_before_ready:78,crash_first:2")
                .unwrap(),
            vec![
                Mode::CrashAfterReady(250),
                Mode::IgnorePause,
                Mode::ExitBeforeReady(78),
                Mode::CrashFirst(2)
            ]
        );
        assert!(parse_modes("exit_code:300").is_err());
        assert!(parse_modes("slow_ready").is_err());
        assert!(parse_modes("nope").is_err());
    }

    #[test]
    fn log_scan_and_torn_tail() {
        let dir = util::test_dir("fblog");
        let home = dir.as_path();
        {
            let mut l = Log::open(home, "s1").unwrap();
            l.append(json!({"type": "message", "input_id": "aaaa"}))
                .unwrap();
        }
        let p = home.join("state/sessions/s1.jsonl");
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{\"id\":\"deadbeef").unwrap();
        drop(f);
        let mut l = Log::open(home, "s1").unwrap();
        assert!(l.input_ids.contains("aaaa"));
        let id = l
            .append(json!({"type": "notice", "notice_id": "n1"}))
            .unwrap();
        let text = fs::read_to_string(&p).unwrap();
        let last: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(last["id"], json!(id));
        assert_eq!(
            text.lines().count(),
            4,
            "session, message, torn line, notice"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
