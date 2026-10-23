//! `--strict deepseek` protocol checks (testing.md §3.4.2). A violation is answered with HTTP 400
//! and an OpenAI-style error body, and recorded as `protocol_error`.

use super::Mode;
use super::wire::{Family, ReqView, Role};
use serde_json::Value;
use std::collections::BTreeSet;

/// Headers that would carry credentials; the body never holds any (spec §3).
const CREDENTIAL_HEADERS: [&str; 3] = ["authorization", "x-api-key", "cookie"];

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// The first violation, if any. Header checks apply to every family; body checks to Chat.
pub fn check(mode: Mode, headers: &[(String, String)], view: &ReqView) -> Option<String> {
    match mode {
        Mode::Socket => {
            for h in CREDENTIAL_HEADERS {
                if header(headers, h).is_some() {
                    return Some(format!(
                        "credential header {h} reached the mock (socket mode)"
                    ));
                }
            }
            for h in ["x-init-session", "x-init-purpose"] {
                if header(headers, h).is_none() {
                    return Some(format!("{h} header missing (socket mode)"));
                }
            }
        }
        Mode::Upstream => {
            if let Some((k, _)) = headers.iter().find(|(k, _)| k.starts_with("x-init-")) {
                return Some(format!(
                    "{k} header reached the upstream (the proxy must strip x-init-*)"
                ));
            }
        }
    }
    if view.family != Family::Chat {
        return None;
    }
    let v = &view.value;
    if v.get("stream") != Some(&Value::Bool(true)) {
        return Some("stream must be true".into());
    }
    if !v.get("model").is_some_and(Value::is_string) {
        return Some("model missing".into());
    }
    if !v.get("messages").is_some_and(Value::is_array) {
        return Some("messages missing".into());
    }
    let tools_present = v
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty());
    if tools_present && !view.thinking_disabled {
        for (i, m) in view.msgs.iter().enumerate() {
            if m.role == Role::Assistant && !m.has_reasoning {
                return Some(format!(
                    "reasoning_content missing in assistant message {i}"
                ));
            }
        }
    }
    // Tool-result pairing: each tool message answers a call of the assistant message right before
    // its group, and every call is answered before anything else follows.
    let mut pending: Option<(usize, BTreeSet<String>)> = None;
    for (i, m) in view.msgs.iter().enumerate() {
        if m.role == Role::Tool {
            let id = m.tool_call_id.clone().unwrap_or_default();
            let answered = pending.as_mut().is_some_and(|(_, ids)| ids.remove(&id));
            if !answered {
                return Some(format!(
                    "tool message {i} answers unknown tool_call_id {id:?}"
                ));
            }
            continue;
        }
        if let Some((ai, ids)) = pending.take()
            && let Some(id) = ids.into_iter().next()
        {
            return Some(format!(
                "tool call {id} of assistant message {ai} is not answered"
            ));
        }
        if m.role == Role::Assistant && !m.call_ids.is_empty() {
            pending = Some((i, m.call_ids.iter().cloned().collect()));
        }
    }
    if let Some((ai, ids)) = pending
        && let Some(id) = ids.into_iter().next()
    {
        return Some(format!(
            "tool call {id} of assistant message {ai} is not answered"
        ));
    }
    None
}

/// The 400 body for a violation.
pub fn error_body(message: &str) -> Value {
    serde_json::json!({"error": {"message": message, "type": "invalid_request_error"}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mockllm::wire::parse;
    use serde_json::json;

    fn hdrs(socket: bool) -> Vec<(String, String)> {
        if socket {
            vec![
                ("x-init-session".into(), "s".into()),
                ("x-init-purpose".into(), "turn".into()),
            ]
        } else {
            vec![]
        }
    }

    fn view(v: Value) -> ReqView {
        parse(Family::Chat, v.to_string().as_bytes()).unwrap()
    }

    fn base(messages: Value) -> Value {
        json!({"model": "deepseek-flash", "stream": true, "messages": messages,
               "tools": [{"type": "function", "function": {"name": "bash"}}]})
    }

    #[test]
    fn missing_reasoning_is_rejected_with_tools() {
        let v = view(base(json!([
            {"role": "system", "content": "S"},
            {"role": "user", "content": "u"},
            {"role": "assistant", "content": "hi"},
            {"role": "user", "content": "again"}
        ])));
        assert_eq!(
            check(Mode::Socket, &hdrs(true), &v).as_deref(),
            Some("reasoning_content missing in assistant message 2")
        );
    }

    #[test]
    fn missing_reasoning_is_fine_without_tools_or_with_thinking_disabled() {
        let msgs = json!([{"role": "user", "content": "u"}, {"role": "assistant", "content": "hi"}, {"role": "user", "content": "x"}]);
        let mut no_tools = base(msgs.clone());
        no_tools.as_object_mut().unwrap().remove("tools");
        assert_eq!(check(Mode::Socket, &hdrs(true), &view(no_tools)), None);
        let mut disabled = base(msgs);
        disabled["thinking"] = json!({"type": "disabled"});
        assert_eq!(check(Mode::Socket, &hdrs(true), &view(disabled)), None);
    }

    #[test]
    fn pairing_rules() {
        let call = |id: &str| {
            json!({"role": "assistant", "content": "", "reasoning_content": "r",
            "tool_calls": [{"id": id, "type": "function", "function": {"name": "bash", "arguments": "{}"}}]})
        };
        let ok = view(base(json!([{"role": "user", "content": "u"}, call("a"),
            {"role": "tool", "tool_call_id": "a", "content": "x"}, {"role": "user", "content": "steer"}])));
        assert_eq!(check(Mode::Socket, &hdrs(true), &ok), None);
        let wrong = view(base(json!([{"role": "user", "content": "u"}, call("a"),
            {"role": "tool", "tool_call_id": "b", "content": "x"}])));
        assert!(
            check(Mode::Socket, &hdrs(true), &wrong)
                .unwrap()
                .contains("unknown tool_call_id")
        );
        let unanswered = view(base(
            json!([{"role": "user", "content": "u"}, call("a"), {"role": "user", "content": "x"}]),
        ));
        assert!(
            check(Mode::Socket, &hdrs(true), &unanswered)
                .unwrap()
                .contains("not answered")
        );
        let dangling = view(base(json!([{"role": "user", "content": "u"}, call("a")])));
        assert!(
            check(Mode::Socket, &hdrs(true), &dangling)
                .unwrap()
                .contains("not answered")
        );
        let dup = view(base(json!([{"role": "user", "content": "u"}, call("a"),
            {"role": "tool", "tool_call_id": "a", "content": "x"}, {"role": "tool", "tool_call_id": "a", "content": "x"}])));
        assert!(check(Mode::Socket, &hdrs(true), &dup).is_some());
    }

    #[test]
    fn header_rules() {
        let v = view(base(json!([{"role": "user", "content": "u"}])));
        assert_eq!(check(Mode::Socket, &hdrs(true), &v), None);
        assert!(
            check(Mode::Socket, &hdrs(false), &v)
                .unwrap()
                .contains("x-init-session")
        );
        let mut with_auth = hdrs(true);
        with_auth.push(("authorization".into(), "Bearer x".into()));
        assert!(
            check(Mode::Socket, &with_auth, &v)
                .unwrap()
                .contains("authorization")
        );
        assert_eq!(check(Mode::Upstream, &[], &v), None);
        assert!(
            check(Mode::Upstream, &hdrs(true), &v)
                .unwrap()
                .contains("x-init-session")
        );
    }

    #[test]
    fn stream_and_model_required() {
        let mut v = base(json!([{"role": "user", "content": "u"}]));
        v["stream"] = json!(false);
        assert_eq!(
            check(Mode::Upstream, &[], &view(v)).as_deref(),
            Some("stream must be true")
        );
        let mut v = base(json!([{"role": "user", "content": "u"}]));
        v.as_object_mut().unwrap().remove("model");
        assert_eq!(
            check(Mode::Upstream, &[], &view(v)).as_deref(),
            Some("model missing")
        );
    }
}
