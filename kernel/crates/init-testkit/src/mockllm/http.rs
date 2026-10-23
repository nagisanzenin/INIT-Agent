//! The HTTP/1.1 server around [`Mock`]: one thread per connection, std only (testing.md §3.3).
//! Listens on a loopback TCP port or a Unix socket; keep-alive is supported (the proxy and the Root
//! egress pool upstream connections), and SSE responses use chunked transfer encoding with one
//! chunk per event, flushed as it is written.

use super::{Body, Mock, MockRequest, MockResponse, Part};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// Cap on a request head (request line plus headers).
pub const MAX_HEAD: usize = 64 * 1024;
/// Cap on a request body.
pub const MAX_BODY: usize = 256 * 1024 * 1024;

/// Where to listen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listen {
    /// `host:port`, e.g. `127.0.0.1:0` for a free port.
    Tcp(String),
    /// A Unix socket path (≤ 103 bytes on macOS). A stale socket at exactly this path is replaced.
    Unix(PathBuf),
}

/// A connection the server can serve.
pub trait Conn: Read + Write + Send + 'static {
    fn shutdown_all(&self);
}

impl Conn for TcpStream {
    fn shutdown_all(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }
}

impl Conn for UnixStream {
    fn shutdown_all(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }
}

#[derive(Clone, Debug)]
enum Bound {
    Tcp(SocketAddr),
    Unix(PathBuf),
}

/// A running mock server. Dropping it stops accepting (open keep-alive connections end when their
/// peer closes them).
#[derive(Debug)]
pub struct MockServer {
    mock: Arc<Mock>,
    bound: Bound,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl MockServer {
    /// Bind and start accepting.
    pub fn start(mock: Arc<Mock>, listen: Listen) -> io::Result<MockServer> {
        let stop = Arc::new(AtomicBool::new(false));
        let (bound, accept) = match listen {
            Listen::Tcp(addr) => {
                let l = TcpListener::bind(addr)?;
                let a = l.local_addr()?;
                let (m, s) = (mock.clone(), stop.clone());
                let h = std::thread::Builder::new()
                    .name("mockllm-accept".into())
                    .spawn(move || {
                        for conn in l.incoming() {
                            if s.load(Ordering::SeqCst) {
                                break;
                            }
                            if let Ok(c) = conn {
                                let _ = c.set_nodelay(true);
                                spawn_conn(m.clone(), c);
                            }
                        }
                    })?;
                (Bound::Tcp(a), h)
            }
            Listen::Unix(path) => {
                if let Ok(md) = std::fs::symlink_metadata(&path)
                    && md.file_type().is_socket()
                {
                    std::fs::remove_file(&path)?;
                }
                let l = UnixListener::bind(&path)?;
                let (m, s) = (mock.clone(), stop.clone());
                let h = std::thread::Builder::new()
                    .name("mockllm-accept".into())
                    .spawn(move || {
                        for conn in l.incoming() {
                            if s.load(Ordering::SeqCst) {
                                break;
                            }
                            if let Ok(c) = conn {
                                spawn_conn(m.clone(), c);
                            }
                        }
                    })?;
                (Bound::Unix(path), h)
            }
        };
        Ok(MockServer {
            mock,
            bound,
            stop,
            accept: Some(accept),
        })
    }

    pub fn mock(&self) -> &Arc<Mock> {
        &self.mock
    }

    /// The bound TCP address (TCP listeners only).
    pub fn addr(&self) -> Option<SocketAddr> {
        match &self.bound {
            Bound::Tcp(a) => Some(*a),
            Bound::Unix(_) => None,
        }
    }

    /// The bound TCP port (TCP listeners only).
    pub fn port(&self) -> Option<u16> {
        self.addr().map(|a| a.port())
    }

    /// The socket path (Unix listeners only).
    pub fn socket_path(&self) -> Option<&Path> {
        match &self.bound {
            Bound::Unix(p) => Some(p),
            Bound::Tcp(_) => None,
        }
    }

    /// Stop accepting, join the accept thread and remove the socket file this server created.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let Some(h) = self.accept.take() else { return };
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept with one connection to ourselves.
        match &self.bound {
            Bound::Tcp(a) => {
                let mut wake = *a;
                if wake.ip().is_unspecified() {
                    wake.set_ip(std::net::Ipv4Addr::LOCALHOST.into());
                }
                let _ = TcpStream::connect_timeout(&wake, Duration::from_secs(2));
            }
            Bound::Unix(p) => {
                let _ = UnixStream::connect(p);
            }
        }
        let _ = h.join();
        if let Bound::Unix(p) = &self.bound {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn spawn_conn<C: Conn>(mock: Arc<Mock>, c: C) {
    let _ = std::thread::Builder::new()
        .name("mockllm-conn".into())
        .spawn(move || serve_conn(&mock, c));
}

/// A parsed request head.
#[derive(Debug)]
struct Head {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    close: bool,
}

fn read_line<R: BufRead>(r: &mut R, buf: &mut Vec<u8>, budget: &mut usize) -> io::Result<usize> {
    buf.clear();
    let n = r.by_ref().take(*budget as u64 + 1).read_until(b'\n', buf)?;
    if n > *budget {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request head too large",
        ));
    }
    *budget -= n;
    Ok(n)
}

fn trim_crlf(b: &[u8]) -> &[u8] {
    let b = b.strip_suffix(b"\n").unwrap_or(b);
    b.strip_suffix(b"\r").unwrap_or(b)
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// `Ok(None)` on a clean EOF before any byte of a new request.
fn read_head<R: BufRead>(r: &mut R) -> io::Result<Option<Head>> {
    let mut budget = MAX_HEAD;
    let mut buf = Vec::new();
    // Tolerate empty lines before the request line (RFC 9112 §2.2).
    let line = loop {
        if read_line(r, &mut buf, &mut budget)? == 0 {
            return Ok(None);
        }
        let l = trim_crlf(&buf);
        if !l.is_empty() {
            break String::from_utf8(l.to_vec()).map_err(|_| bad("request line is not UTF-8"))?;
        }
    };
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(bad("malformed request line"));
    };
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(bad("unsupported HTTP version"));
    }
    let mut headers = Vec::new();
    loop {
        if read_line(r, &mut buf, &mut budget)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "EOF in request head",
            ));
        }
        let l = trim_crlf(&buf);
        if l.is_empty() {
            break;
        }
        if l[0] == b' ' || l[0] == b'\t' {
            return Err(bad("obsolete header folding"));
        }
        let s = std::str::from_utf8(l).map_err(|_| bad("header is not UTF-8"))?;
        let (k, v) = s
            .split_once(':')
            .ok_or_else(|| bad("header without colon"))?;
        headers.push((
            k.trim().to_ascii_lowercase(),
            v.trim_matches([' ', '\t']).to_string(),
        ));
    }
    let conn = headers
        .iter()
        .find(|(k, _)| k == "connection")
        .map(|(_, v)| v.to_ascii_lowercase());
    let close = match version {
        "HTTP/1.0" => !conn.as_deref().is_some_and(|c| c.contains("keep-alive")),
        _ => conn.as_deref().is_some_and(|c| c.contains("close")),
    };
    Ok(Some(Head {
        method: method.into(),
        target: target.into(),
        headers,
        close,
    }))
}

fn header<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn read_body<R: BufRead>(r: &mut R, h: &Head) -> io::Result<Vec<u8>> {
    if header(&h.headers, "transfer-encoding")
        .is_some_and(|te| te.to_ascii_lowercase().ends_with("chunked"))
    {
        let mut out = Vec::new();
        let mut buf = Vec::new();
        loop {
            let mut budget = 4096;
            if read_line(r, &mut buf, &mut budget)? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "EOF in chunk size",
                ));
            }
            let l = std::str::from_utf8(trim_crlf(&buf)).map_err(|_| bad("bad chunk size"))?;
            let size_str = l.split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size_str, 16).map_err(|_| bad("bad chunk size"))?;
            if size == 0 {
                // Trailers until an empty line.
                let mut tb = MAX_HEAD;
                loop {
                    if read_line(r, &mut buf, &mut tb)? == 0 || trim_crlf(&buf).is_empty() {
                        return Ok(out);
                    }
                }
            }
            if out.len() + size > MAX_BODY {
                return Err(bad("request body too large"));
            }
            let start = out.len();
            out.resize(start + size, 0);
            r.read_exact(&mut out[start..])?;
            let mut crlf = MAX_HEAD;
            read_line(r, &mut buf, &mut crlf)?;
            if !trim_crlf(&buf).is_empty() {
                return Err(bad("missing CRLF after chunk"));
            }
        }
    }
    match header(&h.headers, "content-length") {
        Some(cl) => {
            let n: usize = cl.trim().parse().map_err(|_| bad("bad content-length"))?;
            if n > MAX_BODY {
                return Err(bad("request body too large"));
            }
            let mut out = vec![0; n];
            r.read_exact(&mut out)?;
            Ok(out)
        }
        None => Ok(Vec::new()),
    }
}

/// HTTP reason phrase for the statuses the mock produces.
pub fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Content Too Large",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        529 => "Overloaded",
        _ => "Status",
    }
}

/// One HTTP/1.1 chunk carrying `data`.
pub fn encode_chunk(data: &[u8]) -> Vec<u8> {
    let mut v = format!("{:x}\r\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\r\n");
    v
}

/// The response head bytes.
pub fn encode_head(resp: &MockResponse, close: bool) -> Vec<u8> {
    let mut h = format!("HTTP/1.1 {} {}\r\n", resp.status, reason(resp.status));
    for (k, v) in &resp.headers {
        if matches!(
            k.as_str(),
            "content-length" | "transfer-encoding" | "connection"
        ) {
            continue;
        }
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    match &resp.body {
        Body::Full(b) => h.push_str(&format!("content-length: {}\r\n", b.len())),
        Body::Stream(_) => h.push_str("transfer-encoding: chunked\r\n"),
    }
    h.push_str(if close {
        "connection: close\r\n"
    } else {
        "connection: keep-alive\r\n"
    });
    h.push_str("\r\n");
    h.into_bytes()
}

/// The complete wire bytes of a response as the server sends them (stalls ignored; a cut omits the
/// terminating chunk). Used by the framing self-tests.
pub fn encode_response(resp: &MockResponse, close: bool) -> Vec<u8> {
    let mut out = encode_head(resp, close);
    match &resp.body {
        Body::Full(b) => out.extend_from_slice(b),
        Body::Stream(parts) => {
            for p in parts {
                match p {
                    Part::Event(e) => out.extend_from_slice(&encode_chunk(e)),
                    Part::Stall(_) => {}
                    Part::Cut => return out,
                }
            }
            out.extend_from_slice(b"0\r\n\r\n");
        }
    }
    out
}

/// Write a response; `Ok(true)` when the stream was cut (the connection must close).
/// Write a response; `Ok(true)` when the stream was cut (the connection must close).
///
/// `done` runs just before the last byte the client waits for (the last body byte, the terminating
/// chunk, or the cut), so a test that has the complete response also finds its log line on disk.
fn write_response<W: Write>(
    w: &mut W,
    resp: &MockResponse,
    close: bool,
    done: &mut dyn FnMut(),
) -> io::Result<bool> {
    match &resp.body {
        Body::Full(b) => {
            let mut all = encode_head(resp, close);
            all.extend_from_slice(b);
            let (last, rest) = all
                .split_last()
                .map_or((None, &all[..]), |(l, r)| (Some(*l), r));
            w.write_all(rest)?;
            w.flush()?;
            done();
            if let Some(l) = last {
                w.write_all(&[l])?;
            }
        }
        Body::Stream(parts) => {
            w.write_all(&encode_head(resp, close))?;
            w.flush()?;
            for p in parts {
                match p {
                    Part::Event(e) => {
                        w.write_all(&encode_chunk(e))?;
                        w.flush()?;
                    }
                    Part::Stall(ms) => std::thread::sleep(Duration::from_millis(*ms)),
                    Part::Cut => {
                        done();
                        return Ok(true);
                    }
                }
            }
            done();
            w.write_all(b"0\r\n\r\n")?;
        }
    }
    w.flush()?;
    Ok(false)
}

fn serve_conn<C: Conn>(mock: &Mock, c: C) {
    let mut r = BufReader::with_capacity(64 * 1024, c);
    loop {
        let head = match read_head(&mut r) {
            Ok(Some(h)) => h,
            Ok(None) => break,
            Err(e) => {
                let body = format!(
                    "{{\"error\":{{\"message\":{},\"type\":\"invalid_request_error\"}}}}",
                    super::canon::str_lit(&e.to_string())
                );
                let resp = MockResponse {
                    status: 400,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: Body::Full(body.into_bytes()),
                };
                let _ = write_response(r.get_mut(), &resp, true, &mut || {});
                break;
            }
        };
        if header(&head.headers, "expect").is_some_and(|e| e.eq_ignore_ascii_case("100-continue"))
            && r.get_mut()
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                .is_err()
        {
            break;
        }
        let body = match read_body(&mut r, &head) {
            Ok(b) => b,
            Err(_) => break,
        };
        let req = MockRequest {
            method: head.method,
            path: head.target,
            headers: head.headers,
            body,
        };
        let (resp, pending) = mock.begin(&req);
        let mut pending = pending;
        let mut done = || {
            if let Some(p) = pending.take() {
                mock.finish(p);
            }
        };
        let res = write_response(r.get_mut(), &resp, head.close, &mut done);
        // A write error (the client went away) still records the request.
        done();
        match res {
            Ok(false) if !head.close => continue,
            _ => break,
        }
    }
    r.get_ref().shutdown_all();
}
