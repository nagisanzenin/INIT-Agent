//! The head hash, `prefix_hash` (log-context.md §3.10.4). Defined on the request body as sent, so the
//! kernel proxy recomputes the same value from the bytes it forwards (kernel/proxy.md §3.12.1).
//!
//! ```text
//! prefix_hash = "sha256:" + hex(SHA-256(ICJ({"v":1, "provider", "model", "params", "system", "tools"})))
//! ```

use serde_json::{Map, Value, json};

use super::model::Api;
use crate::log::icj;
use crate::sha256;

/// Deletes every `cache_control` and `prompt_cache_breakpoint` key, at any depth.
pub fn strip_cache_markers(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| {
                    k.as_str() != "cache_control" && k.as_str() != "prompt_cache_breakpoint"
                })
                .map(|(k, x)| (k.clone(), strip_cache_markers(x)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(strip_cache_markers).collect()),
        other => other.clone(),
    }
}

fn params(body: &Value, keys: &[&str]) -> Value {
    let mut m = Map::new();
    for k in keys {
        if let Some(v) = body.get(*k) {
            m.insert((*k).to_string(), v.clone());
        }
    }
    Value::Object(m)
}

fn leading_run<'a>(items: Option<&'a Value>, roles: &[&str]) -> Vec<&'a Value> {
    items
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take_while(|m| {
                    m.get("role")
                        .and_then(Value::as_str)
                        .map(|r| roles.contains(&r))
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `head_object` of a request body, by wire format. `provider` is the proxy path id.
/// A missing `tools` (never sent by INIT) hashes as `null`.
pub fn head_object(provider: &str, api: Api, body: &Value) -> Value {
    let (system, param_keys): (Value, &[&str]) = match api {
        Api::ChatCompletions => {
            let sys: Vec<Value> = leading_run(body.get("messages"), &["system"])
                .into_iter()
                .map(|m| m.get("content").cloned().unwrap_or(Value::Null))
                .collect();
            (
                Value::Array(sys),
                &["reasoning_effort", "thinking", "tool_choice", "user_id"],
            )
        }
        Api::Messages => (
            body.get("system").cloned().unwrap_or(Value::Null),
            &["output_config", "speed", "thinking", "tool_choice"],
        ),
        Api::Responses => {
            let items: Vec<Value> = leading_run(body.get("input"), &["developer", "system"])
                .into_iter()
                .cloned()
                .collect();
            (
                json!({"instructions": body.get("instructions").cloned().unwrap_or(Value::Null), "items": items}),
                &["prompt_cache_options", "reasoning", "text", "tool_choice"],
            )
        }
    };
    json!({
        "v": 1,
        "provider": provider,
        "model": body.get("model").cloned().unwrap_or(Value::Null),
        "params": params(body, param_keys),
        "system": strip_cache_markers(&system),
        "tools": strip_cache_markers(body.get("tools").unwrap_or(&Value::Null)),
    })
}

/// `"sha256:" + hex` of the ICJ head object.
pub fn prefix_hash(provider: &str, api: Api, body: &Value) -> Result<String, icj::IcjError> {
    Ok(sha256::tagged(&icj::to_vec(&head_object(
        provider, api, body,
    ))?))
}

/// Display form: `sha256:` plus the first 12 hex digits.
pub fn short(hash: &str) -> String {
    let hex = hash.strip_prefix("sha256:").unwrap_or(hash);
    format!("sha256:{}", &hex[..hex.len().min(12)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_completions_extraction() {
        let body = json!({
            "max_tokens": 10, "stream": true, "model": "deepseek-flash",
            "reasoning_effort": "high", "thinking": {"type": "enabled"},
            "messages": [{"role":"system","content":"A"},{"role":"system","content":"B"},{"role":"user","content":"hi"},{"role":"system","content":"late"}],
            "tools": [{"type":"function","function":{"name":"bash","description":"d","parameters":{}}}]
        });
        let o = head_object("deepseek", Api::ChatCompletions, &body);
        assert_eq!(o["system"], json!(["A", "B"]));
        assert_eq!(
            o["params"],
            json!({"reasoning_effort":"high","thinking":{"type":"enabled"}})
        );
        // Messages after the leading system run, max_tokens and stream do not change the hash.
        let mut b2 = body.clone();
        b2["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"user","content":"more"}));
        b2["max_tokens"] = json!(99);
        b2["stream"] = json!(false);
        assert_eq!(
            prefix_hash("deepseek", Api::ChatCompletions, &body),
            prefix_hash("deepseek", Api::ChatCompletions, &b2)
        );
        // The provider id, a param or a tool byte does.
        assert_ne!(
            prefix_hash("deepseek", Api::ChatCompletions, &body),
            prefix_hash("mock", Api::ChatCompletions, &body)
        );
        let mut b3 = body.clone();
        b3["reasoning_effort"] = json!("max");
        assert_ne!(
            prefix_hash("deepseek", Api::ChatCompletions, &body),
            prefix_hash("deepseek", Api::ChatCompletions, &b3)
        );
    }

    #[test]
    fn markers_removed_anthropic_and_responses() {
        let a = json!({"model":"claude-opus-5-5","system":[{"cache_control":{"type":"ephemeral"},"text":"core","type":"text"}],
                       "tools":[{"name":"bash","input_schema":{},"cache_control":{"type":"ephemeral"}}],"thinking":{"type":"adaptive"},
                       "cache_control":{"type":"ephemeral"},"messages":[]});
        let o = head_object("anthropic", Api::Messages, &a);
        assert_eq!(o["system"], json!([{"text":"core","type":"text"}]));
        assert_eq!(o["tools"], json!([{"input_schema":{},"name":"bash"}]));
        assert_eq!(o["params"], json!({"thinking":{"type":"adaptive"}}));
        let r = json!({"model":"gpt-6-astra","input":[{"role":"developer","type":"message","content":[{"text":"c","type":"input_text","prompt_cache_breakpoint":{"mode":"explicit"}}]},{"role":"user","type":"message","content":[]}],
                       "reasoning":{"effort":"high"},"prompt_cache_key":"sid","tools":[]});
        let o = head_object("openai", Api::Responses, &r);
        assert_eq!(
            o["system"],
            json!({"instructions":null,"items":[{"content":[{"text":"c","type":"input_text"}],"role":"developer","type":"message"}]})
        );
        assert_eq!(o["params"], json!({"reasoning":{"effort":"high"}}));
        assert_eq!(short("sha256:0123456789abcdef0123"), "sha256:0123456789ab");
    }
}
