//! ADR-011 §1.2 (lane X8): the Root conformance probe of a kernel change, run at `change.open{scope:"k"}` (a
//! `sentinel::Hooks.admit` hook, before X4's snapshot) and so before any swap. A refusal opens nothing.
//!
//! In a Root-owned probe root (a sealed run root: the sealed profile, no network, no `INIT_HOME`, no `ROOT_HOME` outside
//! it, removed by its exact path) laid out as a kernel home from `INIT_HOME/kernel/kupgrade-smoke/` (Root-written: the
//! fixture `home/` tree and `config.toml`), the candidate `init-kernel` (from the store) runs twice under a probe Root
//! that speaks the real control link (fds 3 and 4, root.md §4.1) and answers `hello` with `boot.reason =
//! "kupgrade_smoke"` and `boot.probe = {kind, body_mode:"check"}`. The candidate must:
//! - run 1: send `hello` with proto 1 within 30 s; become `ready`; ask `ceilings.get`; `spawn` the LKG body in mode
//!   `check` (it must write `ready` and exit 0 within 90 s, the A4 rule); then on `stop` exit 0 within `stop_grace_s`;
//! - run 2: `hello`, `ready`; on `prepare_swap` answer `ok` and exit 75 within 10 s.
//!
//! Heartbeats are due every 30 s from `hello` (`hb_stale_s`). Any other request is answered as a probe Root would
//! (`store.put` is hashed and discarded; `lineage.append` is accepted and kept nowhere; unknown commands get
//! `root.unknown_cmd`). The probe is $0 and takes seconds to a few minutes. It proves the candidate can be supervised,
//! stopped and swapped by the real protocol; it trusts nothing the candidate says about itself.

use crate::link::{Root, lock};
use crate::sealed::{Env, RunRoot};
use crate::util::{self, icj, now_ms, rfc3339, write_atomic};
use crate::zone::Err3;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The boot reason a conforming kernel recognises: run the smoke steps, nothing else.
pub const REASON: &str = "kupgrade_smoke";
pub const HELLO: Duration = Duration::from_secs(30);
/// From `hello`: a probe home has no LKG body to rebuild.
pub const READY: Duration = Duration::from_secs(300);
/// From `ready`: `ceilings.get` and the check body's spawn and exit.
pub const STEPS: Duration = Duration::from_secs(120);
/// The check body's own bound (A4's arm rule).
pub const BODY: Duration = Duration::from_secs(90);
pub const SWAP_EXIT: Duration = Duration::from_secs(10);
pub const HB_STALE: Duration = Duration::from_secs(30);
/// One kernel run at most.
pub const WALL: Duration = Duration::from_secs(600);
/// The smoke fixture's `home/` tree copied into the probe home: at most this much.
const HOME_MAX: u64 = 8 << 20;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Stop,
    Swap,
}

#[derive(Clone, Debug, Default, PartialEq)]
enum Body {
    #[default]
    None,
    Running(i32),
    Done(Option<i32>, bool),
}

/// What the candidate did so far in one run.
#[derive(Default)]
struct Seen {
    hello: Option<Instant>,
    ready: Option<Instant>,
    hb: Option<Instant>,
    ceilings: bool,
    body: Body,
    bodies: u32,
    stop: Option<(String, Instant)>,
    prep: Option<(String, Instant)>,
    prep_ok: bool,
    bad: Option<String>,
    next: u64,
    appends: u64,
}

/// The probe Root's end of one candidate's control link.
struct Link {
    w: Mutex<UnixStream>,
    fdc: UnixDatagram,
    seen: Mutex<Seen>,
    env: Arc<Env>,
    run: Arc<RunRoot>,
    boot: Value,
    lkg_sha: String,
    /// The run ended: a check body still running is killed by its own waiter (the one holder of its unreaped `Child`).
    done: std::sync::atomic::AtomicBool,
}

impl Link {
    fn line(&self, v: &Value) {
        if let Ok(s) = icj(v) {
            let _ = lock(&self.w).write_all(format!("{s}\n").as_bytes());
        }
    }
    fn request(&self, s: &mut Seen, cmd: &str, args: Value) -> String {
        s.next += 1;
        let id = format!("p-{}", s.next);
        self.line(&json!({"args": args, "cmd": cmd, "id": id, "v": 1}));
        id
    }
    fn bad(&self, why: String) {
        lock(&self.seen).bad.get_or_insert(why);
    }
    /// The fds of the datagram tagged `id` (sent before the request line); others are dropped.
    fn take_fds(&self, id: &str) -> Result<Vec<OwnedFd>, Err3> {
        let _ = self.fdc.set_read_timeout(Some(Duration::from_secs(5)));
        loop {
            let mut buf = [0u8; 256];
            let (n, fds) = crate::sys::recv_fds(self.fdc.as_raw_fd(), &mut buf).map_err(|e| ("root.args", format!("no fds for {id}: {e}"), Value::Null))?;
            let tag: Value = serde_json::from_slice(&buf[..n]).unwrap_or_default();
            if tag["id"] == id && tag["n"].as_u64() == Some(fds.len() as u64) {
                return Ok(fds);
            }
        }
    }
}

fn no(code: &'static str, m: impl Into<String>) -> Err3 {
    (code, m.into(), Value::Null)
}

/// The `hello` result of the probe Root (root.md §4.1.3, with `reason:"kupgrade_smoke"` and the `probe` block).
fn boot_block(kernel: &Value, lkg: &Value, level: i64, charter: &str, run: &Path) -> Value {
    json!({"access": {"access": "none", "full": false, "workspace": null}, "extend": {"mode": "free"},
        "boot": {"cause": null, "kstate": null, "level": level, "mode": "normal", "panic": null, "reason": REASON,
            "tuple": {"audited": null, "current": "probe", "lkg": "probe", "settled": null, "twin": null},
            "tuple_body": {"binary": lkg["body"]["binary"], "ext": lkg["ext"], "sha": lkg["body"]["sha"]}, "tuple_kernel": kernel},
        "charter_sha256": charter, "ladder": {"cap": level, "evolution_share_max_permille": null, "level": level},
        "limits": {"hb_interval_s": 5, "max_instances": 4, "parallel_bodies": 1},
        "paths": {"egress_sock": run.join("run/egress.sock"), "kernel_git": run.join("lineage/kernel.git")},
        "probe": {"body_mode": "check", "kind": "kupgrade-smoke"},
        "root": {"proto": 1, "version": crate::install::VERSION}})
}

/// One candidate request, answered as a probe Root.
fn handle(l: &Arc<Link>, id: &str, cmd: &str, a: &Value) -> Result<Value, Err3> {
    match cmd {
        "hello" if a["proto"] != 1 => {
            l.bad("hello: proto is not 1".into());
            Err(no("root.version", "unsupported proto"))
        }
        "hello" => {
            let mut s = lock(&l.seen);
            s.hello.get_or_insert_with(Instant::now);
            Ok(l.boot.clone())
        }
        "ready" => {
            lock(&l.seen).ready.get_or_insert_with(Instant::now);
            Ok(json!({}))
        }
        "ceilings.get" => {
            lock(&l.seen).ceilings = true;
            let level = l.boot["boot"]["level"].clone();
            Ok(json!({"extend": {"mode": "free"}, "instances": {"active": 0, "limit": 4}, "level": level, "parallel_bodies": {"active": 0, "limit": 1}, "v": 1}))
        }
        "spawn" => spawn_body(l, id, a),
        "instances" => Ok(json!([])),
        "tuple.get" => Ok(json!({"audited": null, "current": {"id": "probe", "tuple": null}, "lkg": {"id": "probe", "tuple": null}})),
        "ledger.get" => Ok(json!({"level": l.boot["boot"]["level"], "v": 1})),
        "lineage.append" => {
            let mut s = lock(&l.seen);
            s.appends += 1;
            Ok(json!({"dup": false, "hash": format!("sha256:{}", "0".repeat(64)), "seq": s.appends}))
        }
        "store.put" => {
            let mut f = File::from(l.take_fds(id)?.pop().ok_or_else(|| no("root.args", "store.put carries one fd"))?);
            let (mut h, mut buf, mut n) = (Vec::new(), vec![0u8; 1 << 16], 0u64);
            loop {
                let k = f.read(&mut buf).map_err(|e| no("root.store", e.to_string()))?;
                if k == 0 {
                    break;
                }
                n += k as u64;
                if n > 64 << 20 {
                    return Err(no("root.store", "the probe keeps no blob over 64 MiB"));
                }
                h.extend_from_slice(&buf[..k]);
            }
            let sha = util::sha256_hex(&h);
            match a["sha256"].as_str() {
                Some(w) if w != sha => Err(no("root.store", format!("hash mismatch: the bytes hash to {sha}"))),
                _ => Ok(json!({"sha256": sha})), // hashed and discarded: nothing a probe sees is kept
            }
        }
        "alert" => Ok(json!({})),
        c => Err(no("root.unknown_cmd", format!("command {c:?} is not served by the conformance probe"))),
    }
}

/// Keep the candidate's boot configuration (especially `model`, required even in check mode),
/// while replacing all run identity and recovery fields with the Root-owned probe values.
fn body_boot(mut template: Value, home: &Path, boot_id: &str, session: &str, sha: &str) -> Value {
    if !template.is_object() {
        template = json!({});
    }
    let identity = json!({"body_id": "kprobe", "boot_id": boot_id, "boot_seq": 0, "branch": null, "cause": null, "crash": null,
        "from_sha": null, "mode": "check", "provisional": null, "reason": "cold", "safe": null,
        "session": session, "sha": sha, "spawned_at": rfc3339(now_ms()), "update_failed": null, "v": 1});
    template.as_object_mut().unwrap().extend(identity.as_object().unwrap().clone());
    if !template["env"].is_object() {
        template["env"] = json!({});
    }
    template["env"]["init_home"] = json!(home);
    // Match the environment supplied to the check body below.
    template["env"]["kernel_version"] = json!("0.1.0");
    template
}

/// `spawn` in the probe: only the LKG body in mode `check` (`env.INIT_MODE`), one at a time, as A4 runs its arms; its
/// stdio goes to the candidate as `[stdin_w, stdout_r, stderr_r]` and its end as an `exited` event.
fn spawn_body(l: &Arc<Link>, id: &str, a: &Value) -> Result<Value, Err3> {
    if a["kind"] != "body" || a["env"]["INIT_MODE"] != "check" {
        l.bad(format!("spawn: the probe wants the LKG body in mode check, got kind {} with INIT_MODE {}", a["kind"], a["env"]["INIT_MODE"]));
        return Err(no("root.args", "the conformance probe spawns only the LKG body, in mode check (INIT_MODE=check)"));
    }
    if matches!(lock(&l.seen).body, Body::Running(_)) {
        return Err(no("root.quota", "one body at a time in the probe"));
    }
    let r = &l.run.dir;
    let int = |m: String| no("root.internal", m);
    let rnd = |n: usize| util::random::<8>().map(|b| util::hex(&b)[..n].to_string()).map_err(|e| int(e.to_string()));
    let (boot_id, session) = (rnd(16)?, format!("kp-{}", rnd(12)?));
    let bj = r.join("state/kernel/boot.json");
    let read_boot = |p: &Path| fs::read(p).ok().and_then(|x| serde_json::from_slice::<Value>(&x).ok()).filter(Value::is_object);
    let template = read_boot(&bj).or_else(|| read_boot(&l.env.ih.join("state/kernel/boot.json"))).unwrap_or(Value::Null);
    let b = body_boot(template, r, &boot_id, &session, &l.lkg_sha);
    write_atomic(&bj, format!("{b}\n").as_bytes(), 0o600).map_err(|e| int(format!("boot.json: {e}")))?;
    let ready = r.join("state/run/ready");
    crate::snapshot::remove(&ready).map_err(|e| int(e.to_string()))?;
    let lim = crate::memwatch::Limits::of(&l.env.ch);
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let mut c = crate::sealed::sandboxed(&l.env, &l.run, None, &[r.join("bin/init-body").into()]);
    c.envs([("INIT_HOME", s(r)), ("INIT_SESSION", session), ("INIT_SHA", l.lkg_sha.clone()), ("INIT_BOOT_ID", boot_id.clone()),
        ("INIT_BOOT_REASON", "cold".into()), ("INIT_BODY_ID", "kprobe".into()), ("INIT_MODE", "check".into()),
        ("INIT_UI", "stdio-jsonl/1".into()), ("INIT_KERNEL_VERSION", "0.1.0".into()), ("INIT_KERNEL_SOCK", s(&r.join("run/kernel.sock"))),
        ("HOME", s(&r.join("work/home"))), ("TMPDIR", format!("{}/", s(&r.join("work/tmp"))))])
        .envs(crate::memwatch::body_env(&lim, &l.run.events_path())).current_dir(r.join("work"))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| int(format!("spawn the check body: {e}")))?;
    let pid = child.id() as i32;
    let fds: Vec<OwnedFd> = match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
        (Some(i), Some(o), Some(e)) => vec![i.into(), o.into(), e.into()],
        _ => vec![],
    };
    {
        let mut st = lock(&l.seen);
        st.body = Body::Running(pid);
        st.bodies += 1;
    }
    let lk = l.clone();
    let (inst, bid) = (format!("kp-body-{pid}"), boot_id);
    let i2 = inst.clone();
    std::thread::spawn(move || {
        let t0 = Instant::now();
        let code = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st.code(),
                Ok(None) if t0.elapsed() > BODY || lk.done.load(std::sync::atomic::Ordering::SeqCst) => {
                    // R-SAFE: `pid` is this waiter's own unreaped child (it holds the `Child` and reaps only below), the
                    // leader of the process group the probe gave it; `sys::signal` re-checks that it is held
                    crate::sys::signal(pid, libc::SIGKILL, true);
                    break child.wait().ok().and_then(|s| s.code());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(_) => break None,
            }
        };
        let rd: Value = fs::read(lk.run.dir.join("state/run/ready")).ok().and_then(|x| serde_json::from_slice(&x).ok()).unwrap_or_default();
        let wrote = rd["boot_id"] == bid.as_str() && rd["mode"] == "check";
        lock(&lk.seen).body = Body::Done(code, wrote);
        lk.line(&json!({"data": {"at": rfc3339(now_ms()), "code": code, "instance": i2, "rusage_ms": 0, "signal": null}, "ev": "exited", "v": 1}));
    });
    let tag = icj(&json!({"id": id, "n": fds.len()})).unwrap_or_default();
    crate::sys::send_fds(l.fdc.as_raw_fd(), tag.as_bytes(), &fds.iter().map(AsRawFd::as_raw_fd).collect::<Vec<_>>())
        .map_err(|e| int(format!("fd hand-over: {e}")))?;
    Ok(json!({"instance": inst, "pid": pid, "started_at": rfc3339(now_ms())})) // `fds` drop here: the probe keeps no copy
}

/// The control-link reader of one run (root.md §4.1.1's limits: three malformed frames in a row, a line over 1 MiB).
fn serve(l: Arc<Link>, s: UnixStream) {
    let mut rd = BufReader::new(s);
    let mut bad = 0;
    loop {
        let mut buf = Vec::new();
        match (&mut rd).take((1 << 20) + 1).read_until(b'\n', &mut buf) {
            Ok(n) if n > 0 && buf.ends_with(b"\n") => {}
            Ok(n) if n > 1 << 20 => return l.bad("a control line over 1 MiB".into()),
            _ => return,
        }
        let Some(v) = serde_json::from_slice::<Value>(&buf).ok().filter(|v| v["v"] == 1) else {
            bad += 1;
            if bad >= 3 {
                return l.bad("three malformed control frames in a row".into());
            }
            continue;
        };
        bad = 0;
        let id = v["id"].as_str().unwrap_or("").to_string();
        if let Some(cmd) = v["cmd"].as_str() {
            let r = handle(&l, &id, cmd, &v["args"]);
            l.line(&match r {
                Ok(res) => json!({"id": id, "ok": true, "result": res, "v": 1}),
                Err((c, m, d)) => json!({"error": {"code": c, "data": d, "details": [], "message": m}, "id": id, "ok": false, "v": 1}),
            });
        } else if v["ev"] == "hb" {
            lock(&l.seen).hb = Some(Instant::now());
        } else if v.get("ev").is_none() {
            let mut s = lock(&l.seen);
            if s.prep.as_ref().is_some_and(|(r, _)| *r == id) {
                s.prep_ok = v["ok"] == true;
            }
        }
    }
}

/// Polled by `sealed::supervise` every 25 ms: the next step, or why the run ends (a step name).
fn drive(l: &Link, phase: Phase, t0: Instant, grace: Duration) -> Option<&'static str> {
    let mut s = lock(&l.seen);
    if s.bad.is_some() {
        return Some("protocol");
    }
    let now = Instant::now();
    let Some(h) = s.hello else { return (t0.elapsed() > HELLO).then_some("hello") };
    if now.duration_since(s.hb.unwrap_or(h)) > HB_STALE {
        return Some("heartbeat");
    }
    let Some(r) = s.ready else { return (now.duration_since(h) > READY).then_some("ready") };
    match phase {
        Phase::Stop => {
            if let Some((_, at)) = s.stop {
                return (now.duration_since(at) > grace).then_some("stop");
            }
            match s.body {
                Body::Done(code, wrote) if s.ceilings => {
                    if code != Some(0) || !wrote {
                        return Some("body");
                    }
                    let id = l.request(&mut s, "stop", json!({"grace_ms": grace.as_millis() as u64, "reason": "kupgrade_smoke"}));
                    s.stop = Some((id, now));
                    None
                }
                _ if now.duration_since(r) > STEPS => Some(if s.ceilings { "body" } else { "ceilings" }),
                _ => None,
            }
        }
        Phase::Swap => {
            if let Some((_, at)) = s.prep {
                return (now.duration_since(at) > SWAP_EXIT).then_some("prepare_swap");
            }
            if matches!(s.body, Body::Running(_)) && now.duration_since(r) < STEPS {
                return None; // a check body of this run finishes first
            }
            let id = l.request(&mut s, "prepare_swap", json!({"deadline_s": SWAP_EXIT.as_secs(), "job": "kprobe"}));
            s.prep = Some((id, now));
            None
        }
    }
}

/// The first step the candidate had not passed when a run ended.
fn step_of(s: &Seen, phase: Phase) -> &'static str {
    match (s.hello, s.ready, phase) {
        (None, _, _) => "hello",
        (_, None, _) => "ready",
        (_, _, Phase::Stop) if !s.ceilings => "ceilings",
        (_, _, Phase::Stop) if !matches!(s.body, Body::Done(Some(0), true)) => "body",
        (_, _, Phase::Stop) => "stop",
        (_, _, Phase::Swap) => "prepare_swap",
    }
}

/// One run of the candidate kernel in the probe root. Ok: the run's timings; Err: (step, why, exit code).
fn run_kernel(l: Arc<Link>, ctl: UnixStream, fds: [OwnedFd; 4], phase: Phase, grace: Duration, test_env: &[(String, String)])
    -> Result<Value, (&'static str, String, Option<i32>)> {
    let r = l.run.dir.clone();
    let log = File::create(r.join(format!("out/kernel-{}.log", if phase == Phase::Stop { 1 } else { 2 }))).map_err(|e| ("probe", e.to_string(), None))?;
    let argv: [OsString; 3] = [r.join("kernel/bin/init-kernel").into(), "supervise".into(), "--under-root".into()];
    let mut c = crate::sealed::sandboxed(&l.env, &l.run, None, &argv);
    let s = |p: PathBuf| p.to_string_lossy().into_owned();
    c.envs([("INIT_HOME", s(r.clone())), ("INIT_ROOT_FD", "3".into()), ("INIT_ROOT_FDPASS", "4".into()),
        ("HOME", s(r.join("cache/kernel-home"))), ("TMPDIR", format!("{}/", s(r.join("cache/tmp"))))])
        .envs(test_env.iter().map(|(k, v)| (k.as_str(), v.as_str()))).current_dir(&r);
    if let Ok(l2) = log.try_clone() {
        c.stdout(l2);
    }
    c.stderr(log);
    crate::sys::map_fds(&mut c, [fds[0].as_raw_fd(), fds[1].as_raw_fd()]);
    let l2 = l.clone();
    std::thread::spawn(move || serve(l2, ctl));
    let lim = crate::memwatch::Limits::of(&l.env.ch);
    let mut keep = Some(fds); // the parent's copies of the kernel's ends, closed once it runs (so its exit is our EOF)
    let t0 = Instant::now();
    let out = crate::sealed::supervise(c, &l.run, &lim, WALL, None, || {
        keep.take();
        drive(&l, phase, t0, grace)
    });
    drop(keep);
    let (code, why) = match out {
        Ok((code, why, _child)) => (code, why),
        Err(m) => return Err(("probe", m, None)),
    };
    l.done.store(true, std::sync::atomic::Ordering::SeqCst); // a check body this run left behind: its waiter kills it
    let s = lock(&l.seen);
    let since = |t: Option<Instant>| t.map(|t| t.duration_since(t0).as_millis() as u64);
    let ok = match phase {
        Phase::Stop => why == "done" && code == Some(0) && s.stop.is_some(),
        Phase::Swap => why == "done" && code == Some(75) && s.prep_ok && s.prep.as_ref().is_some_and(|(_, at)| at.elapsed() <= SWAP_EXIT + Duration::from_secs(1)),
    };
    if ok {
        return Ok(json!({"hello_ms": since(s.hello), "ms": t0.elapsed().as_millis() as u64, "ready_ms": since(s.ready)}));
    }
    let step = step_of(&s, phase);
    let what = match (&s.bad, why, code) {
        (Some(b), _, _) => b.clone(),
        (None, "done" | "crash", c) if phase == Phase::Swap && s.prep.is_some() && !s.prep_ok => format!("exited {c:?} without answering prepare_swap ok"),
        (None, "done" | "crash", c) if phase == Phase::Swap && s.prep.is_some() => format!("exited {c:?} on prepare_swap (want 75)"),
        (None, "done" | "crash", c) if phase == Phase::Stop && s.stop.is_some() => format!("exited {c:?} on stop (want 0)"),
        (None, "done" | "crash", c) => format!("exited {c:?} before the step"),
        (None, w, _) => format!("{w}: not done in time"),
    };
    Err((step, what, code))
}

/// Copies the smoke fixture's `home/` tree (regular files and directories; links are skipped) into `to`, bounded.
fn copy_home(from: &Path, to: &Path, budget: &mut u64) -> Result<(), String> {
    let Ok(rd) = fs::read_dir(from) else { return Ok(()) };
    for e in rd.flatten() {
        let name = e.file_name();
        if name.to_string_lossy().starts_with("..") {
            continue;
        }
        let (src, dst) = (e.path(), to.join(&name));
        let m = fs::symlink_metadata(&src).map_err(|e| e.to_string())?;
        if m.is_dir() {
            fs::DirBuilder::new().recursive(true).mode(0o700).create(&dst).map_err(|e| e.to_string())?;
            copy_home(&src, &dst, budget)?;
        } else if m.is_file() {
            *budget = budget.checked_sub(m.len()).ok_or("the kupgrade-smoke home is over 8 MiB")?;
            fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", src.display()))?;
        }
    }
    Ok(())
}

fn blob_to(rh: &Path, sha: &str, dst: &Path, mode: u32) -> Result<(), String> {
    let src = crate::snapshot::store_blob(rh, sha)?;
    fs::copy(&src, dst).and_then(|_| fs::set_permissions(dst, fs::Permissions::from_mode(mode))).map(|_| ()).map_err(|e| format!("{}: {e}", dst.display()))
}

/// The probe home: the smoke fixture, the candidate kernel, the LKG body (binary, T0, ext set).
fn lay_out(env: &Env, run: &RunRoot, kernel: &Value, lkg: &Value) -> Result<(), Err3> {
    let r = &run.dir;
    let int = |m: String| no("root.internal", format!("probe home: {m}"));
    for d in ["kernel/bin", "bin/ext", "body", "state/kernel", "state/sessions", "state/run", "work/tmp", "work/home", "run", "lineage", "vault",
        "cache/kernel-home", "cache/tmp"] {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(r.join(d)).map_err(|e| int(e.to_string()))?;
    }
    let smoke = env.ih.join("kernel/kupgrade-smoke");
    let mut budget = HOME_MAX;
    copy_home(&smoke.join("home"), r, &mut budget).map_err(int)?;
    for (src, dst) in [(smoke.join("config.toml"), r.join("kernel/config.toml")), (env.ih.join("kernel/config.toml"), r.join("kernel/config.toml")),
        (env.ih.join("kernel/CONSTITUTION.md"), r.join("kernel/CONSTITUTION.md"))] {
        if !dst.exists() && fs::symlink_metadata(&src).is_ok_and(|m| m.is_file()) {
            fs::copy(&src, &dst).map_err(|e| int(e.to_string()))?;
        }
    }
    let store = |m: String| no("root.store", format!("probe: {m}"));
    blob_to(&env.rh, kernel["init_kernel"].as_str().unwrap_or(""), &r.join("kernel/bin/init-kernel"), 0o500).map_err(store)?;
    if let Some(c) = kernel["initctl"].as_str().filter(|c| !c.is_empty()) {
        blob_to(&env.rh, c, &r.join("kernel/bin/initctl"), 0o500).map_err(store)?;
    }
    let arm = crate::snapshot::arm_of(lkg).map_err(|m| no("extend.kernel_probe", format!("the LKG tuple has no body to probe with ({m})")))?;
    blob_to(&env.rh, &arm.binary, &r.join("bin/init-body"), 0o500).map_err(store)?;
    for (name, sha) in &arm.ext {
        blob_to(&env.rh, sha, &r.join("bin/ext").join(format!("{name}.wasm")), 0o400).map_err(store)?;
    }
    let _ = crate::snapshot::place_t0(&env.ih, &env.rh, r, &arm); // no T0 (a seedless home): the body's own exit decides
    Ok(())
}

/// The conformance probe of `kernel` (a tuple's kernel half) with the LKG tuple's body. Ok: both runs' timings.
pub fn probe(env: Arc<Env>, kernel: &Value, lkg: &Value, charter: &str, grace: Duration) -> Result<Value, Err3> {
    let run = Arc::new(RunRoot::new(&env.rh).map_err(|m| no("root.internal", m))?);
    lay_out(&env, &run, kernel, lkg)?;
    let test_env: Vec<(String, String)> = if env.ch.root.test_mode {
        std::env::vars().filter(|(k, _)| k == "INIT_TEST" || k.starts_with("INIT_TEST_") || k == "INIT_FAULT").collect()
    } else {
        Vec::new()
    };
    let level = env.level;
    let boot = boot_block(kernel, lkg, level, charter, &run.dir);
    let lkg_sha = lkg["body"]["sha"].as_str().unwrap_or("").to_string();
    let mut out = json!({});
    for phase in [Phase::Stop, Phase::Swap] {
        let int = |e: std::io::Error| no("root.internal", format!("probe link: {e}"));
        let (ctl, ctl_k) = UnixStream::pair().map_err(int)?;
        let (fdc, fdc_k) = UnixDatagram::pair().map_err(int)?;
        let (a, b) = (crate::sys::dup_high(&ctl_k).map_err(int)?, crate::sys::dup_high(&fdc_k).map_err(int)?);
        let l = Arc::new(Link { w: Mutex::new(ctl.try_clone().map_err(int)?), fdc, seen: Mutex::new(Seen::default()), env: env.clone(),
            run: run.clone(), boot: boot.clone(), lkg_sha: lkg_sha.clone(), done: Default::default() });
        let name = if phase == Phase::Stop { "stop" } else { "swap" };
        match run_kernel(l, ctl, [a, b, ctl_k.into(), fdc_k.into()], phase, grace, &test_env) {
            Ok(v) => out[name] = v,
            Err((step, why, code)) => {
                let m = format!("the candidate kernel fails the Root conformance probe at {step}: {why} (ADR-011 §1.2)");
                return Err(("extend.kernel_probe", m, json!({"exit": code, "step": step, "why": why})));
            }
        }
    }
    Ok(out)
}

/// `sentinel::Hooks.admit` for `scope:"k"`: the candidate kernel half is the agent's (`source:"agent"`), then the probe
/// on the LKG body. Ok: `{"K": "pass"}` (the change's `checks.K`); every other scope: `{}`.
pub fn admit(root: &Root, a: &Value, _first: bool) -> Result<Value, Err3> {
    if a["scope"] != "k" {
        return Ok(json!({}));
    }
    let k = &a["candidate"]["kernel"];
    let text = |v: &Value, max: usize| v.as_str().is_some_and(|s| s.len() <= max && s.bytes().all(|c| c.is_ascii_graphic()));
    if k["source"] != "agent" || !text(&k["version"], 64) || !text(&k["built_from"], 64) {
        return Err(no("root.args", "candidate.kernel: {built_from, init_kernel, initctl, source:\"agent\", version} (printable, ≤ 64 bytes)"));
    }
    let lkg = lock(&root.sup.ptrs).lkg.clone().and_then(|id| crate::tuple::load(&root.p.rh, &id))
        .ok_or_else(|| no("root.store", "the conformance probe needs the LKG tuple"))?;
    let env = Arc::new(Env::of(root).map_err(|m| no("root.internal", m))?);
    let Some(_g) = crate::sealed::try_begin(&root.p.rh) else {
        return Err(("root.busy", "a sealed operation is running; retry the kernel change after it".into(), json!({"reason": "sealed"})));
    };
    let grace = Duration::from_secs(super::cfg(root).stop_grace_s.max(1) as u64);
    let charter = lock(&root.st).charter.as_ref().map(|l| l.digest.clone()).unwrap_or_default();
    let r = probe(env, k, &lkg, &charter, grace)?;
    util::log(&root.p.rh, "info", &format!("kernel conformance probe passed: {}", icj(&r).unwrap_or_default()));
    Ok(json!({"K": "pass"}))
}

/// Registers the probe as an admission hook; `main` calls it after X6's and before X4's (the snapshot comes last).
pub fn register() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| lock(crate::sentinel::hooks()).admit.push(admit));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_boot_preserves_model_and_replaces_live_identity() {
        let model = json!({"provider": "deepseek", "model": "deepseek-flash", "effort": "high"});
        let template = json!({"model": model, "body_id": "live", "boot_id": "old", "session": "old",
            "sha": "old", "mode": "normal", "reason": "rollback", "provisional": {"depth": 1},
            "branch": "old", "cause": {}, "crash": {}, "safe": {}, "from_sha": "old", "update_failed": {},
            "env": {"init_home": "/live", "kernel_version": "old", "model": {"context_window": 128000}}});
        let b = body_boot(template, Path::new("/probe"), "0123456789abcdef", "kp-session", "lkg");
        assert_eq!(b["model"], model);
        assert_eq!(b["env"]["model"]["context_window"], 128000);
        assert_eq!(b["env"]["init_home"], "/probe");
        assert_eq!(b["env"]["kernel_version"], "0.1.0");
        assert_eq!(b["boot_id"], "0123456789abcdef");
        assert_eq!(b["body_id"], "kprobe");
        assert_eq!(b["session"], "kp-session");
        assert_eq!(b["sha"], "lkg");
        assert_eq!(b["mode"], "check");
        assert_eq!(b["reason"], "cold");
        for field in ["provisional", "branch", "cause", "crash", "safe", "from_sha", "update_failed"] {
            assert!(b[field].is_null(), "{field}");
        }
    }

    #[test]
    fn check_boot_handles_missing_or_malformed_optional_env_without_panicking() {
        for template in [Value::Null, json!([]), json!({"env": "invalid"}), json!({"env": null})] {
            let b = body_boot(template, Path::new("/probe"), "0123456789abcdef", "kp-session", "lkg");
            assert_eq!(b["env"]["init_home"], "/probe");
            assert_eq!(b["mode"], "check");
            // Do not invent a provider/model when neither the candidate nor the live boot has one.
            assert!(b["model"].is_null());
        }
    }

    #[test]
    fn the_failing_step_is_the_first_one_not_passed() {
        let mut s = Seen::default();
        assert_eq!(step_of(&s, Phase::Stop), "hello");
        s.hello = Some(Instant::now());
        assert_eq!(step_of(&s, Phase::Swap), "ready");
        s.ready = Some(Instant::now());
        assert_eq!(step_of(&s, Phase::Stop), "ceilings");
        s.ceilings = true;
        s.body = Body::Done(Some(0), false);
        assert_eq!(step_of(&s, Phase::Stop), "body", "a body that wrote no ready in mode check");
        s.body = Body::Done(Some(0), true);
        assert_eq!(step_of(&s, Phase::Stop), "stop");
        assert_eq!(step_of(&s, Phase::Swap), "prepare_swap");
    }

    #[test]
    fn the_boot_block_names_the_smoke_reason_and_the_candidate() {
        let lkg = json!({"body": {"binary": "b", "sha": "c"}, "ext": {"components": [], "set": null}});
        let b = boot_block(&json!({"init_kernel": "k", "source": "agent"}), &lkg, 1, "d1", Path::new("/r"));
        assert_eq!((b["boot"]["reason"].as_str(), b["probe"]["body_mode"].as_str()), (Some(REASON), Some("check")));
        assert_eq!(b["boot"]["tuple_kernel"]["init_kernel"], "k");
        assert_eq!(b["boot"]["tuple_body"]["binary"], "b");
        assert_eq!((b["root"]["proto"].as_i64(), b["charter_sha256"].as_str()), (Some(1), Some("d1")), "init-proto's BootBlock needs a string");
    }
}
