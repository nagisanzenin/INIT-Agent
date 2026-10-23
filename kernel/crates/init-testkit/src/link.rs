//! Both ends of the Root↔kernel control link (root/root.md §4.1.1; kernel side: kernel/supervise.md
//! §3.13.1), shared by `init-fakeroot` and `init-fakekernel`.
//!
//! - fd 3: a `SOCK_STREAM` socketpair of ICJ lines (≤ 1 MiB): requests, responses, events, all
//!   typed by `init_proto::envelope::{Request, Response, Event, LinkFrame}`.
//! - fd 4: a `SOCK_DGRAM` socketpair carrying fds; each datagram is tagged `{"id","n"}`
//!   (`init_proto::root_link::FdTag`) and is sent **before** the line with the same id. Unmatched
//!   datagrams are held 60 s, at most 32, then closed.
//!
//! Requests may be pipelined; a reader thread routes responses to their waiting caller by id, and
//! hands requests and events to the owner of the link through a channel.

use crate::sys;
use init_proto::envelope::{Event, LinkFrame, MAX_RESPONSE_LINE, Request, Response};
use init_proto::icj::{self, V1};
use init_proto::root_link::{FD_HOLD_MAX, FD_HOLD_S, FdTag};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the reader hands to the owner of the link.
#[derive(Debug)]
pub enum Incoming {
    Request(Request),
    Event(Event),
    /// A line that is not a valid frame (the Root counts three in a row as a crash).
    Bad(String),
    /// The peer closed fd 3.
    Eof,
}

/// A frame-trace hook: `(direction, line)` with direction `"in"` or `"out"`.
pub type Trace = Arc<dyn Fn(&'static str, &[u8]) + Send + Sync>;

/// One end of a control link.
pub struct Link {
    writer: Mutex<UnixStream>,
    fdch: UnixDatagram,
    fd_recv: Mutex<Vec<(String, Instant, Vec<OwnedFd>)>>,
    prefix: &'static str,
    next: AtomicU64,
    pending: Mutex<HashMap<String, Sender<Response>>>,
    closed: AtomicBool,
    trace: Option<Trace>,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link")
            .field("prefix", &self.prefix)
            .field("closed", &self.closed)
            .finish()
    }
}

fn link_err(m: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.into())
}

impl Link {
    /// Wrap the two sockets. `prefix` is `"k-"` for the kernel side and `"r-"` for the Root side.
    /// Spawns the reader thread; the returned receiver yields requests, events, bad lines and EOF.
    pub fn new(
        stream: UnixStream,
        fdch: UnixDatagram,
        prefix: &'static str,
        trace: Option<Trace>,
    ) -> io::Result<(Arc<Link>, Receiver<Incoming>)> {
        let reader = stream.try_clone()?;
        let link = Arc::new(Link {
            writer: Mutex::new(stream),
            fdch,
            fd_recv: Mutex::new(Vec::new()),
            prefix,
            next: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            trace,
        });
        let (tx, rx) = mpsc::channel();
        let l = link.clone();
        std::thread::Builder::new()
            .name(format!("link-reader-{prefix}"))
            .spawn(move || l.read_loop(reader, tx))?;
        Ok((link, rx))
    }

    fn read_loop(self: Arc<Self>, s: UnixStream, tx: Sender<Incoming>) {
        let mut r = BufReader::new(s);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match (&mut r)
                .take(MAX_RESPONSE_LINE as u64 + 2)
                .read_until(b'\n', &mut buf)
            {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if buf.last() != Some(&b'\n') {
                if buf.len() > MAX_RESPONSE_LINE {
                    let _ = tx.send(Incoming::Bad("line longer than 1 MiB".into()));
                }
                break;
            }
            if let Some(t) = &self.trace {
                t("in", &buf);
            }
            match LinkFrame::parse_line(&buf) {
                Ok(LinkFrame::Response(resp)) => {
                    let waiter = resp.id.as_ref().and_then(|id| {
                        self.pending
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(id)
                    });
                    match waiter {
                        Some(w) => drop(w.send(resp)),
                        None => drop(tx.send(Incoming::Bad(format!(
                            "response to unknown id {:?}",
                            resp.id
                        )))),
                    }
                }
                Ok(LinkFrame::Request(req)) => drop(tx.send(Incoming::Request(req))),
                Ok(LinkFrame::Event(ev)) => drop(tx.send(Incoming::Event(ev))),
                Err(e) => drop(tx.send(Incoming::Bad(e.to_string()))),
            }
        }
        self.closed.store(true, Ordering::SeqCst);
        // Fail every waiter: dropping their senders wakes them with a disconnect.
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let _ = tx.send(Incoming::Eof);
    }

    /// True after the peer closed fd 3.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Write one line (newline included).
    pub fn send_line(&self, line: &[u8]) -> io::Result<()> {
        if let Some(t) = &self.trace {
            t("out", line);
        }
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        w.write_all(line)?;
        w.flush()
    }

    /// Close both directions of fd 3 (the peer sees EOF).
    pub fn shutdown(&self) {
        let w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let _ = w.shutdown(Shutdown::Both);
    }

    /// The next request id (`k-<n>` or `r-<n>`).
    pub fn next_id(&self) -> String {
        format!(
            "{}{}",
            self.prefix,
            self.next.fetch_add(1, Ordering::SeqCst)
        )
    }

    fn send_fd_tag(&self, id: &str, fds: &[RawFd]) -> io::Result<()> {
        let tag = icj::to_vec(&FdTag {
            id: id.to_string(),
            n: fds.len() as u32,
        })
        .map_err(|e| link_err(e.to_string()))?;
        sys::send_fds(&self.fdch, &tag, fds)
    }

    /// Send a request and wait for its response. `fds` (if any) travel on fd 4 first, tagged
    /// with the request id (e.g. `store.put`).
    pub fn request_with_fds(
        &self,
        cmd: &str,
        args: Value,
        fds: &[RawFd],
        timeout: Duration,
    ) -> io::Result<Response> {
        if self.is_closed() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "control link closed",
            ));
        }
        let id = self.next_id();
        let req = Request {
            v: V1,
            id: id.clone(),
            cmd: cmd.to_string(),
            args,
        };
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), tx);
        let sent = (|| {
            if !fds.is_empty() {
                self.send_fd_tag(&id, fds)?;
            }
            self.send_line(&req.to_line())
        })();
        if let Err(e) = sent {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(timeout) {
            Ok(r) => Ok(r),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("no response to {cmd} ({id}) within {timeout:?}"),
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "control link closed",
            )),
        }
    }

    /// Send a request and wait for its response.
    pub fn request(&self, cmd: &str, args: Value, timeout: Duration) -> io::Result<Response> {
        self.request_with_fds(cmd, args, &[], timeout)
    }

    /// Send a request with typed args.
    pub fn request_typed<A: Serialize>(
        &self,
        cmd: &str,
        args: &A,
        timeout: Duration,
    ) -> io::Result<Response> {
        let v = icj::to_value(args).map_err(|e| link_err(e.to_string()))?;
        self.request(cmd, v, timeout)
    }

    /// Send a final response; `fds` (if any) go first on fd 4, tagged with the response id.
    pub fn respond_with_fds(&self, resp: &Response, fds: &[RawFd]) -> io::Result<()> {
        if !fds.is_empty() {
            let id = resp.id.clone().unwrap_or_default();
            self.send_fd_tag(&id, fds)?;
        }
        self.send_line(&resp.to_line())
    }

    /// Send a final response.
    pub fn respond(&self, resp: &Response) -> io::Result<()> {
        self.respond_with_fds(resp, &[])
    }

    /// Send an event `{"data","ev","v":1}`.
    pub fn event(&self, ev: &str, data: Value) -> io::Result<()> {
        let e = Event {
            v: V1,
            ev: ev.to_string(),
            data,
        };
        self.send_line(&icj::to_line(&e).map_err(|e| link_err(e.to_string()))?)
    }

    /// Take the fds tagged `id` from fd 4, waiting up to `timeout`. Other datagrams are held (60 s,
    /// at most 32, oldest closed first).
    pub fn take_fds(&self, id: &str, timeout: Duration) -> io::Result<Vec<OwnedFd>> {
        let deadline = Instant::now() + timeout;
        let mut held = self.fd_recv.lock().unwrap_or_else(|e| e.into_inner());
        held.retain(|(_, t, _)| t.elapsed() < Duration::from_secs(FD_HOLD_S));
        if let Some(i) = held.iter().position(|(h, _, _)| h == id) {
            return Ok(held.remove(i).2);
        }
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("no fds tagged {id}"),
                ));
            }
            let (tag, fds) = match sys::recv_fds(&self.fdch, Some(left)) {
                Ok(x) => x,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(e) => return Err(e),
            };
            let tag: FdTag = match serde_json::from_slice(&tag) {
                Ok(t) => t,
                Err(_) => continue, // an untagged datagram: its fds are closed by drop
            };
            if tag.n as usize != fds.len() {
                return Err(link_err(format!(
                    "fd tag {} says {} fds, got {}",
                    tag.id,
                    tag.n,
                    fds.len()
                )));
            }
            if tag.id == id {
                return Ok(fds);
            }
            if held.len() >= FD_HOLD_MAX {
                held.remove(0);
            }
            held.push((tag.id, Instant::now(), fds));
        }
    }

    /// The raw fd of fd 4 (for tests).
    pub fn fd_channel_raw(&self) -> RawFd {
        self.fdch.as_raw_fd()
    }
}

/// One end of a link with its incoming channel.
pub type End = (Arc<Link>, Receiver<Incoming>);

/// A connected pair of links over fresh socketpairs, for in-process tests: (kernel side, Root side).
pub fn pair(trace: Option<Trace>) -> io::Result<(End, End)> {
    let (sk, sr) = UnixStream::pair()?;
    let (dk, dr) = UnixDatagram::pair()?;
    Ok((
        Link::new(sk, dk, "k-", trace.clone())?,
        Link::new(sr, dr, "r-", trace)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use init_proto::root_link::{KernelRequest, SpawnResult};
    use serde_json::json;
    use std::io::Read;

    #[test]
    fn request_response_events_and_fds() {
        let ((k, k_rx), (r, r_rx)) = pair(None).unwrap();
        // Root side: answer one request with 1 fd, then send an event.
        let rr = r.clone();
        let h = std::thread::spawn(move || {
            let Incoming::Request(req) = r_rx.recv().unwrap() else {
                panic!("expected a request")
            };
            assert_eq!(req.cmd, "spawn");
            let (rd, mut wr) = std::io::pipe().unwrap();
            wr.write_all(b"via fd 4").unwrap();
            drop(wr);
            let res = SpawnResult {
                instance: "a3f09c1e5b7d".into(),
                pid: 4242,
                started_at: "t".into(),
            };
            rr.respond_with_fds(
                &Response::ok(req.id.clone(), &res).unwrap(),
                &[rd.as_raw_fd()],
            )
            .unwrap();
            rr.event("level", json!({"x": 1})).unwrap();
            req
        });
        let resp = k
            .request("spawn", json!({"kind": "body"}), Duration::from_secs(5))
            .unwrap();
        let req = h.join().unwrap();
        assert_eq!(resp.id.as_deref(), Some(req.id.as_str()));
        assert_eq!(resp.result_as::<SpawnResult>().unwrap().pid, 4242);
        let fds = k.take_fds(&req.id, Duration::from_secs(2)).unwrap();
        let mut s = String::new();
        std::fs::File::from(fds.into_iter().next().unwrap())
            .read_to_string(&mut s)
            .unwrap();
        assert_eq!(s, "via fd 4");
        let Incoming::Event(e) = k_rx.recv_timeout(Duration::from_secs(2)).unwrap() else {
            panic!()
        };
        assert_eq!(e.ev, "level");
        // Ids: k-1, then k-2.
        assert_eq!(req.id, "k-1");
        assert_eq!(KernelRequest::Ready.to_request(2).id, k.next_id());
        // EOF reaches both sides; a pending request fails fast.
        r.shutdown();
        assert!(matches!(
            k_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Incoming::Eof
        ));
        assert!(
            k.request("ready", json!({}), Duration::from_secs(2))
                .is_err()
        );
    }

    #[test]
    fn unmatched_fds_are_held() {
        let ((k, _k_rx), (r, _r_rx)) = pair(None).unwrap();
        let (a, _b) = std::io::pipe().unwrap();
        r.send_fd_tag("r-9", &[a.as_raw_fd()]).unwrap();
        r.send_fd_tag("r-10", &[a.as_raw_fd(), a.as_raw_fd()])
            .unwrap();
        assert_eq!(k.take_fds("r-10", Duration::from_secs(2)).unwrap().len(), 2);
        assert_eq!(k.take_fds("r-9", Duration::from_secs(2)).unwrap().len(), 1);
        assert!(k.take_fds("r-11", Duration::from_millis(30)).is_err());
    }
}
