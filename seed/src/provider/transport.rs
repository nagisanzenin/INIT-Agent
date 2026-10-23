//! The shared transport (body/provider.md §3.9): one `EventStream` per model call that connects to
//! the proxy socket, decodes the SSE stream through the adapter's `Decoder`, and applies the retry,
//! backoff, timeout and abort rules. Exactly one terminal event (`Done` or `Failed`) per stream.

use std::collections::VecDeque;
use std::hash::BuildHasher;
use std::io::BufReader;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::http::{self, Body, Cx, HttpRequest, Timeouts, header, read_capped};
use super::sse::SseReader;
use super::{Decoder, Provider, error_message};
use crate::model::{Abort, ProviderError, StreamEvent};

pub const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
pub const HEADERS_TIMEOUT: Duration = Duration::from_secs(120);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
pub const MAX_ATTEMPTS: u32 = 12;
pub const RETRY_WINDOW: Duration = Duration::from_secs(30 * 60);
pub const BACKOFF_BASE: Duration = Duration::from_secs(1);
pub const BACKOFF_CAP: Duration = Duration::from_secs(60);
pub const RETRY_AFTER_CAP: Duration = Duration::from_secs(600);
pub const MAX_ERROR_BODY: usize = 64 * 1024;
/// `sun_path` holds 104 bytes on macOS (103 + NUL), 108 on Linux.
pub const MAX_SOCK_PATH: usize = if cfg!(target_os = "macos") { 103 } else { 107 };

/// Where and how to reach the proxy. Defaults are §3.9's table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transport {
    pub sock: PathBuf,
    pub timeouts: Timeouts,
    pub max_attempts: u32,
    pub retry_window: Duration,
    pub backoff_base: Duration,
    pub backoff_cap: Duration,
}

impl Transport {
    pub fn new(sock: impl Into<PathBuf>) -> Self {
        Transport {
            sock: sock.into(),
            timeouts: Timeouts {
                write: WRITE_TIMEOUT,
                headers: HEADERS_TIMEOUT,
                idle: IDLE_TIMEOUT,
            },
            max_attempts: MAX_ATTEMPTS,
            retry_window: RETRY_WINDOW,
            backoff_base: BACKOFF_BASE,
            backoff_cap: BACKOFF_CAP,
        }
    }

    /// `INIT_PROXY_SOCK`, plus the test overrides (only when `INIT_TEST=1`): `INIT_TEST_IDLE_TIMEOUT_MS`,
    /// `INIT_TEST_RETRY_BASE_MS` (testing.md §3.4.6).
    pub fn from_env() -> Result<Self, String> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let sock = get("INIT_PROXY_SOCK")
            .filter(|s| !s.is_empty())
            .ok_or("INIT_PROXY_SOCK is not set")?;
        if sock.len() > MAX_SOCK_PATH {
            return Err(format!(
                "INIT_PROXY_SOCK is {} bytes; the limit is {MAX_SOCK_PATH}",
                sock.len()
            ));
        }
        let mut t = Transport::new(sock);
        if get("INIT_TEST").as_deref() == Some("1") {
            let ms = |k: &str| {
                get(k)
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(Duration::from_millis)
            };
            if let Some(d) = ms("INIT_TEST_IDLE_TIMEOUT_MS") {
                t.timeouts.idle = d;
            }
            if let Some(d) = ms("INIT_TEST_RETRY_BASE_MS") {
                t.backoff_base = d;
            }
        }
        Ok(t)
    }

    /// `delay = min(cap, base · 2^(k−1))`, then full jitter: uniform in `[delay/2, delay]`.
    pub fn backoff(&self, k: u32, rand: u64) -> Duration {
        let exp = k.saturating_sub(1).min(32);
        let d = self
            .backoff_base
            .saturating_mul(1u32 << exp.min(31))
            .min(self.backoff_cap);
        let nanos = d.as_nanos() as u64;
        let half = nanos / 2;
        Duration::from_nanos(half + rand % (nanos - half + 1))
    }
}

/// OS-seeded random bits with no dependency (never reaches a prompt).
fn jitter_bits(attempt: u32) -> u64 {
    std::collections::hash_map::RandomState::new().hash_one(attempt)
}

/// `retry-after-ms` (integer ms), then `retry-after` (integer seconds); capped at 600 s. HTTP-date
/// values are ignored.
pub fn retry_after(headers: &[(String, String)]) -> Option<Duration> {
    let d = if let Some(ms) =
        header(headers, "retry-after-ms").and_then(|v| v.trim().parse::<u64>().ok())
    {
        Duration::from_millis(ms)
    } else {
        Duration::from_secs(header(headers, "retry-after")?.trim().parse::<u64>().ok()?)
    };
    Some(d.min(RETRY_AFTER_CAP))
}

enum State {
    Connect,
    Read(Box<SseReader<BufReader<Body>>>),
    Backoff(Duration),
    Ended,
}

/// One model call. Iterating yields deltas and exactly one terminal event, then `None`.
pub struct EventStream<'a> {
    p: &'a dyn Provider,
    t: &'a Transport,
    abort: &'a Abort,
    req: HttpRequest,
    cx: Cx,
    /// 1-based; sent as `x-init-attempt`.
    attempt: u32,
    /// Backoff exponent; jumps by the proxy's `x-init-retries`.
    delay_step: u32,
    first_failure: Option<Instant>,
    decoder: Box<dyn Decoder>,
    state: State,
    pending: VecDeque<StreamEvent>,
    fallbacks: Vec<&'static str>,
    done: bool,
}

/// provider.md §3.4 `stream()`: sends `req` (already built) and decodes the reply.
pub fn stream<'a>(
    p: &'a dyn Provider,
    t: &'a Transport,
    req: HttpRequest,
    cx: Cx,
    abort: &'a Abort,
) -> EventStream<'a> {
    EventStream {
        p,
        t,
        abort,
        req,
        cx,
        attempt: 1,
        delay_step: 1,
        first_failure: None,
        decoder: p.decoder(),
        state: State::Connect,
        pending: VecDeque::new(),
        fallbacks: Vec::new(),
        done: false,
    }
}

impl EventStream<'_> {
    /// The causes of one-shot fallback rebuilds applied so far (for the loop's `error` event).
    pub fn fallbacks(&self) -> &[&'static str] {
        &self.fallbacks
    }

    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    fn terminal(&mut self, ev: StreamEvent) {
        self.pending.push_back(ev);
        self.done = true;
        self.state = State::Ended;
        self.abort.clear_stream();
    }

    fn abort_end(&mut self) {
        let partial = self.decoder.partial();
        self.terminal(StreamEvent::Done(partial));
    }

    fn note_proxy_retries(&mut self, headers: &[(String, String)]) {
        if let Some(n) =
            header(headers, "x-init-retries").and_then(|v| v.trim().parse::<u32>().ok())
        {
            self.delay_step = self.delay_step.saturating_add(n);
        }
    }

    fn step(&mut self) {
        if self.abort.is_set() {
            return self.abort_end();
        }
        match std::mem::replace(&mut self.state, State::Ended) {
            State::Connect => self.connect(),
            State::Read(sse) => self.read(sse),
            State::Backoff(d) => {
                if self.abort.sleep(d) {
                    self.abort_end();
                } else {
                    self.state = State::Connect;
                }
            }
            State::Ended => self.done = true,
        }
    }

    fn connect(&mut self) {
        match http::send(
            &self.t.sock,
            &self.req,
            self.attempt,
            &self.cx,
            self.abort,
            &self.t.timeouts,
        ) {
            Ok(resp) if resp.status == 200 => {
                self.note_proxy_retries(&resp.headers);
                let h = |k: &str| resp.header(k).map(str::to_string);
                self.pending.push_back(StreamEvent::Started {
                    attempt: self.attempt,
                    request_id: h("request-id").or_else(|| h("x-request-id")),
                    proxy_req_id: h("x-init-req-id"),
                    head_hash: h("x-init-head-hash"),
                });
                let r = BufReader::with_capacity(http::READ_BUF, resp.body);
                self.state = State::Read(Box::new(SseReader::new(r)));
            }
            Ok(mut resp) => {
                self.note_proxy_retries(&resp.headers);
                let raw = read_capped(&mut resp.body, MAX_ERROR_BODY);
                let (err, proxy_generated) =
                    classify_http(self.p, resp.status, &resp.headers, &raw);
                self.fail(err, proxy_generated);
            }
            Err(e) => self.fail(e, false),
        }
    }

    fn read(&mut self, mut sse: Box<SseReader<BufReader<Body>>>) {
        match sse.next_event() {
            Ok(Some(ev)) => {
                let mut buf = Vec::new();
                let r = self.decoder.feed(&ev, &mut buf);
                self.pending.extend(buf);
                if let Err(e) = r {
                    return self.fail(e, false);
                }
                if self.decoder.finished() {
                    match self.decoder.finish() {
                        Ok(m) => self.terminal(StreamEvent::Done(m)),
                        Err(e) => self.fail(e, false),
                    }
                } else {
                    self.state = State::Read(sse);
                }
            }
            // `Abort::trigger` shuts the socket down, which reads as EOF.
            Ok(None) if self.abort.is_set() => self.abort_end(),
            Ok(None) => self.fail(ProviderError::Truncated, false),
            Err(_) if self.abort.is_set() => self.abort_end(),
            Err(e) => self.fail(e, false),
        }
    }

    fn next_attempt(&mut self) {
        self.attempt += 1;
        self.delay_step = self.delay_step.saturating_add(1);
        self.decoder = self.p.decoder();
    }

    fn fail(&mut self, err: ProviderError, proxy_generated: bool) {
        if self.abort.is_set() || matches!(err, ProviderError::Aborted) {
            return self.abort_end();
        }
        if matches!(err, ProviderError::Budget { .. }) {
            return self.terminal(StreamEvent::Failed(err)); // invariant 3: never retried
        }
        if let Some((req, cause)) = self.p.fallback(&self.req, &err, &self.fallbacks) {
            self.fallbacks.push(cause);
            self.req = req;
            // The cause reaches the loop through the ModelClient seam as a Warning, which it logs
            // as `error{code}` (provider.md §3.8.2 rule 4, §3.10).
            self.pending.push_back(StreamEvent::Warning {
                code: cause,
                detail: err.to_string(),
            });
            self.pending.push_back(StreamEvent::Retry {
                attempt: self.attempt,
                delay: Duration::ZERO,
                error: err,
            });
            self.next_attempt();
            self.state = State::Connect;
            return;
        }
        if proxy_generated {
            // The proxy already applied its own retry policy, or the error is local.
            return self.terminal(StreamEvent::Failed(err));
        }
        let first = *self.first_failure.get_or_insert_with(Instant::now);
        if err.retryable()
            && self.attempt < self.t.max_attempts
            && first.elapsed() < self.t.retry_window
        {
            let backoff = self.t.backoff(self.delay_step, jitter_bits(self.attempt));
            let hint = match &err {
                ProviderError::Http {
                    retry_after: Some(d),
                    ..
                } => *d,
                _ => Duration::ZERO,
            };
            let delay = backoff.max(hint);
            self.pending.push_back(StreamEvent::Retry {
                attempt: self.attempt,
                delay,
                error: err,
            });
            self.next_attempt();
            self.state = State::Backoff(delay);
        } else {
            self.terminal(StreamEvent::Failed(err));
        }
    }
}

impl Iterator for EventStream<'_> {
    type Item = StreamEvent;

    fn next(&mut self) -> Option<StreamEvent> {
        loop {
            if let Some(ev) = self.pending.pop_front() {
                return Some(ev);
            }
            if self.done {
                return None;
            }
            self.step();
        }
    }
}

/// A non-200 response → `(error, proxy_generated)`. Proxy caps (`x-init-cap`, or an error of type
/// `init_cap_exceeded`) are `Budget`; everything else is classified by the adapter.
pub fn classify_http(
    p: &dyn Provider,
    status: u16,
    headers: &[(String, String)],
    raw: &[u8],
) -> (ProviderError, bool) {
    let json: Option<Value> = serde_json::from_slice(raw).ok();
    let message = error_message(json.as_ref(), raw);
    let proxy_generated = header(headers, "x-init-error").is_some();
    let cap_type = json
        .as_ref()
        .and_then(|v| v.pointer("/error/type"))
        .and_then(Value::as_str)
        == Some("init_cap_exceeded");
    if header(headers, "x-init-cap").is_some() || cap_type {
        return (ProviderError::Budget { message }, true);
    }
    let kind = p.classify(status, headers, json.as_ref(), &message);
    let request_id = header(headers, "x-init-req-id")
        .or_else(|| header(headers, "request-id"))
        .or_else(|| header(headers, "x-request-id"))
        .map(str::to_string)
        .or_else(|| {
            json.as_ref()
                .and_then(|v| v.get("request_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    (
        ProviderError::Http {
            status,
            kind,
            message,
            retry_after: retry_after(headers),
            request_id,
        },
        proxy_generated,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn retry_after_parsing() {
        assert_eq!(
            retry_after(&h(&[("retry-after", "3")])),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            retry_after(&h(&[("retry-after-ms", "250"), ("retry-after", "3")])),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            retry_after(&h(&[("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT")])),
            None
        );
        assert_eq!(
            retry_after(&h(&[("retry-after", "86400")])),
            Some(RETRY_AFTER_CAP)
        );
        assert_eq!(retry_after(&h(&[])), None);
    }

    /// §8 item 10: the jitter range and the cap.
    #[test]
    fn backoff_and_jitter_range() {
        let t = Transport::new("/x");
        for k in 1..=12u32 {
            let full = Duration::from_secs(1u64 << (k - 1)).min(BACKOFF_CAP);
            for r in [0u64, 1, 12345, u64::MAX / 3, u64::MAX] {
                let d = t.backoff(k, r);
                assert!(d >= full / 2 && d <= full, "k={k} d={d:?}");
            }
            let real = t.backoff(k, jitter_bits(k));
            assert!(real >= full / 2 && real <= full);
        }
        assert_eq!(t.backoff(1, 0), Duration::from_millis(500));
        assert_eq!(
            t.backoff(40, u64::MAX),
            t.backoff(40, u64::MAX).min(BACKOFF_CAP)
        );
    }

    #[test]
    fn env_overrides_only_in_test_mode() {
        let vars = |test: &'static str| {
            move |k: &str| -> Option<String> {
                match k {
                    "INIT_PROXY_SOCK" => Some("/tmp/p.sock".into()),
                    "INIT_TEST" => Some(test.into()),
                    "INIT_TEST_IDLE_TIMEOUT_MS" => Some("2000".into()),
                    "INIT_TEST_RETRY_BASE_MS" => Some("10".into()),
                    _ => None,
                }
            }
        };
        let t = Transport::from_vars(vars("1")).unwrap();
        assert_eq!(t.timeouts.idle, Duration::from_millis(2000));
        assert_eq!(t.backoff_base, Duration::from_millis(10));
        let t = Transport::from_vars(vars("0")).unwrap();
        assert_eq!(t.timeouts.idle, IDLE_TIMEOUT);
        assert!(Transport::from_vars(|_| None).is_err());
        let long = "/".repeat(200);
        assert!(
            Transport::from_vars(move |k| (k == "INIT_PROXY_SOCK").then(|| long.clone())).is_err()
        );
    }
}
