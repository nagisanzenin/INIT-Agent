//! Request analysis: which wire family a request uses, a normalized view of its messages, and the
//! raw bytes the prefix hash and the cache simulation are computed over.

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

/// The wire family, chosen by the request path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// DeepSeek / OpenAI Chat Completions (`…/chat/completions`).
    Chat,
    /// Anthropic Messages (`…/messages`).
    Messages,
    /// OpenAI Responses (`…/responses`).
    Responses,
}

impl Family {
    /// The family for an API path (without the proxy's `/<provider_id>` part).
    pub fn from_api_path(path: &str) -> Option<Family> {
        let p = path.split('?').next().unwrap_or(path).trim_end_matches('/');
        if p.ends_with("/chat/completions") || p == "chat/completions" {
            Some(Family::Chat)
        } else if p.ends_with("/messages") || p == "messages" {
            Some(Family::Messages)
        } else if p.ends_with("/responses") || p == "responses" {
            Some(Family::Responses)
        } else {
            None
        }
    }
}

/// Normalized message role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// One message, normalized across the three wire families.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NMsg {
    pub role: Role,
    /// Concatenated text (tool results: the result text).
    pub text: String,
    /// Tool call ids this assistant message made.
    pub call_ids: Vec<String>,
    /// The call a tool message answers.
    pub tool_call_id: Option<String>,
    /// Whether an assistant message carries reasoning (DeepSeek: `reasoning_content` is a string).
    pub has_reasoning: bool,
}

impl NMsg {
    fn new(role: Role, text: String) -> NMsg {
        NMsg {
            role,
            text,
            call_ids: vec![],
            tool_call_id: None,
            has_reasoning: false,
        }
    }
}

/// A parsed request.
#[derive(Clone, Debug)]
pub struct ReqView {
    pub family: Family,
    pub value: Value,
    pub model: Option<String>,
    pub msgs: Vec<NMsg>,
    pub tool_names: Vec<String>,
    /// Prefix bytes as sent: DeepSeek `messages[0]` + `tools`; Anthropic `system` + `tools`;
    /// OpenAI the leading developer message + `tools` (provider.md §3.12 `prefix`).
    pub prefix_bytes: Vec<u8>,
    /// The rendered prompt R of testing.md §3.4.3: the prefix bytes, then every later message.
    pub prompt_bytes: Vec<u8>,
    pub thinking_disabled: bool,
    pub body_len: usize,
}

impl ReqView {
    /// The number of assistant messages already in the conversation (the index of the reply).
    pub fn assistant_count(&self) -> usize {
        self.msgs
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .count()
    }
    /// The last non-system message.
    pub fn last(&self) -> Option<&NMsg> {
        self.msgs.iter().rev().find(|m| m.role != Role::System)
    }
    /// The most recent user message (not a tool result).
    pub fn last_user(&self) -> Option<&NMsg> {
        self.msgs.iter().rev().find(|m| m.role == Role::User)
    }
}

#[derive(Deserialize)]
struct RawChat<'a> {
    #[serde(borrow, default)]
    messages: Option<Vec<&'a RawValue>>,
    #[serde(borrow, default)]
    tools: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct RawMessages<'a> {
    #[serde(borrow, default)]
    system: Option<&'a RawValue>,
    #[serde(borrow, default)]
    tools: Option<&'a RawValue>,
    #[serde(borrow, default)]
    messages: Option<Vec<&'a RawValue>>,
}

#[derive(Deserialize)]
struct RawResponses<'a> {
    #[serde(borrow, default)]
    tools: Option<&'a RawValue>,
    #[serde(borrow, default)]
    input: Option<Vec<&'a RawValue>>,
}

/// Parse a request body of `family`. Errors are invalid JSON or a body that is not an object.
pub fn parse(family: Family, body: &[u8]) -> Result<ReqView, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|e| format!("invalid JSON body: {e}"))?;
    if !value.is_object() {
        return Err("request body is not a JSON object".into());
    }
    let model = value.get("model").and_then(Value::as_str).map(String::from);
    let thinking_disabled = value
        .get("thinking")
        .and_then(|t| t.get("type"))
        .and_then(Value::as_str)
        == Some("disabled");
    let mut prefix = Vec::new();
    let mut prompt = Vec::new();
    let (msgs, tool_names);
    match family {
        Family::Chat => {
            let raw: RawChat =
                serde_json::from_slice(body).map_err(|e| format!("invalid request: {e}"))?;
            let rm = raw.messages.unwrap_or_default();
            if let Some(first) = rm.first() {
                prefix.extend_from_slice(first.get().as_bytes());
            }
            if let Some(t) = raw.tools {
                prefix.extend_from_slice(t.get().as_bytes());
            }
            prompt.extend_from_slice(&prefix);
            for m in rm.iter().skip(1) {
                prompt.extend_from_slice(m.get().as_bytes());
            }
            msgs = chat_msgs(&value);
            tool_names = names(&value, |t| t.get("function").and_then(|f| f.get("name")));
        }
        Family::Messages => {
            let raw: RawMessages =
                serde_json::from_slice(body).map_err(|e| format!("invalid request: {e}"))?;
            if let Some(s) = raw.system {
                prefix.extend_from_slice(s.get().as_bytes());
            }
            if let Some(t) = raw.tools {
                prefix.extend_from_slice(t.get().as_bytes());
            }
            prompt.extend_from_slice(&prefix);
            for m in raw.messages.unwrap_or_default() {
                prompt.extend_from_slice(m.get().as_bytes());
            }
            msgs = messages_msgs(&value);
            tool_names = names(&value, |t| t.get("name"));
        }
        Family::Responses => {
            let raw: RawResponses =
                serde_json::from_slice(body).map_err(|e| format!("invalid request: {e}"))?;
            let items = raw.input.unwrap_or_default();
            let lead_dev = value
                .get("input")
                .and_then(|i| i.get(0))
                .and_then(|m| m.get("role"))
                .and_then(Value::as_str)
                .is_some_and(|r| r == "developer" || r == "system");
            let skip = usize::from(lead_dev);
            if lead_dev {
                prefix.extend_from_slice(items[0].get().as_bytes());
            }
            if let Some(t) = raw.tools {
                prefix.extend_from_slice(t.get().as_bytes());
            }
            prompt.extend_from_slice(&prefix);
            for m in items.iter().skip(skip) {
                prompt.extend_from_slice(m.get().as_bytes());
            }
            msgs = responses_msgs(&value);
            tool_names = names(&value, |t| t.get("name"));
        }
    }
    Ok(ReqView {
        family,
        model,
        msgs,
        tool_names,
        prefix_bytes: prefix,
        prompt_bytes: prompt,
        thinking_disabled,
        body_len: body.len(),
        value,
    })
}

fn names(v: &Value, f: impl Fn(&Value) -> Option<&Value>) -> Vec<String> {
    v.get("tools")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|t| f(t).and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Text of a `content` value: a string, or an array of parts with `text` fields (joined by `\n`).
fn content_text(c: Option<&Value>) -> String {
    match c {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => {
            let texts: Vec<&str> = parts
                .iter()
                .filter_map(|p| match p {
                    Value::String(s) => Some(s.as_str()),
                    _ => p.get("text").and_then(Value::as_str),
                })
                .collect();
            texts.join("\n")
        }
        _ => String::new(),
    }
}

fn chat_msgs(v: &Value) -> Vec<NMsg> {
    let mut out = Vec::new();
    for m in v
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let role = match m.get("role").and_then(Value::as_str) {
            Some("system") | Some("developer") => Role::System,
            Some("assistant") => Role::Assistant,
            Some("tool") => Role::Tool,
            _ => Role::User,
        };
        let mut n = NMsg::new(role, content_text(m.get("content")));
        if role == Role::Assistant {
            n.has_reasoning = m.get("reasoning_content").is_some_and(Value::is_string);
            for c in m
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(id) = c.get("id").and_then(Value::as_str) {
                    n.call_ids.push(id.into());
                }
            }
        }
        if role == Role::Tool {
            n.tool_call_id = m
                .get("tool_call_id")
                .and_then(Value::as_str)
                .map(String::from);
        }
        out.push(n);
    }
    out
}

fn messages_msgs(v: &Value) -> Vec<NMsg> {
    let mut out = Vec::new();
    if let Some(s) = v.get("system") {
        out.push(NMsg::new(Role::System, content_text(Some(s))));
    }
    for m in v
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let assistant = m.get("role").and_then(Value::as_str) == Some("assistant");
        match m.get("content") {
            Some(Value::Array(blocks)) => {
                if assistant {
                    let mut n = NMsg::new(Role::Assistant, String::new());
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("text") => n
                                .text
                                .push_str(b.get("text").and_then(Value::as_str).unwrap_or("")),
                            Some("tool_use") => {
                                if let Some(id) = b.get("id").and_then(Value::as_str) {
                                    n.call_ids.push(id.into());
                                }
                            }
                            Some("thinking") | Some("redacted_thinking") => n.has_reasoning = true,
                            _ => {}
                        }
                    }
                    out.push(n);
                } else {
                    let mut user_text = Vec::new();
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("tool_result") => {
                                let mut n = NMsg::new(Role::Tool, content_text(b.get("content")));
                                n.tool_call_id = b
                                    .get("tool_use_id")
                                    .and_then(Value::as_str)
                                    .map(String::from);
                                out.push(n);
                            }
                            Some("text") => {
                                user_text.push(b.get("text").and_then(Value::as_str).unwrap_or(""))
                            }
                            _ => {}
                        }
                    }
                    if !user_text.is_empty() {
                        out.push(NMsg::new(Role::User, user_text.join("\n")));
                    }
                }
            }
            c => out.push(NMsg::new(
                if assistant {
                    Role::Assistant
                } else {
                    Role::User
                },
                content_text(c),
            )),
        }
    }
    out
}

fn responses_msgs(v: &Value) -> Vec<NMsg> {
    let mut out: Vec<NMsg> = Vec::new();
    let items: Vec<Value> = match v.get("input") {
        Some(Value::String(s)) => vec![serde_json::json!({"role":"user","content":s})],
        Some(Value::Array(a)) => a.clone(),
        _ => vec![],
    };
    // Consecutive assistant-side items (assistant message, function_call, reasoning) form one turn.
    let mut open_assistant = false;
    for it in &items {
        let ty = it.get("type").and_then(Value::as_str).unwrap_or("message");
        match ty {
            "function_call" | "reasoning" => {
                if !open_assistant {
                    out.push(NMsg::new(Role::Assistant, String::new()));
                    open_assistant = true;
                }
                if let Some(last) = out.last_mut() {
                    if ty == "reasoning" {
                        last.has_reasoning = true;
                    } else if let Some(id) = it.get("call_id").and_then(Value::as_str) {
                        last.call_ids.push(id.into());
                    }
                }
            }
            "function_call_output" => {
                open_assistant = false;
                let mut n = NMsg::new(Role::Tool, content_text(it.get("output")));
                n.tool_call_id = it.get("call_id").and_then(Value::as_str).map(String::from);
                out.push(n);
            }
            _ => {
                let role = match it.get("role").and_then(Value::as_str) {
                    Some("developer") | Some("system") => Role::System,
                    Some("assistant") => Role::Assistant,
                    _ => Role::User,
                };
                let text = content_text(it.get("content"));
                if role == Role::Assistant {
                    if !open_assistant {
                        out.push(NMsg::new(Role::Assistant, String::new()));
                        open_assistant = true;
                    }
                    if let Some(last) = out.last_mut() {
                        last.text.push_str(&text);
                    }
                } else {
                    open_assistant = false;
                    out.push(NMsg::new(role, text));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_from_path() {
        assert_eq!(
            Family::from_api_path("/chat/completions"),
            Some(Family::Chat)
        );
        assert_eq!(
            Family::from_api_path("/v1/chat/completions"),
            Some(Family::Chat)
        );
        assert_eq!(
            Family::from_api_path("chat/completions"),
            Some(Family::Chat)
        );
        assert_eq!(
            Family::from_api_path("/v1/messages"),
            Some(Family::Messages)
        );
        assert_eq!(
            Family::from_api_path("/v1/responses"),
            Some(Family::Responses)
        );
        assert_eq!(Family::from_api_path("/v1/models"), None);
    }

    #[test]
    fn chat_prefix_is_raw_bytes_as_sent() {
        // Unsorted keys and odd spacing must survive: the prefix is the bytes as sent.
        let body = br#"{"model":"m","messages":[{"role":"system", "content":"S"},{"role":"user","content":"hi"}],"tools":[{"type":"function","function":{"name":"bash"}}],"stream":true}"#;
        let v = parse(Family::Chat, body).unwrap();
        assert_eq!(
            String::from_utf8(v.prefix_bytes.clone()).unwrap(),
            r#"{"role":"system", "content":"S"}[{"type":"function","function":{"name":"bash"}}]"#
        );
        assert!(v.prompt_bytes.starts_with(&v.prefix_bytes));
        assert!(
            v.prompt_bytes
                .ends_with(br#"{"role":"user","content":"hi"}"#)
        );
        assert_eq!(v.tool_names, vec!["bash"]);
        assert_eq!(v.last().unwrap().role, Role::User);
        assert!(!v.thinking_disabled);
    }

    #[test]
    fn chat_normalizes_calls_and_reasoning() {
        let body = serde_json::json!({"messages":[
            {"role":"system","content":"S"},
            {"role":"user","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]},
            {"role":"assistant","content":"","reasoning_content":"r","tool_calls":[{"id":"c1","type":"function","function":{"name":"bash","arguments":"{}"}}]},
            {"role":"tool","tool_call_id":"c1","content":"out"}
        ],"thinking":{"type":"disabled"}});
        let v = parse(Family::Chat, body.to_string().as_bytes()).unwrap();
        assert_eq!(v.msgs[1].text, "a\nb");
        assert!(v.msgs[2].has_reasoning);
        assert_eq!(v.msgs[2].call_ids, vec!["c1"]);
        assert_eq!(v.msgs[3].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(v.assistant_count(), 1);
        assert!(v.thinking_disabled);
    }

    #[test]
    fn anthropic_tool_results_become_tool_messages() {
        let body = serde_json::json!({"system":"S","messages":[
            {"role":"user","content":"go"},
            {"role":"assistant","content":[{"type":"thinking","thinking":"","signature":"x"},{"type":"tool_use","id":"t1","name":"bash","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"},{"type":"text","text":"steer"}]}
        ],"tools":[{"name":"bash"}]});
        let v = parse(Family::Messages, body.to_string().as_bytes()).unwrap();
        let roles: Vec<Role> = v.msgs.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![
                Role::System,
                Role::User,
                Role::Assistant,
                Role::Tool,
                Role::User
            ]
        );
        assert_eq!(v.msgs[3].text, "ok");
        assert_eq!(v.tool_names, vec!["bash"]);
    }

    #[test]
    fn responses_groups_assistant_items() {
        let body = serde_json::json!({"input":[
            {"role":"developer","content":"S"},
            {"role":"user","content":"go"},
            {"type":"reasoning","id":"rs","encrypted_content":"e","summary":[]},
            {"type":"function_call","call_id":"c1","name":"bash","arguments":"{}"},
            {"type":"function_call_output","call_id":"c1","output":"ok"}
        ],"tools":[{"type":"function","name":"bash"}]});
        let raw = body.to_string();
        let v = parse(Family::Responses, raw.as_bytes()).unwrap();
        let roles: Vec<Role> = v.msgs.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![Role::System, Role::User, Role::Assistant, Role::Tool]
        );
        assert_eq!(v.msgs[2].call_ids, vec!["c1"]);
        assert!(v.prefix_bytes.starts_with(br#"{"#));
        assert_eq!(v.assistant_count(), 1);
    }

    #[test]
    fn rejects_non_objects() {
        assert!(parse(Family::Chat, b"[1]").is_err());
        assert!(parse(Family::Chat, b"{").is_err());
    }
}
