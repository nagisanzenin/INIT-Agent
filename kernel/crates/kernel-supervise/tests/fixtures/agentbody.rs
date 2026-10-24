//! `sv-agentbody`: a minimal model-driven body for WP8b's gate tests. Not the seed body: just
//! enough of body/boot.md and stdio-jsonl/1 to exercise G2, G3 and G4 for real through the
//! kernel proxy and `init-mockllm`:
//! - the session log (`state/sessions/<sid>.jsonl`, locked while running), the `boot` event,
//!   `ready` and heartbeats;
//! - a DeepSeek chat-completions loop over `INIT_PROXY_SOCK` (HTTP/1.1 on a Unix socket, SSE),
//!   with one tool, `bash`, run in the body's cwd;
//! - probe mode (body/boot.md §3.9): the `[init:probe]` round trip before `ready`;
//! - `--manifest` (body/boot.md §4.3): one JSON line with the `bash` tool and its `init-tools-v1`;
//! - pause/resume/exit/stop as the supervisor's handoff needs them.
//!
//! Behaviour per candidate comes from the committed file `agent-plan.json` at `INIT_SHA` (read with
//! `git -C $INIT_HOME/body show <sha>:agent-plan.json`), so a candidate's behaviour is part of its
//! commit, as with a real self-edit. Plan keys: `system` (the system prompt), `system_timestamp`
//! (G-12), `extra_request_tool` (G-11), `rerender_tool_results` (G-13), `fake_boot_prefix_hash`
//! (G-14), `crash_before_ready`, `hang_before_ready`, `ready_delay_ms` (G-10), `bash_broken`
//! (G-16), `task_broken` (bash refused in G4 task sessions only: a candidate that is worse on the
//! pool but can still self-edit), `crash_live` (F9: crash on a live `promote` boot), `hb_ms`.

use init_proto::stdio::{self, Display, DisplayKind, FromBody, ToBody};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MODEL: &str = "deepseek-flash";
const PROVIDER: &str = "deepseek";
const BASH_DESC: &str = "Run a command with bash -c in the working directory.";

fn now() -> String {
    kernel_supervise::clock::now_rfc3339()
}

fn hex(n: usize) -> String {
    kernel_supervise::sys::random_hex(n)
}

fn bash_tool() -> Value {
    json!({"name": "bash", "description": BASH_DESC,
           "input_schema": {"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}})
}

fn pty_tool() -> Value {
    json!({"name": "pty", "description": "An interactive terminal.", "input_schema": {"type": "object"}})
}

/// `init-tools-v1` over registry-shaped tools (sorted by name; serde_json sorts keys).
fn tools_sha256(tools: &[Value]) -> String {
    let mut t = tools.to_vec();
    t.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let d = Sha256::digest(Value::Array(t).to_string().as_bytes());
    format!("sha256:{}", d.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn manifest() {
    let tools = vec![bash_tool()];
    let m = json!({"manifest": 1, "body_version": "0.1.0", "control_channel": "stdio-jsonl/1",
                   "modes": ["normal", "probe", "safe"], "tools": tools, "tools_sha256": tools_sha256(&tools)});
    println!("{m}");
}

fn plan(home: &Path, sha: &str) -> Value {
    let show = |path: &str| -> Option<Value> {
        let o = std::process::Command::new("/usr/bin/git")
            .arg("-C")
            .arg(home.join("body"))
            .args(["show", &format!("{sha}:{path}")])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .ok()?;
        if !o.status.success() {
            return None;
        }
        serde_json::from_slice(&o.stdout).ok()
    };
    let mut p = show("agent-plan.json").unwrap_or(json!({}));
    // WP7b: a T1 candidate (changes only under ext/) carries its behaviour in ext/agent-plan.json,
    // whose keys override the root plan's.
    if let (Some(Value::Object(over)), Value::Object(base)) = (show("ext/agent-plan.json"), &mut p) {
        for (k, v) in over {
            base.insert(k, v);
        }
    }
    p
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
    fn append(&mut self, mut ev: Value) {
        let id = hex(8);
        ev["id"] = json!(id);
        ev["parent_id"] = json!(self.leaf);
        ev["ts"] = json!(now());
        let f = self.file.as_mut().expect("log locked");
        let mut line = serde_json::to_vec(&ev).unwrap();
        line.push(b'\n');
        f.write_all(&line).unwrap();
        f.sync_data().unwrap();
        self.leaf = Some(id);
    }
    /// The wire messages of the conversation so far, in order.
    fn messages(&self) -> Vec<Value> {
        fs::read_to_string(&self.path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v["type"] == "message")
            .map(|v| v["msg"].clone())
            .collect()
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

/// (status, lowercase headers, body).
type HttpReply = (u16, Vec<(String, String)>, Vec<u8>);

/// One HTTP/1.1 POST over the proxy's Unix socket.
fn post(sock: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> std::io::Result<HttpReply> {
    let mut s = UnixStream::connect(sock)?;
    s.set_read_timeout(Some(Duration::from_secs(120)))?;
    let mut req = format!("POST {path} HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\naccept: text/event-stream\r\nconnection: close\r\ncontent-length: {}\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes())?;
    s.write_all(body)?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| std::io::Error::other("no header end"))?;
    let head = String::from_utf8_lossy(&raw[..end]).to_string();
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|c| c.parse().ok()).unwrap_or(0);
    let hs: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let mut b = raw[end + 4..].to_vec();
    if hs.iter().any(|(k, v)| k == "transfer-encoding" && v.contains("chunked")) {
        let mut out = Vec::new();
        let mut i = 0;
        while let Some(nl) = b[i..].windows(2).position(|w| w == b"\r\n") {
            let size = usize::from_str_radix(String::from_utf8_lossy(&b[i..i + nl]).split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
            i += nl + 2;
            if size == 0 || i + size > b.len() {
                break;
            }
            out.extend_from_slice(&b[i..i + size]);
            i += size + 2;
        }
        b = out;
    }
    Ok((status, hs, b))
}

/// A parsed streamed reply.
#[derive(Default)]
struct Reply {
    text: String,
    calls: Vec<(String, String, String)>,
    finish: String,
    head_hash: Option<String>,
}

fn parse_sse(body: &[u8]) -> Reply {
    let mut r = Reply::default();
    for line in String::from_utf8_lossy(body).lines() {
        let Some(data) = line.strip_prefix("data:") else { continue };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
        let ch = &v["choices"][0];
        if let Some(t) = ch["delta"]["content"].as_str() {
            r.text.push_str(t);
        }
        if let Some(tcs) = ch["delta"]["tool_calls"].as_array() {
            for tc in tcs {
                let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                while r.calls.len() <= idx {
                    r.calls.push(Default::default());
                }
                if let Some(id) = tc["id"].as_str() {
                    r.calls[idx].0 = id.to_string();
                }
                if let Some(n) = tc["function"]["name"].as_str() {
                    r.calls[idx].1.push_str(n);
                }
                if let Some(a) = tc["function"]["arguments"].as_str() {
                    r.calls[idx].2.push_str(a);
                }
            }
        }
        if let Some(f) = ch["finish_reason"].as_str() {
            r.finish = f.to_string();
        }
    }
    r
}

struct Body {
    sock: String,
    sid: String,
    plan: Value,
    log: Log,
    in_task: bool,
}

impl Body {
    fn system(&self) -> Value {
        let mut s = self.plan.get("system").and_then(Value::as_str).unwrap_or("You are INIT, a test agent body.").to_string();
        if flag(&self.plan, "system_timestamp") {
            s.push_str(&format!(" Now: {}", now()));
        }
        json!({"role": "system", "content": s})
    }

    fn request_tools(&self) -> Vec<Value> {
        let mut t = vec![bash_tool()];
        if flag(&self.plan, "extra_request_tool") {
            t.push(pty_tool());
        }
        t.into_iter()
            .map(|x| json!({"type": "function", "function": {"name": x["name"], "description": x["description"], "parameters": x["input_schema"]}}))
            .collect()
    }

    fn history(&self) -> Vec<Value> {
        let mut m = vec![self.system()];
        for mut msg in self.log.messages() {
            if flag(&self.plan, "rerender_tool_results") && msg["role"] == "tool" {
                msg["content"] = json!(format!("output: {}", msg["content"].as_str().unwrap_or("")));
            }
            m.push(msg);
        }
        m
    }

    fn call(&self, purpose: &str, extra: Option<&Value>) -> Result<Reply, String> {
        let mut messages = self.history();
        if let Some(e) = extra {
            messages.push(e.clone());
        }
        let body = json!({"model": MODEL, "stream": true, "max_tokens": 1024, "messages": messages, "tools": self.request_tools()});
        let (status, hs, b) = post(&self.sock, &format!("/{PROVIDER}/chat/completions"), &[("x-init-session", &self.sid), ("x-init-purpose", purpose)], body.to_string().as_bytes()).map_err(|e| e.to_string())?;
        if status != 200 {
            return Err(format!("HTTP {status}: {}", String::from_utf8_lossy(&b)));
        }
        let mut r = parse_sse(&b);
        r.head_hash = hs.iter().find(|(k, _)| k == "x-init-head-hash").map(|x| x.1.clone());
        Ok(r)
    }

    fn run_tool(&self, name: &str, args: &str, probe_policy: bool) -> String {
        let cmd = serde_json::from_str::<Value>(args).ok().and_then(|v| v["command"].as_str().map(str::to_string)).unwrap_or_default();
        if name != "bash" {
            return format!("error: unknown tool {name}");
        }
        if probe_policy && cmd != "true" {
            return "error: probe_policy: not executed in probe mode".into();
        }
        if flag(&self.plan, "bash_broken") || (self.in_task && flag(&self.plan, "task_broken")) {
            return "error: bash is broken in this build".into();
        }
        match std::process::Command::new("/bin/bash").arg("-c").arg(&cmd).stdin(std::process::Stdio::null()).output() {
            Ok(o) => {
                let mut s = String::from_utf8_lossy(&o.stdout).to_string();
                s.push_str(&String::from_utf8_lossy(&o.stderr));
                s.truncate(30_000);
                format!("exit {}\n{s}", o.status.code().unwrap_or(-1))
            }
            Err(e) => format!("error: {e}"),
        }
    }

    /// Append the assistant message and run its calls; returns the reply.
    fn turn(&mut self, purpose: &str, extra: Option<Value>, probe_policy: bool) -> Result<Reply, String> {
        let r = self.call(purpose, extra.as_ref())?;
        if let Some(e) = extra {
            self.log.append(json!({"type": "message", "msg": e}));
        }
        let mut msg = json!({"role": "assistant", "content": r.text});
        if !r.calls.is_empty() {
            msg["tool_calls"] = Value::Array(r.calls.iter().map(|(id, n, a)| json!({"id": id, "type": "function", "function": {"name": n, "arguments": a}})).collect());
        }
        self.log.append(json!({"type": "message", "msg": msg}));
        for (id, n, a) in &r.calls {
            let out = self.run_tool(n, a, probe_policy);
            self.log.append(json!({"type": "message", "msg": {"role": "tool", "tool_call_id": id, "content": out}}));
        }
        Ok(r)
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--manifest") {
        manifest();
        return;
    }
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let home = PathBuf::from(env("INIT_HOME"));
    let boot_id = env("INIT_BOOT_ID");
    let sha = env("INIT_SHA");
    let mode = env("INIT_MODE");
    let reason = env("INIT_BOOT_REASON");
    let mut sid = env("INIT_SESSION");
    if sid.is_empty() {
        sid = format!("{}-{}", kernel_supervise::clock::compact_utc(kernel_supervise::clock::now_ms()), hex(3));
    }
    let p = plan(&home, &sha);
    let sha12: String = sha.chars().take(12).collect();
    eprintln!("agentbody: start sha={sha12} boot_id={boot_id} mode={mode} reason={reason} body={}", env("INIT_BODY_ID"));
    if flag(&p, "crash_before_ready") {
        eprintln!("thread 'main' panicked at agentbody: crash_before_ready");
        std::process::exit(101);
    }
    if flag(&p, "crash_live") && mode == "normal" && reason == "promote" {
        eprintln!("thread 'main' panicked at agentbody: crash_live");
        std::process::exit(101);
    }
    if flag(&p, "hang_before_ready") {
        // Exits on stdin EOF (the kernel is gone) so it never outlives the test.
        let mut b = Vec::new();
        let _ = std::io::stdin().lock().read_until(b'\n', &mut b);
        std::process::exit(0);
    }
    let _ = fs::create_dir_all(home.join("state/sessions"));
    let _ = fs::create_dir_all(home.join("state/run"));
    let log = Log { path: home.join("state/sessions").join(format!("{sid}.jsonl")), file: None, leaf: None };
    let mut b = Body { sock: env("INIT_PROXY_SOCK"), sid: sid.clone(), plan: p.clone(), log, in_task: false };
    if !b.log.lock() {
        eprintln!("log: session {sid} locked by another writer");
        std::process::exit(1);
    }
    let _ = fs::write(home.join("state/session"), format!("{sid}\n"));
    let mut prefix_hash = format!("sha256:{}", "0".repeat(64));
    if mode == "probe" {
        // body/boot.md §3.9: one tool round trip through the proxy before `ready`.
        let item = json!({"role": "user", "content": "[init:probe] Gate probe: call the bash tool once with the command \"true\", then reply with the single word ready."});
        match b.turn("probe", Some(item), true) {
            Ok(r) => {
                if let Some(h) = &r.head_hash {
                    prefix_hash = h.clone();
                }
                let claimed = if flag(&p, "fake_boot_prefix_hash") { format!("sha256:{}", "f".repeat(64)) } else { prefix_hash.clone() };
                b.log.append(json!({"type": "boot", "boot_id": boot_id, "sha": sha, "reason": reason, "mode": mode, "prefix_hash": claimed}));
                if !r.calls.is_empty()
                    && let Err(e) = b.turn("probe", None, true)
                {
                    eprintln!("{{\"type\":\"boot_error\",\"step\":\"probe\",\"error\":{:?}}}", e);
                    std::process::exit(70);
                }
            }
            Err(e) => {
                eprintln!("{{\"type\":\"boot_error\",\"step\":\"probe\",\"error\":{:?}}}", e);
                std::process::exit(70);
            }
        }
    } else {
        b.log.append(json!({"type": "boot", "boot_id": boot_id, "sha": sha, "reason": reason, "mode": mode}));
    }
    if let Some(ms) = num(&p, "ready_delay_ms") {
        std::thread::sleep(Duration::from_millis(ms));
    }
    let out = Arc::new(Out(Mutex::new(std::io::stdout())));
    let busy = Arc::new(AtomicBool::new(false));
    {
        let (home, boot_id, busy) = (home.clone(), boot_id.clone(), busy.clone());
        let hb_ms = num(&p, "hb_ms").unwrap_or(200);
        std::thread::spawn(move || loop {
            write_atomic(&home.join("state/run/heartbeat"), &json!({"boot_id": boot_id, "busy": busy.load(Ordering::SeqCst), "ts": now(), "v": 1}));
            std::thread::sleep(Duration::from_millis(hb_ms));
        });
    }
    write_atomic(&home.join("state/run/ready"), &json!({"boot_id": boot_id, "pid": std::process::id(), "prefix_hash": prefix_hash, "sha": sha, "ts": now(), "v": 1}));
    out.display(DisplayKind::Info, &format!("ready {sha12}"));

    let stdin = std::io::stdin();
    let mut lines = stdin.lock();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if lines.read_until(b'\n', &mut buf).unwrap_or(0) == 0 {
            b.log.unlock();
            std::process::exit(0);
        }
        let Ok(frame) = stdio::decode_to_body(&buf) else { continue };
        match frame {
            ToBody::Input { id, seq, text, .. } => {
                busy.store(true, Ordering::SeqCst);
                out.send(&FromBody::State { busy: true });
                out.send(&FromBody::Ack { seq });
                if text.starts_with("[task ") {
                    b.in_task = true;
                }
                if !b.log.has_input(&id) {
                    let mut extra = Some(json!({"role": "user", "content": text}));
                    b.log.append(json!({"type": "input", "input_id": id}));
                    for _ in 0..16 {
                        match b.turn("turn", extra.take(), false) {
                            Ok(r) => {
                                if !r.text.is_empty() {
                                    out.display(DisplayKind::Text, &r.text);
                                }
                                if r.calls.is_empty() {
                                    break;
                                }
                            }
                            Err(e) => {
                                out.display(DisplayKind::Error, &e);
                                break;
                            }
                        }
                    }
                }
                out.display(DisplayKind::TurnEnd, "");
                busy.store(false, Ordering::SeqCst);
                out.send(&FromBody::State { busy: false });
            }
            ToBody::Notice { id, kind, data } => {
                b.log.append(json!({"type": "notice", "notice_id": id, "kind": kind, "data": data}));
                out.send(&FromBody::NoticeAck { id });
            }
            ToBody::Interrupt { .. } => out.display(DisplayKind::Info, "interrupted"),
            ToBody::Stop { .. } => {
                b.log.unlock();
                std::process::exit(0);
            }
            ToBody::Pause { token, .. } => {
                let leaf = b.log.leaf.clone().unwrap_or_default();
                b.log.unlock();
                out.send(&FromBody::Paused { token: token.clone(), log_len: b.log.len(), leaf });
                loop {
                    buf.clear();
                    if lines.read_until(b'\n', &mut buf).unwrap_or(0) == 0 {
                        std::process::exit(0);
                    }
                    match stdio::decode_to_body(&buf) {
                        Ok(ToBody::Resume { token: t, update_failed }) if t == token => {
                            if b.log.file.is_none() && !b.log.lock() {
                                eprintln!("log: session {sid} locked by another writer");
                                std::process::exit(1);
                            }
                            if let Some(uf) = update_failed {
                                b.log.append(json!({"type": "update_failed", "stage": uf.stage.as_str(), "reason": uf.reason, "seq": uf.seq}));
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
            // WP7b: a T0/T1 reload at this (idle) step boundary; nothing to re-read here.
            ToBody::Reload { job, components, .. } => {
                b.log.append(json!({"type": "reload", "job": job}));
                out.send(&FromBody::ReloadDone { job, ok: true, prefix_changed: false, components: components.unwrap_or_default(), error: None });
            }
            ToBody::Resume { .. } | ToBody::Retract { .. } | ToBody::Unknown => {}
        }
    }
}
