//! `sv-testbody`: a scripted body speaking stdio-jsonl/1 (supervise.md §4.4, §4.5) for WP4's
//! tests. Not the seed body and not testing.md's `init-fakebody` (WP3); just enough of the
//! contract to drive the supervisor: the session-log lock, `boot`/`message` events, `ready`,
//! heartbeats, acks, display frames, pause/resume/exit/stop, and the exit codes.
//!
//! Behaviour comes from `$INIT_HOME/work/testbody-plan.json`: `{"<sha prefix>"|"*": {…}}`, keyed
//! by `INIT_SHA` (the longest matching prefix wins). Fields: `crash_before_ready`,
//! `exit_before_ready` (code), `hang_before_ready`, `hang_after_ready` (no more heartbeats),
//! `ready_delay_ms`, `crash_after_ready_ms`, `crash_on_input`, `ignore_pause`,
//! `keep_lock_on_pause`, `crash_first` (crash before ready on the first N boots of this sha), `crash_on_input_boots`
//! (crash on any input during the first N boots),
//! `hb_ms` (default 1000), `ignore_stop`. Input texts `!crash` and `!exit0` act at once.
//! K-LINE: `!big <kind> <bytes>` displays [`big_text`]`(bytes)` as `<kind>` frames, each as large
//! as one stdout line allows (a `tool_start`/`tool_end` pair for `tool_end`: one frame, cut to
//! the line); `!bigdetail <bytes>` sends a `tool_end` whose `detail` field holds that many bytes;
//! `!flood <n>` sends `n` text deltas `<0>`, `<1>`, … as fast as stdout takes them.

use init_proto::stdio::{self, Display, DisplayKind, FromBody, ToBody};
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn now() -> String {
    kernel_supervise::clock::now_rfc3339()
}

fn hex(n: usize) -> String {
    kernel_supervise::sys::random_hex(n)
}

fn plan(home: &Path, sha: &str) -> Value {
    let all: Value = fs::read(home.join("work/testbody-plan.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(json!({}));
    let mut best: Option<(usize, Value)> = None;
    if let Some(m) = all.as_object() {
        for (k, v) in m {
            let score = if k == "*" { 0 } else if sha.starts_with(k.as_str()) { k.len() } else { continue };
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, v.clone()));
            }
        }
    }
    best.map(|b| b.1).unwrap_or(json!({}))
}

fn flag(p: &Value, k: &str) -> bool {
    p.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn num(p: &Value, k: &str) -> Option<u64> {
    p.get(k).and_then(Value::as_u64)
}

struct Out(Mutex<std::io::Stdout>);

impl Out {
    fn send(&self, f: &FromBody) {
        let b = stdio::encode_from_body(f).unwrap();
        let mut o = self.0.lock().unwrap();
        let _ = o.write_all(&b);
        let _ = o.flush();
    }
    fn display(&self, kind: DisplayKind, text: &str) {
        self.send(&FromBody::Display(Display { kind, text: text.into(), call_id: None, name: None, ok: None }));
    }
    fn raw(&self, line: &[u8]) {
        let mut o = self.0.lock().unwrap();
        let _ = o.write_all(line);
        let _ = o.flush();
    }
    /// `text` as `d.kind` frames, each the longest that fits one 1 MiB stdout line; `once` sends
    /// only the first frame (a frame kind that cannot be continued).
    fn big(&self, d: Display, text: &str, once: bool) {
        let line = |t: &str| stdio::encode_from_body(&FromBody::Display(Display { text: t.into(), ..d.clone() })).unwrap();
        let mut rest = text;
        while !rest.is_empty() {
            // The longest prefix whose line fits: `lo` fits, `hi` does not.
            let (mut lo, mut hi) = (0, rest.len() + 1);
            if line(rest).len() <= stdio::MAX_LINE {
                lo = rest.len();
            }
            while hi - lo > 4 {
                let mut mid = (lo + hi) / 2;
                while !rest.is_char_boundary(mid) {
                    mid -= 1;
                }
                if mid <= lo {
                    break;
                }
                if line(&rest[..mid]).len() <= stdio::MAX_LINE { lo = mid } else { hi = mid }
            }
            self.raw(&line(&rest[..lo]));
            rest = &rest[lo..];
            if once {
                break;
            }
        }
    }
}

/// K-LINE: `n` bytes of text that JSON escaping grows (quotes, a backslash, a tab, a newline,
/// U+0001), with two- and four-byte chars. tests/owner_state.rs builds the same text.
fn big_text(n: usize) -> String {
    const P: &str = "lorem \"q\" \\ \t\n\u{1} é 😀 ipsum ";
    let mut s = P.repeat(n / P.len() + 1);
    let mut cut = n.min(s.len());
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s
}

struct Log {
    path: PathBuf,
    file: Option<File>,
    leaf: Option<String>,
}

impl Log {
    fn lock(&mut self) -> bool {
        let f = OpenOptions::new().create(true).append(true).read(true).open(&self.path).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if f.try_lock().is_ok() {
                self.file = Some(f);
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    fn unlock(&mut self) {
        if let Some(f) = self.file.take() {
            let _ = f.sync_all();
            let _ = f.unlock();
        }
    }
    fn append(&mut self, mut ev: Value) -> String {
        let id = hex(8);
        ev["id"] = json!(id);
        ev["parent_id"] = json!(self.leaf);
        ev["ts"] = json!(now());
        let f = self.file.as_mut().expect("log locked");
        let mut line = serde_json::to_vec(&ev).unwrap();
        line.push(b'\n');
        f.write_all(&line).unwrap();
        f.sync_data().unwrap();
        self.leaf = Some(id.clone());
        id
    }
    fn has_input(&self, input_id: &str) -> bool {
        fs::read_to_string(&self.path).unwrap_or_default().lines().any(|l| serde_json::from_str::<Value>(l).ok().is_some_and(|v| v["input_id"] == json!(input_id)))
    }
    fn len(&self) -> u64 {
        fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
    }
}

fn write_atomic(p: &Path, v: &Value) {
    let tmp = p.with_extension(format!("tmp.{}", std::process::id()));
    let mut b = serde_json::to_vec(v).unwrap();
    b.push(b'\n');
    fs::write(&tmp, b).unwrap();
    fs::rename(&tmp, p).unwrap();
}

fn main() {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let home = PathBuf::from(env("INIT_HOME"));
    let boot_id = env("INIT_BOOT_ID");
    let sha = env("INIT_SHA");
    let mut sid = env("INIT_SESSION");
    if sid.is_empty() {
        sid = format!("{}-{}", kernel_supervise::clock::compact_utc(kernel_supervise::clock::now_ms()), hex(3));
    }
    let p = plan(&home, &sha);
    let sha12: String = sha.chars().take(12).collect();
    let boots_file = home.join(format!("state/testbody-{sha12}.boots"));
    let boots = fs::read_to_string(&boots_file).ok().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0) + 1;
    let _ = fs::write(&boots_file, boots.to_string());
    eprintln!("testbody: start sha={sha12} boot_id={boot_id} mode={} reason={} boot #{boots}", env("INIT_MODE"), env("INIT_BOOT_REASON"));

    if let Some(c) = num(&p, "exit_before_ready") {
        eprintln!("testbody: exit_before_ready {c}");
        std::process::exit(c as i32);
    }
    if flag(&p, "crash_before_ready") || num(&p, "crash_first").is_some_and(|n| boots <= n) {
        eprintln!("thread 'main' panicked at testbody: crash_before_ready");
        std::process::exit(101);
    }
    if flag(&p, "hang_before_ready") {
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    let _ = fs::create_dir_all(home.join("state/sessions"));
    let _ = fs::create_dir_all(home.join("state/run"));
    let mut log = Log { path: home.join("state/sessions").join(format!("{sid}.jsonl")), file: None, leaf: None };
    if !log.lock() {
        eprintln!("log: session {sid} locked by another writer");
        std::process::exit(1);
    }
    log.append(json!({"type": "boot", "boot_id": boot_id, "sha": sha, "reason": env("INIT_BOOT_REASON"), "mode": env("INIT_MODE")}));
    let _ = fs::write(home.join("state/session"), format!("{sid}\n"));
    if let Some(ms) = num(&p, "ready_delay_ms") {
        std::thread::sleep(Duration::from_millis(ms));
    }
    let out = Arc::new(Out(Mutex::new(std::io::stdout())));
    let busy = Arc::new(AtomicBool::new(false));
    let beating = Arc::new(AtomicBool::new(true));
    {
        let (home, boot_id, busy, beating) = (home.clone(), boot_id.clone(), busy.clone(), beating.clone());
        let hb_ms = num(&p, "hb_ms").unwrap_or(1000);
        std::thread::spawn(move || {
            while beating.load(Ordering::SeqCst) {
                write_atomic(&home.join("state/run/heartbeat"), &json!({"boot_id": boot_id, "busy": busy.load(Ordering::SeqCst), "ts": now(), "v": 1}));
                std::thread::sleep(Duration::from_millis(hb_ms));
            }
        });
    }
    write_atomic(&home.join("state/run/ready"), &json!({"boot_id": boot_id, "pid": std::process::id(), "prefix_hash": format!("sha256:{}", "0".repeat(64)), "sha": sha, "ts": now(), "v": 1}));
    out.display(DisplayKind::Info, &format!("ready {sha12}"));
    if flag(&p, "hang_after_ready") {
        std::thread::sleep(Duration::from_millis(1500));
        beating.store(false, Ordering::SeqCst);
    }
    if let Some(ms) = num(&p, "crash_after_ready_ms") {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            eprintln!("thread 'main' panicked at testbody: crash_after_ready");
            std::process::exit(101);
        });
    }

    let stdin = std::io::stdin();
    let mut lines = stdin.lock();
    let mut buf = Vec::new();
    let mut held: Vec<u64> = Vec::new(); // STEER: `!hold` inputs, neither logged nor acked until retracted
    loop {
        buf.clear();
        if lines.read_until(b'\n', &mut buf).unwrap_or(0) == 0 {
            log.unlock();
            std::process::exit(0); // stdin EOF: the kernel is gone
        }
        let Ok(frame) = stdio::decode_to_body(&buf) else { continue };
        match frame {
            ToBody::Input { id, seq, text, .. } => {
                let poison = num(&p, "crash_on_input_boots").is_some_and(|n| boots <= n);
                if flag(&p, "crash_on_input") || poison || text == "!crash" {
                    eprintln!("thread 'main' panicked at testbody: crash_on_input");
                    std::process::exit(101);
                }
                if text == "!hold" {
                    held.push(seq);
                    continue;
                }
                if text == "!exit0" {
                    std::process::exit(0);
                }
                busy.store(true, Ordering::SeqCst);
                out.send(&FromBody::State { busy: true });
                if !log.has_input(&id) {
                    log.append(json!({"type": "message", "role": "user", "input_id": id, "text": text}));
                }
                out.send(&FromBody::Ack { seq });
                if let Some(ms) = text.strip_prefix("!busy ").and_then(|s| s.parse::<u64>().ok()) {
                    std::thread::sleep(Duration::from_millis(ms));
                }
                let big: Vec<&str> = text.split(' ').collect();
                match big[..] {
                    ["!big", kind, n] => {
                        let kind: DisplayKind = serde_json::from_value(json!(kind)).unwrap();
                        let n: usize = n.parse().unwrap();
                        let d = Display { kind, text: String::new(), call_id: None, name: None, ok: None };
                        if kind == DisplayKind::ToolEnd {
                            let id = Some("c-big".to_string());
                            out.send(&FromBody::Display(Display { kind: DisplayKind::ToolStart, text: "big".into(), call_id: id.clone(), name: Some("bash".into()), ok: None }));
                            out.big(Display { call_id: id, name: Some("bash".into()), ok: Some(true), ..d }, &big_text(n), true);
                        } else {
                            out.big(d, &big_text(n), false);
                        }
                    }
                    ["!bigdetail", n] => {
                        let n: usize = n.parse().unwrap();
                        let f = json!({"call_id": "c-detail", "detail": "d".repeat(n), "kind": "tool_end", "name": "bash", "ok": true, "t": "display", "text": "exit 0", "v": 1});
                        out.raw(format!("{f}\n").as_bytes());
                    }
                    ["!flood", n] => {
                        for i in 0..n.parse::<usize>().unwrap() {
                            out.display(DisplayKind::Text, &format!("<{i}>"));
                        }
                    }
                    _ => out.display(DisplayKind::Text, &format!("echo: {text}")),
                }
                out.display(DisplayKind::TurnEnd, "");
                busy.store(false, Ordering::SeqCst);
                out.send(&FromBody::State { busy: false });
            }
            ToBody::Notice { id, kind, data } => {
                log.append(json!({"type": "notice", "notice_id": id, "kind": kind, "data": data}));
                out.send(&FromBody::NoticeAck { id });
            }
            ToBody::Interrupt { .. } => out.display(DisplayKind::Info, "interrupted"),
            ToBody::Stop { .. } => {
                if flag(&p, "ignore_stop") {
                    continue;
                }
                log.unlock();
                std::process::exit(0);
            }
            ToBody::Pause { token, .. } => {
                if flag(&p, "ignore_pause") {
                    continue;
                }
                let leaf = log.leaf.clone().unwrap_or_default();
                if flag(&p, "keep_lock_on_pause") {
                    if let Some(f) = log.file.as_ref() {
                        let _ = f.sync_all();
                    }
                } else {
                    log.unlock();
                }
                out.send(&FromBody::Paused { token: token.clone(), log_len: log.len(), leaf });
                // Paused: only resume, exit or stop (D11) are honoured.
                loop {
                    buf.clear();
                    if lines.read_until(b'\n', &mut buf).unwrap_or(0) == 0 {
                        std::process::exit(0);
                    }
                    match stdio::decode_to_body(&buf) {
                        Ok(ToBody::Resume { token: t, update_failed }) if t == token => {
                            if log.file.is_none() && !log.lock() {
                                eprintln!("log: session {sid} locked by another writer");
                                std::process::exit(1);
                            }
                            if let Some(uf) = update_failed {
                                log.append(json!({"type": "update_failed", "stage": uf.stage.as_str(), "reason": uf.reason, "seq": uf.seq}));
                            }
                            break;
                        }
                        Ok(ToBody::Exit { token: t, code }) if t == token => std::process::exit(code),
                        Ok(ToBody::Stop { .. }) => std::process::exit(0),
                        _ => {}
                    }
                }
            }
            ToBody::Exit { code, .. } => std::process::exit(code),
            ToBody::Retract { seq } => {
                let ok = held.contains(&seq);
                held.retain(|s| *s != seq);
                out.send(&FromBody::Retracted { seq, ok });
            }
            ToBody::Resume { .. } | ToBody::Reload { .. } | ToBody::Unknown => {}
        }
    }
}
