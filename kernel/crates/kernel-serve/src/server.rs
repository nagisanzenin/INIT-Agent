//! The initctl server (kernel/initctl.md §3): one acceptor thread per lane socket, one thread per
//! connection, one request at a time per connection.

use crate::backend::{Backend, Begun, LaneCtx, LaneSpec, LedgerSnapshot};
use crate::fit::fit;
use crate::fsx::{self, klog};
use crate::gating::{self, LEDGER_FRESH_S, StoreFacts};
use crate::jobs::{JobConfig, JobStore, WaitEnd, WaitFrom, WaiterGuard};
use crate::limits::{LaneLimiter, Limits};
use crate::paths::{self, Want};
use crate::peer::{self, PeerPolicy};
use crate::propose;
use crate::quota::{QuotaLimits, QuotaTicket, Quotas};
use init_proto::envelope::{MAX_MESSAGE_BYTES, MAX_REQUEST_LINE};
use init_proto::errors::{ErrorCode, RootReason};
use init_proto::initctl::*;
use init_proto::{ErrorBody, PROTO, ProgressFrame, Request, Response, validate};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// `sun_path` limits: 103 bytes on macOS, 107 on Linux (supervise.md §3.1).
#[cfg(target_vendor = "apple")]
pub const MAX_SOCKET_PATH: usize = 103;
#[cfg(not(target_vendor = "apple"))]
pub const MAX_SOCKET_PATH: usize = 107;

/// Server configuration.
#[derive(Clone, Debug)]
pub struct ServeConfig {
    /// `INIT_HOME`.
    pub home: PathBuf,
    /// `ping.kernel_version`.
    pub kernel_version: String,
    pub limits: Limits,
    pub quotas: QuotaLimits,
    pub jobs: JobConfig,
    /// A connection whose peer does not read for this long is dropped.
    pub write_timeout: Duration,
}

impl ServeConfig {
    /// The defaults of initctl.md for `home`.
    pub fn new(home: &Path, kernel_version: &str) -> ServeConfig {
        ServeConfig {
            home: home.to_path_buf(),
            kernel_version: kernel_version.to_string(),
            limits: Limits::default(),
            quotas: QuotaLimits::default(),
            jobs: JobConfig::default(),
            write_timeout: Duration::from_secs(10),
        }
    }
}

/// One error message line, at most 500 bytes (§3.2), whatever the backend built.
pub fn sanitize_error(mut e: ErrorBody) -> ErrorBody {
    e.message = validate::one_line(&e.message, MAX_MESSAGE_BYTES);
    e
}

fn internal(msg: impl AsRef<str>) -> ErrorBody {
    ErrorBody::new(ErrorCode::Internal, msg)
}

/// `data.reason` of the `E_BUSY` that a lane answers while the server behind it is not ready
/// (FIX6): the kernel binds `run/kernel.sock` before its cold start and serves it afterwards.
pub const STARTING: &str = "starting";

/// The answer of a lane whose server is not ready yet (FIX6, [`EarlyLane`]; also a backend that
/// still waits for its start-up state): `E_BUSY` (retry: yes, CLI exit 75) with
/// `data: {reason: "starting", retry_after_ms: 1000}`.
pub fn starting_error() -> ErrorBody {
    ErrorBody::new(ErrorCode::Busy, "the kernel is starting; retry in 1 s").with_data(&json!({"reason": STARTING, "retry_after_ms": 1000}))
}

/// How long an [`EarlyLane`] connection may take to send its one request line.
const EARLY_READ_TIMEOUT: Duration = Duration::from_secs(5);

fn to_val<T: Serialize>(r: Result<T, ErrorBody>) -> Result<Value, ErrorBody> {
    let v = r?;
    init_proto::icj::to_value(&v).map_err(|e| internal(format!("result is not ICJ: {e}")))
}

/// True while the peer has not closed its end (a non-blocking `MSG_PEEK`).
fn peer_alive(s: &UnixStream) -> bool {
    let mut b = [0u8; 1];
    // SAFETY: a 1-byte peek into a valid buffer on a live fd; MSG_DONTWAIT never blocks.
    let n = unsafe { libc::recv(s.as_raw_fd(), b.as_mut_ptr().cast(), 1, libc::MSG_PEEK | libc::MSG_DONTWAIT) };
    if n > 0 {
        return true;
    }
    if n == 0 {
        return false;
    }
    matches!(io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted)
}

fn send(s: &UnixStream, line: &[u8]) -> io::Result<()> {
    let mut w = s;
    w.write_all(line)?;
    w.flush()
}

enum ReadErr {
    TooLong,
    Closed,
}

/// Read one request line of at most `max` bytes (without the newline).
fn read_request(r: &mut BufReader<UnixStream>, max: usize) -> Result<Vec<u8>, ReadErr> {
    let mut buf = Vec::new();
    loop {
        let chunk = match r.fill_buf() {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(ReadErr::Closed), // idle timeout or a broken connection
        };
        if chunk.is_empty() {
            return Err(ReadErr::Closed);
        }
        match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => {
                buf.extend_from_slice(&chunk[..i]);
                r.consume(i + 1);
                return if buf.len() > max { Err(ReadErr::TooLong) } else { Ok(buf) };
            }
            None => {
                let n = chunk.len();
                buf.extend_from_slice(chunk);
                r.consume(n);
                if buf.len() > max {
                    return Err(ReadErr::TooLong);
                }
            }
        }
    }
}

struct LaneRt {
    spec: Arc<LaneSpec>,
    limiter: Arc<LaneLimiter>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

struct Inner {
    cfg: ServeConfig,
    backend: Arc<dyn Backend>,
    policy: Arc<dyn PeerPolicy>,
    jobs: JobStore,
    quotas: Quotas,
    recovered: Vec<String>,
    constitution: Option<String>,
    lanes: Mutex<BTreeMap<String, LaneRt>>,
    conns: Mutex<HashMap<u64, (String, UnixStream)>>,
    next_conn: AtomicU64,
    down: AtomicBool,
}

/// A running initctl server. Cheap to clone; [`ServerHandle::shutdown`] stops it.
#[derive(Clone)]
pub struct ServerHandle {
    inner: Arc<Inner>,
}

/// Start the server for `cfg.home`: open the job store (marking interrupted jobs
/// `kernel_restart`), load the quotas, hash `kernel/CONSTITUTION.md`. No lane is bound yet: call
/// [`ServerHandle::add_lane`] (the live lane at kernel start; probe and member lanes when their
/// instance is created).
pub fn start(cfg: ServeConfig, backend: Arc<dyn Backend>, policy: Arc<dyn PeerPolicy>) -> io::Result<ServerHandle> {
    let home = fs::canonicalize(&cfg.home)?;
    let cfg = ServeConfig { home: home.clone(), ..cfg };
    let (jobs, recovered) = JobStore::open(&home.join("state/kernel/jobs"), cfg.jobs)?;
    if !recovered.is_empty() {
        klog(&home, "jobs_recovered", json!({"jobs": recovered, "reason": "kernel_restart"}));
    }
    let quotas = Quotas::open(&home.join("state/kernel/serve-quotas.json"), cfg.quotas);
    let constitution = fs::read(home.join("kernel/CONSTITUTION.md")).ok().map(|b| {
        let d: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(&b).into();
        let hex: String = d.iter().map(|x| format!("{x:02x}")).collect();
        format!("sha256:{hex}")
    });
    Ok(ServerHandle {
        inner: Arc::new(Inner {
            cfg,
            backend,
            policy,
            jobs,
            quotas,
            recovered,
            constitution,
            lanes: Mutex::new(BTreeMap::new()),
            conns: Mutex::new(HashMap::new()),
            next_conn: AtomicU64::new(1),
            down: AtomicBool::new(false),
        }),
    })
}

/// A lane socket's directory: create it, and chmod it to 0700 only when its mode differs.
/// `run/` (the live lane's directory) is created 0700 by the Root, and the Root's zone tail (Z4,
/// root/root.md §3.3.2) denies every write to `run/` itself, chmod included, even to the same
/// mode. An unconditional chmod therefore failed the live lane with `EPERM` (build/status/S1.md
/// gap 3). Lane directories under `run/fleet/` and `probe/` stay chmod-able.
fn prepare_lane_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    if fs::metadata(dir)?.permissions().mode() & 0o777 != 0o700 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Rebase `p` from `from` onto `to` when it lies under `from`.
fn rebase(p: &Path, from: &Path, to: &Path) -> PathBuf {
    match p.strip_prefix(from) {
        Ok(r) => to.join(r),
        Err(_) => p.to_path_buf(),
    }
}

/// §3.3 "Socket files": create the lane's directory (mode 0700), make the paths canonical, check
/// the path length, unlink a stale socket by exact path, and bind (a non-blocking listener).
fn bind_lane(spec: LaneSpec) -> io::Result<(LaneSpec, UnixListener)> {
    let dir = spec.socket.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "socket path has no parent"))?;
    prepare_lane_dir(dir)?;
    // Canonical paths, so path arguments compare exactly (the CLI sends absolute paths).
    let home_c = fs::canonicalize(&spec.home)?;
    let spec = LaneSpec {
        socket: rebase(&spec.socket, &spec.home, &home_c),
        allowed: spec.allowed.iter().map(|a| rebase(a, &spec.home, &home_c)).collect(),
        home: home_c,
        ctx: spec.ctx,
    };
    let sock_len = spec.socket.as_os_str().len();
    if sock_len > MAX_SOCKET_PATH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("socket path is {sock_len} bytes; the limit is {MAX_SOCKET_PATH}: {}", spec.socket.display()),
        ));
    }
    match fs::symlink_metadata(&spec.socket) {
        Ok(m) if m.file_type().is_socket() => fs::remove_file(&spec.socket)?, // exact path, stale socket
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} exists and is not a socket; not removing it", spec.socket.display()),
            ));
        }
        Err(_) => {}
    }
    let listener = UnixListener::bind(&spec.socket)?;
    listener.set_nonblocking(true)?;
    Ok((spec, listener))
}

/// A lane socket bound before its server exists (FIX6).
///
/// The kernel binds `run/kernel.sock` right after it takes `run/kernel.lock` (supervise.md §3.9
/// step 1), before the slow part of its cold start, so a body never meets a missing or refusing
/// socket while the kernel starts. Until [`ServerHandle::adopt_lane`] takes the listener over,
/// every request gets [`starting_error`] (with the request's `id`) and the connection is closed.
/// No backend runs and no peer policy is consulted (the uid check of §3.3 still applies), so the
/// only thing disclosed is "starting". The lane's connection and rate limits apply.
///
/// Adoption keeps the same listener, so connections still in the backlog are served by the
/// real server. Dropped without adoption (the server failed to start, or the kernel exits), the
/// lane stops and unlinks its socket by exact path.
pub struct EarlyLane {
    spec: Option<LaneSpec>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<UnixListener>>,
}

impl EarlyLane {
    /// Bind `spec`'s socket as [`ServerHandle::add_lane`] would and answer "starting" on it.
    pub fn bind(spec: LaneSpec, limits: Limits) -> io::Result<EarlyLane> {
        let (spec, listener) = bind_lane(spec)?;
        let stop = Arc::new(AtomicBool::new(false));
        let limiter = Arc::new(LaneLimiter::new(limits));
        let thread = {
            let stop = stop.clone();
            std::thread::Builder::new().name(format!("initctl-early-{}", spec.ctx.key())).spawn(move || early_loop(listener, limiter, stop))?
        };
        Ok(EarlyLane { spec: Some(spec), stop, thread: Some(thread) })
    }

    /// The bound socket (canonical).
    pub fn socket(&self) -> &Path {
        self.spec.as_ref().map(|s| s.socket.as_path()).unwrap_or(Path::new(""))
    }

    /// Stop answering and hand over the spec and the listener (the socket stays bound).
    fn take(&mut self) -> io::Result<(LaneSpec, UnixListener)> {
        self.stop.store(true, Ordering::SeqCst);
        let t = self.thread.take().ok_or_else(|| io::Error::other("the early lane was already taken"))?;
        let listener = t.join().map_err(|_| io::Error::other("the early lane's thread panicked"))?;
        let spec = self.spec.take().ok_or_else(|| io::Error::other("the early lane was already taken"))?;
        Ok((spec, listener))
    }
}

impl Drop for EarlyLane {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Some(spec) = self.spec.take()
            && is_socket(&spec.socket)
        {
            let _ = fs::remove_file(&spec.socket); // exact path: the socket this lane bound
        }
    }
}

fn early_loop(listener: UnixListener, limiter: Arc<LaneLimiter>, stop: Arc<AtomicBool>) -> UnixListener {
    while !stop.load(Ordering::SeqCst) {
        let mut pfd = libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd; a 50 ms timeout bounds how late `stop` is noticed.
        let rc = unsafe { libc::poll(&mut pfd, 1, 50) };
        // A connection that arrives once `stop` is set stays in the backlog for the adopter.
        if rc <= 0 || stop.load(Ordering::SeqCst) {
            continue;
        }
        match listener.accept() {
            Ok((s, _)) => {
                if s.set_nonblocking(false).is_err() {
                    continue;
                }
                let limiter = limiter.clone();
                let _ = std::thread::Builder::new().name("initctl-early-conn".into()).spawn(move || early_conn(s, &limiter));
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    listener
}

/// One connection on an [`EarlyLane`]: one request line, one answer, close.
fn early_conn(stream: UnixStream, limiter: &LaneLimiter) {
    match peer::peer_of(&stream) {
        Ok(p) if p.uid == peer::euid() => {}
        _ => return, // close silently, as serve_conn does
    }
    let Some(_slot) = limiter.enter() else {
        let max = limiter.limits().max_conns;
        let _ = send(&stream, &Response::err(None, ErrorBody::new(ErrorCode::Rate, format!("too many connections on this lane (max {max})"))).to_line());
        return;
    };
    let _ = stream.set_read_timeout(Some(EARLY_READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(EARLY_READ_TIMEOUT));
    let Ok(rd) = stream.try_clone() else { return };
    let mut reader = BufReader::with_capacity(64 * 1024, rd);
    let resp = match read_request(&mut reader, MAX_REQUEST_LINE) {
        Err(ReadErr::Closed) => return,
        Err(ReadErr::TooLong) => Response::err(None, ErrorBody::new(ErrorCode::Proto, format!("request line longer than {} bytes", MAX_REQUEST_LINE))),
        Ok(line) => match Request::parse_line(&line) {
            Err(rej) => rej.response(),
            Ok(req) if !limiter.take() => {
                let l = limiter.limits();
                Response::err(Some(req.id), ErrorBody::new(ErrorCode::Rate, format!("rate limit: {} requests/s per lane (burst {}); retry after 1 s", l.rate_per_sec, l.burst)))
            }
            Ok(req) => Response::err(Some(req.id), starting_error()),
        },
    };
    let _ = send(&stream, &resp.to_line());
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

impl ServerHandle {
    /// `INIT_HOME` (canonical).
    pub fn home(&self) -> &Path {
        &self.inner.cfg.home
    }

    /// The job store (kernel-initiated jobs, `kernel status`).
    pub fn jobs(&self) -> &JobStore {
        &self.inner.jobs
    }

    /// Jobs marked `failed` with `reason:"kernel_restart"` at start (the backend deletes their
    /// candidate refs, §3.7).
    pub fn recovered(&self) -> &[String] {
        &self.inner.recovered
    }

    /// Open connections on lane `key`.
    pub fn connections(&self, key: &str) -> Option<usize> {
        self.inner.lanes.lock().unwrap_or_else(|e| e.into_inner()).get(key).map(|l| l.limiter.connections())
    }

    /// Keys of the bound lanes.
    pub fn lanes(&self) -> Vec<String> {
        self.inner.lanes.lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect()
    }

    /// Bind a lane socket (§3.3 "Socket files"): create its directory (mode 0700), check the path
    /// length, unlink a stale socket by exact path, bind, and start accepting.
    pub fn add_lane(&self, spec: LaneSpec) -> io::Result<()> {
        self.check_new(&spec.ctx.key())?;
        let (spec, listener) = bind_lane(spec)?;
        self.run_lane(spec, listener)
    }

    /// Take over a lane bound by [`EarlyLane::bind`] (FIX6): the same socket and listener, so a
    /// client never meets a missing or refusing socket in between, and connections still in the
    /// backlog are served by this server under its peer policy.
    pub fn adopt_lane(&self, mut early: EarlyLane) -> io::Result<()> {
        let key = early.spec.as_ref().map(|s| s.ctx.key()).ok_or_else(|| io::Error::other("the early lane was already taken"))?;
        self.check_new(&key)?; // on error the early lane is dropped: its socket is unlinked
        let (spec, listener) = early.take()?;
        self.run_lane(spec, listener)
    }

    fn check_new(&self, key: &str) -> io::Result<()> {
        if self.inner.down.load(Ordering::SeqCst) {
            return Err(io::Error::other("the server is shut down"));
        }
        if self.inner.lanes.lock().unwrap_or_else(|e| e.into_inner()).contains_key(key) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("lane {key} is already bound")));
        }
        Ok(())
    }

    fn run_lane(&self, spec: LaneSpec, listener: UnixListener) -> io::Result<()> {
        let inner = &self.inner;
        let key = spec.ctx.key();
        let spec = Arc::new(spec);
        let limiter = Arc::new(LaneLimiter::new(inner.cfg.limits));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (inner, spec, limiter, stop) = (self.inner.clone(), spec.clone(), limiter.clone(), stop.clone());
            std::thread::Builder::new().name(format!("initctl-accept-{key}")).spawn(move || accept_loop(inner, listener, spec, limiter, stop))?
        };
        inner.lanes.lock().unwrap_or_else(|e| e.into_inner()).insert(key, LaneRt { spec, limiter, stop, thread: Some(thread) });
        Ok(())
    }

    /// Stop accepting on a lane, close its connections and unlink its socket (exact path).
    pub fn remove_lane(&self, key: &str) -> io::Result<()> {
        let rt = self.inner.lanes.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
        let Some(mut rt) = rt else {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("no lane {key}")));
        };
        rt.stop.store(true, Ordering::SeqCst);
        if let Some(t) = rt.thread.take() {
            let _ = t.join();
        }
        for (lane, s) in self.inner.conns.lock().unwrap_or_else(|e| e.into_inner()).values() {
            if lane == key {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }
        if fs::symlink_metadata(&rt.spec.socket).is_ok_and(|m| m.file_type().is_socket()) {
            fs::remove_file(&rt.spec.socket)?;
        }
        Ok(())
    }

    /// Stop the server: refuse new work, wake every waiter, close every connection, unbind every
    /// lane. Jobs keep their records; the backend stops driving them.
    pub fn shutdown(&self) {
        self.inner.down.store(true, Ordering::SeqCst);
        self.inner.jobs.shutdown();
        for key in self.lanes() {
            let _ = self.remove_lane(&key);
        }
        for (_, s) in self.inner.conns.lock().unwrap_or_else(|e| e.into_inner()).values() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
}

fn accept_loop(inner: Arc<Inner>, listener: UnixListener, spec: Arc<LaneSpec>, limiter: Arc<LaneLimiter>, stop: Arc<AtomicBool>) {
    loop {
        if stop.load(Ordering::SeqCst) || inner.down.load(Ordering::SeqCst) {
            return;
        }
        let mut pfd = libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd; a 100 ms timeout bounds how late `stop` is noticed.
        let rc = unsafe { libc::poll(&mut pfd, 1, 100) };
        if rc <= 0 {
            continue;
        }
        match listener.accept() {
            Ok((s, _)) => {
                // BSD sockets inherit O_NONBLOCK from the listener.
                if s.set_nonblocking(false).is_err() {
                    continue;
                }
                let (inner, spec, limiter) = (inner.clone(), spec.clone(), limiter.clone());
                let _ = std::thread::Builder::new().name("initctl-conn".into()).spawn(move || serve_conn(inner, spec, limiter, s));
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
            Err(e) => {
                klog(&inner.cfg.home, "accept_error", json!({"lane": spec.ctx.key(), "error": e.to_string()}));
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

struct ConnReg<'a> {
    inner: &'a Inner,
    id: u64,
}

impl Drop for ConnReg<'_> {
    fn drop(&mut self) {
        self.inner.conns.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
    }
}

fn serve_conn(inner: Arc<Inner>, spec: Arc<LaneSpec>, limiter: Arc<LaneLimiter>, stream: UnixStream) {
    let home = inner.cfg.home.clone();
    let lane_key = spec.ctx.key();
    // §3.3: peer credentials before any request.
    let peer = match peer::peer_of(&stream) {
        Ok(p) => p,
        Err(e) => {
            klog(&home, "peer_unreadable", json!({"lane": lane_key, "error": e.to_string()}));
            return;
        }
    };
    if peer.uid != peer::euid() {
        klog(&home, "peer_refused", json!({"lane": lane_key, "pid": peer.pid, "reason": "uid mismatch"}));
        return; // close silently
    }
    let marks = catch_unwind(AssertUnwindSafe(|| inner.backend.marks(&spec.ctx))).unwrap_or_default();
    if let Err(reason) = inner.policy.attribute(&spec.ctx, &marks, &peer) {
        klog(&home, "peer_refused", json!({"lane": lane_key, "pid": peer.pid, "reason": reason}));
        let _ = send(&stream, &Response::err(None, ErrorBody::new(ErrorCode::Peer, "peer not attributable to this lane")).to_line());
        return;
    }
    let Some(_slot) = limiter.enter() else {
        let max = limiter.limits().max_conns;
        let _ = send(&stream, &Response::err(None, ErrorBody::new(ErrorCode::Rate, format!("too many connections on this lane (max {max})"))).to_line());
        return;
    };
    let conn_id = inner.next_conn.fetch_add(1, Ordering::SeqCst);
    if let Ok(c) = stream.try_clone() {
        inner.conns.lock().unwrap_or_else(|e| e.into_inner()).insert(conn_id, (lane_key.clone(), c));
    }
    let _reg = ConnReg { inner: &inner, id: conn_id };
    if inner.down.load(Ordering::SeqCst) {
        return;
    }
    let _ = stream.set_read_timeout(Some(limiter.limits().idle));
    let _ = stream.set_write_timeout(Some(inner.cfg.write_timeout));
    let Ok(rd) = stream.try_clone() else { return };
    let mut reader = BufReader::with_capacity(64 * 1024, rd);
    loop {
        let line = match read_request(&mut reader, MAX_REQUEST_LINE) {
            Ok(l) => l,
            Err(ReadErr::Closed) => return,
            Err(ReadErr::TooLong) => {
                let e = ErrorBody::new(ErrorCode::Proto, format!("request line longer than {} bytes", MAX_REQUEST_LINE));
                let _ = send(&stream, &Response::err(None, e).to_line());
                return;
            }
        };
        if inner.down.load(Ordering::SeqCst) {
            return;
        }
        let req = match Request::parse_line(&line) {
            Ok(r) => r,
            Err(rej) => {
                if send(&stream, &rej.response().to_line()).is_err() || rej.closes_connection() {
                    return;
                }
                continue;
            }
        };
        if !limiter.take() {
            let l = limiter.limits();
            let e = ErrorBody::new(ErrorCode::Rate, format!("rate limit: {} requests/s per lane (burst {}); retry after 1 s", l.rate_per_sec, l.burst));
            if send(&stream, &Response::err(Some(req.id.clone()), e).to_line()).is_err() {
                return;
            }
            continue;
        }
        let out = catch_unwind(AssertUnwindSafe(|| inner.dispatch(&spec, &stream, &req)));
        match out {
            Ok(Some((resp, guard))) => {
                let ok = send(&stream, &fit(resp).to_line()).is_ok();
                drop(guard); // the final response is written: release the milestone
                if !ok {
                    return;
                }
            }
            Ok(None) => return, // the peer left during a wait, or the server is going down
            Err(p) => {
                let what = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                klog(&home, "internal_error", json!({"cmd": req.cmd, "lane": lane_key, "panic": what}));
                let e = internal("kernel bug; details in state/kernel/logs/kernel.jsonl");
                if send(&stream, &Response::err(Some(req.id.clone()), e).to_line()).is_err() {
                    return;
                }
            }
        }
    }
}

type Reply = Option<(Response, Option<WaiterGuard>)>;

fn reply(id: &str, r: Result<Value, ErrorBody>) -> Reply {
    Some((
        match r {
            Ok(v) => Response::ok_value(id, v),
            Err(e) => Response::err(Some(id.to_string()), sanitize_error(e)),
        },
        None,
    ))
}

/// Commands whose `wait` answer is the job's terminal `result` (others answer at a milestone).
fn returns_result(cmd: &Command) -> bool {
    matches!(cmd, Command::Gate(_) | Command::Vendor(_) | Command::TaskSubmit(_) | Command::EvoScreen(_))
}

/// `evo propose` drafts are at most 64 KiB (evolution-v2 §3.4.7, kernel-evo `MAX_DRAFT_BYTES`).
const EVO_DRAFT_MAX: usize = 64 * 1024;
/// `evo variant` patches are at most 200 KiB (evolution-v2 §3.5.1).
const EVO_PATCH_MAX: usize = 200 * 1024;

impl Inner {
    fn store_facts(&self, cmd: &Command) -> StoreFacts {
        let name = cmd.name();
        let mut f = StoreFacts { quota_full: self.quotas.peek(name), ..StoreFacts::default() };
        if matches!(name, CommandName::Promote | CommandName::Rollback) {
            // An active job for the same `--sha` is answered idempotently, not refused.
            let same_sha = match cmd {
                Command::Promote(a) => a.sha.clone(),
                _ => None,
            };
            f.pipeline_job = self
                .jobs
                .active_of("promote")
                .into_iter()
                .chain(self.jobs.active_of("rollback"))
                .find(|r| same_sha.is_none() || r.sha != same_sha)
                .map(|r| r.id);
        }
        if matches!(name, CommandName::Promote | CommandName::KernelPromote) {
            f.kernel_job = self.jobs.active_of("kernel.promote").into_iter().next().map(|r| r.id);
        }
        if name == CommandName::Gate {
            f.queued_gates = self.jobs.active_of("gate").iter().filter(|r| r.state == JobState::Queued).count();
        }
        f
    }

    fn dispatch(&self, spec: &LaneSpec, stream: &UnixStream, req: &Request) -> Reply {
        let id = req.id.as_str();
        let ctx = &spec.ctx;
        // §3.4's lane column depends on the name only, so it comes before the arguments: a command
        // this lane never offers is `E_LANE` whatever its arguments (IC-4).
        if let Some(name) = CommandName::parse(&req.cmd)
            && let Err(e) = gating::check_lane_name(name, ctx)
        {
            return reply(id, Err(e));
        }
        let cmd = match Command::from_request(req) {
            Ok(c) => c,
            Err(e) => return reply(id, Err(e)),
        };
        if let Err(e) = gating::check_lane(&cmd, ctx) {
            return reply(id, Err(e));
        }
        let view = self.backend.view(ctx);
        if let Err(e) = gating::check_state(&cmd, ctx, &view, &self.store_facts(&cmd)) {
            return reply(id, Err(e));
        }
        let ticket = match self.quotas.take(cmd.name()) {
            Ok(t) => t,
            Err(q) => return reply(id, Err(ErrorBody::new(ErrorCode::Quota, "quota used up").with_data(&q))),
        };
        let b = &*self.backend;
        let r = match &cmd {
            Command::Ping => to_val(Ok(PingResult {
                kernel_version: self.cfg.kernel_version.clone(),
                lane: ctx.lane,
                proto: PROTO,
                time: crate::clock::now_rfc3339(),
            })),
            Command::Status => self.status(ctx),
            Command::Level => self.level(&view),
            Command::JobStatus(a) => self.own_job(ctx, &a.job).map(|(_, r)| r).and_then(|r| to_val(Ok(r))),
            Command::JobList(a) => {
                let active = a.state.unwrap_or(JobListState::Active) == JobListState::Active;
                let jobs = self.jobs.list(&ctx.key(), active, a.limit.unwrap_or(JOB_LIST_DEFAULT) as usize);
                to_val(Ok(JobListResult { jobs }))
            }
            Command::JobCancel(a) => self.cancel(ctx, &a.job),
            Command::JobWait(a) => return self.job_wait(ctx, stream, id, a),
            Command::TaskTemplate => b.task_template(ctx),
            Command::TaskList => to_val(b.task_list(ctx)),
            Command::TaskShow(a) => b.task_show(ctx, a),
            Command::BudgetSet(a) => to_val(b.budget_set(ctx, a)),
            Command::ModelSet(a) => to_val(b.model_set(ctx, a)),
            Command::FleetList => to_val(b.fleet_list(ctx)),
            Command::FleetSend(a) => to_val(b.fleet_send(ctx, a)),
            Command::FleetStop(a) => to_val(b.fleet_stop(ctx, a)),
            Command::KernelStatus => to_val(b.kernel_status(ctx)),
            Command::KernelPropose(a) => {
                let r = to_val(propose::run(&self.cfg.home, spec, b, a));
                if r.is_ok()
                    && let Some(t) = ticket
                {
                    t.commit();
                }
                r
            }
            Command::EvoPropose(a) => paths::resolve(spec, "file", &a.file, Want::File)
                .and_then(|p| paths::read_text(&p, "file", EVO_DRAFT_MAX))
                .and_then(|t| b.evo(ctx, &cmd, Some(&t))),
            Command::EvoVariant(a) => paths::resolve(spec, "patch", &a.patch, Want::File)
                .and_then(|p| paths::read_text(&p, "patch", EVO_PATCH_MAX))
                .and_then(|t| b.evo(ctx, &cmd, Some(&t))),
            Command::EvoFrictions(_) | Command::EvoStatus(_) | Command::EvoAbandon(_) => b.evo(ctx, &cmd, None),
            Command::Grant(a) => to_val(b.grant_request(ctx, a)),
            Command::Reload(_)
            | Command::Gate(_)
            | Command::Promote(_)
            | Command::Rollback(_)
            | Command::Vendor(_)
            | Command::TaskSubmit(_)
            | Command::FleetSpawn(_)
            | Command::KernelPromote(_)
            | Command::EvoScreen(_) => return self.start_job(spec, stream, id, &cmd, ticket),
        };
        reply(id, r)
    }

    fn status(&self, ctx: &LaneCtx) -> Result<Value, ErrorBody> {
        let mut s = self.backend.status(ctx)?;
        s.lane = ctx.lane;
        s.jobs = self.jobs.summaries(&ctx.key());
        if let Some(c) = &self.constitution {
            s.constitution_sha256 = c.clone();
        }
        to_val(Ok(s))
    }

    fn level(&self, view: &crate::backend::KernelView) -> Result<Value, ErrorBody> {
        if !view.link_up {
            return Err(ErrorBody::root(RootReason::LinkDown, "the control link to the Root is down"));
        }
        let snap: LedgerSnapshot = match self.backend.ledger() {
            Some(s) if s.age_s < LEDGER_FRESH_S => s,
            _ => self.backend.refresh_ledger().map_err(|e| {
                if e.code == ErrorCode::Root {
                    e
                } else {
                    ErrorBody::new(ErrorCode::Root, format!("the Root did not answer ledger.get/ceilings.get: {}", e.message))
                        .with_data(&json!({"reason": "refused", "root_code": e.code.as_str()}))
                }
            })?,
        };
        to_val(Ok(LevelResult { age_s: snap.age_s, ceilings: snap.ceilings, ledger: snap.ledger }))
    }

    /// A job of this lane: `E_NOT_FOUND`, or `E_LANE` for another lane's job.
    fn own_job(&self, ctx: &LaneCtx, job: &str) -> Result<(u64, JobRecord), ErrorBody> {
        let (seq, rec, owner) = self.jobs.lookup(job).ok_or_else(|| ErrorBody::new(ErrorCode::NotFound, format!("no job {job}")))?;
        if owner.as_deref() != Some(ctx.key().as_str()) {
            return Err(ErrorBody::new(ErrorCode::Lane, format!("job {job} belongs to another lane")));
        }
        Ok((seq, rec))
    }

    fn cancel(&self, ctx: &LaneCtx, job: &str) -> Result<Value, ErrorBody> {
        let (seq, _) = self.own_job(ctx, job)?;
        let rec = self.jobs.cancel(seq)?;
        let _ = catch_unwind(AssertUnwindSafe(|| self.backend.cancel(ctx, job)));
        to_val(Ok(rec))
    }

    fn sink<'a>(stream: &'a UnixStream, id: &'a str) -> impl FnMut(&Progress) -> io::Result<()> + 'a {
        move |p: &Progress| send(stream, &ProgressFrame::new(id, p.clone()).to_line())
    }

    fn job_wait(&self, ctx: &LaneCtx, stream: &UnixStream, id: &str, a: &JobWaitArgs) -> Reply {
        let job = match &a.job {
            Some(j) => j.clone(),
            None => {
                let cmd = a.cmd.as_deref().and_then(CommandName::parse).map(|c| c.as_str());
                match self.jobs.newest(&ctx.key(), cmd) {
                    Some(j) => j,
                    None => {
                        let what = cmd.map(|c| format!(" of `{c}`")).unwrap_or_default();
                        return reply(id, Err(ErrorBody::new(ErrorCode::NotFound, format!("no job{what} on this lane"))));
                    }
                }
            }
        };
        let seq = match self.own_job(ctx, &job) {
            Ok((s, _)) => s,
            Err(e) => return reply(id, Err(e)),
        };
        let guard = self.jobs.add_waiter(seq);
        let deadline = Instant::now() + Duration::from_secs(a.timeout());
        let mut sink = Self::sink(stream, id);
        let end = self.jobs.wait(seq, WaitFrom::Now, Some(deadline), &mut sink, &mut || peer_alive(stream));
        let rec = match end {
            WaitEnd::Gone => return None,
            WaitEnd::Reached(r) => r,
            WaitEnd::TimedOut(mut r) => {
                r.timed_out = Some(true);
                r
            }
        };
        let (resp, _) = reply(id, to_val(Ok(rec)))?;
        Some((resp, Some(guard)))
    }

    fn start_job(&self, spec: &LaneSpec, stream: &UnixStream, id: &str, cmd: &Command, ticket: Option<QuotaTicket>) -> Reply {
        let ctx = &spec.ctx;
        let owner = ctx.key();
        // Serve-side idempotency: a pending reload of this lane; an active promote of this SHA.
        let existing = match cmd {
            Command::Reload(_) => self.jobs.active_owned(&owner, "reload").map(|(s, _)| s),
            Command::Promote(PromoteArgs { sha: Some(sha), .. }) => self
                .jobs
                .active_of("promote")
                .into_iter()
                .find(|r| r.sha.as_deref() == Some(sha.as_str()))
                .and_then(|r| self.jobs.lookup(&r.id))
                .map(|(s, _, _)| s),
            _ => None,
        };
        if let Some(seq) = existing {
            let guard = self.jobs.add_waiter(seq);
            return self.job_reply(cmd, seq, Map::new(), guard, stream, id);
        }
        // Path hardening (§7).
        let path = match cmd {
            Command::Gate(a) => Some(paths::resolve(spec, "repo", &a.repo, Want::Dir)),
            Command::TaskSubmit(a) => Some(paths::resolve(spec, "dir", &a.dir, Want::Dir)),
            _ => None,
        };
        let path = match path.transpose() {
            Ok(p) => p,
            Err(e) => return reply(id, Err(e)),
        };
        let ctl = match self.jobs.reserve(ctx, cmd.name().as_str(), cmd.args()) {
            Ok(c) => c,
            Err(e) => return reply(id, Err(internal(format!("cannot create the job record: {e}")))),
        };
        let seq = ctl.seq();
        // Every response counts as a pending final (wait or not): the backend's milestones wait
        // for it before `pause` (§4.1 ordering guarantee).
        let mut guard = self.jobs.add_waiter(seq);
        let b = &*self.backend;
        let p = path.as_deref().unwrap_or(Path::new(""));
        let begun = catch_unwind(AssertUnwindSafe(|| match cmd {
            Command::Reload(a) => b.reload(ctx, a, ctl.clone()),
            Command::Gate(a) => b.gate(ctx, a, p, ctl.clone()),
            Command::Promote(a) => b.promote(ctx, a, ctl.clone()),
            Command::Rollback(a) => b.rollback(ctx, a, ctl.clone()),
            Command::Vendor(a) => b.vendor(ctx, a, ctl.clone()),
            Command::TaskSubmit(a) => b.task_submit(ctx, a, p, ctl.clone()),
            Command::FleetSpawn(a) => b.fleet_spawn(ctx, a, ctl.clone()),
            Command::KernelPromote(a) => b.kernel_promote(ctx, a, ctl.clone()),
            Command::EvoScreen(a) => b.evo_screen(ctx, a, ctl.clone()),
            _ => Err(internal("not a job command")),
        }))
        .unwrap_or_else(|_| {
            klog(&self.cfg.home, "internal_error", json!({"cmd": cmd.name().as_str(), "lane": owner, "panic": "backend panicked"}));
            Err(internal("kernel bug; details in state/kernel/logs/kernel.jsonl"))
        });
        match begun {
            Err(e) => {
                drop(guard);
                self.jobs.discard(seq);
                reply(id, Err(e)) // the quota ticket is dropped: refunded
            }
            Ok(Begun::Existing(job)) => {
                self.jobs.discard(seq);
                let Some((eseq, _, _)) = self.jobs.lookup(&job) else {
                    return reply(id, Err(internal(format!("the backend named an unknown job {job}"))));
                };
                guard.retarget(eseq);
                self.job_reply(cmd, eseq, Map::new(), guard, stream, id)
            }
            Ok(Begun::Started(fields)) => {
                if let Some(t) = ticket {
                    t.commit();
                }
                if let Some(Value::String(sha)) = fields.get("sha") {
                    ctl.set_sha(sha);
                }
                self.jobs.publish(seq);
                self.job_reply(cmd, seq, fields, guard, stream, id)
            }
        }
    }

    /// The response to a job command: at once (`wait:false`), or at the milestone / terminal
    /// state with progress frames before it (`wait:true`).
    fn job_reply(&self, cmd: &Command, seq: u64, mut fields: Map<String, Value>, guard: WaiterGuard, stream: &UnixStream, id: &str) -> Reply {
        let job = job_id(seq);
        let milestone_fields = |fields: &mut Map<String, Value>, rec: &JobRecord| {
            fields.insert("job".into(), Value::String(rec.id.clone()));
            if !matches!(cmd, Command::FleetSpawn(_)) {
                fields.insert("state".into(), Value::String(rec.state.as_str().to_string()));
            }
            if let Some(sha) = &rec.sha
                && matches!(cmd, Command::Promote(_) | Command::KernelPromote(_) | Command::Gate(_))
            {
                fields.entry("sha").or_insert_with(|| Value::String(sha.clone()));
            }
        };
        if !cmd.wait() {
            let Some(mut rec) = self.jobs.record(&job) else {
                return reply(id, Err(internal(format!("job {job} vanished"))));
            };
            // The immediate answer says `queued` (§4.1) even if the backend's thread already
            // started; a milestone or terminal state set by then is reported as is.
            if matches!(rec.state, JobState::Running | JobState::Gating | JobState::HandingOff) {
                rec.state = JobState::Queued;
            }
            milestone_fields(&mut fields, &rec);
            return Some((Response::ok_value(id, Value::Object(fields)), Some(guard)));
        }
        let mut sink = Self::sink(stream, id);
        let rec = match self.jobs.wait(seq, WaitFrom::Start, None, &mut sink, &mut || peer_alive(stream)) {
            WaitEnd::Gone => return None,
            WaitEnd::Reached(r) | WaitEnd::TimedOut(r) => r,
        };
        let resp = match rec.state {
            JobState::Failed => {
                let e = rec.error.clone().unwrap_or_else(|| {
                    internal(format!("job {} failed{}", rec.id, rec.reason.as_deref().map(|r| format!(": {r}")).unwrap_or_default()))
                });
                Response::err(Some(id.to_string()), sanitize_error(e))
            }
            JobState::Cancelled => Response::err(
                Some(id.to_string()),
                rec.error.clone().unwrap_or_else(|| ErrorBody::new(ErrorCode::Cancelled, "job was cancelled")),
            ),
            _ if returns_result(cmd) && rec.state == JobState::Succeeded && rec.result.is_some() => {
                Response::ok_value(id, rec.result.clone().unwrap_or(Value::Null))
            }
            _ => {
                milestone_fields(&mut fields, &rec);
                if matches!(cmd, Command::Promote(_) | Command::KernelPromote(_)) {
                    fields.insert("stages".into(), init_proto::icj::to_value(&rec.stages).unwrap_or(Value::Array(vec![])));
                }
                if let (Command::KernelPromote(_), Some(r)) = (cmd, &rec.root_job) {
                    fields.insert("root_job".into(), Value::String(r.clone()));
                }
                Response::ok_value(id, Value::Object(fields))
            }
        };
        Some((resp, Some(guard)))
    }
}

/// `true` when `path` is an existing Unix socket.
pub fn is_socket(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_socket())
}

/// Append a diagnostic line to the kernel log (for backends that want the same file).
pub fn log(home: &Path, ev: &str, fields: Value) {
    fsx::klog(home, ev, fields);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn ctime(p: &Path) -> (i64, i64) {
        let m = fs::metadata(p).unwrap();
        (m.ctime(), m.ctime_nsec())
    }

    /// FIX1 (S1 gap 3): an existing 0700 lane directory is left untouched (no chmod, so its ctime
    /// does not move); any other mode is corrected; a missing directory is created 0700.
    #[test]
    fn prepare_lane_dir_chmods_only_when_needed() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/fix1");
        fs::create_dir_all(&base).unwrap();
        let home = base.canonicalize().unwrap().join(format!("lanedir-{}", std::process::id()));
        if home.exists() {
            fs::remove_dir_all(&home).unwrap(); // exact path: our own leftover
        }
        let run = home.join("run");
        fs::create_dir_all(&run).unwrap();
        fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).unwrap();
        let before = ctime(&run);
        std::thread::sleep(Duration::from_millis(20));
        prepare_lane_dir(&run).unwrap();
        assert_eq!(ctime(&run), before, "an existing 0700 run/ must not be chmod-ed (Z4 denies it)");

        let fleet = home.join("run/fleet/b1");
        fs::create_dir_all(&fleet).unwrap();
        fs::set_permissions(&fleet, fs::Permissions::from_mode(0o755)).unwrap();
        prepare_lane_dir(&fleet).unwrap();
        assert_eq!(fs::metadata(&fleet).unwrap().permissions().mode() & 0o777, 0o700);

        let probe = home.join("probe/0123456789ab/run");
        prepare_lane_dir(&probe).unwrap();
        assert_eq!(fs::metadata(&probe).unwrap().permissions().mode() & 0o777, 0o700);

        fs::remove_dir_all(&home).unwrap(); // exact path
    }
}
