//! A minimal blocking HTTP/1.1 client for tests that talk to a running mock (TCP or Unix socket).

use super::MockRequest;
use super::sse::ChunkedDecoder;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::Path;

/// A response as read off the wire.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// False when the connection closed before the body's end (a `cut`).
    pub complete: bool,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// One request over a new TCP connection (`connection: close`).
pub fn request_tcp(addr: &str, req: &MockRequest) -> io::Result<HttpResponse> {
    let s = TcpStream::connect(addr)?;
    exchange(&mut BufReader::new(s), req, true)
}

/// One request over a new Unix-socket connection (`connection: close`).
pub fn request_unix(path: &Path, req: &MockRequest) -> io::Result<HttpResponse> {
    let s = UnixStream::connect(path)?;
    exchange(&mut BufReader::new(s), req, true)
}

/// Send `req` on an open connection and read one response. With `close == false` the connection
/// stays usable for the next exchange.
pub fn exchange<S: Read + Write>(
    r: &mut BufReader<S>,
    req: &MockRequest,
    close: bool,
) -> io::Result<HttpResponse> {
    let mut head = format!("{} {} HTTP/1.1\r\nhost: mock\r\n", req.method, req.path);
    for (k, v) in &req.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if req.method != "GET" || !req.body.is_empty() {
        head.push_str(&format!("content-length: {}\r\n", req.body.len()));
    }
    head.push_str(if close {
        "connection: close\r\n\r\n"
    } else {
        "connection: keep-alive\r\n\r\n"
    });
    let w = r.get_mut();
    w.write_all(head.as_bytes())?;
    w.write_all(&req.body)?;
    w.flush()?;
    read_response(r)
}

/// Read one response (1xx interim responses are skipped).
pub fn read_response<R: BufRead>(r: &mut R) -> io::Result<HttpResponse> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "EOF before status line",
            ));
        }
        let mut it = line.trim_end().splitn(3, ' ');
        let (_, code) = (it.next(), it.next());
        let status: u16 = code
            .and_then(|c| c.parse().ok())
            .ok_or_else(|| bad("bad status line"))?;
        let mut headers = Vec::new();
        loop {
            let mut h = String::new();
            if r.read_line(&mut h)? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "EOF in headers",
                ));
            }
            let h = h.trim_end_matches(['\r', '\n']);
            if h.is_empty() {
                break;
            }
            let (k, v) = h.split_once(':').ok_or_else(|| bad("bad header"))?;
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
        if (100..200).contains(&status) {
            continue;
        }
        let get = |n: &str| {
            headers
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, v): &(String, String)| v.clone())
        };
        let mut resp = HttpResponse {
            status,
            headers: headers.clone(),
            body: Vec::new(),
            complete: false,
        };
        if get("transfer-encoding").is_some_and(|t| t.ends_with("chunked")) {
            let mut dec = ChunkedDecoder::new();
            while !dec.is_done() {
                let buf = r.fill_buf()?;
                if buf.is_empty() {
                    return Ok(resp);
                }
                // Feed at most up to the end of this response: the decoder stops at its last chunk.
                let n = buf.len();
                let mut taken = 0;
                while taken < n && !dec.is_done() {
                    let out = dec.feed(&buf[taken..taken + 1]).map_err(|e| bad(&e))?;
                    resp.body.extend_from_slice(&out);
                    taken += 1;
                }
                r.consume(taken);
            }
            resp.complete = true;
        } else if let Some(cl) = get("content-length") {
            let n: usize = cl.parse().map_err(|_| bad("bad content-length"))?;
            let mut body = Vec::with_capacity(n);
            r.take(n as u64).read_to_end(&mut body)?;
            resp.complete = body.len() == n;
            resp.body = body;
        } else {
            r.read_to_end(&mut resp.body)?;
            resp.complete = true;
        }
        return Ok(resp);
    }
}
