//! `init-mockllm`: the deterministic $0 model (testing.md §3.4; script format and rendering rules:
//! body/provider.md §3.12).
//!
//! Two ways to use it:
//! - **In process** (unit tests): build a [`Mock`] and call [`Mock::respond`] with a [`MockRequest`];
//!   no socket is involved.
//! - **As a server**: [`http::MockServer::start`] on a loopback TCP port or a Unix socket, in
//!   `upstream` mode (the provider's own API, behind the kernel proxy or the (fake) Root) or `socket`
//!   mode (the kernel proxy's API, `POST /<provider_id>/<path>`, for body-only tests). The
//!   `init-mockllm` binary is a thin CLI over this.

pub mod assemble;
pub mod cache;
pub mod canon;
pub mod client;
pub mod http;
pub mod render;
pub mod responders;
pub mod script;
pub mod sse;
pub mod strict;
pub mod wire;

pub use render::{Part, Usage};
pub use script::{Action, Call, Expect, Fault, Meta, Raw, Reply, Script, Stop, Turn, UsageSpec};

use render::RenderCx;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use wire::{Family, ReqView};

/// Which API the mock speaks (testing.md §3.4.1). The listener (TCP or Unix) is chosen separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The provider's own API (`POST /chat/completions`, `/v1/messages`, `/v1/responses`), reached
    /// through the kernel proxy and the Root egress, which strip every `x-init-*` header.
    Upstream,
    /// The kernel proxy's API: `POST /<provider_id>/<path>` plus `GET /_init/stats` and `/_init/health`.
    Socket,
}

/// Provider ids accepted as the first path segment in socket mode.
pub const SOCKET_PROVIDERS: [&str; 4] = ["deepseek", "anthropic", "openai", "mock"];

/// The task script for task sessions without their own `<id>.mock.jsonl` (in the tasks dir).
pub const DEFAULT_TASK_SCRIPT: &str = "_default.mock.jsonl";

/// Mock configuration (the CLI flags of testing.md §3.4.1).
#[derive(Clone, Debug)]
pub struct MockConfig {
    pub mode: Mode,
    pub script: Script,
    /// Per-task scripts `<tasks_dir>/<id>.mock.jsonl` for G4 task sessions, and `_default.mock.jsonl` for the rest.
    pub tasks_dir: Option<PathBuf>,
    /// Where `mock.log.jsonl` and the request bodies go; `None` keeps the log in memory only.
    pub record_dir: Option<PathBuf>,
    /// `--record-bodies all` (true) or `none`.
    pub record_bodies: bool,
    /// `--strict deepseek`.
    pub strict_deepseek: bool,
    /// `--simulate-cache`.
    pub simulate_cache: bool,
    /// `--cache-unit-tokens` (default 64).
    pub cache_unit_tokens: u64,
    /// Disabled by `--no-probe-responder`.
    pub probe_responder: bool,
    /// Disabled by `--no-g3-responder`.
    pub g3_responder: bool,
}

impl MockConfig {
    /// Defaults: responders on, recording of bodies on (when a record dir is set), no strict mode,
    /// no cache simulation.
    pub fn new(mode: Mode, script: Script) -> MockConfig {
        MockConfig {
            mode,
            script,
            tasks_dir: None,
            record_dir: None,
            record_bodies: true,
            strict_deepseek: false,
            simulate_cache: false,
            cache_unit_tokens: cache::DEFAULT_UNIT_TOKENS,
            probe_responder: true,
            g3_responder: true,
        }
    }
}

/// An HTTP request as the mock sees it. Header names are lowercase.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MockRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl MockRequest {
    /// A POST with the given headers (names are lowercased).
    pub fn post(path: &str, headers: &[(&str, &str)], body: impl Into<Vec<u8>>) -> MockRequest {
        MockRequest {
            method: "POST".into(),
            path: path.into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
                .collect(),
            body: body.into(),
        }
    }
    /// A socket-mode DeepSeek request (`POST /deepseek/chat/completions`) for `session` and `purpose`.
    pub fn deepseek(session: &str, purpose: &str, body: &Value) -> MockRequest {
        MockRequest::post(
            "/deepseek/chat/completions",
            &[("x-init-session", session), ("x-init-purpose", purpose)],
            body.to_string(),
        )
    }
    /// A GET.
    pub fn get(path: &str) -> MockRequest {
        MockRequest {
            method: "GET".into(),
            path: path.into(),
            ..MockRequest::default()
        }
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A response body: complete (content-length) or a stream of SSE events (chunked).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Full(Vec<u8>),
    Stream(Vec<Part>),
}

/// A response as the mock produces it, before HTTP framing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

impl MockResponse {
    fn json(status: u16, v: &Value) -> MockResponse {
        MockResponse {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: Body::Full(canon::to_string(v).into_bytes()),
        }
    }
    fn sse(parts: Vec<Part>) -> MockResponse {
        MockResponse {
            status: 200,
            headers: vec![
                (
                    "content-type".into(),
                    "text/event-stream; charset=utf-8".into(),
                ),
                ("cache-control".into(), "no-cache".into()),
            ],
            body: Body::Stream(parts),
        }
    }
    /// The body bytes a client receives (stalls ignored, nothing after a cut).
    pub fn body_bytes(&self) -> Vec<u8> {
        match &self.body {
            Body::Full(b) => b.clone(),
            Body::Stream(parts) => {
                let mut out = Vec::new();
                for p in parts {
                    match p {
                        Part::Event(e) => out.extend_from_slice(e),
                        Part::Stall(_) => {}
                        Part::Cut => break,
                    }
                }
                out
            }
        }
    }
    /// The body as UTF-8 text (lossy).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body_bytes()).into_owned()
    }
    /// The body parsed as JSON, if it is JSON.
    pub fn json_body(&self) -> Option<Value> {
        serde_json::from_slice(&self.body_bytes()).ok()
    }
    /// Whether the stream ends in a cut.
    pub fn is_cut(&self) -> bool {
        matches!(&self.body, Body::Stream(p) if p.last() == Some(&Part::Cut))
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// One `mock.log.jsonl` line: provider.md's `{seq, purpose, expect_ok, prefix_sha256, fault}` plus
/// testing.md §3.4.5's `protocol_error`, `responder`, `cache_read`, `t_recv_ms`, `t_done_ms`, plus
/// `session`, `path`, `status`, `exhausted` and `mock_error` for assertions and diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub seq: u64,
    pub session: String,
    pub purpose: String,
    pub path: String,
    /// `script`, `probe`, `g3`, `task:<id>`, `give_up`; null when rejected before routing.
    pub responder: Option<String>,
    pub status: u16,
    /// null when the turn had no `expect`.
    pub expect_ok: Option<bool>,
    /// Hex SHA-256 of the prefix bytes.
    pub prefix_sha256: Option<String>,
    /// The fault kind served, if any.
    pub fault: Option<String>,
    pub protocol_error: Option<String>,
    pub exhausted: bool,
    /// The simulated cache read (only with `--simulate-cache`).
    pub cache_read: Option<u64>,
    pub mock_error: Option<String>,
    pub t_recv_ms: u64,
    pub t_done_ms: u64,
}

impl LogLine {
    /// The canonical JSON line (sorted keys).
    pub fn to_json(&self) -> String {
        match serde_json::to_value(self) {
            Ok(v) => canon::to_string(&v),
            Err(_) => "{}".into(),
        }
    }
}

/// Totals over a log, for scenario assertions (testing.md §3.4.5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub requests: usize,
    pub expect_failures: usize,
    pub protocol_errors: usize,
    pub exhausted: bool,
}

/// Summarize a log.
pub fn summarize(lines: &[LogLine]) -> Summary {
    Summary {
        requests: lines.len(),
        expect_failures: lines.iter().filter(|l| l.expect_ok == Some(false)).count(),
        protocol_errors: lines.iter().filter(|l| l.protocol_error.is_some()).count(),
        exhausted: lines.iter().any(|l| l.exhausted),
    }
}

/// `prefix_constant`: every scripted `turn` request of `session` had the same prefix hash.
pub fn prefix_constant(lines: &[LogLine], session: &str) -> bool {
    let mut hashes = lines
        .iter()
        .filter(|l| {
            l.session == session && l.purpose == "turn" && l.responder.as_deref() == Some("script")
        })
        .map(|l| l.prefix_sha256.as_deref());
    match hashes.next() {
        None => true,
        Some(first) => hashes.all(|h| h == first),
    }
}

/// Read a `mock.log.jsonl` file.
pub fn read_log(path: &Path) -> io::Result<Vec<LogLine>> {
    let text = std::fs::read_to_string(path)?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)))
        .collect()
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in d.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[derive(Debug, Default)]
struct Cursor {
    pos: usize,
    last_prefix: Option<String>,
}

#[derive(Debug, Default)]
struct Stats {
    requests: u64,
    last_permille: Option<u64>,
    input_total: u64,
    cache_total: u64,
}

#[derive(Debug)]
struct State {
    seq: u64,
    cursors: BTreeMap<(String, String), Cursor>,
    task_cursors: BTreeMap<(String, String, String), Cursor>,
    task_scripts: BTreeMap<String, Result<Option<Arc<Script>>, String>>,
    cache: cache::CacheSim,
    stats: Stats,
}

/// A log line waiting for its response to finish (`t_done_ms`).
#[derive(Debug)]
pub struct Pending {
    line: LogLine,
}

/// The mock model.
#[derive(Debug)]
pub struct Mock {
    cfg: MockConfig,
    start: Instant,
    state: Mutex<State>,
    log: Mutex<(Vec<LogLine>, Option<File>)>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn sanitize(s: &str) -> String {
    let t: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    if t.is_empty() { "other".into() } else { t }
}

fn mock_error(msg: &str) -> MockResponse {
    MockResponse::json(500, &json!({"mock_error": msg}))
}

/// The budget refusal of kernel/proxy.md §4.2, shaped per wire family.
fn budget_response(family: Family) -> MockResponse {
    let init = json!({"cap": "day", "limit_musd": 5_000_000, "reserved_musd": 0, "resets_at": "1970-01-02T00:00:00Z",
                      "retryable": true, "scope": "live", "spent_musd": 5_000_000, "task": null});
    let msg = "daily budget reached (mock): spent 5000000 of 5000000 micro-USD";
    let body = match family {
        Family::Messages => {
            json!({"error": {"message": msg, "type": "init_cap_exceeded"}, "init": init, "type": "error"})
        }
        _ => {
            json!({"error": {"code": "budget.day", "message": msg, "param": null, "type": "init_cap_exceeded"}, "init": init})
        }
    };
    let mut r = MockResponse::json(429, &body);
    r.headers.push(("retry-after".into(), "3600".into()));
    r.headers.push(("x-init-error".into(), "budget.day".into()));
    r.headers.push(("x-init-cap".into(), "day".into()));
    r
}

fn check_expect(
    e: &Expect,
    view: &ReqView,
    prefix: &str,
    last_prefix: Option<&str>,
) -> Result<(), String> {
    if let Some(want) = &e.last_role {
        let got = view.last().map(|m| m.role.as_str()).unwrap_or("none");
        if got != want {
            return Err(format!("last_role: want {want}, got {got}"));
        }
    }
    if let Some(t) = &e.last_text_contains {
        let got = view.last().map(|m| m.text.as_str()).unwrap_or("");
        if !got.contains(t.as_str()) {
            return Err(format!("last_text_contains: {t:?} not in the last message"));
        }
    }
    if let Some(want) = &e.tools
        && &view.tool_names != want
    {
        return Err(format!("tools: want {want:?}, got {:?}", view.tool_names));
    }
    if let Some(p) = &e.prefix {
        if p == "same" {
            if let Some(lp) = last_prefix
                && lp != prefix
            {
                return Err(format!("prefix: changed from {lp} to {prefix}"));
            }
        } else if p != prefix {
            return Err(format!("prefix: want {p}, got {prefix}"));
        }
    }
    Ok(())
}

enum TurnOutcome {
    /// The action, and whether the turn carried an `expect` (which passed).
    Action(Box<Action>, bool),
    Exhausted,
    ExpectFailed(String),
}

fn take_turn(
    script: &Script,
    cur: &mut Cursor,
    purpose: &str,
    view: &ReqView,
    prefix: &str,
) -> TurnOutcome {
    let Some(turn) = script.turn(purpose, cur.pos) else {
        return TurnOutcome::Exhausted;
    };
    if let Some(e) = &turn.expect
        && let Err(msg) = check_expect(e, view, prefix, cur.last_prefix.as_deref())
    {
        // Not consumed: the transport's retries meet the same failure (provider.md §3.12).
        return TurnOutcome::ExpectFailed(format!("expect failed (line {}): {msg}", turn.line));
    }
    cur.pos += 1;
    cur.last_prefix = Some(prefix.to_string());
    TurnOutcome::Action(Box::new(turn.action.clone()), turn.expect.is_some())
}

impl Mock {
    /// Create a mock. With a record dir, creates it and starts a fresh `mock.log.jsonl`.
    pub fn new(cfg: MockConfig) -> io::Result<Arc<Mock>> {
        let file = match &cfg.record_dir {
            Some(d) => {
                std::fs::create_dir_all(d)?;
                Some(File::create(d.join("mock.log.jsonl"))?)
            }
            None => None,
        };
        let unit = cfg.cache_unit_tokens;
        Ok(Arc::new(Mock {
            cfg,
            start: Instant::now(),
            state: Mutex::new(State {
                seq: 0,
                cursors: BTreeMap::new(),
                task_cursors: BTreeMap::new(),
                task_scripts: BTreeMap::new(),
                cache: cache::CacheSim::new(unit),
                stats: Stats::default(),
            }),
            log: Mutex::new((Vec::new(), file)),
        }))
    }

    pub fn config(&self) -> &MockConfig {
        &self.cfg
    }

    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Handle one request completely (in-process use): the response and its log line.
    pub fn respond(&self, req: &MockRequest) -> MockResponse {
        let (resp, pending) = self.begin(req);
        if let Some(p) = pending {
            self.finish(p);
        }
        resp
    }

    /// Every log line so far, in completion order.
    pub fn log(&self) -> Vec<LogLine> {
        lock(&self.log).0.clone()
    }

    /// Totals over the log.
    pub fn summary(&self) -> Summary {
        summarize(&lock(&self.log).0)
    }

    /// Record the end of a response (`t_done_ms`) and append its log line.
    pub fn finish(&self, mut p: Pending) {
        p.line.t_done_ms = self.now_ms();
        let mut g = lock(&self.log);
        if let Some(f) = g.1.as_mut() {
            let mut s = p.line.to_json();
            s.push('\n');
            // A log write failure must not take the mock down; the in-memory log still has the line.
            let _ = f.write_all(s.as_bytes());
        }
        g.0.push(p.line);
    }

    fn stats_json(&self) -> Value {
        let st = lock(&self.state);
        let today =
            (st.stats.input_total > 0).then(|| st.stats.cache_total * 1000 / st.stats.input_total);
        json!({
            "cache": {"last_permille": st.stats.last_permille, "session_permille": null,
                      "today_permille": today, "window_1h_permille": null},
            "last_head_hash": null,
            "requests_today": st.stats.requests,
            "spend": {"cap_today_musd": null, "today_musd": 0},
            "ts": "1970-01-01T00:00:00.000Z",
            "v": 1
        })
    }

    /// Route and render a request. The returned `Pending` (absent for `GET /_init/*` and 404s) must
    /// be passed to [`Mock::finish`] once the response has been written.
    pub fn begin(&self, req: &MockRequest) -> (MockResponse, Option<Pending>) {
        let t_recv_ms = self.now_ms();
        let path = req.path.split('?').next().unwrap_or("").to_string();
        if req.method == "GET" {
            if self.cfg.mode == Mode::Socket {
                match path.as_str() {
                    "/_init/stats" => return (MockResponse::json(200, &self.stats_json()), None),
                    "/_init/health" => {
                        let h = json!({"mock": true, "ok": true, "providers": {"deepseek": {"enabled": true, "forwarded": true}},
                                       "test_mode": true, "v": 1});
                        return (MockResponse::json(200, &h), None);
                    }
                    _ => {}
                }
            }
            return (not_found(&path), None);
        }
        if req.method != "POST" {
            return (
                MockResponse::json(
                    405,
                    &json!({"error": {"message": "method not allowed", "type": "invalid_request_error"}}),
                ),
                None,
            );
        }
        let api_path = match self.cfg.mode {
            Mode::Upstream => Some(path.as_str()),
            Mode::Socket => {
                let rest = path.strip_prefix('/').unwrap_or("");
                match rest.split_once('/') {
                    Some((provider, api)) if SOCKET_PROVIDERS.contains(&provider) => Some(api),
                    _ => None,
                }
            }
        };
        let Some(family) = api_path.and_then(Family::from_api_path) else {
            return (not_found(&path), None);
        };

        let session = req.header("x-init-session").unwrap_or("_none").to_string();
        let header_purpose = req.header("x-init-purpose").map(sanitize);
        let mut st = lock(&self.state);
        st.seq += 1;
        st.stats.requests += 1;
        let mut line = LogLine {
            seq: st.seq,
            session: session.clone(),
            path: path.clone(),
            t_recv_ms,
            ..LogLine::default()
        };

        let view = match wire::parse(family, &req.body) {
            Ok(v) => v,
            Err(e) => {
                line.purpose = header_purpose.unwrap_or_else(|| "turn".into());
                self.record_body(&line, &req.body);
                line.protocol_error = Some(e.clone());
                line.status = 400;
                return (
                    MockResponse::json(400, &strict::error_body(&e)),
                    Some(Pending { line }),
                );
            }
        };
        let purpose = header_purpose.unwrap_or_else(|| {
            if responders::looks_like_probe(&view.msgs) {
                "probe".into()
            } else {
                "turn".into()
            }
        });
        line.purpose = purpose.clone();
        self.record_body(&line, &req.body);
        let prefix = sha256_hex(&view.prefix_bytes);
        line.prefix_sha256 = Some(prefix.clone());

        if self.cfg.strict_deepseek
            && let Some(err) = strict::check(self.cfg.mode, &req.headers, &view)
        {
            line.protocol_error = Some(err.clone());
            line.status = 400;
            return (
                MockResponse::json(400, &strict::error_body(&err)),
                Some(Pending { line }),
            );
        }

        // Routing (testing.md §3.4.4): probe, then G3, then G4 task sessions, then the script.
        let meta_chunk = self.cfg.script.meta.chunk;
        let (responder, outcome, chunk) = if purpose == "probe" && self.cfg.probe_responder {
            (
                "probe".to_string(),
                TurnOutcome::Action(
                    Box::new(Action::Reply(responders::probe_reply(&view.msgs))),
                    false,
                ),
                meta_chunk,
            )
        } else if let Some(g3) = self
            .cfg
            .g3_responder
            .then(|| responders::detect_g3(&view.msgs))
            .flatten()
        {
            (
                "g3".to_string(),
                TurnOutcome::Action(
                    Box::new(Action::Reply(responders::g3_reply(&view.msgs, &g3))),
                    false,
                ),
                meta_chunk,
            )
        } else if let Some((task, at)) = responders::detect_task(&view.msgs) {
            match self.task_script(&mut st, &task) {
                Ok(Some(script)) => {
                    let key = match req.header("x-init-session") {
                        Some(s) => s.to_string(),
                        None => format!(
                            "content:{}",
                            &sha256_hex(view.msgs[at].text.as_bytes())[..16]
                        ),
                    };
                    let cur = st
                        .task_cursors
                        .entry((task.clone(), key, purpose.clone()))
                        .or_default();
                    let out = take_turn(&script, cur, &purpose, &view, &prefix);
                    (format!("task:{task}"), out, script.meta.chunk)
                }
                Ok(None) => (
                    "give_up".to_string(),
                    TurnOutcome::Action(
                        Box::new(Action::Reply(responders::give_up_reply())),
                        false,
                    ),
                    meta_chunk,
                ),
                Err(e) => (
                    format!("task:{task}"),
                    TurnOutcome::ExpectFailed(format!("task script {task}: {e}")),
                    meta_chunk,
                ),
            }
        } else {
            let cur = st
                .cursors
                .entry((session.clone(), purpose.clone()))
                .or_default();
            (
                "script".to_string(),
                take_turn(&self.cfg.script, cur, &purpose, &view, &prefix),
                meta_chunk,
            )
        };
        line.responder = Some(responder.clone());

        let action = match outcome {
            TurnOutcome::Action(a, had_expect) => {
                if had_expect {
                    line.expect_ok = Some(true);
                }
                *a
            }
            TurnOutcome::Exhausted => {
                line.exhausted = true;
                line.status = 500;
                line.mock_error = Some("script exhausted".into());
                return (mock_error("script exhausted"), Some(Pending { line }));
            }
            TurnOutcome::ExpectFailed(msg) => {
                line.expect_ok = Some(false);
                line.status = 500;
                line.mock_error = Some(msg.clone());
                return (mock_error(&msg), Some(Pending { line }));
            }
        };

        let model = view
            .model
            .clone()
            .unwrap_or_else(|| self.cfg.script.meta.model.clone());
        let label = match responder.split(':').next() {
            Some("script") | None => purpose.clone(),
            Some(l) => l.to_string(),
        };
        let cx = RenderCx {
            family,
            model: &model,
            label: &label,
            n: view.assistant_count(),
            chunk,
            thinking_disabled: view.thinking_disabled,
        };
        let resp = match action {
            Action::Reply(reply) => {
                let (events, hit) = self.render_events(&mut st, &cx, &view, &reply);
                line.cache_read = hit;
                MockResponse::sse(events.into_iter().map(Part::Event).collect())
            }
            Action::Raw(raw) => MockResponse {
                status: raw.status,
                headers: raw.headers,
                body: Body::Full(raw.body),
            },
            Action::Fault { fault, reply } => {
                line.fault = Some(fault.kind().to_string());
                match fault {
                    Fault::Http {
                        status,
                        headers,
                        body,
                    } => {
                        let (bytes, ct) = match &body {
                            Value::String(s) => {
                                (s.clone().into_bytes(), "text/plain; charset=utf-8")
                            }
                            v => (canon::to_string(v).into_bytes(), "application/json"),
                        };
                        let mut h = headers;
                        if !h.iter().any(|(k, _)| k == "content-type") {
                            h.insert(0, ("content-type".into(), ct.into()));
                        }
                        MockResponse {
                            status,
                            headers: h,
                            body: Body::Full(bytes),
                        }
                    }
                    Fault::Budget => budget_response(family),
                    stream_fault => {
                        let reply = reply.unwrap_or_else(render::filler_reply);
                        let (events, hit) = self.render_events(&mut st, &cx, &view, &reply);
                        line.cache_read = hit;
                        MockResponse::sse(render::apply_stream_fault(&cx, events, &stream_fault))
                    }
                }
            }
        };
        line.status = resp.status;
        (resp, Some(Pending { line }))
    }

    /// Render a reply's events, running the cache simulation and updating the stats.
    fn render_events(
        &self,
        st: &mut State,
        cx: &RenderCx,
        view: &ReqView,
        reply: &Reply,
    ) -> (Vec<Vec<u8>>, Option<u64>) {
        let simulated = self
            .cfg
            .simulate_cache
            .then(|| st.cache.observe(cx.model, &view.prompt_bytes));
        let think = render::effective_think(cx, reply);
        let usage = render::compute_usage(&reply.usage, view.body_len, simulated, &think, reply);
        st.stats.last_permille = (usage.input > 0).then(|| usage.cache_read * 1000 / usage.input);
        st.stats.input_total += usage.input;
        st.stats.cache_total += usage.cache_read;
        (render::render(cx, reply, &usage), simulated)
    }

    fn task_script(&self, st: &mut State, task: &str) -> Result<Option<Arc<Script>>, String> {
        if let Some(r) = st.task_scripts.get(task) {
            return r.clone();
        }
        let r = match &self.cfg.tasks_dir {
            None => Ok(None),
            Some(dir) => {
                // `_default.mock.jsonl`, when present, serves task sessions that have no script of their own
                // (REAL-M7: the Root draws instances with random ids, so the dry run's health canary needs it)
                let p = dir.join(format!("{task}.mock.jsonl"));
                let d = dir.join(DEFAULT_TASK_SCRIPT);
                if p.exists() {
                    Script::load(&p).map(|s| Some(Arc::new(s)))
                } else if d.exists() {
                    Script::load(&d).map(|s| Some(Arc::new(s)))
                } else {
                    Ok(None)
                }
            }
        };
        st.task_scripts.insert(task.to_string(), r.clone());
        r
    }

    fn record_body(&self, line: &LogLine, body: &[u8]) {
        if !self.cfg.record_bodies {
            return;
        }
        if let Some(d) = &self.cfg.record_dir {
            let _ = std::fs::write(
                d.join(format!("{:06}-{}.json", line.seq, line.purpose)),
                body,
            );
        }
    }
}

fn not_found(path: &str) -> MockResponse {
    MockResponse::json(
        404,
        &json!({"error": {"message": format!("no route for {path}"), "type": "not_found_error"}}),
    )
}

#[cfg(test)]
mod tests;
