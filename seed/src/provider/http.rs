//! HTTP/1.1 client over the proxy's Unix socket (body/provider.md §3.6): one request per connection,
//! response head parser, `ChunkedReader` (RFC 9112 §7.1) and body framing (§6.3). std only.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::model::{Abort, ProviderError, TimeoutPhase};

pub const MAX_HEAD: usize = 64 * 1024;
pub const MAX_CHUNK_LINE: usize = 4096;
pub const MAX_CHUNK: u64 = 1 << 30;
pub const MAX_TRAILERS: usize = 64 * 1024;
pub const READ_BUF: usize = 64 * 1024;

/// One request to the proxy. `headers` are provider headers only; `send` adds the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    pub path: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

/// The `x-init-*` request context (kernel/proxy.md §3.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cx {
    pub session: String,
    /// `turn` | `probe` | `compaction`.
    pub purpose: &'static str,
    pub task: Option<String>,
    /// `x-init-subtask` (kernel/proxy.md §3.5a): a sub-task's label (`delegate-<call id>`).
    pub subtask: Option<String>,
    /// `x-init-caps` (ADR-011 §5.2): the zone-evidence caps that shaped this request
    /// ([`crate::provider::caps`]); the same value on every attempt.
    pub caps: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    pub write: Duration,
    pub headers: Duration,
    pub idle: Duration,
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }
}

pub fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

pub enum Body {
    Chunked(ChunkedReader<BufReader<UnixStream>>),
    Length(io::Take<BufReader<UnixStream>>),
    UntilClose(BufReader<UnixStream>),
}

impl Read for Body {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Body::Chunked(r) => r.read(buf),
            Body::Length(r) => {
                let want = r.limit();
                let n = r.read(buf)?;
                if n == 0 && want > 0 && !buf.is_empty() {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                Ok(n)
            }
            Body::UntilClose(r) => r.read(buf),
        }
    }
}

fn io_or_timeout(e: io::Error, phase: TimeoutPhase, abort: &Abort) -> ProviderError {
    if abort.is_set() {
        return ProviderError::Aborted;
    }
    match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => ProviderError::Timeout(phase),
        io::ErrorKind::InvalidData => ProviderError::Protocol(e.to_string()),
        io::ErrorKind::UnexpectedEof => ProviderError::Truncated,
        _ => ProviderError::Io(e),
    }
}

/// The request head bytes (header names lowercase, fixed order, CRLF).
pub fn request_head(req: &HttpRequest, attempt: u32, cx: &Cx) -> Vec<u8> {
    let mut head = Vec::with_capacity(512);
    let _ = write!(
        head,
        "POST {} HTTP/1.1\r\nhost: init-proxy\r\ncontent-type: application/json\r\n\
         accept: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n",
        req.path,
        req.body.len()
    );
    for (k, v) in &req.headers {
        let _ = write!(head, "{k}: {v}\r\n");
    }
    let _ = write!(
        head,
        "x-init-session: {}\r\nx-init-purpose: {}\r\nx-init-attempt: {attempt}\r\n",
        cx.session, cx.purpose
    );
    if let Some(t) = &cx.task {
        let _ = write!(head, "x-init-task: {t}\r\n");
    }
    if let Some(t) = &cx.subtask {
        let _ = write!(head, "x-init-subtask: {t}\r\n");
    }
    if let Some(c) = &cx.caps {
        let _ = write!(head, "x-init-caps: {c}\r\n");
    }
    head.extend_from_slice(b"\r\n");
    head
}

/// Sends one request and returns the response with its body framed. The connection is registered
/// with `abort` so `Abort::trigger` can shut it down.
pub fn send(
    sock: &Path,
    req: &HttpRequest,
    attempt: u32,
    cx: &Cx,
    abort: &Abort,
    t: &Timeouts,
) -> Result<Response, ProviderError> {
    let mut s = UnixStream::connect(sock).map_err(ProviderError::Connect)?;
    abort.register_stream(s.try_clone().map_err(ProviderError::Io)?)?;
    // macOS returns EINVAL from setsockopt once the peer has closed; the read or write that
    // follows then reports the real condition, so a failed timeout set is not an error here.
    let _ = s.set_write_timeout(Some(t.write));
    let head = request_head(req, attempt, cx);
    s.write_all(&head)
        .and_then(|_| s.write_all(&req.body))
        .and_then(|_| s.flush())
        .map_err(|e| io_or_timeout(e, TimeoutPhase::Write, abort))?;
    let _ = s.set_read_timeout(Some(t.headers));
    let mut r = BufReader::with_capacity(READ_BUF, s);
    let (status, headers) =
        read_head(&mut r).map_err(|e| io_or_timeout(e, TimeoutPhase::Headers, abort))?;
    let _ = r.get_ref().set_read_timeout(Some(t.idle));
    let body = frame_body(r, &headers)?;
    Ok(Response {
        status,
        headers,
        body,
    })
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Reads one line ending in LF (CRLF or bare LF), without the terminator. `budget` bounds the bytes.
fn read_line<R: BufRead>(r: &mut R, budget: &mut usize, what: &str) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let (take, done) = match buf.iter().position(|&b| b == b'\n') {
            Some(p) => (p + 1, true),
            None => (buf.len(), false),
        };
        if take > *budget {
            return Err(invalid(what));
        }
        *budget -= take;
        line.extend_from_slice(&buf[..take]);
        r.consume(take);
        if done {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(line);
        }
    }
}

/// Parses the response head (status line + headers); skips `1xx` interim responses.
pub fn read_head<R: BufRead>(r: &mut R) -> io::Result<(u16, Vec<(String, String)>)> {
    let mut budget = MAX_HEAD;
    loop {
        let line = read_line(r, &mut budget, "header block too large")?;
        let status = parse_status(&line)?;
        let mut headers = Vec::new();
        loop {
            let line = read_line(r, &mut budget, "header block too large")?;
            if line.is_empty() {
                break;
            }
            if line[0] == b' ' || line[0] == b'\t' {
                return Err(invalid("obsolete header line folding"));
            }
            let p = line
                .iter()
                .position(|&b| b == b':')
                .ok_or_else(|| invalid("header line without a colon"))?;
            let name =
                std::str::from_utf8(&line[..p]).map_err(|_| invalid("header name is not UTF-8"))?;
            if name.is_empty() || name.bytes().any(|b| b == b' ' || b == b'\t') {
                return Err(invalid("bad header name"));
            }
            let value = String::from_utf8_lossy(&line[p + 1..])
                .trim_matches([' ', '\t'])
                .to_string();
            headers.push((name.to_ascii_lowercase(), value));
        }
        if (100..200).contains(&status) {
            continue;
        }
        return Ok((status, headers));
    }
}

fn parse_status(line: &[u8]) -> io::Result<u16> {
    let rest = line
        .strip_prefix(b"HTTP/1.1 ")
        .or_else(|| line.strip_prefix(b"HTTP/1.0 "))
        .ok_or_else(|| invalid("bad status line"))?;
    if rest.len() < 3
        || !rest[..3].iter().all(u8::is_ascii_digit)
        || rest.get(3).is_some_and(|&b| b != b' ')
    {
        return Err(invalid("bad status code"));
    }
    Ok(rest[..3]
        .iter()
        .fold(0u16, |n, &d| n * 10 + (d - b'0') as u16))
}

/// RFC 9112 §6.3: chunked > content-length > until close.
pub fn frame_body(
    r: BufReader<UnixStream>,
    headers: &[(String, String)],
) -> Result<Body, ProviderError> {
    match body_framing(headers)? {
        Framing::Chunked => Ok(Body::Chunked(ChunkedReader::new(r))),
        Framing::Length(n) => Ok(Body::Length(r.take(n))),
        Framing::UntilClose => Ok(Body::UntilClose(r)),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Framing {
    Chunked,
    Length(u64),
    UntilClose,
}

pub fn body_framing(headers: &[(String, String)]) -> Result<Framing, ProviderError> {
    let te: Vec<&str> = headers
        .iter()
        .filter(|(k, _)| k == "transfer-encoding")
        .flat_map(|(_, v)| v.split(','))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(last) = te.last() {
        if last.eq_ignore_ascii_case("chunked") && te.len() == 1 {
            return Ok(Framing::Chunked);
        }
        return Err(ProviderError::Protocol(format!(
            "unsupported transfer-encoding {te:?}"
        )));
    }
    let mut len: Option<u64> = None;
    for (_, v) in headers.iter().filter(|(k, _)| k == "content-length") {
        for part in v.split(',') {
            let n: u64 = part
                .trim()
                .parse()
                .map_err(|_| ProviderError::Protocol(format!("bad content-length {v:?}")))?;
            if len.is_some_and(|l| l != n) {
                return Err(ProviderError::Protocol(
                    "differing content-length values".into(),
                ));
            }
            len = Some(n);
        }
    }
    Ok(match len {
        Some(n) => Framing::Length(n),
        None => Framing::UntilClose,
    })
}

/// RFC 9112 §7.1 chunked transfer coding.
pub struct ChunkedReader<R> {
    inner: R,
    remaining: u64,
    done: bool,
}

impl<R> ChunkedReader<R> {
    pub fn new(inner: R) -> Self {
        ChunkedReader {
            inner,
            remaining: 0,
            done: false,
        }
    }
    pub fn get_ref(&self) -> &R {
        &self.inner
    }
}

impl<R: BufRead> Read for ChunkedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let size = read_chunk_size(&mut self.inner)?;
            if size == 0 {
                read_trailers(&mut self.inner)?;
                self.done = true;
                return Ok(0);
            }
            self.remaining = size;
        }
        let want = buf
            .len()
            .min(self.remaining.min(usize::MAX as u64) as usize);
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.remaining -= n as u64;
        if self.remaining == 0 {
            expect_crlf(&mut self.inner)?;
        }
        Ok(n)
    }
}

fn read_chunk_size<R: BufRead>(r: &mut R) -> io::Result<u64> {
    let mut budget = MAX_CHUNK_LINE;
    let line = read_line(r, &mut budget, "chunk size line too long")?;
    let size = match line.iter().position(|&b| b == b';') {
        Some(p) => &line[..p],
        None => &line[..],
    };
    let size = std::str::from_utf8(size)
        .map_err(|_| invalid("bad chunk size"))?
        .trim_matches([' ', '\t']);
    if size.is_empty() || size.len() > 16 || !size.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("bad chunk size"));
    }
    let n = u64::from_str_radix(size, 16).map_err(|_| invalid("bad chunk size"))?;
    if n > MAX_CHUNK {
        return Err(invalid("chunk too large"));
    }
    Ok(n)
}

fn read_trailers<R: BufRead>(r: &mut R) -> io::Result<()> {
    let mut budget = MAX_TRAILERS;
    loop {
        if read_line(r, &mut budget, "trailers too large")?.is_empty() {
            return Ok(());
        }
    }
}

fn expect_crlf<R: BufRead>(r: &mut R) -> io::Result<()> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    if b[0] == b'\n' {
        return Ok(());
    }
    if b[0] != b'\r' {
        return Err(invalid("missing CRLF after chunk data"));
    }
    r.read_exact(&mut b)?;
    if b[0] != b'\n' {
        return Err(invalid("missing CRLF after chunk data"));
    }
    Ok(())
}

/// Reads at most `cap` bytes of an error body.
pub fn read_capped<R: Read>(r: &mut R, cap: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = r.take(cap as u64).read_to_end(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::sse::tests_support::Trickle;

    fn dechunk(input: &[u8], n: usize) -> io::Result<Vec<u8>> {
        let mut r = ChunkedReader::new(BufReader::with_capacity(
            n.max(1),
            Trickle { data: input, n },
        ));
        let mut out = Vec::new();
        r.read_to_end(&mut out)?;
        Ok(out)
    }

    #[test]
    fn chunked_cases() {
        assert_eq!(
            dechunk(b"5\r\nhello\r\nA\r\n0123456789\r\n0\r\n\r\n", 64).unwrap(),
            b"hello0123456789"
        );
        assert_eq!(
            dechunk(b"a\r\n0123456789\r\n0\r\n\r\n", 64).unwrap(),
            b"0123456789"
        );
        assert_eq!(
            dechunk(b"5;ext=1\r\nhello\r\n0;x=y\r\n\r\n", 3).unwrap(),
            b"hello"
        );
        assert_eq!(
            dechunk(b"5\r\nhello\r\n0\r\nx-trailer: 1\r\nother: 2\r\n\r\n", 2).unwrap(),
            b"hello"
        );
        assert_eq!(dechunk(b"0\r\n\r\n", 1).unwrap(), b"");
        assert_eq!(dechunk(b"5\nhello\n0\n\n", 1).unwrap(), b"hello"); // bare LF accepted
        let e = dechunk(b"5\r\nhel", 64).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
        let mut long = vec![b'1'; 4100];
        long.extend_from_slice(b"\r\n");
        assert_eq!(
            dechunk(&long, 64).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            dechunk(b"zz\r\n", 64).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            dechunk(b"5\r\nhelloXY0\r\n\r\n", 64).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            dechunk(b"80000000\r\n", 64).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    /// Property: every split of a recorded stream into 1..=k byte reads decodes the same.
    #[test]
    fn chunked_every_read_size() {
        let payload = include_bytes!("testdata/deepseek/tool-calls.sse");
        let mut enc = Vec::new();
        for (i, c) in payload.chunks(37).enumerate() {
            let ext = if i % 3 == 0 { ";e=1" } else { "" };
            enc.extend_from_slice(format!("{:x}{ext}\r\n", c.len()).as_bytes());
            enc.extend_from_slice(c);
            enc.extend_from_slice(b"\r\n");
        }
        enc.extend_from_slice(b"0\r\n\r\n");
        for n in 1..=64 {
            assert_eq!(dechunk(&enc, n).unwrap(), payload, "n={n}");
        }
    }

    #[test]
    fn read_head_cases() {
        let (s, h) =
            read_head(&mut &b"HTTP/1.0 200 OK\r\nContent-Type: text/event-stream\r\n\r\n"[..])
                .unwrap();
        assert_eq!(s, 200);
        assert_eq!(h, vec![("content-type".into(), "text/event-stream".into())]);
        let (s, h) = read_head(
            &mut &b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 429 Too Many\r\nRetry-After:  3 \r\n\r\n"
                [..],
        )
        .unwrap();
        assert_eq!(s, 429);
        assert_eq!(header(&h, "retry-after"), Some("3"));
        let e = read_head(&mut &b"HTTP/1.1 200 OK\r\nA: b\r\n  folded\r\n\r\n"[..]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(read_head(&mut &b"HTTP/2 200\r\n\r\n"[..]).is_err());
        let mut big = b"HTTP/1.1 200 OK\r\n".to_vec();
        for i in 0..5000 {
            big.extend_from_slice(format!("x-h{i}: 0123456789\r\n").as_bytes());
        }
        big.extend_from_slice(b"\r\n");
        assert_eq!(
            read_head(&mut &big[..]).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        // Bare LF line endings are accepted.
        assert_eq!(
            read_head(&mut &b"HTTP/1.1 204 No\nA: 1\n\n"[..]).unwrap().0,
            204
        );
    }

    #[test]
    fn framing_rules() {
        let h = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        };
        assert_eq!(
            body_framing(&h(&[
                ("transfer-encoding", "chunked"),
                ("content-length", "5")
            ]))
            .unwrap(),
            Framing::Chunked
        );
        assert_eq!(
            body_framing(&h(&[("content-length", "5"), ("content-length", "5")])).unwrap(),
            Framing::Length(5)
        );
        assert!(body_framing(&h(&[("content-length", "5"), ("content-length", "6")])).is_err());
        assert!(body_framing(&h(&[("transfer-encoding", "gzip")])).is_err());
        assert_eq!(body_framing(&h(&[])).unwrap(), Framing::UntilClose);
    }

    #[test]
    fn request_head_bytes() {
        let req = HttpRequest {
            path: "/anthropic/v1/messages".into(),
            headers: vec![("anthropic-version", "2023-06-01".into())],
            body: b"{}".to_vec(),
        };
        let cx = Cx {
            session: "s1".into(),
            purpose: "turn",
            task: Some("t9".into()),
            subtask: None,
            caps: None,
        };
        let got = String::from_utf8(request_head(&req, 2, &cx)).unwrap();
        assert_eq!(
            got,
            "POST /anthropic/v1/messages HTTP/1.1\r\nhost: init-proxy\r\ncontent-type: application/json\r\n\
             accept: text/event-stream\r\ncontent-length: 2\r\nconnection: close\r\nanthropic-version: 2023-06-01\r\n\
             x-init-session: s1\r\nx-init-purpose: turn\r\nx-init-attempt: 2\r\nx-init-task: t9\r\n\r\n"
        );
        assert!(!got.to_ascii_lowercase().contains("authorization"));
        // X5: the zone-evidence caps travel as `x-init-caps`, after the task.
        let cx = Cx { caps: Some("loop/plan,mem/notes".into()), ..cx };
        let got = String::from_utf8(request_head(&req, 1, &cx)).unwrap();
        assert!(got.ends_with("x-init-task: t9\r\nx-init-caps: loop/plan,mem/notes\r\n\r\n"), "{got}");
        // DELEGATE: a sub-task's requests carry its label after the task (kernel/proxy.md §3.5a).
        let cx = Cx { subtask: Some("delegate-call_7".into()), caps: None, ..cx };
        let got = String::from_utf8(request_head(&req, 1, &cx)).unwrap();
        assert!(got.ends_with("x-init-task: t9\r\nx-init-subtask: delegate-call_7\r\n\r\n"), "{got}");
    }
}
