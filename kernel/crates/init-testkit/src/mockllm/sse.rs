//! Push-style decoders for the client side of tests: an SSE parser (WHATWG "interpreting an event
//! stream", the same rules as body/provider.md §3.7) and an HTTP/1.1 chunked-body decoder.
//!
//! Both accept input split at any byte boundary: feeding `a` then `b` gives exactly what feeding
//! `a ++ b` gives. The split-fuzz self-tests rely on that.

/// One dispatched SSE event. `event` is `""` when the stream named none (meaning `message`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
    pub id: Option<String>,
}

/// Incremental SSE parser.
#[derive(Debug, Default)]
pub struct SseParser {
    line: Vec<u8>,
    data: Vec<u8>,
    has_data: bool,
    event: Vec<u8>,
    id: Option<String>,
    skip_lf: bool,
    started: bool,
    /// `:` comment lines seen (keep-alives).
    pub comments: u64,
}

/// Parse a complete byte stream at once. A trailing event without its blank line is discarded.
pub fn parse_all(bytes: &[u8]) -> Result<Vec<SseEvent>, String> {
    SseParser::default().feed(bytes)
}

impl SseParser {
    pub fn new() -> SseParser {
        SseParser::default()
    }

    /// Feed bytes; returns the events completed by them.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, String> {
        let mut out = Vec::new();
        for &b in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if b == b'\n' {
                    continue;
                }
            }
            match b {
                b'\r' => {
                    self.skip_lf = true;
                    self.end_line(&mut out)?;
                }
                b'\n' => self.end_line(&mut out)?,
                _ => self.line.push(b),
            }
        }
        Ok(out)
    }

    fn end_line(&mut self, out: &mut Vec<SseEvent>) -> Result<(), String> {
        let mut line = std::mem::take(&mut self.line);
        if !self.started {
            self.started = true;
            if line.starts_with(&[0xEF, 0xBB, 0xBF]) {
                line.drain(..3);
            }
        }
        if line.is_empty() {
            if !self.has_data {
                self.event.clear();
                return Ok(());
            }
            let mut data = std::mem::take(&mut self.data);
            if data.last() == Some(&b'\n') {
                data.pop();
            }
            self.has_data = false;
            let data =
                String::from_utf8(data).map_err(|_| "invalid UTF-8 in SSE data".to_string())?;
            let event = String::from_utf8(std::mem::take(&mut self.event))
                .map_err(|_| "invalid UTF-8 in SSE event name".to_string())?;
            out.push(SseEvent {
                event,
                data,
                id: self.id.clone(),
            });
            return Ok(());
        }
        if line[0] == b':' {
            self.comments += 1;
            return Ok(());
        }
        let (field, mut value): (&[u8], &[u8]) = match line.iter().position(|&c| c == b':') {
            Some(i) => (&line[..i], &line[i + 1..]),
            None => (&line[..], &[]),
        };
        if value.first() == Some(&b' ') {
            value = &value[1..];
        }
        match field {
            b"event" => self.event = value.to_vec(),
            b"data" => {
                self.data.extend_from_slice(value);
                self.data.push(b'\n');
                self.has_data = true;
            }
            b"id" if !value.contains(&0) => {
                self.id = Some(String::from_utf8_lossy(value).into_owned())
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CState {
    Size,
    Data,
    AfterData,
    AfterDataCr,
    Trailer,
    Done,
}

/// Incremental decoder of an HTTP/1.1 chunked body (RFC 9112 §7.1).
#[derive(Debug)]
pub struct ChunkedDecoder {
    state: CState,
    line: Vec<u8>,
    remaining: u64,
}

impl Default for ChunkedDecoder {
    fn default() -> Self {
        ChunkedDecoder {
            state: CState::Size,
            line: Vec::new(),
            remaining: 0,
        }
    }
}

impl ChunkedDecoder {
    pub fn new() -> ChunkedDecoder {
        ChunkedDecoder::default()
    }

    /// Whether the terminating chunk and trailers have been read.
    pub fn is_done(&self) -> bool {
        self.state == CState::Done
    }

    /// Feed bytes; returns the decoded payload bytes. Bytes after the end are an error.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match self.state {
                CState::Done => return Err("bytes after the last chunk".into()),
                CState::Data => {
                    let n = (self.remaining as usize).min(bytes.len() - i);
                    out.extend_from_slice(&bytes[i..i + n]);
                    i += n;
                    self.remaining -= n as u64;
                    if self.remaining == 0 {
                        self.state = CState::AfterData;
                    }
                }
                CState::AfterData => {
                    self.state = match bytes[i] {
                        b'\r' => CState::AfterDataCr,
                        b'\n' => CState::Size,
                        _ => return Err("missing CRLF after chunk data".into()),
                    };
                    i += 1;
                }
                CState::AfterDataCr => {
                    if bytes[i] != b'\n' {
                        return Err("missing LF after chunk data".into());
                    }
                    self.state = CState::Size;
                    i += 1;
                }
                CState::Size | CState::Trailer => {
                    let b = bytes[i];
                    i += 1;
                    if b != b'\n' {
                        self.line.push(b);
                        if self.line.len() > 4096 {
                            return Err("chunk line too long".into());
                        }
                        continue;
                    }
                    let mut line = std::mem::take(&mut self.line);
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    if self.state == CState::Trailer {
                        if line.is_empty() {
                            self.state = CState::Done;
                        }
                        continue;
                    }
                    let size_part = line.split(|&c| c == b';').next().unwrap_or(&[]);
                    let size_str = std::str::from_utf8(size_part)
                        .map_err(|_| "bad chunk size")?
                        .trim_matches([' ', '\t']);
                    if size_str.is_empty() || size_str.len() > 16 {
                        return Err(format!("bad chunk size {size_str:?}"));
                    }
                    let size = u64::from_str_radix(size_str, 16)
                        .map_err(|_| format!("bad chunk size {size_str:?}"))?;
                    if size == 0 {
                        self.state = CState::Trailer;
                    } else {
                        self.remaining = size;
                        self.state = CState::Data;
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_bom_comments_and_fields() {
        let s = b"\xEF\xBB\xBF: keep-alive\r\nevent: x\r\ndata: a\rdata:b\n\ndata\n\nid: 7\ndata: z\n\ndata: tail-without-blank-line";
        let ev = parse_all(s).unwrap();
        assert_eq!(ev.len(), 3);
        assert_eq!((ev[0].event.as_str(), ev[0].data.as_str()), ("x", "a\nb"));
        assert_eq!((ev[1].event.as_str(), ev[1].data.as_str()), ("", ""));
        assert_eq!(ev[2].id.as_deref(), Some("7"));
        let mut p = SseParser::new();
        p.feed(s).unwrap();
        assert_eq!(p.comments, 1);
    }

    #[test]
    fn cr_at_split_boundary() {
        let mut p = SseParser::new();
        let mut ev = p.feed(b"data: a\r").unwrap();
        ev.extend(p.feed(b"\n\r").unwrap());
        ev.extend(p.feed(b"\n").unwrap());
        assert_eq!(ev, parse_all(b"data: a\r\n\r\n").unwrap());
        assert_eq!(ev.len(), 1);
    }

    #[test]
    fn invalid_utf8_is_an_error() {
        assert!(parse_all(b"data: \xff\n\n").is_err());
    }

    #[test]
    fn chunked_roundtrip_with_extensions_and_trailers() {
        let wire = b"5;ext=1\r\nhello\r\nA\r\n0123456789\r\n0\r\nx-trailer: 1\r\n\r\n";
        let mut d = ChunkedDecoder::new();
        assert_eq!(d.feed(wire).unwrap(), b"hello0123456789");
        assert!(d.is_done());
        assert!(d.feed(b"x").is_err());
        let mut d = ChunkedDecoder::new();
        assert!(d.feed(b"zz\r\n").is_err());
    }
}
