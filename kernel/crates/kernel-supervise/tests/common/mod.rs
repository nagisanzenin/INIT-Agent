//! An in-test fake Root (until lane T's `init-fakeroot` lands): it holds the Root's ends of the
//! control link (fd 3 stream, fd 4 datagrams), answers the kernel's requests (root/root.md
//! §4.1.3), runs a minimal spawn broker that launches the requested program **unsandboxed as
//! this test's own child** (own process group) and passes the pipe ends with `SCM_RIGHTS`, and
//! reports `exited` from its own reaper. It signals only the children it spawned, by the pid it
//! holds (BUILD_AGENT_BRIEF safety rules). No sweep exists here: sweeps are disposable-env only.

#![allow(dead_code)]

use init_proto::envelope::{Event, LinkFrame, Request, Response};
use init_proto::root_link::*;
use kernel_supervise::supervisor::{self, Handle};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub const X: &str = "1111111111111111111111111111111111111111";
pub const Y: &str = "2222222222222222222222222222222222222222";

/// `kernel/target/test-homes/wp4/<name>-<hex>` (canonical).
pub fn test_home(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp4");
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let d = root.join(format!("{name}-{}", kernel_supervise::sys::random_hex(3)));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Remove a test home by its exact path.
pub fn remove_home(d: &Path) {
    let root = std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp4")).unwrap();
    assert!(d.starts_with(&root) && d != root);
    let _ = std::fs::remove_dir_all(d);
}

pub fn testbody() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_sv-testbody"))
}

/// Lay out a minimal `INIT_HOME` with the test body as `bin/init-body` (= LKG) and a config.
pub fn make_home(name: &str, extra_cfg: &str) -> PathBuf {
    let h = test_home(name);
    for d in ["bin", "kernel", "state/kernel", "state/run", "state/sessions", "work", "vault", "cache", "run", "root", "lineage"] {
        std::fs::create_dir_all(h.join(d)).unwrap();
    }
    std::fs::copy(testbody(), h.join("bin/init-body")).unwrap();
    std::fs::hard_link(h.join("bin/init-body"), h.join("bin/init-body.lkg")).unwrap();
    let cfg = format!(
        "[proxy]\ntest_mode = true\n\n[supervise]\ngit = \"/usr/bin/git\"\nready_timeout_s = 10\nsettle_s = 1\nbackoff_initial_ms = 50\nbackoff_max_ms = 400\nexit_grace_s = 3\n{extra_cfg}\n"
    );
    std::fs::write(h.join("kernel/config.toml"), cfg).unwrap();
    h
}

pub fn set_plan(home: &Path, plan: Value) {
    std::fs::write(home.join("work/testbody-plan.json"), serde_json::to_vec(&plan).unwrap()).unwrap();
}

pub fn sha256_file(p: &Path) -> String {
    kernel_supervise::fsx::sha256_file(p).unwrap()
}

/// Everything the fake Root saw from the kernel.
#[derive(Default)]
pub struct Seen {
    pub requests: Vec<Request>,
    pub events: Vec<Event>,
    pub lineage: Vec<LineageAppendArgs>,
    pub spawns: Vec<SpawnArgs>,
    pub kills: Vec<KillArgs>,
    pub alerts: Vec<AlertArgs>,
    pub exits: Vec<ExitedData>,
    pub ready: bool,
    pub hello: Option<HelloArgs>,
    pub store_puts: Vec<StorePutArgs>,
    /// The longest line the kernel sent (newline included); the Root ends the link past 1 MiB (K-LINE).
    pub max_line: usize,
    /// ADR-011 (X2): `change.open` args the fake admitted (with the group it gave), and vetoes.
    pub change_opens: Vec<(u64, Value)>,
    pub vetoes: Vec<ChangeVetoArgs>,
}

struct Inst {
    child: Arc<Mutex<Child>>,
}

/// Knobs.
#[derive(Clone)]
pub struct Opts {
    pub reason: ZoneBootReason,
    pub mode: ZoneMode,
    pub level: u8,
    pub tuple_sha: String,
    pub tuple_binary: String,
    /// Answer `lineage.append` (false: never answer, to test the outbox).
    pub answer_lineage: bool,
    /// sha256 → file, for `store.get`.
    pub store: BTreeMap<String, PathBuf>,
    pub instances: Vec<InstanceInfo>,
    /// Refuse the first N `store.put` requests with `root.store` (SV-37).
    pub refuse_store_puts: u32,
    /// K-RACE: a body that dies within 1 s of its spawn is reported (`exited`) before the spawn
    /// answer, which then follows 100 ms later. The Root's reaper and its spawn reply are
    /// independent (root zone.rs `reaper`), so the real Root can do this too.
    pub exited_first: bool,
    /// ADR-009: the `hello` result's `access` object (None: absent, as an older Root).
    pub access: Option<Value>,
    /// ADR-011: the `hello` result's `extend` object (`{"mode":"free"}`); None: absent (gated).
    pub extend: Option<Value>,
    /// ADR-011: Root codes `change.open` answers with, in order, before it admits again
    /// (`extend.budget`, `extend.rollback_incompatible`, …).
    pub refuse_change_opens: Vec<String>,
    /// ADR-011: a change joins the newest open group (one owner turn) instead of opening one.
    pub join_group: bool,
}

pub struct FakeRoot {
    pub home: PathBuf,
    w: Arc<Mutex<UnixStream>>,
    pub seen: Arc<(Mutex<Seen>, Condvar)>,
    pending: Arc<Mutex<HashMap<String, Sender<Response>>>>,
    next: AtomicU64,
    insts: Arc<Mutex<HashMap<String, Inst>>>,
    kernel_side: Mutex<Option<(OwnedFd, OwnedFd)>>,
}

impl FakeRoot {
    /// Create the control link and start answering. The kernel's ends are taken by
    /// [`FakeRoot::start_in_process`] or [`FakeRoot::start_process`].
    pub fn new(home: &Path, opts: Opts) -> Arc<FakeRoot> {
        let (root_ctl, kernel_ctl) = UnixStream::pair().unwrap();
        let (root_pass, kernel_pass) = std::os::unix::net::UnixDatagram::pair().unwrap();
        let r = Arc::new(FakeRoot {
            home: home.to_path_buf(),
            w: Arc::new(Mutex::new(root_ctl.try_clone().unwrap())),
            seen: Arc::new((Mutex::new(Seen::default()), Condvar::new())),
            pending: Arc::default(),
            next: AtomicU64::new(1),
            insts: Arc::default(),
            kernel_side: Mutex::new(Some((OwnedFd::from(kernel_ctl), OwnedFd::from(kernel_pass)))),
        });
        let me = r.clone();
        let pass = OwnedFd::from(root_pass);
        // The broker spawns bodies on the test's behalf: they belong to the test's watchdog.
        let scope = init_testkit::watchdog::current();
        std::thread::spawn(move || {
            let _in = scope.as_ref().map(|s| s.enter());
            me.serve(root_ctl, pass, opts)
        });
        r
    }

    /// Run the supervisor in a thread of this process. Returns its handle and exit-code receiver.
    pub fn start_in_process(self: &Arc<Self>) -> (Handle, Receiver<i32>) {
        let (ctl, pass) = self.kernel_side.lock().unwrap().take().unwrap();
        let home = self.home.clone();
        let (htx, hrx) = mpsc::channel();
        let (ctx, crx) = mpsc::channel();
        std::thread::spawn(move || {
            let code = supervisor::run(&home, ctl, pass, move |h| {
                let _ = htx.send(h);
            });
            let _ = ctx.send(code);
        });
        (hrx.recv_timeout(Duration::from_secs(5)).unwrap(), crx)
    }

    /// The kernel's ends of the control link (for in-process `install::run`).
    pub fn take_kernel_fds(&self) -> (OwnedFd, OwnedFd) {
        self.kernel_side.lock().unwrap().take().unwrap()
    }

    /// Run `sv-kernel supervise --under-root` as a child process with fds 3 and 4.
    pub fn start_process(self: &Arc<Self>, env: &[(&str, &str)]) -> Child {
        let (ctl, pass) = self.kernel_side.lock().unwrap().take().unwrap();
        spawn_kernel(&self.home, Some((ctl, pass)), env, &["supervise", "--under-root"])
    }

    /// Drop the Root's end of fd 3 (the control link is lost).
    pub fn cut(&self) {
        let _ = self.w.lock().unwrap().shutdown(std::net::Shutdown::Both);
    }

    fn write(&self, b: &[u8]) {
        let mut w = self.w.lock().unwrap();
        let _ = w.write_all(b);
        let _ = w.flush();
    }

    fn respond(&self, id: &str, r: Result<Value, init_proto::envelope::ErrorBody>) {
        self.write(&Response { id: Some(id.into()), result: r }.to_line());
    }

    pub fn event(&self, e: &RootEvent) {
        self.write(&init_proto::icj::to_line(&e.to_event().unwrap()).unwrap());
    }

    /// Send a Root → kernel request and wait for the answer.
    pub fn request(&self, r: &RootRequest, timeout: Duration) -> Option<Response> {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        let req = r.to_request(n);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(req.id.clone(), tx);
        self.write(&req.to_line());
        rx.recv_timeout(timeout).ok()
    }

    /// `owner.in{qid, line}` (answered `ok` once durable).
    pub fn owner_line(&self, text: &str) -> Response {
        let qid = format!("q-{}", self.next.fetch_add(1, Ordering::SeqCst));
        self.request(&RootRequest::OwnerIn(OwnerInArgs { qid, frame: OwnerInFrame::Line { text: text.into() } }), Duration::from_secs(5)).expect("owner.in answered")
    }

    pub fn owner_line_qid(&self, qid: &str, text: &str) -> Response {
        self.request(&RootRequest::OwnerIn(OwnerInArgs { qid: qid.into(), frame: OwnerInFrame::Line { text: text.into() } }), Duration::from_secs(5)).expect("owner.in answered")
    }

    /// Wait until `f(seen)` holds.
    pub fn wait_for<T>(&self, what: &str, timeout: Duration, mut f: impl FnMut(&Seen) -> Option<T>) -> T {
        let (m, cv) = &*self.seen;
        let deadline = Instant::now() + timeout;
        let mut g = m.lock().unwrap();
        loop {
            if let Some(t) = f(&g) {
                return t;
            }
            let now = Instant::now();
            if now >= deadline {
                let kinds: Vec<String> = g.lineage.iter().map(|l| l.kind.clone()).collect();
                let displays: Vec<String> = g.events.iter().filter(|e| e.ev == "owner.out").map(|e| e.data.to_string()).collect();
                panic!("timed out waiting for {what}; spawns={} exits={:?} lineage={kinds:?} owner.out={displays:?}", g.spawns.len(), g.exits);
            }
            g = cv.wait_timeout(g, (deadline - now).min(Duration::from_millis(100))).unwrap().0;
        }
    }

    /// `owner.out` frames so far.
    pub fn owner_out(&self) -> Vec<Value> {
        let g = self.seen.0.lock().unwrap();
        g.events.iter().filter(|e| e.ev == "owner.out").map(|e| e.data["frame"].clone()).collect()
    }

    pub fn displays(&self) -> Vec<String> {
        self.owner_out().iter().filter(|f| f["t"] == "display").map(|f| f["text"].as_str().unwrap_or("").to_string()).collect()
    }

    pub fn lineage_kinds(&self) -> Vec<String> {
        self.seen.0.lock().unwrap().lineage.iter().map(|l| l.kind.clone()).collect()
    }

    pub fn spawn_count(&self) -> usize {
        self.seen.0.lock().unwrap().spawns.len()
    }

    /// Kill every child this fake Root spawned (by held handle) and reap them.
    pub fn kill_all(&self) {
        let insts: Vec<Arc<Mutex<Child>>> = self.insts.lock().unwrap().values().map(|i| i.child.clone()).collect();
        for c in insts {
            let mut c = c.lock().unwrap();
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    pub fn live_instances(&self) -> usize {
        self.insts.lock().unwrap().len()
    }

    fn serve(self: Arc<Self>, ctl: UnixStream, pass: OwnedFd, opts: Opts) {
        let mut r = BufReader::new(ctl);
        let mut line = Vec::new();
        let lineage_seq = AtomicU64::new(1000);
        let mut idems: HashMap<String, u64> = HashMap::new();
        let mut store = opts.store.clone();
        let mut refusals = opts.refuse_store_puts;
        let mut open_refusals: std::collections::VecDeque<String> = opts.refuse_change_opens.iter().cloned().collect();
        let mut groups: u64 = 0;
        let mut change_seq: u64 = 5000;
        let mut longest = 0;
        loop {
            line.clear();
            if r.read_until(b'\n', &mut line).unwrap_or(0) == 0 {
                return;
            }
            if line.len() > longest {
                longest = line.len();
                self.note(|s| s.max_line = longest);
            }
            let Ok(frame) = LinkFrame::parse_line(&line) else { continue };
            match frame {
                LinkFrame::Response(resp) => {
                    if let Some(tx) = resp.id.as_ref().and_then(|id| self.pending.lock().unwrap().remove(id)) {
                        let _ = tx.send(resp);
                    }
                }
                LinkFrame::Event(e) => self.note(|s| s.events.push(e)),
                LinkFrame::Request(req) => {
                    self.note(|s| s.requests.push(req.clone()));
                    let parsed = KernelRequest::parse(&req.cmd, &req.args);
                    match parsed {
                        Ok(KernelRequest::Hello(h)) => {
                            self.note(|s| s.hello = Some(h));
                            self.respond(&req.id, Ok(boot_block(&opts)));
                        }
                        Ok(KernelRequest::Ready) => {
                            self.note(|s| s.ready = true);
                            self.respond(&req.id, Ok(json!({})));
                        }
                        Ok(KernelRequest::Spawn(a)) => {
                            self.note(|s| s.spawns.push(a.clone()));
                            match self.spawn(&a, &pass, &req.id) {
                                Ok(v) => {
                                    if opts.exited_first {
                                        let inst = v["instance"].as_str().unwrap_or_default().to_string();
                                        if self.exit_noted(&inst, Duration::from_secs(1)) {
                                            std::thread::sleep(Duration::from_millis(100));
                                        }
                                    }
                                    self.respond(&req.id, Ok(v))
                                }
                                Err(e) => self.respond(&req.id, Err(RootErrorCode::Args.error(e))),
                            }
                        }
                        Ok(KernelRequest::Kill(k)) => {
                            self.note(|s| s.kills.push(k.clone()));
                            let c = self.insts.lock().unwrap().get(&k.instance).map(|i| i.child.clone());
                            match c {
                                Some(c) => {
                                    let mut c = c.lock().unwrap();
                                    // Like the Root (zone.rs `kill`): the instance's process group
                                    // first, then its leader, only while the leader is unreaped (so
                                    // the group is still the one this fake Root created).
                                    if matches!(c.try_wait(), Ok(None)) {
                                        let sig = if matches!(k.mode, KillMode::Term) { libc::SIGTERM } else { libc::SIGKILL };
                                        // SAFETY: the group led by a pid this fake Root spawned and has not reaped.
                                        unsafe {
                                            libc::killpg(c.id() as i32, sig);
                                        }
                                    }
                                    match k.mode {
                                        // SAFETY: a pid this fake Root spawned and still holds.
                                        KillMode::Term => unsafe {
                                            libc::kill(c.id() as i32, libc::SIGTERM);
                                        },
                                        KillMode::Kill => {
                                            let _ = c.kill();
                                        }
                                    }
                                    self.respond(&req.id, Ok(json!({"processes": 1})));
                                }
                                None => self.respond(&req.id, Err(RootErrorCode::NotFound.error("no such instance"))),
                            }
                        }
                        Ok(KernelRequest::Instances) => self.respond(&req.id, Ok(serde_json::to_value(&opts.instances).unwrap())),
                        Ok(KernelRequest::LineageAppend(a)) => {
                            self.note(|s| s.lineage.push(a.clone()));
                            if opts.answer_lineage {
                                let (seq, dup) = match idems.get(&a.idem) {
                                    Some(s) => (*s, true),
                                    None => {
                                        let s = lineage_seq.fetch_add(1, Ordering::SeqCst);
                                        idems.insert(a.idem.clone(), s);
                                        (s, false)
                                    }
                                };
                                self.respond(&req.id, Ok(json!({"seq": seq, "hash": format!("sha256:{}", "a".repeat(64)), "dup": dup})));
                            }
                        }
                        Ok(KernelRequest::LedgerGet) => self.respond(
                            &req.id,
                            Ok(json!({"v": 1, "cap": 4, "entered_at": "2026-10-20T00:00:00.000Z", "entered_seq": 1, "evolution_share_max_permille": null, "incident": null, "last_change": null, "level": opts.level, "max_level": 4, "next": null, "seq": 1, "supported_max_level": 4})),
                        ),
                        Ok(KernelRequest::CeilingsGet) => self.respond(&req.id, Ok(json!({"v": 1, "parallel_bodies": {"active": 1, "limit": 1}}))),
                        Ok(KernelRequest::Alert(a)) => {
                            self.note(|s| s.alerts.push(a));
                            self.respond(&req.id, Ok(json!({})));
                        }
                        Ok(KernelRequest::StorePut(a)) => {
                            // The datagram with the fd precedes the line.
                            let got = kernel_supervise::sys::recv_fds(pass.as_fd(), Some(Duration::from_secs(2))).ok().flatten();
                            self.note(|s| s.store_puts.push(a.clone()));
                            if refusals > 0 {
                                refusals -= 1;
                                self.respond(&req.id, Err(RootErrorCode::Store.error("store full (test)")));
                                continue;
                            }
                            let Some((_tag, mut fds)) = got else {
                                self.respond(&req.id, Err(RootErrorCode::Args.error("no fd")));
                                continue;
                            };
                            let mut bytes = Vec::new();
                            use std::io::Read;
                            std::fs::File::from(fds.pop().unwrap()).read_to_end(&mut bytes).unwrap();
                            let sha = kernel_supervise::fsx::sha256_bytes(&bytes);
                            if sha != a.sha256 {
                                self.respond(&req.id, Err(RootErrorCode::Store.error("hash mismatch")));
                                continue;
                            }
                            let dir = self.home.join("cache/fakeroot-store");
                            std::fs::create_dir_all(&dir).unwrap();
                            std::fs::write(dir.join(&sha), &bytes).unwrap();
                            store.insert(sha.clone(), dir.join(&sha));
                            self.respond(&req.id, Ok(json!({"sha256": sha})));
                        }
                        Ok(KernelRequest::StoreGet(g)) => match store.get(&g.sha256) {
                            Some(p) => {
                                let f = std::fs::File::open(p).unwrap();
                                let bytes = f.metadata().unwrap().len();
                                let tag = serde_json::to_vec(&FdTag { id: req.id.clone(), n: 1 }).unwrap();
                                kernel_supervise::sys::send_fds(pass.as_fd(), &tag, &[f.as_fd()]).unwrap();
                                self.respond(&req.id, Ok(json!({"bytes": bytes})));
                            }
                            None => self.respond(&req.id, Err(RootErrorCode::NotFound.error("no such blob"))),
                        },
                        // ADR-011 (X2): A0 as the Root runs it (every candidate blob stored with its digest),
                        // a scripted refusal, the group, `change_open` in lineage, `extend.window open`.
                        Ok(KernelRequest::ChangeOpen(a)) => {
                            if let Some(code) = open_refusals.pop_front() {
                                self.respond(&req.id, Err(init_proto::envelope::ErrorBody::new(init_proto::errors::ErrorCode::parse(&code), format!("{code} (fake Root)"))));
                                continue;
                            }
                            let c = &a["candidate"];
                            let mut blobs: Vec<String> = vec![c["body"]["binary"].as_str().unwrap_or("").to_string(), c["body"]["t0"].as_str().unwrap_or("").to_string()];
                            blobs.extend(c["ext"]["components"].as_array().into_iter().flatten().filter_map(|x| x["sha256"].as_str().map(str::to_string)));
                            if let Some(missing) = blobs.iter().find(|b| !store.contains_key(*b)) {
                                self.respond(&req.id, Err(init_proto::envelope::ErrorBody::new(init_proto::errors::ErrorCode::parse("root.boundary"), format!("A0: blob {missing:?} is not in the store"))));
                                continue;
                            }
                            if !(opts.join_group && groups > 0) {
                                groups += 1;
                            }
                            change_seq += 1;
                            self.note(|s| {
                                s.change_opens.push((groups, a.clone()));
                                s.lineage.push(LineageAppendArgs { kind: "change_open".into(), data: json!({"group": groups, "sha": a["sha"], "scope": a["scope"], "caps": a["caps"]}), idem: format!("root:{change_seq}") });
                            });
                            self.respond(&req.id, Ok(json!({"group": groups, "seq": change_seq, "snapshot": groups.to_string(), "window": {"max_h": 72, "min_h": 2, "quiet": 5, "turns": 20}})));
                            self.event(&RootEvent::ExtendWindow(ExtendWindowData { group: groups, state: "open".into(), outcome: None }));
                        }
                        Ok(KernelRequest::ChangeVeto(v)) => {
                            self.note(|s| s.vetoes.push(v));
                            self.respond(&req.id, Ok(json!({})));
                        }
                        Ok(_) => self.respond(&req.id, Ok(json!({}))),
                        Err(e) => self.respond(&req.id, Err(e)),
                    }
                }
            }
        }
    }

    /// Wait up to `t` until the reaper has reported `instance`'s `exited` (true) or not (false).
    fn exit_noted(&self, instance: &str, t: Duration) -> bool {
        let (m, cv) = &*self.seen;
        let deadline = Instant::now() + t;
        let mut g = m.lock().unwrap();
        loop {
            if g.exits.iter().any(|x| x.instance == instance) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            g = cv.wait_timeout(g, deadline - now).unwrap().0;
        }
    }

    fn note(&self, f: impl FnOnce(&mut Seen)) {
        let (m, cv) = &*self.seen;
        f(&mut m.lock().unwrap());
        cv.notify_all();
    }

    fn spawn(self: &Arc<Self>, a: &SpawnArgs, pass: &OwnedFd, id: &str) -> Result<Value, String> {
        let (in_r, in_w) = std::io::pipe().map_err(|e| e.to_string())?;
        let (out_r, out_w) = std::io::pipe().map_err(|e| e.to_string())?;
        let (err_r, err_w) = std::io::pipe().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&a.program);
        cmd.args(&a.args).env_clear().envs(&a.env).current_dir(&a.cwd).process_group(0).stdin(Stdio::from(in_r)).stdout(Stdio::from(out_w)).stderr(Stdio::from(err_w));
        // Leaked fds closed in the body, pid registered with the test's watchdog (K-FLAKE).
        let child = init_testkit::watchdog::spawn(&mut cmd).map_err(|e| format!("spawn {}: {e}", a.program))?;
        drop(cmd);
        let pid = child.id() as i32;
        let instance = kernel_supervise::sys::random_hex(6);
        let tag = serde_json::to_vec(&FdTag { id: id.to_string(), n: 3 }).unwrap();
        kernel_supervise::sys::send_fds(pass.as_fd(), &tag, &[in_w.as_fd(), out_r.as_fd(), err_r.as_fd()]).map_err(|e| e.to_string())?;
        drop((in_w, out_r, err_r)); // the Root keeps no copy of a body's pipes
        let child = Arc::new(Mutex::new(child));
        self.insts.lock().unwrap().insert(instance.clone(), Inst { child: child.clone() });
        let me = self.clone();
        let inst = instance.clone();
        std::thread::spawn(move || {
            let status = loop {
                if let Ok(Some(s)) = child.lock().unwrap().try_wait() {
                    break s;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            me.insts.lock().unwrap().remove(&inst);
            let x = ExitedData { at: kernel_supervise::clock::now_rfc3339(), code: status.code(), instance: inst.clone(), rusage_ms: 0, signal: status.signal() };
            me.note(|s| s.exits.push(x.clone()));
            me.event(&RootEvent::Exited(x));
        });
        Ok(json!({"instance": instance, "pid": pid, "started_at": kernel_supervise::clock::now_rfc3339()}))
    }
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            reason: ZoneBootReason::Cold,
            mode: ZoneMode::Normal,
            level: 0,
            tuple_sha: X.into(),
            tuple_binary: String::new(),
            answer_lineage: true,
            store: BTreeMap::new(),
            instances: vec![],
            refuse_store_puts: 0,
            exited_first: false,
            access: None,
            extend: None,
            refuse_change_opens: vec![],
            join_group: false,
        }
    }
}

fn boot_block(o: &Opts) -> Value {
    let mut b = boot_block_v01(o);
    if let Some(a) = &o.access {
        b["access"] = a.clone();
    }
    if let Some(x) = &o.extend {
        b["extend"] = x.clone();
    }
    b
}

fn boot_block_v01(o: &Opts) -> Value {
    json!({
        "boot": {"cause": null, "kstate": null, "level": o.level, "mode": o.mode, "panic": null, "reason": o.reason.as_str(),
                 "tuple": {"audited": "t1", "current": "t1", "lkg": "t1"},
                 "tuple_body": {"binary": o.tuple_binary, "ext": {"components": [], "set": null}, "sha": o.tuple_sha},
                 "tuple_kernel": {"built_from": X, "init_kernel": "e3b0", "source": "maintainer", "version": "0.1.0"}},
        "charter_sha256": "7d1e",
        "ladder": {"cap": 4, "evolution_share_max_permille": null, "level": o.level},
        "limits": {"hb_interval_s": 1, "max_instances": 12, "parallel_bodies": 1},
        "paths": {"egress_sock": "/nonexistent/egress.sock"},
        "root": {"proto": 1, "version": "0.1.0"}
    })
}

/// Spawn `sv-kernel <args>` with the control link on fds 3 and 4 (or without, when `fds` is None).
pub fn spawn_kernel(home: &Path, fds: Option<(OwnedFd, OwnedFd)>, env: &[(&str, &str)], args: &[&str]) -> Child {
    use std::os::fd::AsRawFd;
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sv-kernel"));
    cmd.args(args).env_clear().env("INIT_HOME", home).env("PATH", "/usr/bin:/bin").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    if let Some((ctl, pass)) = fds {
        cmd.env("INIT_ROOT_FD", "3").env("INIT_ROOT_FDPASS", "4");
        let (c, p) = (ctl.as_raw_fd(), pass.as_raw_fd());
        // SAFETY: dup2 is async-signal-safe; it runs in the child between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                if libc::dup2(c, 3) < 0 || libc::dup2(p, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        // Keep the dup2'd 3 and 4; close any other leaked fd in the kernel (K-FLAKE).
        let child = init_testkit::watchdog::spawn_keeping(&mut cmd, 5).unwrap();
        drop((ctl, pass));
        return child;
    }
    init_testkit::watchdog::spawn(&mut cmd).unwrap()
}

/// Wait for a child to exit within `t` (kills it by its handle otherwise and fails).
pub fn wait_exit(c: &mut Child, t: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + t;
    loop {
        if let Some(s) = c.try_wait().unwrap() {
            return s;
        }
        if Instant::now() >= deadline {
            let _ = c.kill();
            let _ = c.wait();
            panic!("process did not exit within {t:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Read a JSON-lines file.
pub fn jsonl(p: &Path) -> Vec<Value> {
    std::fs::read_to_string(p).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// The session log of `home` (via `state/session`).
pub fn session_log(home: &Path) -> Vec<Value> {
    let sid = std::fs::read_to_string(home.join("state/session")).unwrap_or_default();
    jsonl(&home.join("state/sessions").join(format!("{}.jsonl", sid.trim())))
}

/// Run git and return its trimmed stdout. Through [`init_testkit::watchdog::output`]: it waits
/// for git's exit, not for EOF on a pipe a concurrently spawned body may have inherited (the
/// sv10 hang, K-FLAKE).
pub fn git(dir: &Path, home: &Path, args: &[&str]) -> String {
    let o = init_testkit::watchdog::output(Command::new("/usr/bin/git")
        .args(args)
        .current_dir(dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home.join("cache"))
        .env("GIT_CEILING_DIRECTORIES", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "INIT Agent")
        .env("GIT_AUTHOR_EMAIL", "agent@init.local")
        .env("GIT_COMMITTER_NAME", "INIT Agent")
        .env("GIT_COMMITTER_EMAIL", "agent@init.local"))
    .unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap().trim().to_string()
}

/// `lineage/body.git` with `main` = X and a second commit Y; returns (X, Y).
pub fn lineage(home: &Path) -> (String, String) {
    let src = home.join("cache/src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, home, &["init", "-q", "-b", "main"]);
    std::fs::write(src.join("SELF.md"), "v1\n").unwrap();
    git(&src, home, &["add", "."]);
    git(&src, home, &["commit", "-q", "-m", "x"]);
    let x = git(&src, home, &["rev-parse", "HEAD"]);
    std::fs::write(src.join("SELF.md"), "v2\n").unwrap();
    git(&src, home, &["commit", "-q", "-am", "y"]);
    let y = git(&src, home, &["rev-parse", "HEAD"]);
    git(&home.join("lineage"), home, &["clone", "-q", "--bare", src.to_str().unwrap(), "body.git"]);
    let bare = home.join("lineage/body.git");
    git(&bare, home, &["update-ref", "refs/heads/main", &x]);
    git(&bare, home, &["update-ref", "refs/heads/lkg", &x]);
    git(&bare, home, &["update-ref", &format!("refs/init/archive/{y}"), &y]);
    (x, y)
}

pub fn main_of(home: &Path) -> String {
    git(&home.join("lineage/body.git"), home, &["rev-parse", "refs/heads/main"])
}

