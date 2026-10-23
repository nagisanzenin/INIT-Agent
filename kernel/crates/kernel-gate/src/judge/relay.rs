//! The judge's in-memory observation relay (gate.md §4.1a: "in `judge` mode the in-process proxy
//! features keep observations in memory only, with no trace store and no ledger file").
//!
//! One relay per run: a Unix socket the body talks to (the kernel-proxy wire, `POST /<provider>/<path>`),
//! forwarded request by request to the upstream the judge was given (`INIT_PROXY_SOCK`: the Root's
//! sealed lane). The relay reads each response whole, takes the usage and the finish reason from it,
//! and answers the body with the same status and body (`content-length`, `connection: close`). Nothing
//! is written anywhere; [`Obs`] lives in memory and dies with the run.
//!
//! Costs are reference costs (§3.8 "cost metric"): tokens priced at the default model's reference
//! window (`deepseek-flash`, peak), never the billed cost, so both arms are priced identically.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Reference prices in pico-USD per token: `deepseek-flash` at its reference window (peak), kernel
/// config §4.x defaults (input $0.30, cache read $0.006, output $1.20 per Mtok).
pub const PRICE_INPUT_PUSD: u64 = 300_000;
pub const PRICE_CACHE_READ_PUSD: u64 = 6_000;
pub const PRICE_OUTPUT_PUSD: u64 = 1_200_000;

const HEAD_MAX: usize = 64 << 10;
const BODY_MAX: usize = 64 << 20;
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(900);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(120);

/// What one run's relay observed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Obs {
    pub requests: u64,
    pub inflight: u64,
    /// All prompt tokens, cached or not.
    pub tokens_in: u64,
    pub cache_read: u64,
    pub tokens_out: u64,
    pub cost_ref_pusd: u64,
    /// The last finish, normalized: `stop`, `tool`, or the provider's own word.
    pub last_finish: Option<String>,
    pub last_end: Option<Instant>,
    /// A request was refused because the run's cost cap was reached.
    pub capped: bool,
    pub refused: u64,
}

impl Obs {
    pub fn cost_ref_musd(&self) -> u64 {
        self.cost_ref_pusd / 1_000_000
    }
}

/// A running relay. [`Relay::stop`] closes it and removes its socket by its exact path.
pub struct Relay {
    bound: PathBuf,
    obs: Arc<Mutex<Obs>>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

fn lock(m: &Mutex<Obs>) -> std::sync::MutexGuard<'_, Obs> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Relay {
    /// Binds `bind` (a path short enough for `sun_path`, possibly relative to the process cwd) and
    /// starts accepting. `cap_musd` is the run's cost cap (`task.limits.max_cost_musd`).
    pub fn start(bind: &Path, upstream: Option<PathBuf>, cap_musd: u64) -> io::Result<Relay> {
        let l = UnixListener::bind(bind)?;
        let obs = Arc::new(Mutex::new(Obs::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (o2, s2) = (obs.clone(), stop.clone());
        let accept = std::thread::spawn(move || {
            for c in l.incoming() {
                if s2.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(c) = c else { continue };
                let (o, up) = (o2.clone(), upstream.clone());
                std::thread::spawn(move || serve(c, up.as_deref(), &o, cap_musd));
            }
        });
        Ok(Relay {
            bound: bind.to_path_buf(),
            obs,
            stop,
            accept: Some(accept),
        })
    }

    pub fn snapshot(&self) -> Obs {
        lock(&self.obs).clone()
    }

    pub fn stop(mut self) -> Obs {
        self.close();
        self.snapshot()
    }

    fn close(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.bound); // wakes the accept loop
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
        let _ = std::fs::remove_file(&self.bound);
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        if self.accept.is_some() {
            self.close();
        }
    }
}

struct Head {
    first: String,
    headers: Vec<(String, String)>,
}

impl Head {
    fn get(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
}

fn read_head(r: &mut impl BufRead) -> io::Result<Option<Head>> {
    let mut lines = Vec::new();
    let mut total = 0usize;
    loop {
        let mut l = String::new();
        let n = r.read_line(&mut l)?;
        if n == 0 {
            return if lines.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated head",
                ))
            };
        }
        total += n;
        if total > HEAD_MAX {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "head too large"));
        }
        let l = l.trim_end_matches(['\r', '\n']).to_string();
        if l.is_empty() {
            if lines.is_empty() {
                continue;
            }
            break;
        }
        lines.push(l);
    }
    let first = lines.remove(0);
    let headers = lines
        .into_iter()
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect();
    Ok(Some(Head { first, headers }))
}

fn read_n(r: &mut impl Read, n: usize) -> io::Result<Vec<u8>> {
    if n > BODY_MAX {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
    }
    let mut b = vec![0; n];
    r.read_exact(&mut b)?;
    Ok(b)
}

fn read_chunked(r: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let mut l = String::new();
        if r.read_line(&mut l)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated chunk",
            ));
        }
        let size = usize::from_str_radix(l.trim().split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad chunk size"))?;
        if size == 0 {
            // trailers until the empty line
            loop {
                let mut t = String::new();
                if r.read_line(&mut t)? == 0 || t.trim().is_empty() {
                    return Ok(out);
                }
            }
        }
        if out.len() + size > BODY_MAX {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
        }
        out.extend(read_n(r, size)?);
        let mut crlf = String::new();
        r.read_line(&mut crlf)?;
    }
}

fn read_body(r: &mut impl BufRead, h: &Head, until_eof: bool) -> io::Result<Vec<u8>> {
    if h.get("transfer-encoding")
        .is_some_and(|t| t.to_ascii_lowercase().contains("chunked"))
    {
        return read_chunked(r);
    }
    if let Some(n) = h.get("content-length") {
        let n: usize = n
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad content-length"))?;
        return read_n(r, n);
    }
    let mut b = Vec::new();
    if until_eof {
        r.take(BODY_MAX as u64).read_to_end(&mut b)?;
    }
    Ok(b)
}

fn respond(w: &mut impl Write, status: &str, ctype: &str, body: &[u8]) -> io::Result<()> {
    let head = format!(
        "{status}\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    w.write_all(head.as_bytes())?;
    w.write_all(body)?;
    w.flush()
}

fn error_body(code: &str, msg: &str) -> Vec<u8> {
    json!({"error": {"code": code, "message": msg, "type": "init_judge_error"}})
        .to_string()
        .into_bytes()
}

/// The relay's `GET /_init/stats`, in the sealed lane's shape (root/egress.md §3.8), so a relay can
/// stand in for a lane (tests of the direct egress mode).
fn stats(o: &Obs) -> Value {
    let idle = o
        .last_end
        .map(|e| e.elapsed().as_millis() as u64)
        .unwrap_or(0);
    json!({"requests": o.requests, "inflight": o.inflight, "input_tokens": o.tokens_in,
           "output_tokens": o.tokens_out, "spent_musd": o.cost_ref_musd(), "capped": u64::from(o.capped),
           "refused": o.refused, "last_finish": o.last_finish, "idle_ms": idle, "lane": "judge"})
}

/// `GET /_init/stats` on an upstream (the kernel-proxy wire; the sealed lane answers it).
pub fn upstream_stats(sock: &Path) -> Option<Value> {
    let mut s = UnixStream::connect(sock).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    s.write_all(b"GET /_init/stats HTTP/1.1\r\nhost: lane\r\nconnection: close\r\n\r\n")
        .ok()?;
    let mut r = BufReader::new(s);
    let h = read_head(&mut r).ok()??;
    if !h
        .first
        .split_whitespace()
        .nth(1)
        .is_some_and(|c| c == "200")
    {
        return None;
    }
    let b = read_body(&mut r, &h, true).ok()?;
    serde_json::from_slice::<Value>(&b)
        .ok()
        .filter(Value::is_object)
}

/// One connection: requests until the client closes; each is forwarded on its own upstream connection.
fn serve(c: UnixStream, upstream: Option<&Path>, obs: &Mutex<Obs>, cap_musd: u64) {
    let _ = c.set_read_timeout(Some(CLIENT_TIMEOUT));
    let Ok(mut w) = c.try_clone() else { return };
    let mut r = BufReader::new(c);
    while let Ok(Some(h)) = read_head(&mut r) {
        let Ok(body) = read_body(&mut r, &h, false) else {
            let _ = respond(
                &mut w,
                "HTTP/1.1 400 Bad Request",
                "application/json",
                &error_body("judge.bad_request", "unreadable request"),
            );
            return;
        };
        let mut parts = h.first.split_whitespace();
        let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let path = target.split('?').next().unwrap_or("");
        let res = match (method, path) {
            ("GET", "/_init/stats") => {
                let s = stats(&lock(obs));
                respond(
                    &mut w,
                    "HTTP/1.1 200 OK",
                    "application/json",
                    s.to_string().as_bytes(),
                )
            }
            ("GET", "/_init/health") => respond(
                &mut w,
                "HTTP/1.1 200 OK",
                "application/json",
                br#"{"lane":"judge","ok":true,"v":1}"#,
            ),
            _ => forward(&h, method, target, &body, upstream, obs, cap_musd, &mut w),
        };
        if res.is_err()
            || h.get("connection")
                .is_some_and(|v| v.eq_ignore_ascii_case("close"))
        {
            return;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn forward(
    h: &Head,
    method: &str,
    target: &str,
    body: &[u8],
    upstream: Option<&Path>,
    obs: &Mutex<Obs>,
    cap_musd: u64,
    w: &mut UnixStream,
) -> io::Result<()> {
    {
        let mut o = lock(obs);
        if o.cost_ref_musd() >= cap_musd {
            o.capped = true;
            o.refused += 1;
            drop(o);
            return respond(
                w,
                "HTTP/1.1 429 Too Many Requests",
                "application/json",
                &error_body("ceiling.run", "the run's cost cap is reached"),
            );
        }
        o.inflight += 1;
    }
    let got = upstream
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no upstream (INIT_PROXY_SOCK)"))
        .and_then(|u| round_trip(u, h, method, target, body));
    let mut o = lock(obs);
    o.inflight -= 1;
    o.last_end = Some(Instant::now());
    match got {
        Ok((status, ctype, data)) => {
            o.requests += 1;
            let u = usage(&ctype, &data);
            o.tokens_in += u.input;
            o.cache_read += u.cache_read;
            o.tokens_out += u.output;
            o.cost_ref_pusd += u.input.saturating_sub(u.cache_read) * PRICE_INPUT_PUSD
                + u.cache_read * PRICE_CACHE_READ_PUSD
                + u.output * PRICE_OUTPUT_PUSD;
            o.last_finish = Some(u.finish.unwrap_or_else(|| "other".into()));
            drop(o);
            respond(w, &status, &ctype, &data)
        }
        Err(e) => {
            o.refused += 1;
            o.last_finish = Some("error".into());
            drop(o);
            respond(
                w,
                "HTTP/1.1 502 Bad Gateway",
                "application/json",
                &error_body("judge.upstream", &e.to_string()),
            )
        }
    }
}

fn round_trip(
    upstream: &Path,
    h: &Head,
    method: &str,
    target: &str,
    body: &[u8],
) -> io::Result<(String, String, Vec<u8>)> {
    let s = UnixStream::connect(upstream)?;
    s.set_read_timeout(Some(UPSTREAM_TIMEOUT))?;
    let mut up = s.try_clone()?;
    let mut head = format!("{method} {target} HTTP/1.1\r\n");
    for (k, v) in &h.headers {
        if !matches!(
            k.as_str(),
            "connection" | "content-length" | "transfer-encoding" | "keep-alive"
        ) {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    head.push_str(&format!(
        "content-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    ));
    up.write_all(head.as_bytes())?;
    up.write_all(body)?;
    up.flush()?;
    let mut r = BufReader::new(s);
    let rh = read_head(&mut r)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "upstream closed"))?;
    let data = read_body(&mut r, &rh, true)?;
    let ctype = rh
        .get("content-type")
        .unwrap_or("application/json")
        .to_string();
    Ok((rh.first.clone(), ctype, data))
}

/// Usage and finish of one response.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub cache_read: u64,
    pub output: u64,
    pub finish: Option<String>,
}

fn norm_finish(s: &str) -> String {
    match s {
        "stop" | "end_turn" | "stop_sequence" | "completed" => "stop".into(),
        "tool_calls" | "tool_use" | "function_call" => "tool".into(),
        other => other.into(),
    }
}

fn take_usage(v: &Value, u: &mut Usage) {
    let n = |x: &Value| x.as_u64();
    let us = v.get("usage").or_else(|| v.pointer("/message/usage"));
    if let Some(us) = us.filter(|x| x.is_object()) {
        if let Some(i) = n(&us["prompt_tokens"]).or_else(|| n(&us["input_tokens"])) {
            u.input = i;
        }
        if let Some(c) = n(&us["prompt_cache_hit_tokens"])
            .or_else(|| {
                us.pointer("/prompt_tokens_details/cached_tokens")
                    .and_then(Value::as_u64)
            })
            .or_else(|| n(&us["cache_read_input_tokens"]))
        {
            u.cache_read = c;
        }
        if let Some(o) = n(&us["completion_tokens"]).or_else(|| n(&us["output_tokens"])) {
            u.output = o;
        }
    }
    let f = v
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        .or_else(|| v.pointer("/delta/stop_reason").and_then(Value::as_str))
        .or_else(|| v.get("stop_reason").and_then(Value::as_str));
    if let Some(f) = f {
        u.finish = Some(norm_finish(f));
    }
}

/// Usage from an SSE stream (`data: {…}` lines; the last values win) or a JSON body.
pub fn usage(ctype: &str, data: &[u8]) -> Usage {
    let mut u = Usage::default();
    let text = String::from_utf8_lossy(data);
    if ctype.contains("event-stream") {
        for l in text.lines() {
            if let Some(j) = l.strip_prefix("data:").map(str::trim)
                && let Ok(v) = serde_json::from_str::<Value>(j)
            {
                take_usage(&v, &mut u);
            }
        }
    } else if let Ok(v) = serde_json::from_str::<Value>(&text) {
        take_usage(&v, &mut u);
    }
    u
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_from_deepseek_sse_and_json() {
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n\
                   data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":100,\
                   \"prompt_cache_hit_tokens\":40,\"completion_tokens\":7}}\n\ndata: [DONE]\n\n";
        let u = usage("text/event-stream", sse.as_bytes());
        assert_eq!(
            u,
            Usage {
                input: 100,
                cache_read: 40,
                output: 7,
                finish: Some("stop".into())
            }
        );
        let j = r#"{"choices":[{"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#;
        let u = usage("application/json", j.as_bytes());
        assert_eq!(
            (u.input, u.output, u.finish.as_deref()),
            (5, 2, Some("tool"))
        );
        let a = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":9,\"output_tokens\":1}}}\n\
                 data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4}}\n";
        let u = usage("text/event-stream", a.as_bytes());
        assert_eq!(
            (u.input, u.output, u.finish.as_deref()),
            (9, 4, Some("stop"))
        );
    }

    #[test]
    fn chunked_bodies_are_reassembled() {
        let raw = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let mut r = BufReader::new(&raw[..]);
        assert_eq!(read_chunked(&mut r).unwrap(), b"hello world");
    }
}
