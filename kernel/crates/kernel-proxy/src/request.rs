//! Request side: attribution headers, header allowlists, the minimal borrowed parse and the
//! pre-flight checks (kernel/proxy.md §3.6, §4.1).

use crate::hash;
use crate::tap::Family;
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use std::borrow::Cow;

/// `[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}`.
pub fn valid_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// The `x-init-*` attribution headers (§3.6). Malformed values are treated as absent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attribution {
    pub session: Option<String>,
    pub purpose: Option<String>,
    pub attempt: Option<u32>,
    pub task: Option<String>,
    pub subtask: Option<String>,
}

impl Attribution {
    /// Read from `(lowercase name, value)` pairs.
    pub fn from_headers<'a>(headers: impl Iterator<Item = (&'a str, &'a str)>) -> Attribution {
        let mut a = Attribution::default();
        for (k, v) in headers {
            match k {
                "x-init-session" if valid_id(v) => a.session = Some(v.into()),
                "x-init-purpose" if matches!(v, "turn" | "probe" | "compaction") => {
                    a.purpose = Some(v.into())
                }
                "x-init-attempt" => {
                    a.attempt = v
                        .parse::<u32>()
                        .ok()
                        .filter(|n| (1..=99).contains(n) && !v.starts_with('0'))
                }
                "x-init-task" if valid_id(v) => a.task = Some(v.into()),
                "x-init-subtask" if valid_id(v) => a.subtask = Some(v.into()),
                _ => {}
            }
        }
        a
    }
}

/// Request headers forwarded from the body to the egress (§3.6 allowlist).
pub const FORWARD_REQUEST: [&str; 6] = [
    "content-type",
    "accept",
    "anthropic-version",
    "anthropic-beta",
    "openai-beta",
    "idempotency-key",
];

/// Response headers forwarded from the egress to the body (§4.1 allowlist; prefixes end in `-`).
pub fn forward_response_header(name: &str) -> bool {
    matches!(
        name,
        "content-type"
            | "retry-after"
            | "retry-after-ms"
            | "request-id"
            | "x-request-id"
            | "x-ds-trace-id"
            | "openai-processing-ms"
    ) || name.starts_with("anthropic-ratelimit-")
        || name.starts_with("x-ratelimit-")
}

#[derive(Deserialize)]
struct StreamOptions {
    #[serde(default)]
    include_usage: Option<bool>,
}

/// The minimal borrowed view (§3.6). Unknown keys are ignored.
#[derive(Deserialize)]
struct ReqView<'a> {
    #[serde(borrow, default)]
    model: Option<Cow<'a, str>>,
    #[serde(default)]
    stream: Option<bool>,
    #[serde(default)]
    max_tokens: Option<u64>,
    #[serde(default)]
    max_completion_tokens: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<u64>,
    #[serde(borrow, default)]
    tools: Option<&'a RawValue>,
    #[serde(borrow, default)]
    system: Option<&'a RawValue>,
    #[serde(borrow, default)]
    instructions: Option<&'a RawValue>,
    #[serde(borrow, default)]
    messages: Option<&'a RawValue>,
    #[serde(borrow, default)]
    input: Option<&'a RawValue>,
    #[serde(default)]
    stream_options: Option<StreamOptions>,
    #[serde(borrow, default)]
    service_tier: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    speed: Option<Cow<'a, str>>,
    // Head params (log-context.md §3.10.4), kept raw for the head object.
    #[serde(borrow, default)]
    reasoning_effort: Option<&'a RawValue>,
    #[serde(borrow, default)]
    thinking: Option<&'a RawValue>,
    #[serde(borrow, default)]
    tool_choice: Option<&'a RawValue>,
    #[serde(borrow, default)]
    user_id: Option<&'a RawValue>,
    #[serde(borrow, default)]
    output_config: Option<&'a RawValue>,
    #[serde(borrow, default)]
    prompt_cache_options: Option<&'a RawValue>,
    #[serde(borrow, default)]
    reasoning: Option<&'a RawValue>,
    #[serde(borrow, default)]
    text: Option<&'a RawValue>,
}

/// Minimal view of one message / input item, for roles and tool-result ids.
#[derive(Deserialize)]
struct ItemView<'a> {
    #[serde(borrow, default)]
    role: Option<Cow<'a, str>>,
    #[serde(borrow, default, rename = "type")]
    ty: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    tool_call_id: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    call_id: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    content: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct BlockView<'a> {
    #[serde(borrow, default, rename = "type")]
    ty: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    tool_use_id: Option<Cow<'a, str>>,
}

/// What the proxy extracts from a metered request body.
#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub model: Option<String>,
    pub stream: bool,
    /// `max_tokens | max_completion_tokens | max_output_tokens`.
    pub max_out: Option<u64>,
    pub include_usage: Option<bool>,
    pub service_tier: Option<String>,
    pub speed: Option<String>,
    /// Byte range of the message list (`messages`, or `input` when an array), brackets included.
    pub list: Option<(usize, usize)>,
    pub elements: Vec<(usize, usize)>,
    pub prefix_hash: String,
    pub head_bytes_hash: String,
    pub tools_sha256: String,
    /// `(name, schema_sha256)` in request order.
    pub tools: Vec<(String, String)>,
    /// Call ids of tool results carried by the request (for `tool_round_trip`).
    pub tool_result_ids: Vec<String>,
}

fn offset(buf: &[u8], raw: &RawValue) -> (usize, usize) {
    let s = raw.get().as_ptr() as usize - buf.as_ptr() as usize;
    (s, s + raw.get().len())
}

fn raw_value(r: Option<&RawValue>) -> Option<Value> {
    r.and_then(|r| serde_json::from_str(r.get()).ok())
}

/// One pass over a metered request (§3.6). `Err` = not valid JSON (`request.bad_json`).
pub fn parse(provider: &str, family: Family, buf: &[u8]) -> Result<Parsed, String> {
    let text = std::str::from_utf8(buf).map_err(|e| e.to_string())?;
    let v: ReqView = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut p = Parsed {
        model: v.model.as_deref().map(str::to_string),
        stream: v.stream.unwrap_or(false),
        max_out: v
            .max_tokens
            .or(v.max_completion_tokens)
            .or(v.max_output_tokens),
        include_usage: v.stream_options.as_ref().and_then(|s| s.include_usage),
        service_tier: v.service_tier.as_deref().map(str::to_string),
        speed: v.speed.as_deref().map(str::to_string),
        ..Parsed::default()
    };
    // The message list.
    let list_raw = match family {
        Family::OpenAiResponses => v.input.filter(|r| r.get().trim_start().starts_with('[')),
        _ => v.messages.filter(|r| r.get().trim_start().starts_with('[')),
    };
    let mut items: Vec<&RawValue> = Vec::new();
    if let Some(l) = list_raw {
        p.list = Some(offset(buf, l));
        items = serde_json::from_str::<Vec<&RawValue>>(l.get()).map_err(|e| e.to_string())?;
        p.elements = items.iter().map(|e| offset(buf, e)).collect();
    }
    // Leading system items and tool-result ids from the message list.
    let mut leading_sys: Vec<&RawValue> = Vec::new();
    let mut leading_vals: Vec<Value> = Vec::new();
    let mut in_lead = true;
    for it in &items {
        let Ok(iv) = serde_json::from_str::<ItemView>(it.get()) else {
            in_lead = false;
            continue;
        };
        let role = iv.role.as_deref().unwrap_or("");
        let sys_roles: &[&str] = match family {
            Family::OpenAiResponses => &["system", "developer"],
            Family::Anthropic => &[],
            _ => &["system"],
        };
        if in_lead && sys_roles.contains(&role) {
            leading_sys.push(it);
            if let Ok(val) = serde_json::from_str::<Value>(it.get()) {
                leading_vals.push(val);
            }
        } else {
            in_lead = false;
        }
        match family {
            Family::OpenAiChat | Family::None => {
                if role == "tool"
                    && let Some(id) = iv.tool_call_id
                {
                    p.tool_result_ids.push(id.into_owned());
                }
            }
            Family::Anthropic => {
                if let Some(c) = iv.content.filter(|c| c.get().trim_start().starts_with('['))
                    && let Ok(blocks) = serde_json::from_str::<Vec<BlockView>>(c.get())
                {
                    for b in blocks {
                        if b.ty.as_deref() == Some("tool_result")
                            && let Some(id) = b.tool_use_id
                        {
                            p.tool_result_ids.push(id.into_owned());
                        }
                    }
                }
            }
            Family::OpenAiResponses => {
                if matches!(
                    iv.ty.as_deref(),
                    Some("function_call_output" | "custom_tool_call_output")
                ) && let Some(id) = iv.call_id
                {
                    p.tool_result_ids.push(id.into_owned());
                }
            }
        }
    }
    // The head object, built only from the head keys.
    let mut head = serde_json::Map::new();
    let put = |m: &mut serde_json::Map<String, Value>, k: &str, r: Option<&RawValue>| {
        if let Some(x) = raw_value(r) {
            m.insert(k.to_string(), x);
        }
    };
    if let Some(m) = &p.model {
        head.insert("model".into(), Value::String(m.clone()));
    }
    put(&mut head, "tools", v.tools);
    put(&mut head, "system", v.system);
    put(&mut head, "instructions", v.instructions);
    for (k, r) in [
        ("reasoning_effort", v.reasoning_effort),
        ("thinking", v.thinking),
        ("tool_choice", v.tool_choice),
        ("user_id", v.user_id),
        ("output_config", v.output_config),
        ("prompt_cache_options", v.prompt_cache_options),
        ("reasoning", v.reasoning),
        ("text", v.text),
    ] {
        put(&mut head, k, r);
    }
    if let Some(s) = &p.speed {
        head.insert("speed".into(), Value::String(s.clone()));
    }
    let lead_key = if family == Family::OpenAiResponses {
        "input"
    } else {
        "messages"
    };
    head.insert(lead_key.into(), Value::Array(leading_vals));
    let head = Value::Object(head);
    p.prefix_hash = hash::prefix_hash(provider, family, &head);
    let mut sys_items: Vec<&[u8]> = Vec::new();
    match family {
        Family::Anthropic => {
            if let Some(s) = v.system {
                sys_items.push(s.get().as_bytes());
            }
        }
        Family::OpenAiResponses => {
            if let Some(s) = v.instructions {
                sys_items.push(s.get().as_bytes());
            }
            sys_items.extend(leading_sys.iter().map(|r| r.get().as_bytes()));
        }
        _ => sys_items.extend(leading_sys.iter().map(|r| r.get().as_bytes())),
    }
    p.head_bytes_hash = hash::head_bytes_hash(
        family,
        p.model.as_deref().unwrap_or(""),
        v.tools.map(|t| t.get().as_bytes()),
        &sys_items,
    );
    let (ts, per) = hash::tools_sha256(family, head.get("tools"));
    p.tools_sha256 = ts;
    p.tools = per;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn attribution_grammar() {
        let h = [
            ("x-init-session", "20261019-132407-a1b2c3"),
            ("x-init-purpose", "turn"),
            ("x-init-attempt", "2"),
            ("x-init-task", "2026-10-20-03"),
            ("x-init-subtask", "-bad"),
        ];
        let a = Attribution::from_headers(h.iter().copied());
        assert_eq!(a.session.as_deref(), Some("20261019-132407-a1b2c3"));
        assert_eq!(a.attempt, Some(2));
        assert_eq!(a.task.as_deref(), Some("2026-10-20-03"));
        assert_eq!(a.subtask, None);
        let b = Attribution::from_headers(
            [("x-init-attempt", "100"), ("x-init-purpose", "chat")].into_iter(),
        );
        assert_eq!((b.attempt, b.purpose), (None, None));
        assert!(!valid_id(&"a".repeat(65)));
    }

    #[test]
    fn parse_offsets_and_hashes() {
        let body = json!({"model":"deepseek-flash","stream":true,"max_tokens":100,
            "messages":[{"role":"system","content":"S"},{"role":"user","content":"hi"},
                        {"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"bash","arguments":"{}"}}]},
                        {"role":"tool","tool_call_id":"call_1","content":"ok"}],
            "tools":[{"type":"function","function":{"name":"bash","parameters":{"type":"object"}}}],
            "thinking":{"type":"enabled"}});
        let buf = serde_json::to_vec(&body).unwrap();
        let p = parse("deepseek", Family::OpenAiChat, &buf).unwrap();
        assert_eq!(p.model.as_deref(), Some("deepseek-flash"));
        assert_eq!(p.max_out, Some(100));
        assert!(p.stream);
        let (s, e) = p.list.unwrap();
        assert_eq!(buf[s], b'[');
        assert_eq!(buf[e - 1], b']');
        assert_eq!(p.elements.len(), 4);
        let el: Value = serde_json::from_slice(&buf[p.elements[3].0..p.elements[3].1]).unwrap();
        assert_eq!(el["tool_call_id"], "call_1");
        assert_eq!(p.tool_result_ids, vec!["call_1".to_string()]);
        // The proxy's prefix_hash equals the hash of the full body's head object.
        assert_eq!(
            p.prefix_hash,
            hash::prefix_hash("deepseek", Family::OpenAiChat, &body)
        );
        assert_eq!(p.tools[0].0, "bash");
        assert!(parse("deepseek", Family::OpenAiChat, b"{not json").is_err());
        // Whitespace-only change: both hashes unchanged; key reorder in a schema: head_bytes only.
        let pretty = serde_json::to_vec_pretty(&body).unwrap();
        let q = parse("deepseek", Family::OpenAiChat, &pretty).unwrap();
        assert_eq!(
            (q.prefix_hash.clone(), q.head_bytes_hash.clone()),
            (p.prefix_hash.clone(), p.head_bytes_hash.clone())
        );
        let reordered = String::from_utf8(buf.clone()).unwrap().replace(
            r#"{"function":{"name":"bash","parameters":{"type":"object"}},"type":"function"}"#,
            r#"{"type":"function","function":{"parameters":{"type":"object"},"name":"bash"}}"#,
        );
        assert_ne!(reordered.as_bytes(), buf.as_slice());
        let r = parse("deepseek", Family::OpenAiChat, reordered.as_bytes()).unwrap();
        assert_eq!(r.prefix_hash, p.prefix_hash);
        assert_ne!(r.head_bytes_hash, p.head_bytes_hash);
    }

    #[test]
    fn anthropic_and_responses_tool_results() {
        let a = json!({"model":"claude-opus-5-5","system":"S","max_tokens":5,
            "messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"x"}]}]});
        let p = parse(
            "anthropic",
            Family::Anthropic,
            &serde_json::to_vec(&a).unwrap(),
        )
        .unwrap();
        assert_eq!(p.tool_result_ids, vec!["toolu_1".to_string()]);
        assert_eq!(
            p.prefix_hash,
            hash::prefix_hash("anthropic", Family::Anthropic, &a)
        );
        let r = json!({"model":"gpt-6-luna","instructions":"I","max_output_tokens":7,
            "input":[{"role":"developer","content":"D"},{"type":"function_call_output","call_id":"call_9","output":"o"}]});
        let p = parse(
            "openai",
            Family::OpenAiResponses,
            &serde_json::to_vec(&r).unwrap(),
        )
        .unwrap();
        assert_eq!(p.tool_result_ids, vec!["call_9".to_string()]);
        assert_eq!(p.max_out, Some(7));
        assert_eq!(
            p.prefix_hash,
            hash::prefix_hash("openai", Family::OpenAiResponses, &r)
        );
        assert_eq!(p.elements.len(), 2);
    }
}
