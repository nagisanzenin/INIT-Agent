//! The mock script format `*.mock.jsonl` (body/provider.md §3.12, owned there; parsed here).
//!
//! One JSON object per line, `"v":1`. The first line may be `meta`; every other line is a `turn`
//! with exactly one of `reply`, `fault` (optionally with a `reply` whose events a stream fault
//! cuts, stalls or errors) or `raw`.

use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The `meta` line. Defaults apply when a script has no `meta` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    /// `deepseek` | `anthropic` | `openai` (informational: the request path picks the wire format).
    pub provider: String,
    /// Fallback model name when a request names none.
    pub model: String,
    /// Script name (diagnostics).
    pub name: String,
    /// Delta size: characters for text and reasoning, bytes (on char boundaries) for arguments.
    pub chunk: usize,
}

impl Default for Meta {
    fn default() -> Self {
        Meta {
            provider: "deepseek".into(),
            model: "mock-frontier-1".into(),
            name: String::new(),
            chunk: 16,
        }
    }
}

/// How a reply ends (provider.md §3.12 `stop`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    EndTurn,
    ToolUse,
    MaxTokens,
    Refusal,
}

impl Stop {
    pub fn parse(s: &str) -> Option<Stop> {
        Some(match s {
            "end_turn" => Stop::EndTurn,
            "tool_use" => Stop::ToolUse,
            "max_tokens" => Stop::MaxTokens,
            "refusal" => Stop::Refusal,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Stop::EndTurn => "end_turn",
            Stop::ToolUse => "tool_use",
            Stop::MaxTokens => "max_tokens",
            Stop::Refusal => "refusal",
        }
    }
}

/// One tool call in a reply. `input` is rendered with sorted keys.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub name: String,
    pub input: Value,
}

/// Explicit usage numbers; omitted ones are computed (provider.md §3.12).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsageSpec {
    pub input: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub output: Option<u64>,
    pub reasoning: Option<u64>,
}

/// A scripted (or responder-made) model reply.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// `None`: absent (DeepSeek in thinking mode then gets a default reasoning text, see render.rs).
    /// `Some("")`: explicitly no reasoning.
    pub think: Option<String>,
    pub text: String,
    pub calls: Vec<Call>,
    pub stop: Stop,
    pub usage: UsageSpec,
}

impl Reply {
    pub fn text(text: &str) -> Reply {
        Reply {
            think: None,
            text: text.into(),
            calls: vec![],
            stop: Stop::EndTurn,
            usage: UsageSpec::default(),
        }
    }
    pub fn call(name: &str, input: Value) -> Reply {
        Reply {
            think: None,
            text: String::new(),
            calls: vec![Call {
                name: name.into(),
                input,
            }],
            stop: Stop::ToolUse,
            usage: UsageSpec::default(),
        }
    }
}

/// A scripted failure (provider.md §3.12 `fault`).
#[derive(Clone, Debug, PartialEq)]
pub enum Fault {
    Http {
        status: u16,
        headers: Vec<(String, String)>,
        body: Value,
    },
    Cut {
        after_events: usize,
    },
    Stall {
        after_events: usize,
        ms: u64,
    },
    StreamError {
        after_events: usize,
        error: String,
    },
    Budget,
}

impl Fault {
    pub fn kind(&self) -> &'static str {
        match self {
            Fault::Http { .. } => "http",
            Fault::Cut { .. } => "cut",
            Fault::Stall { .. } => "stall",
            Fault::StreamError { .. } => "stream_error",
            Fault::Budget => "budget",
        }
    }
}

/// A byte-exact response (`raw`), loaded when the script is parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct Raw {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What a turn answers with.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Reply(Reply),
    /// A stream fault renders `reply` (or a filler reply) and then cuts, stalls or errors.
    Fault {
        fault: Fault,
        reply: Option<Reply>,
    },
    Raw(Raw),
}

/// Request assertions (provider.md §3.12 `expect`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expect {
    pub last_role: Option<String>,
    pub last_text_contains: Option<String>,
    pub tools: Option<Vec<String>>,
    /// `"same"` or a hex SHA-256 (an optional `sha256:` prefix is accepted).
    pub prefix: Option<String>,
}

/// One `turn` line.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub purpose: String,
    pub expect: Option<Expect>,
    pub action: Action,
    /// 1-based line number in the script file (diagnostics).
    pub line: usize,
}

/// A parsed script: turns grouped by purpose, in file order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Script {
    pub meta: Meta,
    pub turns: Vec<Turn>,
    by_purpose: BTreeMap<String, Vec<usize>>,
}

impl Script {
    /// An empty script: every scripted request is answered `script exhausted`.
    pub fn empty() -> Script {
        Script::default()
    }

    /// Load a script file; `raw.body_file` resolves against the script's directory.
    pub fn load(path: &Path) -> Result<Script, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Script::parse(&text, &dir).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Parse script text. `base` is where `raw.body_file` paths resolve.
    pub fn parse(text: &str, base: &Path) -> Result<Script, String> {
        let mut s = Script::default();
        for (i, line) in text.lines().enumerate() {
            let lineno = i + 1;
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            let v: Value =
                serde_json::from_str(t).map_err(|e| format!("line {lineno}: invalid JSON: {e}"))?;
            let o = v
                .as_object()
                .ok_or_else(|| format!("line {lineno}: not an object"))?;
            match o.get("v") {
                Some(Value::Number(n)) if n.as_u64() == Some(1) => {}
                _ => return Err(format!("line {lineno}: \"v\" must be 1")),
            }
            match o.get("kind").and_then(Value::as_str) {
                Some("meta") => {
                    if !s.turns.is_empty() {
                        return Err(format!("line {lineno}: meta must come before every turn"));
                    }
                    s.meta = parse_meta(o).map_err(|e| format!("line {lineno}: {e}"))?;
                }
                Some("turn") => {
                    let turn =
                        parse_turn(o, base, lineno).map_err(|e| format!("line {lineno}: {e}"))?;
                    s.turns.push(turn);
                }
                other => return Err(format!("line {lineno}: unknown kind {other:?}")),
            }
        }
        for (i, t) in s.turns.iter().enumerate() {
            s.by_purpose.entry(t.purpose.clone()).or_default().push(i);
        }
        Ok(s)
    }

    /// The `pos`-th turn of `purpose`, if any.
    pub fn turn(&self, purpose: &str, pos: usize) -> Option<&Turn> {
        self.by_purpose
            .get(purpose)
            .and_then(|v| v.get(pos))
            .map(|&i| &self.turns[i])
    }
}

fn parse_meta(o: &Map<String, Value>) -> Result<Meta, String> {
    let mut m = Meta::default();
    if let Some(p) = o.get("provider") {
        let p = p.as_str().ok_or("meta.provider must be a string")?;
        if !matches!(p, "deepseek" | "anthropic" | "openai") {
            return Err(format!(
                "meta.provider {p:?} is not deepseek, anthropic or openai"
            ));
        }
        m.provider = p.into();
    }
    if let Some(x) = o.get("model") {
        m.model = x.as_str().ok_or("meta.model must be a string")?.into();
    }
    if let Some(x) = o.get("name") {
        m.name = x.as_str().ok_or("meta.name must be a string")?.into();
    }
    if let Some(x) = o.get("chunk") {
        let c = x.as_u64().ok_or("meta.chunk must be a positive integer")?;
        if c == 0 || c > 1 << 20 {
            return Err("meta.chunk must be in 1..=1048576".into());
        }
        m.chunk = c as usize;
    }
    Ok(m)
}

fn parse_turn(o: &Map<String, Value>, base: &Path, line: usize) -> Result<Turn, String> {
    let purpose = match o.get("purpose") {
        None => "turn".to_string(),
        Some(p) => p.as_str().ok_or("purpose must be a string")?.to_string(),
    };
    let expect = o.get("expect").map(parse_expect).transpose()?;
    let reply = o.get("reply").map(parse_reply).transpose()?;
    let action = match (o.get("fault"), o.get("raw"), reply) {
        (Some(f), None, reply) => {
            let fault = parse_fault(f)?;
            if reply.is_some() && matches!(fault, Fault::Http { .. } | Fault::Budget) {
                return Err("an http or budget fault cannot have a reply".into());
            }
            Action::Fault { fault, reply }
        }
        (None, Some(r), None) => Action::Raw(parse_raw(r, base)?),
        (None, None, Some(r)) => Action::Reply(r),
        (None, None, None) => return Err("turn needs one of reply, fault or raw".into()),
        _ => return Err("turn has more than one of reply, fault and raw".into()),
    };
    Ok(Turn {
        purpose,
        expect,
        action,
        line,
    })
}

fn parse_expect(v: &Value) -> Result<Expect, String> {
    let o = v.as_object().ok_or("expect must be an object")?;
    let mut e = Expect::default();
    for (k, x) in o {
        match k.as_str() {
            "last_role" => {
                let r = x.as_str().ok_or("expect.last_role must be a string")?;
                if !matches!(r, "user" | "tool" | "assistant") {
                    return Err(format!(
                        "expect.last_role {r:?} is not user, tool or assistant"
                    ));
                }
                e.last_role = Some(r.into());
            }
            "last_text_contains" => {
                e.last_text_contains = Some(
                    x.as_str()
                        .ok_or("expect.last_text_contains must be a string")?
                        .into(),
                )
            }
            "tools" => {
                let a = x.as_array().ok_or("expect.tools must be an array")?;
                let names = a
                    .iter()
                    .map(|n| {
                        n.as_str()
                            .map(String::from)
                            .ok_or("expect.tools must hold strings")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                e.tools = Some(names);
            }
            "prefix" => {
                let p = x.as_str().ok_or("expect.prefix must be a string")?;
                let hex = p.strip_prefix("sha256:").unwrap_or(p);
                if p != "same" && !(hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())) {
                    return Err("expect.prefix must be \"same\" or a hex SHA-256".into());
                }
                e.prefix = Some(if p == "same" {
                    p.into()
                } else {
                    hex.to_ascii_lowercase()
                });
            }
            other => return Err(format!("unknown expect field {other:?}")),
        }
    }
    Ok(e)
}

fn parse_reply(v: &Value) -> Result<Reply, String> {
    let o = v.as_object().ok_or("reply must be an object")?;
    let mut think = None;
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut stop = None;
    let mut usage = UsageSpec::default();
    for (k, x) in o {
        match k.as_str() {
            "think" => {
                think = Some(
                    x.as_str()
                        .ok_or("reply.think must be a string")?
                        .to_string(),
                )
            }
            "text" => text = x.as_str().ok_or("reply.text must be a string")?.to_string(),
            "calls" => {
                for c in x.as_array().ok_or("reply.calls must be an array")? {
                    let co = c.as_object().ok_or("each call must be an object")?;
                    let name = co
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or("call.name must be a string")?;
                    let input = co
                        .get("input")
                        .cloned()
                        .unwrap_or_else(|| Value::Object(Map::new()));
                    calls.push(Call {
                        name: name.into(),
                        input,
                    });
                }
            }
            "stop" => {
                let s = x.as_str().ok_or("reply.stop must be a string")?;
                stop = Some(Stop::parse(s).ok_or_else(|| format!("unknown reply.stop {s:?}"))?);
            }
            "usage" => usage = parse_usage(x)?,
            other => return Err(format!("unknown reply field {other:?}")),
        }
    }
    let stop = stop.unwrap_or(if calls.is_empty() {
        Stop::EndTurn
    } else {
        Stop::ToolUse
    });
    Ok(Reply {
        think,
        text,
        calls,
        stop,
        usage,
    })
}

fn parse_usage(v: &Value) -> Result<UsageSpec, String> {
    let o = v.as_object().ok_or("reply.usage must be an object")?;
    let mut u = UsageSpec::default();
    for (k, x) in o {
        let n = Some(
            x.as_u64()
                .ok_or_else(|| format!("usage.{k} must be a non-negative integer"))?,
        );
        match k.as_str() {
            "input" => u.input = n,
            "cache_read" => u.cache_read = n,
            "cache_write" => u.cache_write = n,
            "output" => u.output = n,
            "reasoning" => u.reasoning = n,
            other => return Err(format!("unknown usage field {other:?}")),
        }
    }
    Ok(u)
}

fn headers_of(v: Option<&Value>) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    if let Some(h) = v {
        for (k, x) in h.as_object().ok_or("headers must be an object")? {
            let val = match x {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => return Err(format!("header {k} must be a string or number")),
            };
            out.push((k.to_ascii_lowercase(), val));
        }
    }
    Ok(out)
}

fn status_of(v: Option<&Value>, default: u16) -> Result<u16, String> {
    match v {
        None => Ok(default),
        Some(x) => {
            let s = x.as_u64().ok_or("status must be an integer")?;
            if !(100..=599).contains(&s) {
                return Err(format!("status {s} out of range"));
            }
            Ok(s as u16)
        }
    }
}

fn usize_field(o: &Map<String, Value>, k: &str) -> Result<usize, String> {
    match o.get(k) {
        None => Ok(0),
        Some(x) => x
            .as_u64()
            .map(|n| n as usize)
            .ok_or_else(|| format!("fault.{k} must be an integer")),
    }
}

fn parse_fault(v: &Value) -> Result<Fault, String> {
    let o = v.as_object().ok_or("fault must be an object")?;
    match o.get("kind").and_then(Value::as_str) {
        Some("http") => Ok(Fault::Http {
            status: status_of(o.get("status"), 500)?,
            headers: headers_of(o.get("headers"))?,
            body: o.get("body").cloned().unwrap_or(Value::Null),
        }),
        Some("cut") => Ok(Fault::Cut {
            after_events: usize_field(o, "after_events")?,
        }),
        Some("stall") => Ok(Fault::Stall {
            after_events: usize_field(o, "after_events")?,
            ms: o
                .get("ms")
                .and_then(Value::as_u64)
                .ok_or("fault.ms must be an integer")?,
        }),
        Some("stream_error") => Ok(Fault::StreamError {
            after_events: usize_field(o, "after_events")?,
            error: o
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("overloaded_error")
                .to_string(),
        }),
        Some("budget") => Ok(Fault::Budget),
        other => Err(format!("unknown fault kind {other:?}")),
    }
}

fn parse_raw(v: &Value, base: &Path) -> Result<Raw, String> {
    let o = v.as_object().ok_or("raw must be an object")?;
    let status = status_of(o.get("status"), 200)?;
    let headers = headers_of(o.get("headers"))?;
    let body = match (o.get("body_file"), o.get("body")) {
        (Some(f), None) => {
            let rel = f.as_str().ok_or("raw.body_file must be a string")?;
            let p = base.join(rel);
            let p = if p.exists() { p } else { PathBuf::from(rel) };
            std::fs::read(&p).map_err(|e| format!("raw.body_file {}: {e}", p.display()))?
        }
        (None, Some(Value::String(s))) => s.clone().into_bytes(),
        (None, Some(_)) => return Err("raw.body must be a string".into()),
        (None, None) => return Err("raw needs body_file or body".into()),
        (Some(_), Some(_)) => return Err("raw has both body_file and body".into()),
    };
    Ok(Raw {
        status,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC_EXAMPLE: &str = r#"{"v":1,"kind":"meta","provider":"deepseek","model":"deepseek-flash","name":"a4-probe-then-fix","chunk":16}
{"v":1,"kind":"turn","purpose":"probe","reply":{"calls":[{"input":{"command":"true"},"name":"bash"}],"stop":"tool_use","usage":{"cache_read":11776,"input":12034,"output":31}}}
{"v":1,"kind":"turn","purpose":"probe","expect":{"last_role":"tool"},"reply":{"stop":"end_turn","text":"ready"}}
{"v":1,"kind":"turn","expect":{"last_role":"user","last_text_contains":"Fix"},"reply":{"think":"Run the tests.","text":"","calls":[{"input":{"command":"cargo test"},"name":"bash"}],"stop":"tool_use"}}
{"v":1,"kind":"turn","fault":{"kind":"http","status":429,"headers":{"retry-after":"1"},"body":{"error":{"message":"rate limited","type":"rate_limit_error"}}}}
{"v":1,"kind":"turn","expect":{"last_role":"tool","prefix":"same"},"reply":{"text":"Done.","stop":"end_turn"}}
{"v":1,"kind":"turn","raw":{"status":200,"headers":{"content-type":"text/event-stream"},"body":"data: [DONE]\n\n"}}
"#;

    #[test]
    fn parses_the_provider_doc_example() {
        let s = Script::parse(DOC_EXAMPLE, Path::new(".")).unwrap();
        assert_eq!(s.meta.model, "deepseek-flash");
        assert_eq!(s.meta.chunk, 16);
        assert_eq!(s.turns.len(), 6);
        assert_eq!(s.turn("probe", 1).unwrap().line, 3);
        assert_eq!(s.turn("turn", 0).unwrap().line, 4);
        assert_eq!(s.turn("turn", 3).unwrap().line, 7);
        assert!(s.turn("turn", 4).is_none());
        match &s.turn("probe", 0).unwrap().action {
            Action::Reply(r) => {
                assert_eq!(r.usage.cache_read, Some(11776));
                assert_eq!(r.calls[0].input["command"], "true");
            }
            a => panic!("{a:?}"),
        }
        match &s.turn("turn", 1).unwrap().action {
            Action::Fault {
                fault: Fault::Http {
                    status, headers, ..
                },
                reply: None,
            } => {
                assert_eq!(*status, 429);
                assert_eq!(headers[0], ("retry-after".into(), "1".into()));
            }
            a => panic!("{a:?}"),
        }
    }

    #[test]
    fn rejects_bad_lines() {
        for bad in [
            r#"{"v":2,"kind":"turn","reply":{}}"#,
            r#"{"v":1,"kind":"nope"}"#,
            r#"{"v":1,"kind":"turn"}"#,
            r#"{"v":1,"kind":"turn","reply":{"stop":"later"}}"#,
            r#"{"v":1,"kind":"turn","reply":{},"raw":{"body":""}}"#,
            r#"{"v":1,"kind":"turn","expect":{"prefix":"abc"},"reply":{}}"#,
            r#"{"v":1,"kind":"turn","fault":{"kind":"budget"},"reply":{}}"#,
            "not json",
        ] {
            assert!(Script::parse(bad, Path::new(".")).is_err(), "{bad}");
        }
    }

    #[test]
    fn default_stop_follows_calls() {
        let s = Script::parse(
            r#"{"v":1,"kind":"turn","reply":{"calls":[{"name":"bash","input":{}}]}}
{"v":1,"kind":"turn","reply":{"text":"hi"}}"#,
            Path::new("."),
        )
        .unwrap();
        let stops: Vec<Stop> = s
            .turns
            .iter()
            .map(|t| match &t.action {
                Action::Reply(r) => r.stop,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(stops, vec![Stop::ToolUse, Stop::EndTurn]);
    }
}
