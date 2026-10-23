//! The fake Root's **mock egress** on `ROOT_HOME/run/egress.sock` (testing.md §3.5b; kernel/proxy.md
//! Q-TS1; wire: root/egress.md §4.1–§4.2).
//!
//! - `GET /_root/health` and `GET /_root/ceilings` (the money block, fresh, ICJ integers);
//! - `POST /<provider_id>/<path>`: model check against the charter fixture's table (`class.model`,
//!   `class.level`), a peak-priced reservation against the day and total ceilings (`ceiling.day`,
//!   `ceiling.total`, 429 with `retry-after` for the day), then a byte-for-byte forward to the
//!   upstream mock (`init-mockllm` in upstream mode) with the allowlisted request headers, a fake
//!   bearer key and `accept-encoding: identity`. The response streams back unchanged (re-framed as
//!   chunked), plus `x-egress-req-id`, `x-egress-retries`, `x-egress-reserved-musd`,
//!   `x-egress-epoch`. The tap reads usage from the stream and settles at peak prices; a stream
//!   without usage is charged its full reservation.
//! - typed refusals carry `x-init-egress-error: <code>` and the body of root/egress.md §3.6;
//! - `/x/<class>/…`, `/_root/action` and `CONNECT` are refused (no tunnel, no crates upstream).
//!
//! Each request is recorded in `ROOT_HOME/egress/usage/<date>.jsonl` (`start` and `end` records,
//! no content), with whether the kernel sent `x-egress-guard` (a missing header is what the real
//! Root counts as a guard bypass, D6).

use super::{Root, Upstream};
use crate::mockllm::sse::{ChunkedDecoder, SseParser};
use crate::util;
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::time::Duration;

const MAX_HEAD: usize = 64 * 1024;
const MAX_BODY: usize = 64 * 1024 * 1024;
/// Request headers forwarded upstream (root/egress.md §4.1); everything else is dropped.
const FORWARD: [&str; 6] = [
    "content-type",
    "accept",
    "anthropic-version",
    "anthropic-beta",
    "openai-beta",
    "idempotency-key",
];
/// Response headers never copied back (hop-by-hop or re-framed here).
const HOP: [&str; 5] = [
    "connection",
    "transfer-encoding",
    "content-length",
    "keep-alive",
    "upgrade",
];

struct Head {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    close: bool,
}

impl Head {
    fn header(&self, n: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.as_str())
    }
}

fn read_head<R: BufRead>(r: &mut R) -> io::Result<Option<Head>> {
    let mut line = String::new();
    let mut budget = MAX_HEAD;
    loop {
        line.clear();
        let n = r.by_ref().take(budget as u64).read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        budget = budget.saturating_sub(n);
        if !line.trim().is_empty() {
            break;
        }
    }
    let mut parts = line.trim_end().split(' ');
    let (Some(method), Some(target), Some(_ver)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "malformed request line",
        ));
    };
    let (method, target) = (method.to_string(), target.to_string());
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        let n = r.by_ref().take(budget as u64).read_line(&mut h)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "EOF in headers",
            ));
        }
        budget = budget.saturating_sub(n);
        if budget == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
        let h = h.trim_end_matches(['\r', '\n']);
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let close = headers
        .iter()
        .any(|(k, v)| k == "connection" && v.to_ascii_lowercase().contains("close"));
    Ok(Some(Head {
        method,
        target,
        headers,
        close,
    }))
}

fn read_body<R: BufRead>(r: &mut R, h: &Head) -> io::Result<Vec<u8>> {
    if h.header("transfer-encoding")
        .is_some_and(|t| t.to_ascii_lowercase().contains("chunked"))
    {
        let mut dec = ChunkedDecoder::new();
        let mut out = Vec::new();
        while !dec.is_done() {
            let mut b = [0u8; 1];
            if r.read(&mut b)? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "EOF in chunked body",
                ));
            }
            out.extend(dec.feed(&b).map_err(io::Error::other)?);
            if out.len() > MAX_BODY {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
            }
        }
        return Ok(out);
    }
    let n: usize = h
        .header("content-length")
        .map(|c| c.trim().parse().unwrap_or(0))
        .unwrap_or(0);
    if n > MAX_BODY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
    }
    let mut out = vec![0u8; n];
    r.read_exact(&mut out)?;
    Ok(out)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

fn write_simple(
    w: &mut impl Write,
    status: u16,
    headers: &[(&str, String)],
    body: &[u8],
    close: bool,
) -> io::Result<()> {
    let mut h = format!(
        "HTTP/1.1 {status} {}\r\ncontent-length: {}\r\n",
        reason(status),
        body.len()
    );
    for (k, v) in headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str(if close {
        "connection: close\r\n\r\n"
    } else {
        "connection: keep-alive\r\n\r\n"
    });
    w.write_all(h.as_bytes())?;
    w.write_all(body)?;
    w.flush()
}

/// A typed refusal (root/egress.md §3.6).
pub struct Refusal {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub limit: u64,
    pub spent: u64,
    pub reserved: u64,
    pub retryable: bool,
    pub demotes: bool,
}

fn refuse(root: &Root, w: &mut impl Write, r: &Refusal, close: bool) -> io::Result<()> {
    let money = root.money_block();
    let ty = if r.code.starts_with("ceiling.") {
        "init_ceiling_exceeded"
    } else {
        "init_egress_refused"
    };
    let body = json!({
        "error": {"type": ty, "code": r.code, "message": r.message},
        "init": {"source": "root", "code": r.code, "limit_musd": r.limit, "spent_musd": r.spent, "reserved_musd": r.reserved,
                 "epoch": root.ceilings(false).epoch.unwrap_or(0), "resets_at": money.day_resets_at, "retryable": r.retryable,
                 "demotes": r.demotes, "level": root.level(), "class": "providers"}
    });
    let mut headers = vec![
        ("content-type", "application/json".to_string()),
        ("x-init-egress-error", r.code.to_string()),
    ];
    if r.code == "ceiling.day" {
        let secs = (util::parse_rfc3339(&money.day_resets_at).unwrap_or(0) - root.now_ms())
            .max(1000)
            / 1000;
        headers.push(("retry-after", secs.to_string()));
    }
    root.log(json!({"ev": "egress_refused", "code": r.code, "message": r.message}));
    write_simple(
        w,
        r.status,
        &headers,
        &serde_json::to_vec(&body).unwrap_or_default(),
        close,
    )
}

fn simple_refusal(
    status: u16,
    code: &'static str,
    message: impl Into<String>,
    demotes: bool,
) -> Refusal {
    Refusal {
        status,
        code,
        message: message.into(),
        limit: 0,
        spent: 0,
        reserved: 0,
        retryable: false,
        demotes,
    }
}

/// Serve the egress socket forever (one thread per connection).
pub fn serve(root: Arc<Root>, l: UnixListener) {
    for c in l.incoming().flatten() {
        let r = root.clone();
        std::thread::spawn(move || {
            let _ = conn(&r, c);
        });
    }
}

fn conn(root: &Arc<Root>, s: UnixStream) -> io::Result<()> {
    let mut w = s.try_clone()?;
    let mut r = BufReader::new(s);
    while let Some(h) = read_head(&mut r)? {
        let body = read_body(&mut r, &h)?;
        let keep = handle(root, &mut w, &h, body)?;
        if h.close || !keep {
            break;
        }
    }
    Ok(())
}

/// Usage found in a provider response (`(input_total, output)` tokens).
pub fn usage_of(v: &Value) -> Option<(u64, u64)> {
    let u = v
        .get("usage")
        .or_else(|| v.get("message").and_then(|m| m.get("usage")))
        .or_else(|| v.get("response").and_then(|m| m.get("usage")))?;
    let input = u
        .get("prompt_tokens")
        .or_else(|| u.get("input_tokens"))
        .and_then(Value::as_u64);
    let output = u
        .get("completion_tokens")
        .or_else(|| u.get("output_tokens"))
        .and_then(Value::as_u64);
    if input.is_none() && output.is_none() {
        return None;
    }
    Some((input.unwrap_or(0), output.unwrap_or(0)))
}

fn mul_price(tokens: u64, musd_per_mtok: u64) -> u64 {
    ((tokens as u128 * musd_per_mtok as u128).div_ceil(1_000_000)) as u64
}

/// Returns false when the connection must close (a cut upstream stream, or an upstream error).
fn handle(root: &Arc<Root>, w: &mut UnixStream, h: &Head, body: Vec<u8>) -> io::Result<bool> {
    let close = h.close;
    let path = h.target.split('?').next().unwrap_or("").to_string();
    if h.method == "CONNECT" {
        refuse(
            root,
            w,
            &simple_refusal(
                403,
                "class.destination",
                "the Root offers no tunnel (CONNECT refused)",
                true,
            ),
            true,
        )?;
        return Ok(false);
    }
    match (h.method.as_str(), path.as_str()) {
        ("GET", "/_root/health") => {
            write_simple(
                w,
                200,
                &[("content-type", "application/json".into())],
                br#"{"ok":true,"v":1}"#,
                close,
            )?;
            return Ok(true);
        }
        ("GET", "/_root/ceilings") => {
            let c = serde_json::to_vec(&root.ceilings(false)).unwrap_or_default();
            write_simple(
                w,
                200,
                &[("content-type", "application/json".into())],
                &c,
                close,
            )?;
            return Ok(true);
        }
        (_, "/_root/action") => {
            refuse(
                root,
                w,
                &simple_refusal(
                    403,
                    "class.unknown",
                    "helper verbs are refused in Phase 0",
                    true,
                ),
                close,
            )?;
            return Ok(true);
        }
        _ => {}
    }
    if root.is_panicked() {
        refuse(
            root,
            w,
            &simple_refusal(
                503,
                "root.panic",
                "the Root is panicked: egress is closed",
                false,
            ),
            close,
        )?;
        return Ok(true);
    }
    let mut segs = path.trim_start_matches('/').splitn(2, '/');
    let (provider, rest) = (segs.next().unwrap_or(""), segs.next().unwrap_or(""));
    if provider == "x" {
        let class = rest.split('/').next().unwrap_or("");
        let code = if class == "crates" {
            "class.disabled"
        } else {
            "class.unknown"
        };
        let demotes = code == "class.unknown";
        refuse(
            root,
            w,
            &simple_refusal(
                403,
                code,
                format!("class `{class}` is not available in the fake Root"),
                demotes,
            ),
            close,
        )?;
        return Ok(true);
    }
    if !["deepseek", "anthropic", "openai", "mock"].contains(&provider)
        || rest.is_empty()
        || rest.contains("..")
        || rest.contains("//")
    {
        refuse(
            root,
            w,
            &simple_refusal(
                403,
                "class.unknown",
                format!("no model class routes `{path}`"),
                true,
            ),
            close,
        )?;
        return Ok(true);
    }
    let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let model_name = req["model"].as_str().unwrap_or("").to_string();
    let level = root.level();
    let (in_price, out_price, reserve_out) = match root.charter.model(&model_name) {
        None => {
            refuse(
                root,
                w,
                &simple_refusal(
                    403,
                    "class.model",
                    format!("model `{model_name}` is not in the charter's model table"),
                    true,
                ),
                close,
            )?;
            return Ok(true);
        }
        Some(m) if m.min_level > level => {
            refuse(
                root,
                w,
                &simple_refusal(
                    403,
                    "class.level",
                    format!(
                        "model `{model_name}` needs L{}; the level is L{level}",
                        m.min_level
                    ),
                    true,
                ),
                close,
            )?;
            return Ok(true);
        }
        Some(m) => (
            m.input_musd_per_mtok,
            m.output_musd_per_mtok,
            m.reserve_output_tokens,
        ),
    };
    let max_out = ["max_tokens", "max_completion_tokens", "max_output_tokens"]
        .iter()
        .find_map(|k| req[*k].as_u64())
        .unwrap_or(reserve_out);
    let reservation =
        mul_price((body.len() as u64).div_ceil(3), in_price) + mul_price(max_out, out_price);
    let guard = h.header("x-egress-guard").map(str::to_string);
    let (epoch, n) = match root.reserve(reservation) {
        Ok(x) => x,
        Err((code, limit, spent, reserved)) => {
            let r = Refusal {
                status: 429,
                code,
                message: format!(
                    "root {} ceiling reached: {spent} spent + {reserved} reserved + {reservation} of {limit} micro-USD",
                    if code == "ceiling.day" {
                        "daily"
                    } else {
                        "total"
                    }
                ),
                limit,
                spent,
                reserved,
                retryable: code == "ceiling.day",
                demotes: guard.is_none(),
            };
            if guard.is_none() {
                root.append("ceiling_hit", json!({"ceiling": code.trim_start_matches("ceiling."), "code": code, "limit": limit,
                    "attempted": spent + reserved + reservation, "demotes": true, "rule": "c", "source": "egress"}));
            }
            refuse(root, w, &r, close)?;
            return Ok(true);
        }
    };
    let req_id = format!("e-{n:06}");
    let usage_file = root.r(&format!(
        "egress/usage/{}.jsonl",
        &util::rfc3339(root.now_ms())[..10]
    ));
    let _ = util::append_json(
        &usage_file,
        &json!({"ev": "start", "id": req_id, "lane": h.header("x-egress-scope"), "class": "providers",
        "provider": provider, "model": model_name, "reserved_musd": reservation, "guard": guard, "bypass": guard.is_none()}),
        true,
    );
    let up_path = format!("/{rest}");
    let mut up_head = format!("{} {up_path} HTTP/1.1\r\nhost: upstream\r\n", h.method);
    for (k, v) in &h.headers {
        if FORWARD.contains(&k.as_str()) {
            up_head.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    let auth = if provider == "anthropic" {
        format!("x-api-key: {}\r\n", super::FAKE_KEY)
    } else {
        format!("authorization: Bearer {}\r\n", super::FAKE_KEY)
    };
    up_head.push_str(&auth);
    up_head.push_str(&format!("user-agent: init-fakeroot/{}\r\naccept-encoding: identity\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", super::VERSION, body.len()));
    let extra = [
        ("x-egress-req-id", req_id.clone()),
        ("x-egress-retries", "0".to_string()),
        ("x-egress-reserved-musd", reservation.to_string()),
        ("x-egress-epoch", epoch.to_string()),
    ];
    let mut up: Box<dyn ReadWrite> = match connect(root.cfg.upstream.as_ref()) {
        Ok(s) => s,
        Err(e) => {
            root.settle(reservation, 0);
            let _ = util::append_json(
                &usage_file,
                &json!({"ev": "end", "id": req_id, "cost_musd": 0, "finish": "error", "flags": ["upstream_connect"]}),
                false,
            );
            let b = json!({"error": {"type": "init_egress_refused", "code": "upstream.connect", "message": format!("upstream: {e}")}});
            let hd = [
                ("content-type", "application/json".to_string()),
                ("x-init-egress-error", "upstream.connect".to_string()),
            ];
            write_simple(
                w,
                502,
                &hd,
                &serde_json::to_vec(&b).unwrap_or_default(),
                close,
            )?;
            return Ok(true);
        }
    };
    up.write_all(up_head.as_bytes())?;
    up.write_all(&body)?;
    up.flush()?;
    let mut ur = BufReader::new(up);
    // Status line and headers.
    let mut status_line = String::new();
    let (status, uh) = loop {
        status_line.clear();
        if ur.read_line(&mut status_line)? == 0 {
            root.settle(reservation, reservation);
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "upstream closed before the status line",
            ));
        }
        let status: u16 = status_line
            .split(' ')
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(502);
        let mut uh = Vec::new();
        loop {
            let mut l = String::new();
            if ur.read_line(&mut l)? == 0 {
                break;
            }
            let l = l.trim_end_matches(['\r', '\n']);
            if l.is_empty() {
                break;
            }
            if let Some((k, v)) = l.split_once(':') {
                uh.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
            }
        }
        if !(100..200).contains(&status) {
            break (status, uh);
        }
    };
    let get = |n: &str| uh.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    let chunked_up =
        get("transfer-encoding").is_some_and(|t| t.to_ascii_lowercase().contains("chunked"));
    let clen: Option<usize> = get("content-length").and_then(|c| c.parse().ok());
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    for (k, v) in &uh {
        if !HOP.contains(&k.as_str()) {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    for (k, v) in &extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    let mut sse = SseParser::new();
    let mut usage: Option<(u64, u64)> = None;
    let mut finish: Option<String> = None;
    let mut tap = |bytes: &[u8], usage: &mut Option<(u64, u64)>, finish: &mut Option<String>| {
        for ev in sse.feed(bytes).unwrap_or_default() {
            if let Ok(v) = serde_json::from_str::<Value>(&ev.data) {
                if let Some(u) = usage_of(&v) {
                    *usage = Some(usage.map_or(u, |(i, o)| (i.max(u.0), o.max(u.1))));
                }
                if let Some(f) = v["choices"][0]["finish_reason"]
                    .as_str()
                    .or(v["delta"]["stop_reason"].as_str())
                {
                    *finish = Some(f.to_string());
                }
            }
        }
    };
    let mut complete = true;
    if let Some(n) = clen.filter(|_| !chunked_up) {
        let mut b = Vec::with_capacity(n);
        ur.by_ref().take(n as u64).read_to_end(&mut b)?;
        complete = b.len() == n;
        if let Ok(v) = serde_json::from_slice::<Value>(&b) {
            usage = usage_of(&v);
            finish = v["choices"][0]["finish_reason"]
                .as_str()
                .map(str::to_string);
        }
        head.push_str(&format!(
            "content-length: {}\r\n{}",
            b.len(),
            if close {
                "connection: close\r\n\r\n"
            } else {
                "connection: keep-alive\r\n\r\n"
            }
        ));
        w.write_all(head.as_bytes())?;
        w.write_all(&b)?;
        w.flush()?;
    } else {
        head.push_str(&format!(
            "transfer-encoding: chunked\r\n{}",
            if close {
                "connection: close\r\n\r\n"
            } else {
                "connection: keep-alive\r\n\r\n"
            }
        ));
        w.write_all(head.as_bytes())?;
        w.flush()?;
        let mut dec = ChunkedDecoder::new();
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            if chunked_up && dec.is_done() {
                break;
            }
            let n = match ur.read(&mut buf) {
                Ok(0) => {
                    complete = !chunked_up; // close-delimited bodies end at EOF
                    break;
                }
                Ok(n) => n,
                Err(_) => {
                    complete = false;
                    break;
                }
            };
            let data = if chunked_up {
                dec.feed(&buf[..n]).unwrap_or_default()
            } else {
                buf[..n].to_vec()
            };
            if !data.is_empty() {
                tap(&data, &mut usage, &mut finish);
                w.write_all(format!("{:x}\r\n", data.len()).as_bytes())?;
                w.write_all(&data)?;
                w.write_all(b"\r\n")?;
                w.flush()?;
            }
        }
        if complete {
            w.write_all(b"0\r\n\r\n")?;
            w.flush()?;
        }
    }
    let (cost, flags) = match usage {
        Some((i, o)) => (mul_price(i, in_price) + mul_price(o, out_price), vec![]),
        None => (reservation, vec!["usage_estimated"]),
    };
    root.settle(reservation, cost);
    let _ = util::append_json(
        &usage_file,
        &json!({"ev": "end", "id": req_id, "cost_musd": cost, "usage": usage.map(|(i, o)| json!({"input": i, "output": o})),
        "finish": finish, "flags": flags, "status": status, "complete": complete}),
        false,
    );
    if !complete {
        // Propagate a cut: close without the terminal chunk.
        let _ = w.shutdown(std::net::Shutdown::Both);
        return Ok(false);
    }
    Ok(true)
}

trait ReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> ReadWrite for T {}

fn connect(up: Option<&Upstream>) -> io::Result<Box<dyn ReadWrite>> {
    match up {
        Some(Upstream::Tcp(a)) => {
            let s = TcpStream::connect(a)?;
            s.set_nodelay(true)?;
            s.set_read_timeout(Some(Duration::from_secs(600)))?;
            Ok(Box::new(s))
        }
        Some(Upstream::Unix(p)) => Ok(Box::new(UnixStream::connect(p)?)),
        None => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no upstream configured (--upstream)",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_shapes() {
        assert_eq!(
            usage_of(&json!({"usage": {"prompt_tokens": 10, "completion_tokens": 3}})),
            Some((10, 3))
        );
        assert_eq!(
            usage_of(&json!({"message": {"usage": {"input_tokens": 7, "output_tokens": 1}}})),
            Some((7, 1))
        );
        assert_eq!(
            usage_of(&json!({"response": {"usage": {"input_tokens": 2, "output_tokens": 9}}})),
            Some((2, 9))
        );
        assert_eq!(usage_of(&json!({"choices": []})), None);
        assert_eq!(mul_price(1, 300_000), 1);
        assert_eq!(mul_price(1_000_000, 300_000), 300_000);
    }
}
