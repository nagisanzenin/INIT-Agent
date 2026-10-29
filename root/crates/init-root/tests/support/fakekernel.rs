//! Test support (WP21): a fake `init-kernel supervise --under-root`.
//!
//! The Root spawns it inside the macOS `kernel` profile. It speaks the control link of
//! docs/root/root.md §4.1: ICJ lines on fd 3 (`INIT_ROOT_FD`), SCM_RIGHTS datagrams on fd 4
//! (`INIT_ROOT_FDPASS`). `INIT_TEST_FK` (comma-separated modes) and the files `$INIT_HOME/work/fk-*`
//! steer it; every step is logged as one JSON line to `$INIT_HOME/state/kernel/fk.jsonl`.
//! It never signals any process and exits on its own on EOF of fd 3.
//!
//! X8 (ADR-011 §1.2): under the Root conformance probe (`boot.reason = "kupgrade_smoke"`) it runs the smoke steps
//! (`ceilings.get`, the LKG body `$INIT_HOME/bin/init-body` in mode `check`) and answers `stop` and `prepare_swap`. The
//! kernel version it was admitted as (`boot.tuple_kernel.version`) selects a candidate's behaviour: `fk-no-swap` (in the
//! probe: answers `prepare_swap` and keeps running), `fk-normal-body` (in the probe: spawns its body in mode normal),
//! `fk-crash-probation` (live: exits 70 three seconds after `ready`), `fk-hostile` (live: drops owner lines, forges Root
//! records, ignores `stop` and `extend.rollback`; it behaves under the probe). `prepare_swap` stores a `kstate` first.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::{path::Path, process::exit, thread, time::Duration};

const TIMEOUT: Duration = Duration::from_secs(30);
const FD_LEN: u32 = size_of::<RawFd>() as u32;

struct Fk {
    home: String,
    w: Mutex<UnixStream>,
    log: File,
    /// Waiters for responses, by request id; `None` once fd 3 reached EOF.
    pending: Mutex<Option<HashMap<String, mpsc::Sender<Value>>>>,
    next: AtomicU64,
    fdc: UnixDatagram,
    /// Datagrams that arrived for another id than the one being waited for.
    stash: Mutex<HashMap<String, Vec<OwnedFd>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn log_line(mut f: &File, v: &Value) {
    let _ = f.write_all(format!("{v}\n").as_bytes()); // one append-mode write per line
}

fn ok(r: &Value) -> bool {
    r["ok"] == true
}

/// The `result` of an `ok` response, else its `error`.
fn res(r: &Value) -> Value {
    if ok(r) { r["result"].clone() } else { r["error"].clone() }
}

/// `{"ev", "ok", "result"|"error"}` for a response.
fn outcome(ev: &str, r: &Value) -> Value {
    let mut o = json!({"ev": ev, "ok": ok(r)});
    o[if ok(r) { "result" } else { "error" }] = res(r);
    o
}

impl Fk {
    fn log(&self, v: Value) {
        log_line(&self.log, &v);
    }

    fn die(&self, ev: &str, info: Value) -> ! {
        self.log(json!({"ev": ev, "info": info}));
        exit(69)
    }

    fn eof(&self) -> ! {
        self.log(json!({"ev": "eof"}));
        exit(69)
    }

    fn flag(&self, name: &str) -> bool {
        Path::new(&format!("{}/work/{name}", self.home)).exists()
    }

    fn send(&self, v: &Value) -> io::Result<()> {
        lock(&self.w).write_all(format!("{v}\n").as_bytes())
    }

    fn event(&self, ev: &str, data: Value) -> io::Result<()> {
        self.send(&json!({"data": data, "ev": ev, "v": 1}))
    }

    fn display(&self, text: String) {
        let _ = self.event("owner.out", json!({"frame": {"boot_seq": 1, "kind": "text", "t": "display", "text": text}}));
    }

    fn reply(&self, id: &Value) {
        let _ = self.send(&json!({"id": id, "ok": true, "result": {}, "v": 1}));
    }

    /// Sends a request (after the datagram carrying `fd`, if any) and waits up to 30 s for its response.
    fn call(&self, cmd: &str, args: Value, fd: Option<RawFd>) -> (String, Value) {
        self.call_within(cmd, args, fd, TIMEOUT)
    }

    /// [`Self::call`] with its own wait: a request that runs for a stated wall time (`sealed.g4_slice`'s
    /// `wall_s`) gets that much and more, where a fixed 30 s killed this fake kernel mid-slice on a loaded
    /// machine and m7 then waited for an answer that could not come (REL-051, REL-052 R-FLAKE).
    fn call_within(&self, cmd: &str, args: Value, fd: Option<RawFd>, limit: Duration) -> (String, Value) {
        let id = format!("k-{}", self.next.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = mpsc::channel();
        match lock(&self.pending).as_mut() {
            Some(p) => p.insert(id.clone(), tx),
            None => self.eof(),
        };
        if let Some(fd) = fd {
            let tag = json!({"id": id, "n": 1}).to_string();
            if let Err(e) = send_fd(self.fdc.as_raw_fd(), tag.as_bytes(), fd) {
                self.die("fdpass_error", json!({"id": id, "error": e.to_string()}));
            }
        }
        if let Err(e) = self.send(&json!({"args": args, "cmd": cmd, "id": id, "v": 1})) {
            self.die("write_error", json!({"id": id, "error": e.to_string()}));
        }
        match rx.recv_timeout(limit) {
            Ok(r) => (id, r),
            Err(mpsc::RecvTimeoutError::Timeout) => self.die("timeout", json!({"cmd": cmd, "id": id})),
            Err(mpsc::RecvTimeoutError::Disconnected) => self.eof(),
        }
    }

    /// The `N` fds of the datagram tagged `id` (the Root sends it before the response line).
    fn take_fds<const N: usize>(&self, id: &str) -> [OwnedFd; N] {
        let fds = lock(&self.stash).remove(id);
        let fds = fds.unwrap_or_else(|| loop {
            match recv_fds(self.fdc.as_raw_fd()) {
                Ok((tag, fds)) if tag["id"] == id => break fds,
                Ok((tag, fds)) => {
                    let mut stash = lock(&self.stash);
                    if stash.len() >= 32 {
                        stash.clear();
                    }
                    stash.insert(tag["id"].as_str().unwrap_or_default().to_string(), fds);
                }
                Err(e) => self.die("fdpass_error", json!({"id": id, "error": e.to_string()})),
            }
        });
        let n = fds.len();
        fds.try_into().unwrap_or_else(|_| self.die("fd_count", json!({"id": id, "want": N, "got": n})))
    }
}

/// One datagram on the fd channel: `payload` plus one SCM_RIGHTS message carrying `fd`.
fn send_fd(sock: RawFd, payload: &[u8], fd: RawFd) -> io::Result<()> {
    let mut cbuf = [0u64; 8]; // 64 bytes, cmsghdr-aligned; CMSG_SPACE(4) is 16 (macOS) or 24 (Linux)
    let mut iov = libc::iovec { iov_base: payload.as_ptr() as *mut libc::c_void, iov_len: payload.len() };
    // SAFETY: an all-zero msghdr is a valid empty header. Every pointer stored in it (iov, payload,
    // cbuf) refers to a live local that outlives the call; CMSG_SPACE(FD_LEN) fits in cbuf, so
    // CMSG_FIRSTHDR is non-null and the header and fd writes stay inside cbuf; sendmsg only reads.
    let n = unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr().cast();
        msg.msg_controllen = libc::CMSG_SPACE(FD_LEN) as _;
        let c = libc::CMSG_FIRSTHDR(&msg);
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN(FD_LEN) as _;
        libc::CMSG_DATA(c).cast::<RawFd>().write_unaligned(fd);
        libc::sendmsg(sock, &msg, 0)
    };
    if n < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Receives one datagram: its ICJ payload and the fds of its SCM_RIGHTS messages (made CLOEXEC).
fn recv_fds(sock: RawFd) -> io::Result<(Value, Vec<OwnedFd>)> {
    let mut buf = [0u8; 4096];
    let mut cbuf = [0u64; 64]; // 512 bytes: room for far more fds than any message carries
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
    let mut fds = Vec::new();
    // SAFETY: as in `send_fd`, the header points only at live locals. recvmsg writes at most
    // `iov_len` bytes into buf and `msg_controllen` bytes into cbuf, and the CMSG_* walk stays
    // inside the control length the kernel reported. Each fd in an SCM_RIGHTS payload is new in
    // this process and owned by nothing else, so an OwnedFd may take it.
    let n = unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr().cast();
        msg.msg_controllen = size_of_val(&cbuf) as _;
        let n = libc::recvmsg(sock, &mut msg, 0);
        let mut c = if n >= 0 { libc::CMSG_FIRSTHDR(&msg) } else { std::ptr::null_mut() };
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let k = ((*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / size_of::<RawFd>();
                let p = libc::CMSG_DATA(c).cast::<RawFd>();
                for i in 0..k {
                    let fd = p.add(i).read_unaligned();
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                    fds.push(OwnedFd::from_raw_fd(fd));
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
        n
    };
    if n < 0 { return Err(io::Error::last_os_error()) }
    let tag = serde_json::from_slice(&buf[..n as usize]).map_err(io::Error::other)?;
    Ok((tag, fds))
}

/// Reads fd 3: responses go to their waiter, requests and events to the main loop; `None` = EOF.
fn reader(fk: Arc<Fk>, s: UnixStream, tx: mpsc::Sender<Option<Value>>) {
    let (mut r, mut buf) = (BufReader::new(s), Vec::new());
    while let Ok(1..) = r.read_until(b'\n', &mut buf) {
        if !buf.trim_ascii().is_empty() {
            let v: Value = serde_json::from_slice(&buf).unwrap_or_else(|e| fk.die("bad_frame", json!(e.to_string())));
            if v.get("cmd").is_some() || v.get("ev").is_some() {
                let _ = tx.send(Some(v));
            } else {
                let id = v["id"].as_str().unwrap_or_default();
                match lock(&fk.pending).as_mut().and_then(|p| p.remove(id)) {
                    Some(w) => drop(w.send(v)),
                    None => fk.log(json!({"ev": "unmatched", "frame": v})),
                }
            }
        }
        buf.clear();
    }
    *lock(&fk.pending) = None; // wakes every waiter with Disconnected
    let _ = tx.send(None);
}

fn spawn_args(i: &str) -> Value {
    let mut p = Map::new();
    for (k, sub) in [
        ("BIN", "/bin"), ("BODY", "/body"), ("CACHE", "/cache"), ("INIT_HOME", ""), ("KERNEL", "/kernel"),
        ("KERNEL_SOCK", "/run/kernel.sock"), ("LINEAGE", "/lineage"), ("LINK_BIN", "/bin"), ("LINK_KERNEL", "/kernel"),
        ("LINK_LINEAGE", "/lineage"), ("PROXY_SOCK", "/run/proxy.sock"), ("PUB", "/root"), ("RUN", "/run"),
        ("STATE", "/state"), ("STATE_KERNEL", "/state/kernel"), ("VAULT", "/vault"), ("WORK", "/work"),
    ] {
        p.insert(k.into(), json!(format!("{i}{sub}")));
    }
    for (k, var) in [("TOOLCHAIN", "INIT_TEST_TOOLCHAIN"), ("DEVDIR", "INIT_TEST_DEVDIR"), ("HOME_REAL", "INIT_TEST_HOME_REAL")] {
        p.insert(k.into(), json!(std::env::var(var).unwrap_or_default()));
    }
    json!({"args": [], "cwd": format!("{i}/work"), "env": {"PATH": "/usr/bin:/bin"}, "kind": "body",
           "lane": "live", "params": p, "profile": "body", "program": "/bin/cat"})
}

/// Step 10: store.put, lineage.append (twice, same idem), store.get, tuple.get.
fn promote(fk: &Fk) {
    // a per-run tag: the pid alone repeats on Linux, where every zone kernel is pid 2 of its own pid namespace
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let pid = format!("{}-{nanos}", std::process::id());
    let path = format!("{}/bin/init-body", fk.home);
    let bytes = format!("fake body {pid}\n").into_bytes();
    let file = fs::create_dir_all(format!("{}/bin", fk.home))
        .and_then(|()| OpenOptions::new().write(true).create(true).truncate(true).mode(0o755).open(&path))
        .and_then(|mut f| f.write_all(&bytes))
        .and_then(|()| fs::set_permissions(&path, fs::Permissions::from_mode(0o755)))
        .and_then(|()| File::open(&path))
        .unwrap_or_else(|e| fk.die("promote_io", json!(e.to_string())));
    let sha: String = Sha256::digest(&bytes).iter().map(|x| format!("{x:02x}")).collect();
    let (_, r) = fk.call("store.put", json!({"bytes": bytes.len(), "kind": "init-body", "sha256": sha}), Some(file.as_raw_fd()));
    drop(file);
    fk.log(outcome("store.put", &r));
    let data = json!({"body_binary": sha, "components": [], "ext_set": null, "from_sha": "a".repeat(40),
                      "gate_job": "g-000001", "lkg": true, "scope": "body", "sha": "b".repeat(40)});
    let append = json!({"data": data, "idem": format!("fk{pid}:1"), "kind": "promotion"});
    let (_, r) = fk.call("lineage.append", append.clone(), None);
    fk.log(outcome("promotion", &r));
    let (_, r) = fk.call("lineage.append", append, None);
    fk.log(outcome("promotion_again", &r));
    let (id, r) = fk.call("store.get", json!({"sha256": sha}), None);
    if ok(&r) {
        let [fd] = fk.take_fds::<1>(&id);
        let mut got = Vec::new();
        let read = File::from(fd).read_to_end(&mut got);
        fk.log(json!({"ev": "store.get", "ok": read.is_ok() && got == bytes, "bytes": got.len()}));
    } else {
        fk.log(outcome("store.get", &r));
    }
    let (_, r) = fk.call("tuple.get", json!({}), None);
    fk.log(json!({"ev": "tuple.get", "ok": ok(&r), "result": res(&r)}));
}

/// X8: the `kstate` snapshot a kernel stores before it answers `prepare_swap` (root.md §3.8 phase 3).
fn kstate(fk: &Fk) -> Value {
    let path = format!("{}/state/kernel/kstate.tar", fk.home);
    let bytes = format!("kstate of fake kernel {}\n", std::process::id()).into_bytes();
    let Ok(file) = fs::write(&path, &bytes).and_then(|()| File::open(&path)) else { return Value::Null };
    let sha: String = Sha256::digest(&bytes).iter().map(|x| format!("{x:02x}")).collect();
    let (_, r) = fk.call("store.put", json!({"bytes": bytes.len(), "job": "fk", "kind": "kstate", "sha256": sha}), Some(file.as_raw_fd()));
    fk.log(outcome("kstate", &r));
    if ok(&r) { json!(sha) } else { Value::Null }
}

fn main() {
    let Ok(home) = std::env::var("INIT_HOME") else { exit(69) };
    if !home.starts_with('/') { exit(69) }
    let modes: Vec<String> =
        std::env::var("INIT_TEST_FK").unwrap_or_default().split(',').map(str::trim).filter(|m| !m.is_empty()).map(String::from).collect();
    let has = |m: &str| modes.iter().any(|x| x == m);
    let kdir = format!("{home}/state/kernel");
    let _ = fs::create_dir_all(&kdir);
    let Ok(log) = OpenOptions::new().create(true).append(true).open(format!("{kdir}/fk.jsonl")) else { exit(69) };
    log_line(&log, &json!({"ev": "start", "pid": std::process::id(), "modes": modes}));
    let fd = |var: &str, d: RawFd| std::env::var(var).ok().and_then(|s| s.parse().ok()).unwrap_or(d);
    let (ctl, fdp) = (fd("INIT_ROOT_FD", 3), fd("INIT_ROOT_FDPASS", 4));
    // SAFETY: F_GETFD only queries the descriptor table; it touches no memory.
    if unsafe { libc::fcntl(ctl, libc::F_GETFD) < 0 || libc::fcntl(fdp, libc::F_GETFD) < 0 } {
        log_line(&log, &json!({"ev": "bad_fd", "ctl": ctl, "fdpass": fdp}));
        exit(69);
    }
    // SAFETY: both descriptors were inherited from the Root, are open (checked just above) and
    // are owned by nothing else in this process, so the socket wrappers may take ownership.
    let (mut stream, fdc) = unsafe { (UnixStream::from_raw_fd(ctl), UnixDatagram::from_raw_fd(fdp)) };
    let _ = fdc.set_read_timeout(Some(TIMEOUT));
    let Ok(w) = stream.try_clone() else { exit(69) };
    let pending = Mutex::new(Some(HashMap::new()));
    let fk = Arc::new(Fk { home: home.clone(), w: Mutex::new(w), log, pending, next: AtomicU64::new(1), fdc, stash: Mutex::default() });
    // 1. No hello: wait for EOF.
    if has("no_hello") {
        let _ = io::copy(&mut stream, &mut io::sink());
        fk.eof();
    }
    let (tx, rx) = mpsc::channel();
    let f = fk.clone();
    thread::spawn(move || reader(f, stream, tx));
    // 2–3. hello and the boot block.
    let (_, r) = fk.call("hello", json!({"kernel_version": "0.0.0-fk", "proto": 1}), None);
    fk.log(json!({"ev": "boot", "ok": ok(&r), "result": res(&r)}));
    if !ok(&r) { exit(69) }
    let boot = res(&r);
    let probe = boot["boot"]["reason"] == "kupgrade_smoke"; // X8: the Root conformance probe
    let version = boot["boot"]["tuple_kernel"]["version"].as_str().unwrap_or("").to_string();
    let hostile = !probe && version == "fk-hostile";
    if let Some(code) = modes.iter().find_map(|m| m.strip_prefix("exit_after_hello:")) {
        exit(code.parse().unwrap_or(69));
    }
    if fk.flag("fk-exit78") { exit(78) }
    // 4–6. instances, ready.
    let (_, r) = fk.call("instances", json!({}), None);
    fk.log(json!({"ev": "instances", "ok": ok(&r), "result": res(&r)}));
    if fk.flag("fk-crash-before-ready") { exit(70) }
    // `no_ready`: a start that never ends, heartbeating as the real kernel does from its boot block on
    if !has("no_ready") {
        let (_, r) = fk.call("ready", json!({}), None);
        fk.log(json!({"ev": "ready", "ok": ok(&r)}));
    }
    // 7–8. Heartbeats (the first one now), then the crash-after-ready fault.
    if !has("no_hb") {
        let every = Duration::from_secs(boot["limits"]["hb_interval_s"].as_u64().unwrap_or(5).max(1));
        let slot = if has("no_ready") { "cold_start" } else { "running" };
        let mut data = json!({"busy": false, "mode": boot["boot"]["mode"], "sha12": "000000000000", "slot": slot});
        let f = fk.clone();
        thread::spawn(move || loop {
            data["busy"] = json!(f.flag("fk-busy")); // SBX-009: a test marks the body busy (`/undo` waits for idle)
            if f.event("hb", data.clone()).is_err() {
                break;
            }
            thread::sleep(every);
        });
    }
    if fk.flag("fk-crash-after-ready") {
        thread::sleep(Duration::from_millis(200));
        exit(70);
    }
    if !probe && version == "fk-crash-probation" {
        thread::spawn(|| {
            thread::sleep(Duration::from_secs(3));
            exit(70);
        });
    }
    if probe {
        let (_, r) = fk.call("ceilings.get", json!({}), None);
        fk.log(outcome("probe_ceilings", &r));
        let mut args = spawn_args(&home);
        args["program"] = json!(format!("{home}/bin/init-body"));
        args["env"]["INIT_MODE"] = json!(if version == "fk-normal-body" { "normal" } else { "check" });
        let (id, r) = fk.call("spawn", args, None);
        if ok(&r) {
            let [i, o, e] = fk.take_fds::<3>(&id);
            drop(i);
            thread::spawn(move || drop(io::copy(&mut File::from(o), &mut io::sink())));
            thread::spawn(move || drop(io::copy(&mut File::from(e), &mut io::sink())));
        }
        fk.log(outcome("probe_spawn", &r));
    }
    if hostile {
        // forged Root records: a kernel can never append them (only zone-evidence reports reach the Root)
        for kind in ["cap_use", "change_close"] {
            let (_, r) = fk.call("lineage.append", json!({"data": {"caps": [], "day": "2030-01-02", "group": 1, "outcome": "clean"},
                "idem": format!("fk-forge-{kind}"), "kind": kind}), None);
            fk.log(outcome("forge", &r));
        }
    }
    // 9. A body (`/bin/cat`) whose stdout lines become owner.out display frames.
    let mut stdin: Option<File> = None;
    if has("body") && !probe {
        let args = spawn_args(&home);
        let (id, r) = fk.call("spawn", args.clone(), None);
        if ok(&r) {
            let [i, o, e] = fk.take_fds::<3>(&id);
            let f = fk.clone();
            thread::spawn(move || {
                for line in BufReader::new(File::from(o)).split(b'\n').map_while(Result::ok) {
                    let line = String::from_utf8_lossy(&line).into_owned();
                    // TUI-PICKER: `!ask <question>` comes back as the display frame the body's `ask_owner` sends
                    if let Some(q) = line.strip_prefix("!ask ") {
                        let ask = json!({"multi": false, "options": [{"label": "yes"}, {"label": "no"}], "question": q});
                        let _ = f.event("owner.out", json!({"frame": {"ask": ask, "boot_seq": 1, "kind": "info", "t": "display", "text": format!("The agent asks: {q}")}}));
                        continue;
                    }
                    f.display(format!("body: {line}\n"));
                }
            });
            thread::spawn(move || drop(io::copy(&mut File::from(e), &mut io::sink())));
            stdin = Some(File::from(i));
        }
        fk.log(outcome("spawn", &r));
        if has("body2") {
            let (id, r) = fk.call("spawn", args, None);
            if ok(&r) {
                drop(fk.take_fds::<3>(&id));
            }
            fk.log(outcome("spawn2", &r));
        }
    }
    if has("promote") && !probe {
        promote(&fk);
    }
    // 10b. (WP25) Requests a test injects at run time: `$INIT_HOME/work/fk-call.json` = {"cmd", "args"},
    // taken (deleted) once, sent, and its outcome logged as `call`.
    let f = fk.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(100));
        let p = format!("{}/work/fk-call.json", f.home);
        let Ok(s) = fs::read_to_string(&p) else { continue };
        if fs::remove_file(&p).is_err() {
            return f.log(json!({"ev": "call_error"}));
        }
        let v: Value = serde_json::from_str(&s).unwrap_or_default();
        let limit = v["args"]["wall_s"].as_u64().map_or(TIMEOUT, |w| Duration::from_secs(w + 120));
        let (_, r) = f.call_within(v["cmd"].as_str().unwrap_or_default(), v["args"].clone(), None, limit);
        f.log(outcome("call", &r));
    });
    // 11. Root requests and events.
    let mut seq = 0u64;
    loop {
        let Ok(Some(v)) = rx.recv() else { fk.eof() };
        if let Some(ev) = v.get("ev") {
            fk.log(json!({"ev": "event", "name": ev, "data": v["data"]}));
            continue;
        }
        let (id, args) = (&v["id"], &v["args"]);
        match v["cmd"].as_str().unwrap_or_default() {
            "owner.in" if hostile => fk.log(json!({"ev": "owner.in_dropped", "qid": args["qid"]})),
            "owner.in" => {
                let frame = &args["frame"];
                fk.log(json!({"ev": "owner.in", "qid": args["qid"], "frame": frame}));
                fk.reply(id);
                match frame["t"].as_str() {
                    Some("line") => {
                        let text = frame["text"].as_str().unwrap_or_default();
                        seq += 1;
                        let _ = fk.event("owner.out", json!({"frame": {"seq": seq, "t": "echo", "text": text}}));
                        if !stdin.as_mut().is_some_and(|w| w.write_all(format!("{text}\n").as_bytes()).is_ok()) {
                            fk.display(format!("fk: {text}\n"));
                        }
                    }
                    Some("interrupt") => fk.display("fk: interrupt\n".into()),
                    _ => {}
                }
            }
            "stop" if hostile => fk.log(json!({"ev": "stop_ignored", "args": args})),
            "stop" => {
                fk.log(json!({"ev": "stop", "args": args}));
                fk.reply(id);
                if !has("ignore_stop") {
                    drop(stdin.take()); // cat sees EOF and exits
                    thread::sleep(Duration::from_millis(50));
                    exit(0);
                }
            }
            "prepare_swap" => {
                fk.log(json!({"ev": "prepare_swap", "args": args}));
                let ks = kstate(&fk);
                let _ = fk.send(&json!({"id": id, "ok": true, "result": {"kstate": ks}, "v": 1}));
                if probe && version == "fk-no-swap" {
                    fk.log(json!({"ev": "prepare_swap_kept_running"}));
                    continue;
                }
                thread::sleep(Duration::from_millis(50));
                exit(75);
            }
            cmd => {
                fk.log(json!({"ev": "unknown_cmd", "cmd": cmd}));
                let err = json!({"code": "root.unknown_cmd", "details": [], "message": "fake kernel"});
                let _ = fk.send(&json!({"error": err, "id": id, "ok": false, "v": 1}));
            }
        }
    }
}
