//! The control link, kernel side (supervise.md §3.13; root/root.md §4.1 owns the protocol).
//!
//! - fd 3: a `SOCK_STREAM` socketpair carrying ICJ lines ≤ 1 MiB: requests (ids `k-<n>` from
//!   the kernel, `r-<n>` from the Root), responses (out of order, matched by id) and events.
//! - fd 4: a `SOCK_DGRAM` socketpair carrying passed fds; each datagram is tagged `{"id","n"}`
//!   and arrives **before** the response line with that id. Unmatched datagrams are kept for
//!   60 s (at most 32) and then closed.
//!
//! One reader thread parses fd 3. Responses wake the waiting caller; Root requests and events go
//! to the supervisor's channel as [`Inbound`]; `exited` events are also recorded on the
//! [`ExitBoard`] so builds and probes can wait on their own instance. EOF → [`Inbound::Lost`].

use crate::instance::{ExitBoard, ExitInfo, SpawnError, SpawnRequest, Spawned, Spawner};
use init_proto::envelope::{ErrorBody, Event, LinkFrame, Response};
use init_proto::root_link::{
    FdTag, KernelEvent, KernelRequest, KillArgs, KillMode, RootErrorCode, RootEvent, RootRequest, SpawnResult, FD_HOLD_MAX, FD_HOLD_S,
};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `spawn` waits up to 10 s, `store.put` 60 s (§4.9 `root_request_timeout_ms` for the rest).
pub const SPAWN_TIMEOUT: Duration = Duration::from_secs(10);
pub const STORE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_LINE: usize = init_proto::envelope::MAX_RESPONSE_LINE;

/// What the link hands the supervisor.
#[derive(Debug)]
pub enum Inbound {
    /// A Root → kernel request; answer it with [`RootLink::respond`].
    Request { id: String, req: RootRequest },
    /// A Root → kernel event.
    Event(RootEvent),
    /// EOF or an error on fd 3: the control link is gone (§3.13.1).
    Lost,
}

/// Why a call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkError {
    /// The Root answered `ok:false`.
    Root(ErrorBody),
    /// No answer in time.
    Timeout,
    /// The link is down.
    Lost,
    /// A malformed answer or a local error.
    Other(String),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Root(e) => write!(f, "{e}"),
            LinkError::Timeout => write!(f, "control link: no answer in time"),
            LinkError::Lost => write!(f, "control link lost"),
            LinkError::Other(m) => write!(f, "control link: {m}"),
        }
    }
}

impl std::error::Error for LinkError {}

impl LinkError {
    /// The `root.*` code when the Root refused.
    pub fn root_code(&self) -> Option<RootErrorCode> {
        match self {
            LinkError::Root(e) => Some(RootErrorCode::of(e)),
            _ => None,
        }
    }
}

struct FdChan {
    sock: OwnedFd,
    stash: Vec<(String, Instant, Vec<OwnedFd>)>,
}

/// The kernel's end of the control link.
pub struct RootLink {
    w: Mutex<UnixStream>,
    fds: Mutex<FdChan>,
    pending: Mutex<HashMap<String, Sender<Response>>>,
    next: AtomicU64,
    alive: AtomicBool,
    board: Arc<ExitBoard>,
    /// `root_request_timeout_ms`.
    pub request_timeout: Duration,
}

impl RootLink {
    /// Wrap fds 3 and 4 and start the reader thread.
    pub fn start(ctl: OwnedFd, fdpass: OwnedFd, sink: Sender<Inbound>, request_timeout: Duration) -> std::io::Result<Arc<RootLink>> {
        let stream = UnixStream::from(ctl);
        let reader = stream.try_clone()?;
        let link = Arc::new(RootLink {
            w: Mutex::new(stream),
            fds: Mutex::new(FdChan { sock: fdpass, stash: Vec::new() }),
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            alive: AtomicBool::new(true),
            board: ExitBoard::new(),
            request_timeout,
        });
        let l = link.clone();
        std::thread::Builder::new().name("root-link".into()).spawn(move || l.read_loop(reader, sink))?;
        Ok(link)
    }

    /// The board `exited` events are recorded on.
    pub fn board(&self) -> Arc<ExitBoard> {
        self.board.clone()
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn read_loop(self: Arc<Self>, s: UnixStream, sink: Sender<Inbound>) {
        let mut r = BufReader::with_capacity(64 * 1024, s);
        let mut line = Vec::with_capacity(4096);
        loop {
            line.clear();
            match (&mut r).take(MAX_LINE as u64 + 2).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if !line.ends_with(b"\n") {
                if line.len() > MAX_LINE {
                    crate::klog::log("link", "control-link line over 1 MiB; dropping the connection");
                }
                break; // torn line at EOF, or over-long
            }
            match LinkFrame::parse_line(&line) {
                Ok(LinkFrame::Response(resp)) => {
                    let Some(id) = resp.id.clone() else { continue };
                    if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(resp);
                    }
                }
                Ok(LinkFrame::Request(req)) => match RootRequest::parse(&req.cmd, &req.args) {
                    Ok(rr) => {
                        let _ = sink.send(Inbound::Request { id: req.id, req: rr });
                    }
                    Err(e) => self.respond(&req.id, Err(e)),
                },
                Ok(LinkFrame::Event(ev)) => match RootEvent::from_event(&ev) {
                    Ok(re) => {
                        if let RootEvent::Exited(x) = &re {
                            self.board.record(&x.instance, ExitInfo { code: x.code, signal: x.signal, at: x.at.clone() });
                        }
                        let _ = sink.send(Inbound::Event(re));
                    }
                    Err(e) => crate::klog::log("link", &format!("bad event {}: {e}", ev.ev)),
                },
                Err(e) => crate::klog::log("link", &format!("malformed control-link frame: {e}")),
            }
        }
        self.alive.store(false, Ordering::SeqCst);
        self.pending.lock().unwrap().clear(); // waiters see a closed channel → Lost
        let _ = sink.send(Inbound::Lost);
    }

    fn write_line(&self, bytes: &[u8]) -> Result<(), LinkError> {
        if !self.is_alive() {
            return Err(LinkError::Lost);
        }
        // The Root ends the link on a line over 1 MiB, which kills the kernel and counts as a
        // crash (root/root.md §3.6.3): such a line is never sent (K-LINE; `ownerline` bounds
        // `owner.out` before it gets here).
        if bytes.len() > MAX_LINE {
            crate::klog::log("link", &format!("a {}-byte control-link line not sent (over 1 MiB)", bytes.len()));
            return Err(LinkError::Other("line over 1 MiB not sent".into()));
        }
        let mut w = self.w.lock().unwrap();
        w.write_all(bytes).and_then(|_| w.flush()).map_err(|_| LinkError::Lost)
    }

    /// Send a request without waiting; the receiver yields the response.
    pub fn send(&self, req: &KernelRequest) -> Result<(String, Receiver<Response>), LinkError> {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        let frame = req.to_request(n);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(frame.id.clone(), tx);
        if let Err(e) = self.write_line(&frame.to_line()) {
            self.pending.lock().unwrap().remove(&frame.id);
            return Err(e);
        }
        Ok((frame.id, rx))
    }

    fn wait(&self, id: &str, rx: Receiver<Response>, timeout: Duration) -> Result<Value, LinkError> {
        match rx.recv_timeout(timeout) {
            Ok(resp) => resp.result.map_err(LinkError::Root),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(id);
                Err(LinkError::Timeout)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(LinkError::Lost),
        }
    }

    /// A request and its result.
    pub fn call(&self, req: &KernelRequest, timeout: Duration) -> Result<Value, LinkError> {
        let (id, rx) = self.send(req)?;
        self.wait(&id, rx, timeout)
    }

    /// A request whose answer carries `n` fds on fd 4.
    pub fn call_fds(&self, req: &KernelRequest, n: usize, timeout: Duration) -> Result<(Value, Vec<OwnedFd>), LinkError> {
        let start = Instant::now();
        let (id, rx) = self.send(req)?;
        let v = self.wait(&id, rx, timeout)?;
        let left = timeout.saturating_sub(start.elapsed()).max(Duration::from_secs(2));
        let fds = self.take_fds(&id, left)?;
        if fds.len() != n {
            return Err(LinkError::Other(format!("expected {n} fds with {id}, got {}", fds.len())));
        }
        Ok((v, fds))
    }

    /// A request carrying one fd on fd 4 (`store.put`): the tagged datagram goes first, then the
    /// line (root/root.md §4.1.1).
    pub fn call_with_fd(&self, req: &KernelRequest, fd: std::os::fd::BorrowedFd<'_>, timeout: Duration) -> Result<Value, LinkError> {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        let frame = req.to_request(n);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(frame.id.clone(), tx);
        let tag = serde_json::to_vec(&FdTag { id: frame.id.clone(), n: 1 }).expect("tag");
        {
            let ch = self.fds.lock().unwrap();
            if let Err(e) = crate::sys::send_fds(ch.sock.as_fd(), &tag, &[fd]) {
                self.pending.lock().unwrap().remove(&frame.id);
                return Err(LinkError::Other(format!("fd channel: {e}")));
            }
        }
        if let Err(e) = self.write_line(&frame.to_line()) {
            self.pending.lock().unwrap().remove(&frame.id);
            return Err(e);
        }
        self.wait(&frame.id, rx, timeout)
    }

    /// The fds tagged `id` (the datagram precedes the response line, so this rarely waits).
    pub fn take_fds(&self, id: &str, timeout: Duration) -> Result<Vec<OwnedFd>, LinkError> {
        let deadline = Instant::now() + timeout;
        let mut ch = self.fds.lock().unwrap();
        let hold = Duration::from_secs(FD_HOLD_S);
        ch.stash.retain(|(_, at, _)| at.elapsed() < hold);
        if let Some(i) = ch.stash.iter().position(|(t, _, _)| t == id) {
            return Ok(ch.stash.remove(i).2);
        }
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(LinkError::Timeout);
            }
            let got = crate::sys::recv_fds(ch.sock.as_fd(), Some(left)).map_err(|e| LinkError::Other(format!("fd channel: {e}")))?;
            let Some((payload, fds)) = got else { return Err(LinkError::Timeout) };
            let tag: FdTag = match serde_json::from_slice(&payload) {
                Ok(t) => t,
                Err(_) => continue, // untagged: close the fds (dropped here)
            };
            if tag.id == id {
                return Ok(fds);
            }
            if ch.stash.len() >= FD_HOLD_MAX {
                ch.stash.remove(0);
            }
            ch.stash.push((tag.id, Instant::now(), fds));
        }
    }

    /// Answer a Root request.
    pub fn respond(&self, id: &str, result: Result<Value, ErrorBody>) {
        let r = Response { id: Some(id.to_string()), result };
        if let Err(LinkError::Other(_)) = self.write_line(&r.to_line()) {
            // Over the line limit: the Root still gets an answer.
            let e = ErrorBody::new(init_proto::errors::ErrorCode::Internal, "response over 1 MiB");
            let _ = self.write_line(&Response { id: Some(id.to_string()), result: Err(e) }.to_line());
        }
    }

    /// Send a kernel → Root event (`hb`, `owner.out`).
    pub fn event(&self, ev: &KernelEvent) {
        match ev.to_event() {
            Ok(e) => {
                let _ = self.write_line(&init_proto::icj::to_line(&e).unwrap_or_default());
            }
            Err(e) => crate::klog::log("link", &format!("event not ICJ: {e}")),
        }
    }

    /// Send a raw event (used for frames already built as `Event`).
    pub fn raw_event(&self, e: &Event) {
        let _ = self.write_line(&init_proto::icj::to_line(e).unwrap_or_default());
    }
}

impl Spawner for RootLink {
    fn spawn(&self, req: &SpawnRequest) -> Result<Spawned, SpawnError> {
        let (v, mut fds) = self.call_fds(&KernelRequest::Spawn(req.to_args()), 3, SPAWN_TIMEOUT).map_err(|e| match e {
            LinkError::Root(b) => SpawnError::Refused { code: b.code.as_str().to_string(), message: b.message },
            other => SpawnError::Link(other.to_string()),
        })?;
        let r: SpawnResult = serde_json::from_value(v).map_err(|e| SpawnError::Link(format!("spawn result: {e}")))?;
        let stderr = fds.pop().unwrap();
        let stdout = fds.pop().unwrap();
        let stdin = fds.pop().unwrap();
        Ok(Spawned { instance: r.instance, pid: r.pid, started_at: r.started_at, stdin, stdout, stderr })
    }

    fn kill(&self, instance: &str, mode: KillMode) -> Result<(), SpawnError> {
        let req = KernelRequest::Kill(KillArgs { instance: instance.to_string(), mode });
        match self.call(&req, self.request_timeout) {
            Ok(_) => Ok(()),
            // An instance that already ended is not an error for the caller.
            Err(LinkError::Root(b)) if RootErrorCode::of(&b) == RootErrorCode::NotFound => Ok(()),
            Err(LinkError::Root(b)) => Err(SpawnError::Refused { code: b.code.as_str().to_string(), message: b.message }),
            Err(e) => Err(SpawnError::Link(e.to_string())),
        }
    }

    fn wait_exit(&self, instance: &str, timeout: Duration) -> Option<ExitInfo> {
        self.board.wait(instance, timeout)
    }
}
