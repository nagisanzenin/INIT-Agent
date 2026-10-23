//! HTTP/1.1 on the egress socket (root/egress.md §3.1, §4.1): request heads parsed with `httparse`,
//! bodies by `content-length` or chunked, responses written with chunked framing.

use std::io::{self, BufRead, Read, Write};

const HEAD_MAX: usize = 64 * 1024;
pub const BODY_MAX: usize = 32 << 20;

pub struct Head {
    pub method: String,
    pub target: String,
    /// Lowercased names.
    pub headers: Vec<(String, String)>,
    pub close: bool,
}

impl Head {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

fn bad(m: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.to_string())
}

fn line(r: &mut impl BufRead, cap: usize) -> io::Result<Vec<u8>> {
    let mut v = Vec::new();
    r.take(cap as u64 + 1).read_until(b'\n', &mut v)?;
    if v.len() > cap {
        return Err(bad("line too long"));
    }
    Ok(v)
}

/// One request; `Ok(None)` when the peer closed the connection between requests.
pub fn read_request(r: &mut impl BufRead) -> io::Result<Option<(Head, Vec<u8>)>> {
    let mut raw = Vec::new();
    loop {
        let l = line(r, HEAD_MAX)?;
        if l.is_empty() {
            return if raw.is_empty() { Ok(None) } else { Err(bad("truncated head")) };
        }
        if raw.is_empty() && (l == b"\r\n" || l == b"\n") {
            continue; // stray CRLF between requests
        }
        let end = l == b"\r\n" || l == b"\n";
        raw.extend_from_slice(&l);
        if raw.len() > HEAD_MAX {
            return Err(bad("head too large"));
        }
        if end {
            break;
        }
    }
    let mut hs = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Request::new(&mut hs);
    if !req.parse(&raw).map_err(|e| bad(&e.to_string()))?.is_complete() {
        return Err(bad("incomplete head"));
    }
    let headers: Vec<(String, String)> = req.headers.iter()
        .map(|h| (h.name.to_ascii_lowercase(), String::from_utf8_lossy(h.value).trim().to_string())).collect();
    let mut head = Head { method: req.method.unwrap_or("").to_string(), target: req.path.unwrap_or("").to_string(), headers, close: req.version == Some(0) };
    head.close |= head.get("connection").is_some_and(|c| c.eq_ignore_ascii_case("close"));
    let mut body = Vec::new();
    if head.get("transfer-encoding").is_some_and(|t| t.to_ascii_lowercase().contains("chunked")) {
        loop {
            let l = line(r, 1024)?;
            let s = String::from_utf8_lossy(&l);
            let n = usize::from_str_radix(s.split(';').next().unwrap_or("").trim(), 16).map_err(|_| bad("bad chunk size"))?;
            if n == 0 {
                while !matches!(line(r, HEAD_MAX)?.as_slice(), b"\r\n" | b"\n" | b"") {}
                break;
            }
            if body.len() + n > BODY_MAX {
                return Err(io::Error::new(io::ErrorKind::OutOfMemory, "body too large"));
            }
            let at = body.len();
            body.resize(at + n, 0);
            r.read_exact(&mut body[at..])?;
            line(r, 2)?;
        }
    } else if let Some(n) = head.get("content-length") {
        let n: usize = n.parse().map_err(|_| bad("bad content-length"))?;
        if n > BODY_MAX {
            return Err(io::Error::new(io::ErrorKind::OutOfMemory, "body too large"));
        }
        body.resize(n, 0);
        r.read_exact(&mut body)?;
    }
    Ok(Some((head, body)))
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "",
    }
}

pub fn head(w: &mut impl Write, status: u16, headers: &[(String, String)]) -> io::Result<()> {
    let mut s = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    for (k, v) in headers {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    s.push_str("\r\n");
    w.write_all(s.as_bytes())
}

pub fn chunk(w: &mut impl Write, data: &[u8]) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    w.write_all(format!("{:x}\r\n", data.len()).as_bytes())?;
    w.write_all(data)?;
    w.write_all(b"\r\n")
}

/// A complete response with a body and `content-length`.
pub fn full(w: &mut impl Write, status: u16, mut headers: Vec<(String, String)>, body: &[u8]) -> io::Result<()> {
    headers.push(("content-length".into(), body.len().to_string()));
    head(w, status, &headers)?;
    w.write_all(body)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> io::Result<Option<(Head, Vec<u8>)>> {
        read_request(&mut io::BufReader::new(s.as_bytes()))
    }

    #[test]
    fn requests_with_length_chunked_and_keep_alive() {
        let two = "POST /deepseek/chat/completions HTTP/1.1\r\nContent-Length: 5\r\nAuthorization: Bearer x\r\n\r\nhello\
                   GET /_root/ceilings HTTP/1.1\r\nConnection: close\r\n\r\n";
        let mut r = io::BufReader::new(two.as_bytes());
        let (h, b) = read_request(&mut r).unwrap().unwrap();
        assert_eq!((h.method.as_str(), h.target.as_str(), b.as_slice(), h.close), ("POST", "/deepseek/chat/completions", &b"hello"[..], false));
        assert_eq!(h.get("authorization"), Some("Bearer x"));
        let (h2, b2) = read_request(&mut r).unwrap().unwrap();
        assert_eq!((h2.target.as_str(), b2.len(), h2.close), ("/_root/ceilings", 0, true));
        assert!(read_request(&mut r).unwrap().is_none());
        let (_, c) = parse("POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3;ext\r\nabc\r\n2\r\nde\r\n0\r\n\r\n").unwrap().unwrap();
        assert_eq!(c, b"abcde");
        assert!(parse("POST /x HTTP/1.1\r\nContent-Length: 99999999999\r\n\r\n").is_err());
        assert!(parse("GARBAGE\r\n\r\n").is_err());
        assert!(parse("GET /x HTTP/1.1\r\nHost: a").is_err());
        assert!(parse("GET /x HTTP/1.0\r\n\r\n").unwrap().unwrap().0.close);
    }

    #[test]
    fn responses() {
        let mut v = Vec::new();
        full(&mut v, 429, vec![("x-init-egress-error".into(), "ceiling.day".into())], b"{}").unwrap();
        assert_eq!(String::from_utf8(v).unwrap(), "HTTP/1.1 429 Too Many Requests\r\nx-init-egress-error: ceiling.day\r\ncontent-length: 2\r\n\r\n{}");
        let mut c = Vec::new();
        chunk(&mut c, b"data").unwrap();
        chunk(&mut c, b"").unwrap();
        assert_eq!(c, b"4\r\ndata\r\n");
    }
}
