//! Server-sent events parser (body/provider.md §3.7), following WHATWG "interpreting an event
//! stream". Works on bytes; UTF-8 is decoded per event.

use std::io::{self, BufRead};

use crate::model::{ProviderError, TimeoutPhase};

pub const MAX_LINE: usize = 32 * 1024 * 1024;
pub const MAX_DATA: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    /// `""` means `"message"`.
    pub event: String,
    pub data: String,
    pub id: Option<String>,
}

pub struct SseReader<R> {
    inner: R,
    line: Vec<u8>,
    data: Vec<u8>,
    event: Vec<u8>,
    id: Option<String>,
    /// The last byte consumed was CR: a following LF belongs to the same terminator.
    skip_lf: bool,
    /// Strip one leading UTF-8 BOM.
    at_start: bool,
    /// Count of `:` lines (keep-alives).
    pub comments: u64,
    pub max_line: usize,
    pub max_data: usize,
}

/// Maps a read error from the body stream (provider.md §3.6, §3.9).
pub fn map_read_err(e: io::Error) -> ProviderError {
    match e.kind() {
        io::ErrorKind::UnexpectedEof => ProviderError::Truncated,
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            ProviderError::Timeout(TimeoutPhase::Idle)
        }
        io::ErrorKind::InvalidData => ProviderError::Protocol(format!("body framing: {e}")),
        _ => ProviderError::Io(e),
    }
}

impl<R: BufRead> SseReader<R> {
    pub fn new(inner: R) -> Self {
        SseReader {
            inner,
            line: Vec::new(),
            data: Vec::new(),
            event: Vec::new(),
            id: None,
            skip_lf: false,
            at_start: true,
            comments: 0,
            max_line: MAX_LINE,
            max_data: MAX_DATA,
        }
    }

    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    /// Ok(None) at clean EOF. A pending event without its terminating blank line is discarded.
    pub fn next_event(&mut self) -> Result<Option<SseEvent>, ProviderError> {
        loop {
            // Read one complete line into `self.line`.
            let complete = {
                let buf = match self.inner.fill_buf() {
                    Ok(b) => b,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(map_read_err(e)),
                };
                if buf.is_empty() {
                    self.line.clear();
                    self.data.clear();
                    self.event.clear();
                    return Ok(None);
                }
                let mut i = 0;
                if self.skip_lf {
                    self.skip_lf = false;
                    if buf[0] == b'\n' {
                        i = 1;
                    }
                }
                let rest = &buf[i..];
                match rest.iter().position(|&b| b == b'\r' || b == b'\n') {
                    Some(p) => {
                        self.line.extend_from_slice(&rest[..p]);
                        if rest[p] == b'\r' {
                            self.skip_lf = true;
                        }
                        self.inner.consume(i + p + 1);
                        true
                    }
                    None => {
                        let n = rest.len();
                        self.line.extend_from_slice(rest);
                        self.inner.consume(i + n);
                        false
                    }
                }
            };
            if self.line.len() > self.max_line {
                return Err(ProviderError::Protocol("sse line too long".into()));
            }
            if !complete {
                continue;
            }
            let mut line = std::mem::take(&mut self.line);
            if self.at_start {
                self.at_start = false;
                if line.starts_with(b"\xEF\xBB\xBF") {
                    line.drain(..3);
                }
            }
            if let Some(ev) = self.process_line(&line)? {
                line.clear();
                self.line = line;
                return Ok(Some(ev));
            }
            line.clear();
            self.line = line;
        }
    }

    fn process_line(&mut self, line: &[u8]) -> Result<Option<SseEvent>, ProviderError> {
        if line.is_empty() {
            if self.data.is_empty() {
                self.event.clear();
                return Ok(None);
            }
            if self.data.last() == Some(&b'\n') {
                self.data.pop();
            }
            let data = String::from_utf8(std::mem::take(&mut self.data))
                .map_err(|_| ProviderError::Protocol("sse data is not UTF-8".into()))?;
            let event = String::from_utf8(std::mem::take(&mut self.event))
                .map_err(|_| ProviderError::Protocol("sse event name is not UTF-8".into()))?;
            return Ok(Some(SseEvent {
                event,
                data,
                id: self.id.clone(),
            }));
        }
        if line[0] == b':' {
            self.comments += 1;
            return Ok(None);
        }
        let (field, mut value) = match line.iter().position(|&b| b == b':') {
            Some(p) => (&line[..p], &line[p + 1..]),
            None => (line, &line[line.len()..]),
        };
        if value.first() == Some(&b' ') {
            value = &value[1..];
        }
        match field {
            b"event" => {
                self.event.clear();
                self.event.extend_from_slice(value);
            }
            b"data" => {
                if self.data.len() + value.len() + 1 > self.max_data {
                    return Err(ProviderError::Protocol("sse data too long".into()));
                }
                self.data.extend_from_slice(value);
                self.data.push(b'\n');
            }
            b"id" if !value.contains(&0) => {
                self.id = Some(String::from_utf8_lossy(value).into_owned());
            }
            _ => {} // `retry` and unknown fields: ignored
        }
        Ok(None)
    }
}

#[cfg(test)]
pub mod tests_support {
    use std::io::{self, Read};

    /// A reader that returns at most `n` bytes per read, so `fill_buf` boundaries fall everywhere.
    pub struct Trickle<'a> {
        pub data: &'a [u8],
        pub n: usize,
    }
    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let k = self.n.min(buf.len()).min(self.data.len());
            buf[..k].copy_from_slice(&self.data[..k]);
            self.data = &self.data[k..];
            Ok(k)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::Trickle;
    use super::*;
    use std::io::BufReader;

    fn all(input: &[u8], n: usize) -> Result<Vec<SseEvent>, ProviderError> {
        let mut r = SseReader::new(BufReader::with_capacity(
            n.max(1),
            Trickle { data: input, n },
        ));
        let mut out = Vec::new();
        while let Some(e) = r.next_event()? {
            out.push(e);
        }
        Ok(out)
    }

    fn ev(event: &str, data: &str) -> SseEvent {
        SseEvent {
            event: event.into(),
            data: data.into(),
            id: None,
        }
    }

    #[test]
    fn line_endings_crlf_lf_cr() {
        for input in [
            &b"data: a\r\n\r\ndata: b\r\n\r\n"[..],
            b"data: a\n\ndata: b\n\n",
            b"data: a\r\rdata: b\r\r",
            b"data: a\r\n\ndata: b\r\r\n",
        ] {
            for n in [1, 2, 3, 7, 64] {
                assert_eq!(
                    all(input, n).unwrap(),
                    vec![ev("", "a"), ev("", "b")],
                    "{input:?} n={n}"
                );
            }
        }
    }

    #[test]
    fn cr_at_chunk_end_lf_in_next() {
        // `fill_buf` of 8 bytes ends right after the CR of the first line.
        let input = b"data: ab\r\ndata: c\r\n\r\n";
        assert_eq!(all(input, 9).unwrap(), vec![ev("", "ab\nc")]);
        assert_eq!(all(input, 1).unwrap(), vec![ev("", "ab\nc")]);
    }

    #[test]
    fn bom_comments_multiline_nocolon_nospace() {
        let input = "\u{feff}: keep-alive\n\nevent: x\ndata:one\ndata: two\nfoo\ndata\n\n";
        let mut r = SseReader::new(input.as_bytes());
        let e = r.next_event().unwrap().unwrap();
        assert_eq!(e, ev("x", "one\ntwo\n"));
        assert_eq!(r.comments, 1);
        assert_eq!(r.next_event().unwrap(), None);
    }

    #[test]
    fn unterminated_event_discarded_and_id_kept() {
        let input = b"id: 7\ndata: a\n\ndata: lost";
        let got = all(input, 3).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id.as_deref(), Some("7"));
    }

    #[test]
    fn line_cap_and_invalid_utf8() {
        let mut big = b"data: ".to_vec();
        big.resize(33 * 1024 * 1024, b'x');
        big.extend_from_slice(b"\n\n");
        let mut r = SseReader::new(BufReader::with_capacity(64 * 1024, &big[..]));
        assert!(
            matches!(r.next_event(), Err(ProviderError::Protocol(m)) if m.contains("too long"))
        );
        let mut r = SseReader::new(&b"data: \xff\xfe\n\n"[..]);
        assert!(matches!(r.next_event(), Err(ProviderError::Protocol(_))));
    }

    #[test]
    fn eof_mid_chunk_maps_to_truncated() {
        assert!(matches!(
            map_read_err(io::ErrorKind::UnexpectedEof.into()),
            ProviderError::Truncated
        ));
        assert!(matches!(
            map_read_err(io::ErrorKind::WouldBlock.into()),
            ProviderError::Timeout(TimeoutPhase::Idle)
        ));
    }
}
