//! EV-TRACE: the kernel's reader of `trace:` refs (evolution-v2 §3.12, §3.4.7 rule 2) over the
//! proxy's trace store `vault/traces/v1/` (kernel/proxy.md §3.13, `reconstruct`).
//!
//! - [`view`]: one request as the engine sees it: agent visibility, session, task, job, status,
//!   and the tool results it carried (the tool messages after its last assistant message, with
//!   the name and arguments of the matching tool call).
//! - [`failure`]: the failure-bearing predicate: a carried tool result that starts with `error:`
//!   or ends with a non-zero exit trailer, an owner abort, or a 429 refusal.
//! - [`lookup`]: the validator's `TraceLookup` (kernel-gate `evo::repo`): a `trace:` ref exists
//!   iff its request is stored with `vis:"agent"`.
//!
//! Only `vis:"agent"` records are ever read for the engine: holdout and canary runs are
//! `vis:"kernel"` and stay invisible (§3.4.1).

use crate::classify;
use serde_json::Value;
use std::io;
use std::path::{Path, PathBuf};

/// A tool result carried by a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carried {
    pub call_id: String,
    pub name: String,
    pub args: Value,
    pub text: String,
}

/// One request of the agent-visible trace.
#[derive(Clone, Debug)]
pub struct View {
    pub req_id: String,
    pub ts: String,
    pub session: Option<String>,
    pub task: Option<String>,
    pub job_id: Option<String>,
    pub scope: String,
    pub purpose: Option<String>,
    pub prefix_hash: Option<String>,
    /// From the `end` record (`None`: no end yet).
    pub status: Option<u64>,
    pub cache_permille: Option<u64>,
    pub carried: Vec<Carried>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string).filter(|x| x != "_none")
}

fn is_req_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The text of a chat message's `content` (a string, or an array of `{type:"text", text}` parts).
fn content_text(c: &Value) -> String {
    match c {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// The tool results a request carried: the trailing `tool` messages (chat completions) or
/// `tool_result` blocks of the trailing user message (Anthropic messages), joined to the calls of
/// the assistant message before them.
pub fn carried(elems: &[Value]) -> Vec<Carried> {
    let mut out = Vec::new();
    // OpenAI chat: trailing role:"tool" messages.
    let mut i = elems.len();
    while i > 0 && elems[i - 1].get("role").and_then(Value::as_str) == Some("tool") {
        i -= 1;
    }
    if i < elems.len() {
        let mut calls = std::collections::HashMap::new();
        if i > 0 {
            for c in elems[i - 1].get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                let id = c.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                let f = c.get("function").cloned().unwrap_or(Value::Null);
                let name = f.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let args = f.get("arguments").and_then(Value::as_str).and_then(|a| serde_json::from_str(a).ok()).unwrap_or(Value::Null);
                calls.insert(id, (name, args));
            }
        }
        for m in &elems[i..] {
            let id = m.get("tool_call_id").and_then(Value::as_str).unwrap_or("").to_string();
            let (name, args) = calls.get(&id).cloned().unwrap_or_default();
            out.push(Carried { call_id: id, name, args, text: content_text(m.get("content").unwrap_or(&Value::Null)) });
        }
        return out;
    }
    // Anthropic messages: the last user message's tool_result blocks.
    if let Some(last) = elems.last()
        && last.get("role").and_then(Value::as_str) == Some("user")
        && let Some(blocks) = last.get("content").and_then(Value::as_array)
    {
        let mut calls = std::collections::HashMap::new();
        if elems.len() >= 2 {
            for b in elems[elems.len() - 2].get("content").and_then(Value::as_array).into_iter().flatten() {
                if b.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let id = b.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                    calls.insert(id, (b.get("name").and_then(Value::as_str).unwrap_or("").to_string(), b.get("input").cloned().unwrap_or(Value::Null)));
                }
            }
        }
        for b in blocks {
            if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                let id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or("").to_string();
                let (name, args) = calls.get(&id).cloned().unwrap_or_default();
                let mut text = content_text(b.get("content").unwrap_or(&Value::Null));
                if b.get("is_error") == Some(&Value::Bool(true)) && !text.starts_with("error:") {
                    text = format!("error: tool_error: {text}");
                }
                out.push(Carried { call_id: id, name, args, text });
            }
        }
    }
    out
}

/// The view of `req_id`, or `Ok(None)` when the store has no such request or it is not
/// agent-visible. `root` is `vault/traces/v1`.
pub fn view(root: &Path, req_id: &str) -> io::Result<Option<View>> {
    if !is_req_id(req_id) {
        return Ok(None);
    }
    let (start, end) = match kernel_proxy::trace::records(root, req_id) {
        Ok(x) => x,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if start.get("vis").and_then(Value::as_str) != Some("agent") {
        return Ok(None);
    }
    let elems = match kernel_proxy::trace::reconstruct(root, req_id) {
        Ok((_, elems)) => elems.iter().filter_map(|b| serde_json::from_slice::<Value>(b).ok()).collect::<Vec<_>>(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => vec![],
        Err(e) => return Err(e),
    };
    Ok(Some(from_records(req_id, &start, end.as_ref(), &elems)))
}

/// A [`View`] from the records and the parsed message-list elements.
pub fn from_records(req_id: &str, start: &Value, end: Option<&Value>, elems: &[Value]) -> View {
    View {
        req_id: req_id.to_string(),
        ts: s(start, "ts").unwrap_or_default(),
        session: s(start, "session"),
        task: s(start, "task"),
        job_id: s(start, "job_id"),
        scope: s(start, "scope").unwrap_or_default(),
        purpose: s(start, "purpose"),
        prefix_hash: s(start, "prefix_hash"),
        status: end.and_then(|e| e.get("status")).and_then(Value::as_u64),
        cache_permille: end.and_then(|e| e.get("cache_permille")).and_then(Value::as_u64),
        carried: carried(elems),
    }
}

/// Why a request is failure-bearing, or `None` (evolution-v2 §3.4.7 rule 2).
pub fn failure(v: &View) -> Option<String> {
    for c in &v.carried {
        if c.text.trim_start().starts_with(classify::ABORTED_TEXT) {
            return Some(format!("owner abort of {}", c.name));
        }
        if classify::text_is_failure(&c.text) {
            let first = c.text.trim_start().lines().next().unwrap_or("");
            let why = if first.starts_with("error:") { first.to_string() } else { format!("exit {}", classify::exit_from_text(&c.text).unwrap_or(-1)) };
            return Some(classify::cap(&format!("{}: {why}", if c.name.is_empty() { "tool" } else { &c.name }), 200));
        }
    }
    if v.status == Some(429) {
        return Some("429 refusal".into());
    }
    None
}

/// The validator's `trace:` lookup: exists iff stored and agent-visible.
pub fn lookup(root: PathBuf) -> impl Fn(&str) -> io::Result<bool> + Send + Sync + 'static {
    move |id: &str| {
        if !is_req_id(id) {
            return Ok(false);
        }
        match kernel_proxy::trace::records(&root, id) {
            Ok((start, _)) => Ok(start.get("vis").and_then(Value::as_str) == Some("agent")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn carried_tool_results_and_failure() {
        let elems = vec![
            json!({"role": "system", "content": "s"}),
            json!({"role": "user", "content": "go"}),
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "bash", "arguments": "{\"command\":\"cd work/a\"}"}}]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "bash: cd: work/a: No such file or directory\n[exit_code=1 wall_time=0.01s]"}),
        ];
        let v = from_records("0123456789abcdef", &json!({"vis": "agent", "session": "s1", "task": "_none"}), Some(&json!({"status": 200})), &elems);
        assert_eq!(v.carried.len(), 1);
        assert_eq!(v.carried[0].name, "bash");
        assert_eq!(v.carried[0].args["command"], "cd work/a");
        assert_eq!(v.task, None);
        assert!(failure(&v).unwrap().starts_with("bash: exit 1"));
        // A clean request: the last message is the owner's.
        let clean = from_records("0123456789abcdef", &json!({"vis": "agent"}), None, &elems[..2]);
        assert!(failure(&clean).is_none());
        let ok = vec![elems[2].clone(), json!({"role": "tool", "tool_call_id": "c1", "content": "fine\n[exit_code=0 wall_time=0.01s]"})];
        assert!(failure(&from_records("0123456789abcdef", &json!({}), None, &ok)).is_none());
    }
}
