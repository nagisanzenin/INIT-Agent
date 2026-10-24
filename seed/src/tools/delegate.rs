//! The `delegate` tool: run one bounded sub-task in a fresh context and return only its summary.
//!
//! The sub-agent shares this body's head (persona, rules, tool schemas) but not its transcript:
//! the message list it sees starts at the task text and grows only with its own tool calls. The
//! main context therefore pays for the summary, not for the sub-task's tool output.
//!
//! The loop owns the model calls (`loop/delegate.rs`); this module is the pure part: argument
//! parsing, the bounds, the opening message and the result text, so all of it is testable without
//! a model.
//!
//! Based on the `delegate` tool INIT wrote for itself (evo/delegate 4258c2e, amended as 48b6de1);
//! the seed adds `mode`, per-call and configured bounds, parallel fan-out and nested display.
//!
//! @init compensates-for: context-pollution

use std::time::Duration;

use serde_json::{Map, Value};

use super::ToolError;

/// The registered name. Its schema lives in `registry.json`; this module parses the same fields.
pub const NAME: &str = "delegate";

/// The summary is what the main context pays for: the whole tool result (the sub-agent's text
/// plus the cost line) is at most 8 KiB.
pub const SUMMARY_BYTES: usize = 8 << 10;

/// What a sub-task may change. `Read` (default): `write` and `edit` are refused inside it. Its
/// `bash` and `ext` calls run with the body's own rights: Seatbelt cannot nest (kernel/sandbox.md
/// §3.11), so the body cannot put a sub-task's processes under a stricter profile, and read mode
/// does not stop a shell command from writing. `Write`: every tool; writers run one at a time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Access {
    #[default]
    Read,
    Write,
}

impl Access {
    pub fn as_str(self) -> &'static str {
        match self {
            Access::Read => "read",
            Access::Write => "write",
        }
    }
}

/// Per-sub-task bounds, in units the loop enforces itself (money is observe-only, egress.md):
/// model turns, tool calls, model requests (attempts, retries included), tokens (input + output,
/// summed over the sub-task's requests) and wall time. `parallel` is the batch-wide cap on
/// concurrent sub-tasks. Configured by `INIT_DELEGATE_*` (see [`Bounds::from_lookup`]); a call
/// may lower `max_turns`, `max_calls` and `max_seconds`, never raise them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub max_turns: usize,
    pub max_calls: usize,
    pub max_requests: usize,
    pub max_tokens: u64,
    pub max_wall: Duration,
    pub parallel: usize,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds {
            max_turns: 8,
            max_calls: 24,
            max_requests: 12,
            max_tokens: 1_000_000,
            max_wall: Duration::from_secs(600),
            parallel: 4,
        }
    }
}

/// The configured values are clamped to these ceilings (and to at least 1).
pub const CEIL_TURNS: usize = 64;
pub const CEIL_CALLS: usize = 256;
pub const CEIL_REQUESTS: usize = 128;
pub const CEIL_TOKENS: u64 = 20_000_000;
pub const CEIL_SECONDS: u64 = 3_600;
/// At most this many sub-tasks run at once (the batch has 16 workers, loop.md §3.7).
pub const CEIL_PARALLEL: usize = 8;

impl Bounds {
    /// `INIT_DELEGATE_MAX_TURNS`, `_MAX_CALLS`, `_MAX_REQUESTS`, `_MAX_TOKENS`, `_MAX_SECONDS`,
    /// `_PARALLEL`: a missing or unparsable value keeps the default; each is clamped to
    /// `1..=CEIL_*`.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Bounds {
        let d = Bounds::default();
        let num = |k: &str, def: u64, ceil: u64| -> u64 {
            lookup(k)
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map_or(def, |v| v.clamp(1, ceil))
        };
        Bounds {
            max_turns: num("INIT_DELEGATE_MAX_TURNS", d.max_turns as u64, CEIL_TURNS as u64)
                as usize,
            max_calls: num("INIT_DELEGATE_MAX_CALLS", d.max_calls as u64, CEIL_CALLS as u64)
                as usize,
            max_requests: num(
                "INIT_DELEGATE_MAX_REQUESTS",
                d.max_requests as u64,
                CEIL_REQUESTS as u64,
            ) as usize,
            max_tokens: num("INIT_DELEGATE_MAX_TOKENS", d.max_tokens, CEIL_TOKENS),
            max_wall: Duration::from_secs(num(
                "INIT_DELEGATE_MAX_SECONDS",
                d.max_wall.as_secs(),
                CEIL_SECONDS,
            )),
            parallel: num("INIT_DELEGATE_PARALLEL", d.parallel as u64, CEIL_PARALLEL as u64)
                as usize,
        }
    }

    /// This call's bounds: the configured ones, lowered by the call's own arguments.
    pub fn narrow(&self, a: &Args) -> Bounds {
        let mut b = *self;
        if let Some(t) = a.max_turns {
            b.max_turns = b.max_turns.min(t);
        }
        if let Some(c) = a.max_calls {
            b.max_calls = b.max_calls.min(c);
        }
        if let Some(s) = a.max_seconds {
            b.max_wall = b.max_wall.min(Duration::from_secs(s));
        }
        b
    }
}

/// Parsed `delegate` arguments (§3.1: unknown fields are rejected).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub task: String,
    pub context: Option<String>,
    pub mode: Access,
    pub max_turns: Option<usize>,
    pub max_calls: Option<usize>,
    pub max_seconds: Option<u64>,
}

const FIELDS: [&str; 6] = [
    "context",
    "max_calls",
    "max_seconds",
    "max_turns",
    "mode",
    "task",
];

fn positive(input: &Map<String, Value>, k: &str) -> Result<Option<u64>, ToolError> {
    match input.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if n >= 1 => Ok(Some(n)),
            _ => Err(ToolError::invalid(format!(
                "\"{k}\" must be a whole number of at least 1"
            ))),
        },
    }
}

pub fn parse(input: &Map<String, Value>) -> Result<Args, ToolError> {
    for k in input.keys() {
        if !FIELDS.contains(&k.as_str()) {
            return Err(ToolError::invalid(format!(
                "unknown field \"{k}\": delegate takes \"task\" and optional \"context\", \"mode\", \"max_turns\", \"max_calls\", \"max_seconds\""
            )));
        }
    }
    let task = match input.get("task") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
        Some(Value::String(_)) | None => {
            return Err(ToolError::invalid(
                "delegate needs \"task\": what the sub-agent should do and return",
            )
            .hint("give the sub-agent a self-contained task"));
        }
        Some(_) => return Err(ToolError::invalid("\"task\" must be a string")),
    };
    let context = match input.get("context") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.trim().is_empty() => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(ToolError::invalid("\"context\" must be a string")),
    };
    let mode = match input.get("mode") {
        None | Some(Value::Null) => Access::Read,
        Some(Value::String(s)) if s == "read" => Access::Read,
        Some(Value::String(s)) if s == "write" => Access::Write,
        Some(_) => {
            return Err(ToolError::invalid(
                "\"mode\" must be \"read\" (the default) or \"write\"",
            ));
        }
    };
    Ok(Args {
        task,
        context,
        mode,
        max_turns: positive(input, "max_turns")?.map(|n| n.min(CEIL_TURNS as u64) as usize),
        max_calls: positive(input, "max_calls")?.map(|n| n.min(CEIL_CALLS as u64) as usize),
        max_seconds: positive(input, "max_seconds")?.map(|n| n.min(CEIL_SECONDS)),
    })
}

/// The first message the sub-agent sees. It is the whole of its context, so it names the role,
/// the bound, the mode and the required shape of the answer.
pub fn opening(a: &Args, b: &Bounds) -> String {
    let mut s = format!(
        "You are a sub-agent working for the main agent, in a fresh context: you do not see its \
conversation. Do the sub-task below with the tools, then reply with only your final result as \
plain text — no preamble, no restating the task. The main agent reads your reply and nothing else, \
so include every fact it needs (paths, line numbers, names, numbers, errors) and say plainly what \
you could not do. You cannot call `delegate` yourself. Your budget: {} turns, {} tool calls, {} s.",
        b.max_turns,
        b.max_calls,
        b.max_wall.as_secs()
    );
    match a.mode {
        Access::Read => s.push_str(
            " This is a read-only sub-task: change no files. `write` and `edit` are refused, and \
do not use `bash` or `ext` to modify anything; report what should change instead.",
        ),
        Access::Write => s.push_str(
            " You may change files; no other writer runs while you do. Say exactly what you \
changed.",
        ),
    }
    s.push_str("\n\nSub-task:\n");
    s.push_str(a.task.trim());
    if let Some(c) = a.context.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        s.push_str("\n\nGiven context:\n");
        s.push_str(c);
    }
    s
}

/// The note before a sub-task's last allowed turn: answer now, with what it has.
pub fn last_turn_note(why: &str) -> String {
    format!(
        "[init:delegate] {why}: this is your last turn. Reply now with your final result as text; \
tool calls will not run."
    )
}

/// The text of a read-mode refusal (`write`/`edit` inside a read sub-task).
pub fn read_mode_refusal(tool: &str) -> String {
    format!(
        "error: not_available: {tool} is off in a read-mode sub-task (mode \"read\"). Report the \
change instead; the main agent can delegate with mode \"write\"."
    )
}

/// Cut `text` to at most `max` bytes on a character boundary, marking the cut.
pub fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    const MARK: &str = "\n[init:truncated] output cut at ";
    let room = max.saturating_sub(MARK.len() + 16);
    let mut end = room.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{MARK}{max} bytes", &text[..end])
}

/// What one sub-task cost, for its result line and its record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub turns: usize,
    pub calls: usize,
    pub requests: usize,
    pub in_tokens: u64,
    pub out_tokens: u64,
    pub wall_ms: u64,
}

/// The tool result: the sub-agent's final text, cut, then one line of what the sub-task cost and
/// where its full record is. The whole result is at most [`SUMMARY_BYTES`].
pub fn result_text(text: &str, t: &Tally, record: Option<&str>) -> String {
    let mut tail = format!(
        "\n\n[init:delegate] {} turn{}, {} tool call{}, {} request{}, in {} out {} tokens, {:.1} s",
        t.turns,
        plural(t.turns),
        t.calls,
        plural(t.calls),
        t.requests,
        plural(t.requests),
        t.in_tokens,
        t.out_tokens,
        t.wall_ms as f64 / 1000.0
    );
    if let Some(r) = record {
        tail.push_str(&format!("; record {}", cut(r, 512)));
    }
    let body = text.trim();
    let body = if body.is_empty() {
        "[init:empty] the sub-agent finished without a text result".to_string()
    } else {
        cut(body, SUMMARY_BYTES.saturating_sub(tail.len()))
    };
    format!("{body}{tail}")
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The one-line `tool_start`/`tool_end` text: the task, shortened.
pub fn summary_line(task: &str) -> String {
    let first = task.lines().next().unwrap_or("").trim();
    if first.len() <= 80 {
        return first.to_string();
    }
    let mut end = 80;
    while !first.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &first[..end])
}

/// The `x-init-subtask` label of a sub-task's requests (kernel/proxy.md §3.6 grammar
/// `[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}`): `delegate-<call id>`, other bytes as `_`, at most 64.
pub fn subtask_label(call_id: &str) -> String {
    let mut s = String::from("delegate-");
    s.extend(call_id.chars().map(|c| {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
            c
        } else {
            '_'
        }
    }));
    s.truncate(64);
    s
}

/// The display `call_id` of a call inside a sub-task: unique across the batch even when two
/// sub-agents' models reuse ids (`<delegate call id>/<inner id>`).
pub fn inner_call_id(parent: &str, inner: &str) -> String {
    format!("{parent}/{inner}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn parses_task_context_mode_and_bounds() {
        let a = parse(&obj(json!({"task": " count lines ", "context": "in src"}))).unwrap();
        assert_eq!(a.task, " count lines ");
        assert_eq!(a.context.as_deref(), Some("in src"));
        assert_eq!(a.mode, Access::Read, "read is the default");
        let b = parse(&obj(json!({"task": "go", "context": "  ", "mode": "write",
            "max_turns": 3, "max_calls": 5, "max_seconds": 9999}))).unwrap();
        assert_eq!(b.context, None);
        assert_eq!(b.mode, Access::Write);
        assert_eq!((b.max_turns, b.max_calls), (Some(3), Some(5)));
        assert_eq!(b.max_seconds, Some(CEIL_SECONDS), "clamped to the ceiling");
    }

    #[test]
    fn rejects_unknown_fields_bad_mode_and_missing_task() {
        assert!(parse(&obj(json!({"task": "x", "extra": 1}))).is_err());
        assert!(parse(&obj(json!({"context": "x"}))).is_err());
        assert!(parse(&obj(json!({"task": "   "}))).is_err());
        assert!(parse(&obj(json!({"task": 3}))).is_err());
        assert!(parse(&obj(json!({"task": "x", "mode": "admin"}))).is_err());
        assert!(parse(&obj(json!({"task": "x", "max_turns": 0}))).is_err());
        assert!(parse(&obj(json!({"task": "x", "max_calls": -1}))).is_err());
        assert!(parse(&obj(json!({"task": "x", "max_seconds": "5"}))).is_err());
    }

    #[test]
    fn bounds_from_config_and_narrowed_by_the_call() {
        let env = |k: &str| match k {
            "INIT_DELEGATE_MAX_TURNS" => Some("3".to_string()),
            "INIT_DELEGATE_MAX_CALLS" => Some("0".to_string()), // clamped up to 1
            "INIT_DELEGATE_MAX_TOKENS" => Some("junk".to_string()), // default kept
            "INIT_DELEGATE_PARALLEL" => Some("99".to_string()), // clamped down
            "INIT_DELEGATE_MAX_SECONDS" => Some("30".to_string()),
            _ => None,
        };
        let b = Bounds::from_lookup(env);
        assert_eq!(b.max_turns, 3);
        assert_eq!(b.max_calls, 1);
        assert_eq!(b.max_tokens, Bounds::default().max_tokens);
        assert_eq!(b.parallel, CEIL_PARALLEL);
        assert_eq!(b.max_wall, Duration::from_secs(30));
        let a = parse(&obj(json!({"task": "x", "max_turns": 2, "max_seconds": 60}))).unwrap();
        let n = Bounds::default().narrow(&a);
        assert_eq!(n.max_turns, 2);
        assert_eq!(n.max_wall, Duration::from_secs(60));
        // a call never raises a configured bound
        let a = parse(&obj(json!({"task": "x", "max_turns": 50}))).unwrap();
        assert_eq!(b.narrow(&a).max_turns, 3);
    }

    #[test]
    fn opening_names_the_bound_mode_and_carries_the_task() {
        let a = parse(&obj(json!({"task": "find the bug", "context": "in src/x.rs"}))).unwrap();
        let s = opening(&a, &Bounds::default());
        assert!(s.contains("You are a sub-agent"));
        assert!(s.contains("cannot call `delegate`"));
        assert!(s.contains("8 turns, 24 tool calls, 600 s"));
        assert!(s.contains("read-only sub-task"));
        assert!(s.ends_with("find the bug\n\nGiven context:\nin src/x.rs"));
        let w = parse(&obj(json!({"task": "fix it", "mode": "write"}))).unwrap();
        assert!(opening(&w, &Bounds::default()).contains("You may change files"));
    }

    #[test]
    fn cut_respects_char_boundaries_and_the_bound() {
        let s = "é".repeat(100);
        let c = cut(&s, 60);
        assert!(c.starts_with("éé"));
        assert!(c.ends_with("60 bytes"));
        assert!(c.len() <= 60, "{}", c.len());
        assert_eq!(cut("short", 100), "short");
    }

    #[test]
    fn result_text_reports_cost_record_and_stays_under_8_kib() {
        let t = Tally { turns: 3, calls: 2, requests: 3, in_tokens: 10, out_tokens: 5, wall_ms: 1250 };
        let r = result_text("answer", &t, Some("/h/state/spill/s/delegate-c1.json"));
        assert!(r.starts_with("answer\n\n[init:delegate] 3 turns, 2 tool calls, 3 requests, in 10 out 5 tokens, 1.2 s"), "{r}");
        assert!(r.ends_with("; record /h/state/spill/s/delegate-c1.json"));
        assert!(result_text("  ", &t, None).starts_with("[init:empty]"));
        let big = "x".repeat(100_000);
        let r = result_text(&big, &t, Some(&"p".repeat(2000)));
        assert!(r.len() <= SUMMARY_BYTES, "{}", r.len());
        assert!(r.contains("[init:truncated]"));
    }

    #[test]
    fn labels_follow_the_proxy_grammar() {
        assert_eq!(subtask_label("call_00_t1"), "delegate-call_00_t1");
        assert_eq!(subtask_label("toolu:01/x y"), "delegate-toolu_01_x_y");
        assert_eq!(subtask_label(&"a".repeat(200)).len(), 64);
        assert_eq!(inner_call_id("d1", "c2"), "d1/c2");
        assert_eq!(summary_line("first line\nsecond"), "first line");
        assert!(summary_line(&"é".repeat(100)).ends_with('…'));
    }
}
