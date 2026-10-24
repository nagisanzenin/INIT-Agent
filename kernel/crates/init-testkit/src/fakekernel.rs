//! `init-fakekernel` (testing.md §3.5a): the **Root-facing contract of a kernel** (root/root.md
//! §4.1; kernel side: kernel/supervise.md §3.13), so the Root lane can test R1, R2, R4 and R7
//! without the real kernel, and so tests can play a **hostile** kernel.
//!
//! In `ok` mode it: inherits the control link on fds 3/4 (`INIT_ROOT_FD`, `INIT_ROOT_FDPASS`),
//! sends `hello` and reads the typed boot block, asks `instances`, sends `ready`, then `hb` every
//! `hb_interval_s`; spawns one body through the Root's broker (`spawn`, fds over fd 4) with the
//! supervise.md §4.1 environment, waits for `state/run/ready`; relays owner input (`owner.in` →
//! durable inbox → `input`, with `echo`, ack and redelivery) and body output (`display` →
//! `owner.out`); relays `level` events to the body as notices; reports a scripted LKG sequence
//! (`store.put` of `bin/init-body` + `lineage.append` `promotion{lkg:true}`); obeys `stop` (exit 0),
//! `prepare_swap` (pause, stop, exit 75) and link loss (stop the body, exit 69); restarts a crashed
//! body with backoff and reports body safe mode after 5 crashes in 60 s.
//!
//! | `FAKEKERNEL_MODE` (or `INIT_TEST_FAKEKERNEL_MODE`) | Behaviour |
//! |---|---|
//! | `ok` | as above |
//! | `ignore_term` | ignore SIGTERM and `stop`; keep a `setsid` child alive (RT-P5; **disposable only**) |
//! | `bypass_soft[:<n>]` | send `<n>` (3) model requests straight to the Root egress, without `x-egress-guard` (RT-C2, RT-C7) |
//! | `extra_bodies:<n>` | ask the broker for `<n>` more bodies than allowed (RT-C3) |
//! | `egress:<host>` | ask the egress for a non-provider host and a raw `CONNECT` (RT-E2) |
//! | `ignore_level` | try level-gated requests at every level (`system.swap`, `sealed.family_eval`, a member body) |
//! | `no_body` | spawn no body (WP3 extension) |
//! | `no_hb` | never send `hb` (WP3 extension: the Root's 30 s rule) |
//! | `no_hello` | never send `hello` (WP3 extension: the Root's hello timeout) |
//! | `exit_after_hello:<code>` | exit `<code>` right after the boot block (WP3 extension: 78 → `Halted{config}`) |
//! | `crash_after_ready:<ms>` | exit 70 `<ms>` after `ready` (WP3 extension: crash loops) |
//!
//! Outcomes of every hostile request are logged to `$INIT_HOME/state/kernel/fakekernel.jsonl`, with
//! every link frame in and out. `INIT_FAULT` points: `fakekernel.after_hello`,
//! `fakekernel.before_ready`, `fakekernel.after_spawn`, `fakekernel.stop.before_ok`.

use crate::link::{Incoming, Link, Trace};
use crate::mockllm::{MockRequest, client};
use crate::util::{self, ts};
use crate::{fault, guard, sys};
use init_proto::envelope::{Request, Response};
use init_proto::icj;
use init_proto::root_link::{
    AlertArgs, AlertLevel, BootBlock, ExitedData, HbData, HelloArgs, InstanceKind, KillArgs,
    KillMode, LevelData, LineageAppendArgs, OwnerDisplay, OwnerInArgs, OwnerInFrame, OwnerOutData,
    OwnerOutFrame, RootEvent, RootRequest, SpawnArgs, SpawnResult, StoreKind, StorePutArgs,
    ZoneMode,
};
use init_proto::stdio::{self, FromBody, InputMode, PauseReason, ToBody};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Reported as `hello.kernel_version`.
pub const VERSION: &str = "0.0.0-fakekernel";
const REQ: Duration = Duration::from_secs(30);

/// One fake-kernel mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Ok,
    IgnoreTerm,
    BypassSoft(u32),
    ExtraBodies(u32),
    Egress(String),
    IgnoreLevel,
    NoBody,
    NoHb,
    NoHello,
    ExitAfterHello(i32),
    CrashAfterReady(u64),
}

/// Parse a comma-separated mode list (empty = `ok`).
pub fn parse_modes(s: &str) -> Result<Vec<Mode>, String> {
    let mut out = Vec::new();
    for m in s.split(',').map(str::trim).filter(|m| !m.is_empty()) {
        let (k, v) = m.split_once(':').map_or((m, None), |(k, v)| (k, Some(v)));
        let num = |d: Option<u64>| -> Result<u64, String> {
            match v {
                Some(x) => x.parse().map_err(|_| format!("`{m}`: bad number")),
                None => d.ok_or_else(|| format!("`{m}` needs a number")),
            }
        };
        out.push(match k {
            "ok" => Mode::Ok,
            "ignore_term" => Mode::IgnoreTerm,
            "bypass_soft" => Mode::BypassSoft(num(Some(3))? as u32),
            "extra_bodies" => Mode::ExtraBodies(num(None)? as u32),
            "egress" => Mode::Egress(
                v.filter(|h| !h.is_empty())
                    .ok_or("egress:<host> needs a host")?
                    .to_string(),
            ),
            "ignore_level" => Mode::IgnoreLevel,
            "no_body" => Mode::NoBody,
            "no_hb" => Mode::NoHb,
            "no_hello" => Mode::NoHello,
            "exit_after_hello" => Mode::ExitAfterHello(num(None)? as i32),
            "crash_after_ready" => Mode::CrashAfterReady(num(None)?),
            _ => return Err(format!("unknown FAKEKERNEL_MODE `{m}`")),
        });
    }
    if out.is_empty() {
        out.push(Mode::Ok);
    }
    Ok(out)
}

enum Msg {
    Link(Incoming),
    Body(u64, FromBody),
    BodyEof(u64),
    BodyReady(u64),
    ReadyTimeout(u64),
}

struct Body {
    instance: String,
    pid: i32,
    boot_seq: u64,
    boot_id: String,
    stdin: Sender<Vec<u8>>,
    ready: bool,
}

struct InboxEntry {
    id: String,
    seq: u64,
    text: String,
    ts: String,
    mode: InputMode,
    acked: bool,
}

struct Stopping {
    req_id: String,
    code: i32,
    deadline: Instant,
    pause_token: Option<String>,
    killed: bool,
}

#[derive(Default)]
struct HbState {
    busy: bool,
    slot: String,
    mode: String,
    sha12: String,
}

struct K {
    home: PathBuf,
    link: Arc<Link>,
    modes: Vec<Mode>,
    tx: Sender<Msg>,
    log: Mutex<Option<File>>,
    boot: Option<BootBlock>,
    level: u8,
    kboot: String,
    outbox: u64,
    body: Option<Body>,
    extras: Vec<String>,
    boot_seq: u64,
    inbox: Vec<InboxEntry>,
    qids: HashSet<String>,
    next_seq: u64,
    notices: Vec<(String, String, Value, bool)>,
    hb: Arc<Mutex<HbState>>,
    stopping: Option<Stopping>,
    owner_stopped: bool,
    crashes: Vec<Instant>,
    respawn_at: Option<Instant>,
    body_safe: bool,
    config_exits: u32,
    sha: String,
}

fn has(modes: &[Mode], m: &Mode) -> bool {
    modes.contains(m)
}

impl K {
    fn log(&self, rec: Value) {
        let mut rec = rec;
        rec["t"] = Value::String(ts());
        if let Some(f) = self.log.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let mut b = serde_json::to_vec(&rec).unwrap_or_default();
            b.push(b'\n');
            let _ = f.write_all(&b);
        }
    }

    fn owner_out(&self, frame: OwnerOutFrame) {
        if let Ok(v) = icj::to_value(&OwnerOutData { frame }) {
            let _ = self.link.event("owner.out", v);
        }
    }

    fn notice_owner(&self, level: AlertLevel, text: &str) {
        self.owner_out(OwnerOutFrame::Notice {
            level,
            text: text.to_string(),
            ts: ts(),
        });
    }

    fn set_hb(&self, f: impl FnOnce(&mut HbState)) {
        f(&mut self.hb.lock().unwrap_or_else(|e| e.into_inner()));
    }

    fn request(&self, cmd: &str, args: Value) -> io::Result<Response> {
        let r = self.link.request(cmd, args, REQ);
        self.log(json!({"ev": "request", "cmd": cmd, "resp": r.as_ref().map(|x| x.to_value()).unwrap_or_else(|e| json!({"io_error": e.to_string()}))}));
        r
    }

    fn lineage(&mut self, kind: &str, data: Value) -> io::Result<Response> {
        self.outbox += 1;
        let args = LineageAppendArgs {
            kind: kind.into(),
            data,
            idem: format!("{}:{}", self.kboot, self.outbox),
        };
        let local = json!({"at": ts(), "data": args.data, "dedup_key": args.idem, "kind": kind, "seq": self.outbox, "v": 1});
        let _ = util::append_json(
            &self.home.join(init_proto::files::paths::LINEAGE_LOCAL),
            &local,
            true,
        );
        self.request("lineage.append", icj::to_value(&args).unwrap_or(json!({})))
    }

    fn body_env(&self, boot_id: &str, session: &str, reason: &str) -> BTreeMap<String, String> {
        let mode = match self.boot.as_ref().map(|b| b.boot.mode) {
            Some(ZoneMode::Safe) => "safe",
            _ => "normal",
        };
        body_env(&self.home, &self.sha, boot_id, session, reason, mode, "b0")
    }

    fn spawn_body(&mut self, reason: &str) {
        self.boot_seq += 1;
        let seq = self.boot_seq;
        let boot_id = util::random_hex(8);
        let session = fs::read_to_string(self.home.join("state/session"))
            .ok()
            .map(|s| s.trim_end_matches('\n').to_string())
            .filter(|s| util::is_session_id(s))
            .unwrap_or_else(|| util::new_session_id(util::now_ms()));
        for d in ["work/home", "work/tmp", "state/run", "state/kernel/logs"] {
            let _ = fs::create_dir_all(self.home.join(d));
        }
        let _ = util::write_atomic(
            &self.home.join("state/kernel/boot.json"),
            &serde_json::to_vec(&json!({"boot_id": boot_id, "boot_seq": seq, "reason": reason, "session": session, "sha": self.sha, "v": 1, "fake": true})).unwrap_or_default(),
            true,
        );
        let args = SpawnArgs {
            kind: InstanceKind::Body,
            profile: "body".into(),
            params: BTreeMap::new(),
            program: self.home.join("bin/init-body").display().to_string(),
            args: vec![],
            env: self.body_env(&boot_id, &session, reason),
            cwd: self.home.join("work").display().to_string(),
            lane: Some("b0".into()),
            replaces: None,
        };
        let resp = match self.request("spawn", icj::to_value(&args).unwrap_or(json!({}))) {
            Ok(r) => r,
            Err(e) => {
                self.log(json!({"ev": "spawn_failed", "error": e.to_string()}));
                return;
            }
        };
        let Ok(res) = resp.result_as::<SpawnResult>() else {
            self.log(json!({"ev": "spawn_refused", "resp": resp.to_value()}));
            self.notice_owner(
                AlertLevel::Warn,
                &format!("[kernel] body spawn refused: {}", resp.to_value()),
            );
            return;
        };
        let id = resp.id.clone().unwrap_or_default();
        let fds = match self.link.take_fds(&id, Duration::from_secs(5)) {
            Ok(f) if f.len() == 3 => f,
            other => {
                self.log(json!({"ev": "spawn_fds_missing", "got": other.map(|f| f.len()).map_err(|e| e.to_string())}));
                let _ = self.request(
                    "kill",
                    icj::to_value(&KillArgs {
                        instance: res.instance.clone(),
                        mode: KillMode::Kill,
                    })
                    .unwrap_or(json!({})),
                );
                return;
            }
        };
        let _ = fault::point("fakekernel.after_spawn");
        let mut it = fds.into_iter();
        let (stdin_w, stdout_r, stderr_r) = (
            it.next().expect("3 fds"),
            it.next().expect("3 fds"),
            it.next().expect("3 fds"),
        );
        let (wtx, wrx) = mpsc::channel::<Vec<u8>>();
        let mut w = File::from(stdin_w);
        std::thread::spawn(move || {
            while let Ok(line) = wrx.recv() {
                if w.write_all(&line).and_then(|_| w.flush()).is_err() {
                    break;
                }
            }
        });
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut r = BufReader::new(File::from(stdout_r));
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match (&mut r)
                    .take(stdio::MAX_LINE as u64 + 2)
                    .read_until(b'\n', &mut buf)
                {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if let Ok(f) = stdio::decode_from_body(&buf)
                    && tx.send(Msg::Body(seq, f)).is_err()
                {
                    return;
                }
            }
            let _ = tx.send(Msg::BodyEof(seq));
        });
        let errp = self
            .home
            .join(format!("state/kernel/logs/body-{seq}.stderr"));
        std::thread::spawn(move || {
            let mut r = File::from(stderr_r);
            let mut out = File::create(errp).ok();
            let mut buf = [0u8; 8192];
            let mut total = 0usize;
            while let Ok(n) = r.read(&mut buf) {
                if n == 0 {
                    break;
                }
                total += n;
                if total <= 4 * 1024 * 1024
                    && let Some(o) = out.as_mut()
                {
                    let _ = o.write_all(&buf[..n]);
                }
            }
        });
        let (tx, home, bid) = (self.tx.clone(), self.home.clone(), boot_id.clone());
        let timeout_ms = std::env::var("INIT_TEST_FAKEKERNEL_READY_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30_000u64);
        std::thread::spawn(move || {
            let p = home.join(init_proto::files::paths::READY);
            let found = util::poll(
                Duration::from_millis(timeout_ms),
                Duration::from_millis(50),
                || {
                    let b = fs::read(&p).ok()?;
                    let r: init_proto::files::ReadyFile = init_proto::files::decode(&b).ok()?;
                    (r.boot_id == bid).then_some(())
                },
            );
            let _ = tx.send(if found.is_some() {
                Msg::BodyReady(seq)
            } else {
                Msg::ReadyTimeout(seq)
            });
        });
        self.log(json!({"ev": "body_spawned", "instance": res.instance, "pid": res.pid, "boot_seq": seq, "boot_id": boot_id}));
        self.body = Some(Body {
            instance: res.instance,
            pid: res.pid,
            boot_seq: seq,
            boot_id,
            stdin: wtx,
            ready: false,
        });
        self.set_hb(|h| h.slot = "starting".into());
    }

    fn to_body(&self, f: &ToBody) {
        if let Some(b) = &self.body
            && let Ok(line) = stdio::encode_to_body(f)
        {
            let _ = b.stdin.send(line);
        }
    }

    fn deliver_pending(&mut self) {
        if !self.body.as_ref().is_some_and(|b| b.ready) || self.stopping.is_some() {
            return;
        }
        let frames: Vec<ToBody> = self
            .inbox
            .iter()
            .filter(|e| !e.acked)
            .map(|e| ToBody::Input {
                id: e.id.clone(),
                seq: e.seq,
                text: e.text.clone(),
                ts: e.ts.clone(),
                mode: e.mode,
            })
            .chain(
                self.notices
                    .iter()
                    .filter(|n| !n.3)
                    .map(|n| ToBody::Notice {
                        id: n.0.clone(),
                        kind: n.1.clone(),
                        data: n.2.clone(),
                    }),
            )
            .collect();
        for f in &frames {
            self.to_body(f);
        }
    }

    fn add_notice(&mut self, kind: &str, data: Value) {
        self.notices
            .push((util::random_hex(8), kind.to_string(), data, false));
        self.deliver_pending();
    }

    fn owner_in(&mut self, req: &Request, a: OwnerInArgs) {
        if self.qids.contains(&a.qid) {
            let _ = self
                .link
                .respond(&Response::ok_value(req.id.clone(), json!({"dup": true})));
            return;
        }
        match a.frame {
            OwnerInFrame::Interrupt { then } => {
                self.qids.insert(a.qid);
                let _ = self
                    .link
                    .respond(&Response::ok_value(req.id.clone(), json!({})));
                self.to_body(&ToBody::Interrupt { then });
            }
            OwnerInFrame::Retract { seq } => {
                self.qids.insert(a.qid);
                let _ = self
                    .link
                    .respond(&Response::ok_value(req.id.clone(), json!({})));
                self.to_body(&ToBody::Retract { seq });
            }
            OwnerInFrame::Line { text } => {
                let t = text.trim().to_string();
                let (mode, input_text) = match t.strip_prefix("/later ") {
                    Some(rest) => (InputMode::FollowUp, rest.to_string()),
                    None => (InputMode::Steer, text.clone()),
                };
                let command = matches!(t.as_str(), "/stop" | "/resume" | "/status" | "/kernel");
                if !command {
                    self.next_seq += 1;
                    let e = InboxEntry {
                        id: util::random_hex(8),
                        seq: self.next_seq,
                        text: input_text,
                        ts: ts(),
                        mode,
                        acked: false,
                    };
                    let rec = json!({"id": e.id, "qid": a.qid, "seq": e.seq, "text": e.text, "ts": e.ts, "v": 1});
                    if let Err(err) =
                        util::append_json(&self.home.join("state/kernel/inbox.jsonl"), &rec, true)
                    {
                        let _ = self.link.respond(&Response::err(
                            Some(req.id.clone()),
                            init_proto::root_link::RootErrorCode::Internal
                                .error(format!("inbox: {err}")),
                        ));
                        return;
                    }
                    self.qids.insert(a.qid);
                    let _ = self
                        .link
                        .respond(&Response::ok_value(req.id.clone(), json!({})));
                    self.owner_out(OwnerOutFrame::Echo {
                        seq: e.seq,
                        text: text.clone(),
                    });
                    self.inbox.push(e);
                    if self.owner_stopped && self.body.is_none() {
                        self.owner_stopped = false;
                        self.spawn_body("cold");
                    } else {
                        self.deliver_pending();
                    }
                    return;
                }
                self.qids.insert(a.qid);
                let _ = self
                    .link
                    .respond(&Response::ok_value(req.id.clone(), json!({})));
                match t.as_str() {
                    "/stop" => {
                        self.owner_stopped = true;
                        self.to_body(&ToBody::Stop {
                            reason: "owner".into(),
                            grace_ms: stdio::STOP_GRACE_MS,
                        });
                    }
                    "/resume" => {
                        self.owner_stopped = false;
                        if self.body.is_none() {
                            self.spawn_body("cold");
                        }
                    }
                    "/status" => {
                        let s = format!(
                            "[kernel] fake kernel: level L{}, body {:?}, inbox {} ({} unacked)",
                            self.level,
                            self.body.as_ref().map(|b| (&b.instance, b.ready)),
                            self.inbox.len(),
                            self.inbox.iter().filter(|e| !e.acked).count()
                        );
                        self.notice_owner(AlertLevel::Info, &s);
                    }
                    _ => self.notice_owner(
                        AlertLevel::Info,
                        "[kernel] /stop · /resume · /status · /later <text> · /kernel",
                    ),
                }
            }
        }
    }

    fn begin_stop(&mut self, req_id: String, code: i32, reason: &str, grace_ms: u64) {
        let _ = fault::point("fakekernel.stop.before_ok");
        let deadline = Instant::now() + Duration::from_millis(grace_ms.max(100));
        let pause_token = (code == 75 && self.body.is_some()).then(|| util::random_hex(8));
        self.stopping = Some(Stopping {
            req_id,
            code,
            deadline,
            pause_token: pause_token.clone(),
            killed: false,
        });
        self.set_hb(|h| h.slot = "stopping".into());
        for x in self.extras.clone() {
            let _ = self.request(
                "kill",
                icj::to_value(&KillArgs {
                    instance: x,
                    mode: KillMode::Term,
                })
                .unwrap_or(json!({})),
            );
        }
        match pause_token {
            Some(token) => self.to_body(&ToBody::Pause {
                token,
                reason: PauseReason::Swap,
            }),
            None if self.body.is_some() => self.to_body(&ToBody::Stop {
                reason: reason.into(),
                grace_ms,
            }),
            None => self.finish_stop(),
        }
    }

    fn finish_stop(&mut self) -> ! {
        let s = self.stopping.take().expect("stopping");
        let _ = self.link.respond(&Response::ok_value(s.req_id, json!({})));
        self.log(json!({"ev": "exit", "code": s.code}));
        std::thread::sleep(Duration::from_millis(20));
        std::process::exit(s.code)
    }

    fn on_body_exit(&mut self, d: &ExitedData) {
        let Some(b) = self.body.take() else { return };
        self.log(json!({"ev": "body_exit", "instance": d.instance, "code": d.code, "signal": d.signal, "boot_seq": b.boot_seq}));
        self.set_hb(|h| {
            h.slot = "stopped".into();
            h.busy = false;
        });
        if self.stopping.is_some() {
            self.finish_stop();
        }
        if self.owner_stopped || d.code == Some(0) {
            self.owner_stopped = true;
            self.notice_owner(
                AlertLevel::Info,
                "[kernel] the body stopped; send a line to start it again",
            );
            return;
        }
        if d.code == Some(78) && !b.ready {
            self.config_exits += 1;
            if self.config_exits >= 2 {
                let _ = self.request(
                    "alert",
                    icj::to_value(&AlertArgs {
                        code: "config_error".into(),
                        level: AlertLevel::Alert,
                        message: "the body exited 78 twice before ready".into(),
                    })
                    .unwrap_or(json!({})),
                );
                self.body_safe = true;
                return;
            }
        }
        let now = Instant::now();
        self.crashes
            .retain(|t| now.duration_since(*t) < Duration::from_secs(60));
        self.crashes.push(now);
        if self.crashes.len() > 4 {
            self.body_safe = true;
            let n = self.crashes.len();
            let _ = self.lineage(
                "body_safe",
                json!({"on": true, "sha": self.sha, "crashes": n}),
            );
            let _ = self.request(
                "alert",
                icj::to_value(&AlertArgs {
                    code: "body_safe".into(),
                    level: AlertLevel::Alert,
                    message: format!("{n} body crashes within 60 s"),
                })
                .unwrap_or(json!({})),
            );
            self.set_hb(|h| h.mode = "safe".into());
            return;
        }
        let base = std::env::var("INIT_TEST_FAKEKERNEL_BACKOFF_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100u64);
        let delay = base.saturating_mul(1 << (self.crashes.len() - 1).min(6));
        self.respawn_at = Some(now + Duration::from_millis(delay));
    }

    fn handle_request(&mut self, req: Request) {
        let parsed = match RootRequest::parse(&req.cmd, &req.args) {
            Ok(p) => p,
            Err(e) => {
                let _ = self.link.respond(&Response::err(Some(req.id.clone()), e));
                return;
            }
        };
        match parsed {
            RootRequest::OwnerIn(a) => self.owner_in(&req, a),
            RootRequest::Stop(s) => {
                if has(&self.modes, &Mode::IgnoreTerm) {
                    self.log(json!({"ev": "ignored_stop"}));
                    return;
                }
                self.begin_stop(req.id, 0, &s.reason, s.grace_ms);
            }
            RootRequest::PrepareSwap(p) => {
                if has(&self.modes, &Mode::IgnoreTerm) {
                    self.log(json!({"ev": "ignored_prepare_swap"}));
                    return;
                }
                self.begin_stop(req.id, 75, "swap", (p.deadline_s * 1000).min(10_000));
            }
        }
    }

    fn handle_event(&mut self, ev: init_proto::envelope::Event) {
        match RootEvent::from_event(&ev) {
            Ok(RootEvent::Exited(d)) => {
                if self.body.as_ref().is_some_and(|b| b.instance == d.instance) {
                    self.on_body_exit(&d);
                } else {
                    self.extras.retain(|x| *x != d.instance);
                    self.log(
                        json!({"ev": "instance_exit", "instance": d.instance, "code": d.code}),
                    );
                }
            }
            Ok(RootEvent::Level(l)) => self.on_level(l),
            Ok(other) => self.log(json!({"ev": "root_event", "event": format!("{other:?}")})),
            Err(e) => self.log(json!({"ev": "bad_root_event", "error": e.to_string()})),
        }
    }

    fn on_level(&mut self, l: LevelData) {
        self.level = l.to;
        self.log(json!({"ev": "level", "from": l.from, "to": l.to, "cause": l.cause}));
        self.add_notice(
            "level",
            json!({"level": l.to, "from": l.from, "ceilings": null, "trigger": l.trigger}),
        );
    }

    fn on_body_frame(&mut self, seq: u64, f: FromBody) {
        if self.body.as_ref().is_none_or(|b| b.boot_seq != seq) {
            return;
        }
        match f {
            FromBody::Display(d) => self.owner_out(OwnerOutFrame::Display(OwnerDisplay {
                boot_seq: seq,
                display: d,
                extra: Default::default(),
            })),
            FromBody::Ack { seq: s } => {
                if let Some(e) = self.inbox.iter_mut().find(|e| e.seq == s) {
                    e.acked = true;
                }
            }
            FromBody::NoticeAck { id } => {
                if let Some(n) = self.notices.iter_mut().find(|n| n.0 == id) {
                    n.3 = true;
                }
            }
            FromBody::State { busy } => {
                self.set_hb(|h| h.busy = busy);
                let unacked = self.inbox.iter().filter(|e| !e.acked).count() as u64;
                let (slot, mode, sha12) = {
                    let h = self.hb.lock().unwrap_or_else(|e| e.into_inner());
                    (h.slot.clone(), h.mode.clone(), h.sha12.clone())
                };
                self.owner_out(OwnerOutFrame::State {
                    busy,
                    level: self.level,
                    members: 0,
                    mode,
                    model: None,
                    queued: unacked,
                    sha12,
                    slot,
                    task_musd: None,
                    evolution_checks_today_musd: None,
                    steer_pending: None,
                    steer_seen: None,
                    steer_retracted: None,
                });
            }
            FromBody::Paused { token, .. } => {
                if let Some(s) = &self.stopping
                    && s.pause_token.as_deref() == Some(token.as_str())
                {
                    self.to_body(&ToBody::Stop {
                        reason: "swap".into(),
                        grace_ms: 5_000,
                    });
                }
            }
            FromBody::ReloadDone { job, ok, .. } => {
                self.log(json!({"ev": "reload_done", "job": job, "ok": ok}))
            }
            FromBody::Retracted { .. } | FromBody::Unknown => {}
        }
    }

    fn report_lkg(&mut self) {
        let script = std::env::var("INIT_TEST_FAKEKERNEL_LKG").unwrap_or_default();
        let shas: Vec<String> = match script.as_str() {
            "none" => vec![],
            "" => vec![util::sha256_hex(b"fake-lkg-1")[..40].to_string()],
            s => s
                .split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect(),
        };
        let bin = self.home.join("bin/init-body");
        let mut from = "0".repeat(40);
        for (i, sha) in shas.iter().enumerate() {
            let (sha256, bytes) =
                util::sha256_file(&bin).unwrap_or_else(|_| (util::sha256_hex(b""), 0));
            if let Ok(f) = File::open(&bin) {
                let args = StorePutArgs {
                    kind: StoreKind::InitBody,
                    name: None,
                    sha256: sha256.clone(),
                    bytes,
                    job: None,
                };
                let r = self.link.request_with_fds(
                    "store.put",
                    icj::to_value(&args).unwrap_or(json!({})),
                    &[f.as_raw_fd()],
                    REQ,
                );
                self.log(json!({"ev": "request", "cmd": "store.put", "resp": r.as_ref().map(|x| x.to_value()).unwrap_or_else(|e| json!({"io_error": e.to_string()}))}));
            }
            let data = json!({"sha": sha, "from_sha": from, "gate_job": format!("j-fake-{}", i + 1), "scope": "T2", "lkg": true,
                "body_binary": sha256, "ext_set": null, "components": []});
            let _ = self.lineage("promotion", data);
            from = sha.clone();
        }
        if let Some(last) = shas.last() {
            self.sha = last.clone();
            let s12 = last[..last.len().min(12)].to_string();
            self.set_hb(|h| h.sha12 = s12);
        }
    }

    fn hostile(&mut self) {
        for m in self.modes.clone() {
            match m {
                Mode::ExtraBodies(n) => {
                    for i in 1..=n {
                        let mut args = SpawnArgs {
                            kind: InstanceKind::Body,
                            profile: "body".into(),
                            params: BTreeMap::new(),
                            program: self.home.join("bin/init-body").display().to_string(),
                            args: vec![],
                            env: self.body_env(&util::random_hex(8), "extra", "cold"),
                            cwd: self.home.join("work").display().to_string(),
                            lane: Some(format!("b{i}")),
                            replaces: None,
                        };
                        args.env.insert("INIT_BODY_ID".into(), format!("b{i}"));
                        if let Ok(r) =
                            self.request("spawn", icj::to_value(&args).unwrap_or(json!({})))
                            && let Ok(res) = r.result_as::<SpawnResult>()
                        {
                            let _ = self
                                .link
                                .take_fds(r.id.as_deref().unwrap_or(""), Duration::from_secs(5));
                            self.extras.push(res.instance);
                        }
                    }
                }
                Mode::BypassSoft(n) => {
                    let sock = self.egress_sock();
                    let model = std::env::var("INIT_TEST_FAKEKERNEL_MODEL")
                        .unwrap_or_else(|_| "deepseek-flash".into());
                    for i in 0..n {
                        let body = json!({"model": model, "messages": [{"role": "user", "content": format!("bypass {i}")}], "stream": true, "max_tokens": 64});
                        let req = MockRequest::post(
                            "/deepseek/chat/completions",
                            &[("content-type", "application/json")],
                            body.to_string(),
                        );
                        let r = client::request_unix(&sock, &req);
                        self.log(json!({"ev": "egress", "mode": "bypass_soft", "i": i, "status": r.as_ref().map(|x| x.status).ok(),
                            "error_code": r.as_ref().ok().and_then(|x| x.header("x-init-egress-error").map(str::to_string)),
                            "epoch": r.as_ref().ok().and_then(|x| x.header("x-egress-epoch").map(str::to_string))}));
                    }
                }
                Mode::Egress(host) => {
                    let sock = self.egress_sock();
                    for path in [
                        format!("/x/crates/{host}/"),
                        format!("/x/web/{host}/index.html"),
                    ] {
                        let r = client::request_unix(&sock, &MockRequest::get(&path));
                        self.log(json!({"ev": "egress", "mode": "egress", "path": path, "status": r.as_ref().map(|x| x.status).ok(),
                            "error_code": r.as_ref().ok().and_then(|x| x.header("x-init-egress-error").map(str::to_string))}));
                    }
                    let r = client::request_unix(
                        &sock,
                        &MockRequest {
                            method: "CONNECT".into(),
                            path: format!("{host}:443"),
                            ..MockRequest::default()
                        },
                    );
                    self.log(json!({"ev": "egress", "mode": "egress", "path": "CONNECT", "status": r.as_ref().map(|x| x.status).ok(),
                        "error_code": r.as_ref().ok().and_then(|x| x.header("x-init-egress-error").map(str::to_string))}));
                }
                Mode::IgnoreLevel => {
                    let swap = json!({"version": "0.0.1", "source_repo": self.home.join("lineage/kernel.git").display().to_string(),
                        "source_sha": "0".repeat(40), "init_kernel": util::sha256_hex(b"k"), "initctl": util::sha256_hex(b"c"), "report": {}});
                    let _ = self.request("system.swap", swap);
                    let _ =
                        self.request("sealed.family_eval", json!({"family": "f", "fake": true}));
                    let _ = self.request("sealed.holdout_rotate", json!({}));
                }
                _ => {}
            }
        }
    }

    fn egress_sock(&self) -> PathBuf {
        PathBuf::from(
            self.boot
                .as_ref()
                .map(|b| b.paths.egress_sock.clone())
                .unwrap_or_default(),
        )
    }
}

/// The body environment of kernel/supervise.md §4.1 (test mode: `INIT_TEST=1` plus every
/// `INIT_TEST_*` variable of this process, and `FAKEBODY_MODE`). Shared with the scenario runner,
/// which plays the kernel for body scenarios.
pub fn body_env(
    home: &std::path::Path,
    sha: &str,
    boot_id: &str,
    session: &str,
    reason: &str,
    mode: &str,
    body_id: &str,
) -> BTreeMap<String, String> {
    let h = home.display().to_string();
    let mut env: BTreeMap<String, String> = [
        ("INIT_HOME", h.clone()),
        ("INIT_SESSION", session.to_string()),
        ("INIT_SHA", sha.to_string()),
        ("INIT_BOOT_REASON", reason.to_string()),
        ("INIT_BODY_ID", body_id.to_string()),
        ("INIT_MODE", mode.to_string()),
        ("INIT_KERNEL_SOCK", format!("{h}/run/kernel.sock")),
        ("INIT_PROXY_SOCK", format!("{h}/run/proxy.sock")),
        ("INIT_BOOT_ID", boot_id.to_string()),
        ("INIT_UI", init_proto::UI_PROTO.into()),
        ("INIT_KERNEL_VERSION", VERSION.into()),
        (
            "PATH",
            format!("{h}/kernel/bin:/usr/bin:/bin:/usr/sbin:/sbin"),
        ),
        ("HOME", format!("{h}/work/home")),
        ("TMPDIR", format!("{h}/work/tmp/")),
        (
            "LANG",
            if cfg!(target_os = "macos") {
                "en_US.UTF-8".into()
            } else {
                "C.UTF-8".into()
            },
        ),
        ("TERM", "dumb".into()),
        ("NO_COLOR", "1".into()),
        ("GIT_AUTHOR_NAME", "INIT Agent".into()),
        ("GIT_COMMITTER_NAME", "INIT Agent".into()),
        ("GIT_AUTHOR_EMAIL", "agent@init.local".into()),
        ("GIT_COMMITTER_EMAIL", "agent@init.local".into()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_TERMINAL_PROMPT", "0".into()),
        ("INIT_TEST", "1".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    for (k, v) in std::env::vars() {
        if (k.starts_with("INIT_TEST_") && k.len() <= 64 && v.len() <= 256) || k == "FAKEBODY_MODE"
        {
            env.insert(k, v);
        }
    }
    env
}

fn link_from_env() -> Result<(UnixStream, UnixDatagram), String> {
    let fd = |k: &str, d: i32| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(d)
    };
    let (f3, f4) = (fd("INIT_ROOT_FD", 3), fd("INIT_ROOT_FDPASS", 4));
    for f in [f3, f4] {
        // SAFETY-free check: fcntl through sys reports EBADF for a missing fd.
        sys::set_cloexec(f, true).map_err(|e| format!("fd {f}: {e} (not started by a Root?)"))?;
    }
    // SAFETY: fds 3 and 4 are inherited from the Root for this purpose and owned by nobody else
    // in this process; we take ownership exactly once.
    let (s, d) = unsafe { (OwnedFd::from_raw_fd(f3), OwnedFd::from_raw_fd(f4)) };
    Ok((UnixStream::from(s), UnixDatagram::from(d)))
}

/// The `init-fakekernel` entry point; returns the exit code.
pub fn main() -> i32 {
    let modes = match parse_modes(
        &std::env::var("FAKEKERNEL_MODE")
            .or_else(|_| std::env::var("INIT_TEST_FAKEKERNEL_MODE"))
            .unwrap_or_default(),
    ) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("init-fakekernel: {e}");
            return 64;
        }
    };
    let Some(home) = std::env::var_os("INIT_HOME").map(PathBuf::from) else {
        eprintln!("init-fakekernel: INIT_HOME is not set");
        return 78;
    };
    if has(&modes, &Mode::IgnoreTerm) {
        guard::require_disposable_or_exit("init-fakekernel ignore_term");
    }
    let (stream, dgram) = match link_from_env() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("init-fakekernel: {e}");
            return 69;
        }
    };
    let _ = fs::create_dir_all(home.join("state/kernel"));
    let logf = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join("state/kernel/fakekernel.jsonl"))
        .ok();
    let log = Arc::new(Mutex::new(logf));
    let lg = log.clone();
    let trace: Trace = Arc::new(move |dir: &'static str, line: &[u8]| {
        let frame: Value = serde_json::from_slice(line).unwrap_or(Value::Null);
        if let Some(f) = lg.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let mut b =
                serde_json::to_vec(&json!({"dir": dir, "frame": frame, "link": true, "t": ts()}))
                    .unwrap_or_default();
            b.push(b'\n');
            let _ = f.write_all(&b);
        }
    });
    let (link, link_rx) = match Link::new(stream, dgram, "k-", Some(trace)) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("init-fakekernel: {e}");
            return 69;
        }
    };
    let (tx, rx) = mpsc::channel::<Msg>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(m) = link_rx.recv() {
                let eof = matches!(m, Incoming::Eof);
                if tx.send(Msg::Link(m)).is_err() || eof {
                    return;
                }
            }
        });
    }
    if has(&modes, &Mode::IgnoreTerm) {
        sys::ignore_sigterm();
        let mut cmd = std::process::Command::new("/bin/sleep");
        cmd.arg("3600")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        sys::new_session(&mut cmd);
        let _ = cmd.spawn();
    }
    let mut k = K {
        home: home.clone(),
        link: link.clone(),
        modes: modes.clone(),
        tx,
        log: Mutex::new(
            log.lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .and_then(|f| f.try_clone().ok()),
        ),
        boot: None,
        level: 0,
        kboot: util::random_hex(8),
        outbox: 0,
        body: None,
        extras: Vec::new(),
        boot_seq: 0,
        inbox: Vec::new(),
        qids: util::read_jsonl(&home.join("state/kernel/inbox.jsonl"))
            .iter()
            .filter_map(|v| v["qid"].as_str().map(str::to_string))
            .collect(),
        next_seq: util::read_jsonl(&home.join("state/kernel/inbox.jsonl"))
            .iter()
            .filter_map(|v| v["seq"].as_u64())
            .max()
            .unwrap_or(0),
        notices: Vec::new(),
        hb: Arc::new(Mutex::new(HbState {
            busy: false,
            slot: "cold_start".into(),
            mode: "normal".into(),
            sha12: "0".repeat(12),
        })),
        stopping: None,
        owner_stopped: false,
        crashes: Vec::new(),
        respawn_at: None,
        body_safe: false,
        config_exits: 0,
        sha: "0".repeat(40),
    };
    if has(&modes, &Mode::NoHello) {
        k.log(json!({"ev": "no_hello"}));
        loop {
            match rx.recv() {
                Ok(Msg::Link(Incoming::Eof)) | Err(_) => return 69,
                _ => {}
            }
        }
    }
    let hello = HelloArgs {
        kernel_version: VERSION.into(),
        proto: init_proto::ROOT_PROTO,
    };
    let boot = match link
        .request_typed("hello", &hello, REQ)
        .map(|r| r.result_as::<BootBlock>())
    {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            eprintln!("init-fakekernel: hello refused or malformed boot block: {e}");
            return 70;
        }
        Err(e) => {
            eprintln!("init-fakekernel: hello: {e}");
            return 69;
        }
    };
    k.level = boot.ladder.level;
    let safe = boot.boot.mode == ZoneMode::Safe;
    k.set_hb(|h| h.mode = if safe { "safe".into() } else { "normal".into() });
    let hb_every = Duration::from_secs(boot.limits.hb_interval_s.max(1));
    k.boot = Some(boot);
    k.log(json!({"ev": "hello_ok", "level": k.level, "safe": safe}));
    let _ = fault::point("fakekernel.after_hello");
    if let Some(c) = modes.iter().find_map(|m| {
        if let Mode::ExitAfterHello(c) = m {
            Some(*c)
        } else {
            None
        }
    }) {
        k.log(json!({"ev": "exit", "code": c}));
        return c;
    }
    let _ = k.request("instances", json!({}));
    // supervise.md §3.16: the kernel reads its level and ceilings at start.
    let _ = k.request("ledger.get", json!({}));
    let _ = k.request("ceilings.get", json!({}));
    let _ = fault::point("fakekernel.before_ready");
    if k.request("ready", json!({})).is_err() {
        return 69;
    }
    if !has(&modes, &Mode::NoHb) {
        let (hb, l) = (k.hb.clone(), link.clone());
        std::thread::spawn(move || {
            loop {
                let d = {
                    let h = hb.lock().unwrap_or_else(|e| e.into_inner());
                    HbData {
                        busy: h.busy,
                        mode: h.mode.clone(),
                        sha12: h.sha12.clone(),
                        slot: h.slot.clone(),
                    }
                };
                if l.event("hb", icj::to_value(&d).unwrap_or(json!({})))
                    .is_err()
                    || l.is_closed()
                {
                    return;
                }
                std::thread::sleep(hb_every);
            }
        });
    }
    if let Some(ms) = modes.iter().find_map(|m| {
        if let Mode::CrashAfterReady(ms) = m {
            Some(*ms)
        } else {
            None
        }
    }) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            eprintln!("init-fakekernel: crash_after_ready");
            std::process::exit(70);
        });
    }
    k.report_lkg();
    if !has(&modes, &Mode::NoBody) {
        k.spawn_body("cold");
    }
    k.hostile();
    loop {
        let msg = rx.recv_timeout(Duration::from_millis(50));
        match msg {
            Ok(Msg::Link(Incoming::Request(r))) => k.handle_request(r),
            Ok(Msg::Link(Incoming::Event(e))) => k.handle_event(e),
            Ok(Msg::Link(Incoming::Bad(e))) => k.log(json!({"ev": "bad_frame", "error": e})),
            Ok(Msg::Link(Incoming::Eof)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Link lost (supervise.md §3.13.1): stop the body, exit 69 within 10 s.
                k.log(json!({"ev": "link_lost"}));
                if k.body.is_some() {
                    k.to_body(&ToBody::Stop {
                        reason: "root_gone".into(),
                        grace_ms: 5_000,
                    });
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while Instant::now() < deadline {
                        match rx.recv_timeout(Duration::from_millis(50)) {
                            Ok(Msg::BodyEof(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                return 69;
            }
            Ok(Msg::Body(seq, f)) => k.on_body_frame(seq, f),
            Ok(Msg::BodyEof(seq)) => k.log(json!({"ev": "body_stdout_eof", "boot_seq": seq})),
            Ok(Msg::BodyReady(seq)) => {
                if let Some(b) = k.body.as_mut().filter(|b| b.boot_seq == seq) {
                    b.ready = true;
                    let (id, pid) = (b.boot_id.clone(), b.pid);
                    k.log(json!({"ev": "body_ready", "boot_seq": seq, "boot_id": id, "pid": pid}));
                    k.set_hb(|h| h.slot = "running".into());
                    k.deliver_pending();
                }
            }
            Ok(Msg::ReadyTimeout(seq)) => {
                if let Some(b) = k.body.as_ref().filter(|b| b.boot_seq == seq && !b.ready) {
                    let inst = b.instance.clone();
                    k.log(json!({"ev": "ready_timeout", "boot_seq": seq}));
                    let _ = k.request(
                        "kill",
                        icj::to_value(&KillArgs {
                            instance: inst,
                            mode: KillMode::Kill,
                        })
                        .unwrap_or(json!({})),
                    );
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Some(s) = k.stopping.as_mut()
            && Instant::now() >= s.deadline
            && !s.killed
        {
            s.killed = true;
            s.deadline = Instant::now() + Duration::from_secs(5);
            if let Some(inst) = k.body.as_ref().map(|b| b.instance.clone()) {
                let _ = k.request(
                    "kill",
                    icj::to_value(&KillArgs {
                        instance: inst,
                        mode: KillMode::Kill,
                    })
                    .unwrap_or(json!({})),
                );
            } else {
                k.finish_stop();
            }
        } else if k
            .stopping
            .as_ref()
            .is_some_and(|s| s.killed && Instant::now() >= s.deadline)
        {
            k.finish_stop();
        }
        if let Some(t) = k.respawn_at
            && Instant::now() >= t
            && k.stopping.is_none()
            && !k.body_safe
        {
            k.respawn_at = None;
            k.spawn_body("crash");
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
            parse_modes("bypass_soft,extra_bodies:2,egress:example.com,exit_after_hello:78")
                .unwrap(),
            vec![
                Mode::BypassSoft(3),
                Mode::ExtraBodies(2),
                Mode::Egress("example.com".into()),
                Mode::ExitAfterHello(78)
            ]
        );
        assert!(parse_modes("extra_bodies").is_err());
        assert!(parse_modes("egress:").is_err());
        assert!(parse_modes("nope").is_err());
    }
}
