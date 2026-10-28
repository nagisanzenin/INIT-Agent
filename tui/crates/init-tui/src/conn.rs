//! Connections to `ROOT_HOME/run/owner.sock` (docs/tui.md §3.2, §3.8, §3.11): the attach connection
//! with its reader thread, the poller's request connection, the backoff, and the `/panic` fallback.
//! T-1: this socket is the only IPC endpoint. T-3: the only program ever run is
//! `ROOT_HOME/bin/init-root --root-home <ROOT_HOME> panic`, on the fallback path.

use crate::model::epoch_of;
use crate::proto;
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Events for the main loop, from every thread.
#[derive(Debug)]
pub enum Ev {
    Attached { cn: u64, writer: UnixStream, epoch: Option<String>, started_at: Option<String> },
    AttachFailed { cn: u64, unreachable: bool, msg: String },
    Frame { cn: u64, raw: String, v: Value, at: Instant },
    Malformed { cn: u64 },
    Disconnected { cn: u64 },
    Status(Value),
    Egress(Value),
    /// The answer to an ADR-009 access request (`crate::access`).
    Access { op: crate::access::Op, result: Result<Value, ReqErr> },
    ReqFailed { cmd: &'static str },
    /// The answer to a UX-KEYMODEL setup request (`crate::setup`).
    Setup { op: crate::setup::Op, result: Result<Value, ReqErr> },
    PanicDone(Result<String, String>),
    /// TUI-OBSERVE: the Root's `lineage.tail` answer, for the transcript view's lineage panel.
    Lineage(Result<Value, ReqErr>),
    Key(crossterm::event::KeyEvent, Instant),
    Paste(String, Instant),
    Resize(u16, u16),
    Line(String),
    StdinEof,
    Signal(i32),
}

pub fn sock(root_home: &Path) -> PathBuf {
    root_home.join("run/owner.sock")
}

#[derive(Debug)]
pub enum ReqErr {
    Unreachable(io::Error),
    Proto(String),
    /// `error.code`, `error.message`, `error.data` (null when absent).
    Refused(String, String, Value),
}

fn read_answer(s: &UnixStream, id: &str) -> Result<Value, ReqErr> {
    let mut r = BufReader::new(s);
    loop {
        let mut line = String::new();
        match (&mut r).take(proto::FRAME_IN_MAX).read_line(&mut line) {
            Ok(0) => return Err(ReqErr::Proto("connection closed".into())),
            Ok(_) => {}
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => return Err(ReqErr::Unreachable(e)),
            Err(e) => return Err(ReqErr::Proto(e.to_string())),
        }
        let v: Value = serde_json::from_str(&line).map_err(|_| ReqErr::Proto("malformed answer".into()))?;
        if v.get("t").is_some() {
            continue; // not an answer (a server that broadcasts here); skip it
        }
        if v["id"] != id && !v["id"].is_null() {
            continue;
        }
        if v["ok"] == true {
            return Ok(v["result"].clone());
        }
        let e = &v["error"];
        return Err(ReqErr::Refused(e["code"].as_str().unwrap_or("").into(), e["message"].as_str().unwrap_or("").into(), e["data"].clone()));
    }
}

/// One request on a fresh connection, answered within `timeout`.
pub fn request_once(sock: &Path, cmd: &str, timeout: Duration) -> Result<Value, ReqErr> {
    request_args(sock, cmd, serde_json::json!({}), timeout)
}

/// One request with arguments on a fresh connection, answered within `timeout`.
pub fn request_args(sock: &Path, cmd: &str, args: Value, timeout: Duration) -> Result<Value, ReqErr> {
    let mut s = UnixStream::connect(sock).map_err(ReqErr::Unreachable)?;
    s.set_read_timeout(Some(timeout)).map_err(|e| ReqErr::Proto(e.to_string()))?;
    s.set_write_timeout(Some(timeout)).map_err(|e| ReqErr::Proto(e.to_string()))?;
    writeln!(s, "{}", proto::request_with(1, cmd, args)).map_err(|e| ReqErr::Proto(e.to_string()))?;
    read_answer(&s, "t-1")
}

/// An ADR-009 access request on its own thread; the answer comes back as `Ev::Access`.
pub fn spawn_access(sock: PathBuf, req: crate::access::Req, tx: Sender<Ev>) {
    std::thread::spawn(move || {
        let result = request_args(&sock, req.op.cmd(), req.args.clone(), req.op.timeout());
        let _ = tx.send(Ev::Access { op: req.op, result });
    });
}

/// A UX-KEYMODEL setup request on its own thread; the answer comes back as `Ev::Setup`.
/// TUI-OBSERVE: `lineage.tail{n}` on its own connection (the lineage panel, when Ctrl-O opens).
pub fn spawn_lineage(sock: PathBuf, n: u64, tx: Sender<Ev>) {
    std::thread::spawn(move || {
        let result = request_args(&sock, "lineage.tail", serde_json::json!({"n": n}), Duration::from_secs(5));
        let _ = tx.send(Ev::Lineage(result));
    });
}

pub fn spawn_setup(sock: PathBuf, req: crate::setup::Req, tx: Sender<Ev>) {
    std::thread::spawn(move || {
        let result = request_args(&sock, req.op.cmd(), req.args.clone(), req.op.timeout());
        let _ = tx.send(Ev::Setup { op: req.op, result });
    });
}

/// `keys.set` on its own connection: the frame is built in one buffer, written, and zeroed with the secret. The
/// secret is never in a log, an argv or the transcript (docs/ux-v1.md §3.4.5).
pub fn spawn_key(sock: PathBuf, provider: String, mut secret: Vec<u8>, tx: Sender<Ev>) {
    std::thread::spawn(move || {
        let frame = crate::setup::key_frame(&provider, &secret);
        crate::setup::zero(&mut secret);
        let op = crate::setup::Op::Key;
        let Some(mut frame) = frame else {
            let e = ReqErr::Refused("keys.invalid".into(), "a key is 1–512 bytes of UTF-8 without control characters".into(), Value::Null);
            let _ = tx.send(Ev::Setup { op, result: Err(e) });
            return;
        };
        let result = (|| {
            let mut s = UnixStream::connect(&sock).map_err(ReqErr::Unreachable)?;
            s.set_read_timeout(Some(op.timeout())).map_err(|e| ReqErr::Proto(e.to_string()))?;
            s.set_write_timeout(Some(op.timeout())).map_err(|e| ReqErr::Proto(e.to_string()))?;
            let w = s.write_all(&frame);
            crate::setup::zero(&mut frame);
            w.map_err(|e| ReqErr::Proto(e.to_string()))?;
            read_answer(&s, "t-1")
        })();
        crate::setup::zero(&mut frame);
        let _ = tx.send(Ev::Setup { op, result });
    });
}

/// One attach attempt on its own thread: `status` first (the epoch, §3.11), then the attach
/// connection and its reader. The reader drains the socket continuously into the unbounded channel,
/// so the Root's 1 s write timeout never trips on this client (§3.2).
pub fn spawn_attach(cn: u64, sock: PathBuf, replay: bool, since: Option<(u64, String)>, tx: Sender<Ev>) {
    std::thread::spawn(move || {
        let (epoch, started) = match request_once(&sock, "status", Duration::from_secs(2)) {
            Ok(v) => {
                let e = epoch_of(&v);
                let st = v["root"]["started_at"].as_str().map(str::to_string);
                let _ = tx.send(Ev::Status(v));
                (e, st)
            }
            Err(ReqErr::Unreachable(e)) => {
                let _ = tx.send(Ev::AttachFailed { cn, unreachable: true, msg: e.to_string() });
                return;
            }
            Err(ReqErr::Proto(m)) => {
                let _ = tx.send(Ev::AttachFailed { cn, unreachable: false, msg: format!("status: {m}") });
                return;
            }
            Err(ReqErr::Refused(..)) => (None, None),
        };
        let s = match UnixStream::connect(&sock) {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(Ev::AttachFailed { cn, unreachable: true, msg: e.to_string() });
                return;
            }
        };
        let _ = s.set_write_timeout(Some(Duration::from_secs(1)));
        let since_ref = since.as_ref().map(|(n, e)| (*n, e.as_str()));
        // `since` only with the same epoch as the status just read (the Root checks it too)
        let since_ref = since_ref.filter(|(_, e)| epoch.as_deref() == Some(*e));
        let hello = proto::attach(replay, since_ref);
        let (Ok(mut w), Ok(r)) = (s.try_clone(), s.try_clone()) else {
            let _ = tx.send(Ev::AttachFailed { cn, unreachable: true, msg: "socket clone".into() });
            return;
        };
        if writeln!(w, "{hello}").is_err() {
            let _ = tx.send(Ev::AttachFailed { cn, unreachable: true, msg: "attach write failed".into() });
            return;
        }
        if tx.send(Ev::Attached { cn, writer: w, epoch, started_at: started }).is_err() {
            return;
        }
        reader(cn, r, tx);
    });
}

fn reader(cn: u64, s: UnixStream, tx: Sender<Ev>) {
    let mut r = BufReader::with_capacity(64 * 1024, s);
    loop {
        let mut buf = Vec::new();
        let n = match (&mut r).take(proto::FRAME_IN_MAX + 1).read_until(b'\n', &mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => 0,
        };
        let at = Instant::now();
        if n == 0 {
            break;
        }
        if !buf.ends_with(b"\n") {
            if buf.len() as u64 > proto::FRAME_IN_MAX {
                let _ = r.get_ref().shutdown(std::net::Shutdown::Both); // over 2 MiB: drop and re-attach (§5)
            }
            break;
        }
        let raw = String::from_utf8_lossy(&buf).into_owned();
        let ev = match serde_json::from_str::<Value>(&raw) {
            Ok(v) => Ev::Frame { cn, raw, v, at },
            Err(_) => Ev::Malformed { cn },
        };
        if tx.send(ev).is_err() {
            return;
        }
    }
    let _ = tx.send(Ev::Disconnected { cn });
}

#[derive(Debug)]
pub enum PollCmd {
    Status,
    Egress,
    Stop,
}

/// The poller (§3.2): its own connection for requests; `status` every 2 s, `egress.stats` every 5 s,
/// or at once on demand.
pub fn spawn_poller(sock: PathBuf, rx: Receiver<PollCmd>, tx: Sender<Ev>) {
    std::thread::spawn(move || {
        let mut conn: Option<(UnixStream, u64)> = None;
        let mut due_status = Instant::now();
        let mut due_egress = Instant::now();
        loop {
            let now = Instant::now();
            let next = due_status.min(due_egress);
            match rx.recv_timeout(next.saturating_duration_since(now)) {
                Ok(PollCmd::Status) => due_status = Instant::now(),
                Ok(PollCmd::Egress) => due_egress = Instant::now(),
                Ok(PollCmd::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            for (cmd, due, every) in [("status", &mut due_status, 2), ("egress.stats", &mut due_egress, 5)] {
                if now < *due {
                    continue;
                }
                *due = now + Duration::from_secs(every);
                let r = poll_one(&sock, &mut conn, cmd);
                let ev = match (cmd, r) {
                    ("status", Ok(v)) => Ev::Status(v),
                    (_, Ok(v)) => Ev::Egress(v),
                    (c, Err(e)) => {
                        if !matches!(e, ReqErr::Refused(..)) {
                            conn = None;
                        }
                        Ev::ReqFailed { cmd: if c == "status" { "status" } else { "egress.stats" } }
                    }
                };
                if tx.send(ev).is_err() {
                    return;
                }
            }
        }
    });
}

fn poll_one(sock: &Path, conn: &mut Option<(UnixStream, u64)>, cmd: &str) -> Result<Value, ReqErr> {
    if conn.is_none() {
        let s = UnixStream::connect(sock).map_err(ReqErr::Unreachable)?;
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
        *conn = Some((s, 0));
    }
    let (s, n) = conn.as_mut().expect("just set");
    *n += 1;
    let id = format!("t-{n}");
    if writeln!(s, "{}", proto::request(*n, cmd)).is_err() {
        *conn = None;
        return Err(ReqErr::Proto("write failed".into()));
    }
    let r = read_answer(s, &id);
    if matches!(r, Err(ReqErr::Proto(_) | ReqErr::Unreachable(_))) {
        *conn = None;
    }
    r
}

/// `/panic` when the attach connection is down (§3.8): the `panic` request on a fresh connection
/// within 2 s; if the socket is unreachable or silent, the Root's own fallback that writes the latch.
pub fn panic_fallback(root_home: PathBuf, tx: Sender<Ev>) {
    std::thread::spawn(move || {
        let r = match request_once(&sock(&root_home), "panic", Duration::from_secs(2)) {
            Ok(v) => Ok(v["message"].as_str().unwrap_or("panic requested").to_string()),
            Err(ReqErr::Refused(c, m, _)) => Err(format!("{c}: {m}")),
            Err(_) => run_init_root_panic(&root_home),
        };
        let _ = tx.send(Ev::PanicDone(r));
    });
}

/// T-3: exactly `ROOT_HOME/bin/init-root --root-home <ROOT_HOME> panic`, never looked up in PATH.
pub fn run_init_root_panic(root_home: &Path) -> Result<String, String> {
    let exe = root_home.join("bin/init-root");
    let mut c = Command::new(&exe);
    c.arg("--root-home").arg(root_home).arg("panic").env_clear().env("PATH", "/usr/bin:/bin").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(h) = std::env::var_os("HOME") {
        c.env("HOME", h);
    }
    match c.output() {
        Ok(o) if o.status.success() => Ok(format!("panic latched by {} ({})", exe.display(), String::from_utf8_lossy(&o.stdout).trim())),
        Ok(o) => Err(format!("{} panic exited {:?}: {}", exe.display(), o.status.code(), String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Err(format!("{}: {e}", exe.display())),
    }
}

/// Backoff for reconnects: 100 ms doubling to 2 s (§3.11).
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_millis((100u64 << attempt.min(5)).min(2000))
}

/// Writes one frame on the attach connection.
pub fn send(w: &mut UnixStream, frame: &str) -> io::Result<()> {
    let mut b = Vec::with_capacity(frame.len() + 1);
    b.extend_from_slice(frame.as_bytes());
    b.push(b'\n');
    w.write_all(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_two_seconds() {
        let v: Vec<u64> = (0..8).map(|a| backoff(a).as_millis() as u64).collect();
        assert_eq!(v, [100, 200, 400, 800, 1600, 2000, 2000, 2000]);
    }
}
