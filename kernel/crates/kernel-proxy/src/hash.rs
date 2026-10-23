//! Head hashes and the tools hash (kernel/proxy.md §3.12.1, §3.12.2).
//!
//! - `prefix_hash`: body/log-context.md §3.10.4 (recomputed from the forwarded bytes; the same
//!   extraction as the body's `context::hash::head_object`).
//! - `head_bytes_hash`: `init-headbytes-v1`, proxy-owned.
//! - `tools_sha256`: `init-tools-v1`, proxy-owned (also used by selfmap and body/boot.md).

use crate::tap::Family;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// `"sha256:" + lowercase hex`.
pub fn tagged(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    let mut s = String::with_capacity(71);
    s.push_str("sha256:");
    for b in d.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

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

fn leading_run<'a>(items: Option<&'a Value>, roles: &[&str]) -> Vec<&'a Value> {
    items
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take_while(|m| {
                    m.get("role")
                        .and_then(Value::as_str)
                        .is_some_and(|r| roles.contains(&r))
                })
                .collect()
        })
        .unwrap_or_default()
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

/// The head object of log-context.md §3.10.4. `body` only needs the head keys (the message list
/// may be truncated to its leading system run).
pub fn head_object(provider: &str, family: Family, body: &Value) -> Value {
    let (system, keys): (Value, &[&str]) = match family {
        Family::OpenAiChat | Family::None => {
            let sys: Vec<Value> = leading_run(body.get("messages"), &["system"])
                .into_iter()
                .map(|m| m.get("content").cloned().unwrap_or(Value::Null))
                .collect();
            (
                Value::Array(sys),
                &["reasoning_effort", "thinking", "tool_choice", "user_id"],
            )
        }
        Family::Anthropic => (
            body.get("system").cloned().unwrap_or(Value::Null),
            &["output_config", "speed", "thinking", "tool_choice"],
        ),
        Family::OpenAiResponses => {
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
        "params": params(body, keys),
        "system": strip_cache_markers(&system),
        "tools": strip_cache_markers(body.get("tools").unwrap_or(&Value::Null)),
    })
}

/// `prefix_hash = "sha256:" + hex(SHA-256(ICJ(head_object)))`. serde_json's `Map` is a `BTreeMap`
/// here (no `preserve_order`), so keys come out byte-sorted.
pub fn prefix_hash(provider: &str, family: Family, body: &Value) -> String {
    tagged(&serde_json::to_vec(&head_object(provider, family, body)).unwrap_or_default())
}

/// Deletes JSON whitespace (0x20, 0x09, 0x0A, 0x0D) outside string literals, nothing else.
pub fn minify(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let (mut in_str, mut esc) = (false, false);
    for &c in raw {
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else {
            match c {
                b' ' | b'\t' | b'\n' | b'\r' => {}
                b'"' => {
                    in_str = true;
                    out.push(c);
                }
                _ => out.push(c),
            }
        }
    }
    out
}

/// `head_bytes_hash` (`init-headbytes-v1`): over the raw bytes as sent, so key order and escape
/// spelling count. `system_items` are the raw leading system items in order (§3.12.1).
pub fn head_bytes_hash(
    family: Family,
    model: &str,
    tools_raw: Option<&[u8]>,
    system_items: &[&[u8]],
) -> String {
    let mut h = Sha256::new();
    h.update(b"init-headbytes-v1\0");
    h.update(family.as_str().as_bytes());
    h.update(b"\0");
    h.update(model.as_bytes());
    h.update(b"\0");
    h.update(minify(tools_raw.unwrap_or(b"")));
    h.update(b"\0");
    for s in system_items {
        h.update(minify(s));
        h.update(b"\0");
    }
    let d = h.finalize();
    let mut s = String::from("sha256:");
    for b in d.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// One tool, normalized to the body's registry shape `{description, input_schema, name}`.
pub fn normalize_tool(family: Family, t: &Value) -> Value {
    let s = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").to_string();
    // The schema is taken as sent (selfmap.md's manifest normalization does the same); only the
    // tool-level `cache_control` key disappears, because normalization keeps three keys.
    let schema = |v: Option<&Value>| v.cloned().unwrap_or_else(|| json!({}));
    let (name, desc, input_schema) = match family {
        Family::OpenAiChat | Family::None => {
            let f = t.get("function").unwrap_or(t);
            (
                s(f.get("name")),
                s(f.get("description")),
                schema(f.get("parameters")),
            )
        }
        Family::Anthropic => (
            s(t.get("name")),
            s(t.get("description")),
            schema(t.get("input_schema")),
        ),
        Family::OpenAiResponses => {
            if t.get("type").and_then(Value::as_str) == Some("custom") {
                let fmt = t.get("format").cloned().unwrap_or(Value::Null);
                (
                    s(t.get("name")),
                    s(t.get("description")),
                    json!({"custom": fmt}),
                )
            } else {
                (
                    s(t.get("name")),
                    s(t.get("description")),
                    schema(t.get("parameters")),
                )
            }
        }
    };
    json!({"description": desc, "input_schema": input_schema, "name": name})
}

/// `(tools_sha256, [(name, schema_sha256)] in request order)` for a request's `tools` value.
pub fn tools_sha256(family: Family, tools: Option<&Value>) -> (String, Vec<(String, String)>) {
    let list: Vec<Value> = tools
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|t| normalize_tool(family, t)).collect())
        .unwrap_or_default();
    let per: Vec<(String, String)> = list
        .iter()
        .map(|t| {
            let name = t["name"].as_str().unwrap_or("").to_string();
            (name, tagged(&serde_json::to_vec(t).unwrap_or_default()))
        })
        .collect();
    let mut sorted = list;
    sorted.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .as_bytes()
            .cmp(b["name"].as_str().unwrap_or("").as_bytes())
    });
    (
        tagged(&serde_json::to_vec(&Value::Array(sorted)).unwrap_or_default()),
        per,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_vectors() {
        assert_eq!(
            tagged(b""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            tagged(b"abc"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// The body's own extraction tests (seed/src/context/hash.rs), recomputed here: the proxy and
    /// the body must agree on the head object.
    #[test]
    fn head_object_matches_body_extraction() {
        let body = json!({
            "max_tokens": 10, "stream": true, "model": "deepseek-flash",
            "reasoning_effort": "high", "thinking": {"type": "enabled"},
            "messages": [{"role":"system","content":"A"},{"role":"system","content":"B"},{"role":"user","content":"hi"},{"role":"system","content":"late"}],
            "tools": [{"type":"function","function":{"name":"bash","description":"d","parameters":{}}}]
        });
        let o = head_object("deepseek", Family::OpenAiChat, &body);
        assert_eq!(o["system"], json!(["A", "B"]));
        assert_eq!(
            o["params"],
            json!({"reasoning_effort":"high","thinking":{"type":"enabled"}})
        );
        let mut b2 = body.clone();
        b2["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"user","content":"more"}));
        b2["max_tokens"] = json!(99);
        assert_eq!(
            prefix_hash("deepseek", Family::OpenAiChat, &body),
            prefix_hash("deepseek", Family::OpenAiChat, &b2)
        );
        assert_ne!(
            prefix_hash("deepseek", Family::OpenAiChat, &body),
            prefix_hash("mock", Family::OpenAiChat, &body)
        );
        let a = json!({"model":"claude-opus-5-5","system":[{"cache_control":{"type":"ephemeral"},"text":"core","type":"text"}],
                       "tools":[{"name":"bash","input_schema":{},"cache_control":{"type":"ephemeral"}}],"thinking":{"type":"adaptive"},
                       "cache_control":{"type":"ephemeral"},"messages":[]});
        let o = head_object("anthropic", Family::Anthropic, &a);
        assert_eq!(o["system"], json!([{"text":"core","type":"text"}]));
        assert_eq!(o["tools"], json!([{"input_schema":{},"name":"bash"}]));
        assert_eq!(o["params"], json!({"thinking":{"type":"adaptive"}}));
        let r = json!({"model":"gpt-6-astra","input":[{"role":"developer","type":"message","content":[{"text":"c","type":"input_text","prompt_cache_breakpoint":{"mode":"explicit"}}]},{"role":"user","type":"message","content":[]}],
                       "reasoning":{"effort":"high"},"prompt_cache_key":"sid","tools":[]});
        let o = head_object("openai", Family::OpenAiResponses, &r);
        assert_eq!(
            o["system"],
            json!({"instructions":null,"items":[{"content":[{"text":"c","type":"input_text"}],"role":"developer","type":"message"}]})
        );
        assert_eq!(o["params"], json!({"reasoning":{"effort":"high"}}));
        // A frozen vector: the ICJ head object of a minimal mock request.
        let m = json!({"model":"mock-frontier-1","messages":[{"role":"system","content":"S"}],"tools":[]});
        let icj = serde_json::to_string(&head_object("mock", Family::OpenAiChat, &m)).unwrap();
        assert_eq!(
            icj,
            r#"{"model":"mock-frontier-1","params":{},"provider":"mock","system":["S"],"tools":[],"v":1}"#
        );
        assert_eq!(
            prefix_hash("mock", Family::OpenAiChat, &m),
            tagged(icj.as_bytes())
        );
    }

    #[test]
    fn minify_keeps_strings() {
        assert_eq!(
            minify(b" { \"a b\" : [ 1 ,\t\"\\\" x\" ] }\n"),
            b"{\"a b\":[1,\"\\\" x\"]}".to_vec()
        );
    }

    #[test]
    fn tools_hash_is_a_set_hash() {
        // The vector of kernel-selfmap's manifest test (init-tools-v1 is shared with selfmap.md).
        let m =
            json!([{"name":"b"},{"name":"a","description":"d","input_schema":{"type":"object"}}]);
        assert_eq!(
            tools_sha256(Family::Anthropic, Some(&m)).0,
            tagged(br#"[{"description":"d","input_schema":{"type":"object"},"name":"a"},{"description":"","input_schema":{},"name":"b"}]"#)
        );
        let a = json!([{"type":"function","function":{"name":"bash","description":"run","parameters":{"type":"object"}}},
                       {"type":"function","function":{"name":"edit","parameters":{"type":"object"}}}]);
        let b = json!([{"type":"function","function":{"name":"edit","parameters":{"type":"object"}}},
                       {"type":"function","function":{"name":"bash","description":"run","parameters":{"type":"object"}}}]);
        let (ha, per) = tools_sha256(Family::OpenAiChat, Some(&a));
        let (hb, _) = tools_sha256(Family::OpenAiChat, Some(&b));
        assert_eq!(ha, hb);
        assert_eq!(per[0].0, "bash");
        // The same tool in Anthropic shape, with a cache marker, normalizes identically.
        let an = json!([{"name":"edit","input_schema":{"type":"object"}},
                        {"name":"bash","description":"run","input_schema":{"type":"object"},"cache_control":{"type":"ephemeral"}}]);
        assert_eq!(tools_sha256(Family::Anthropic, Some(&an)).0, ha);
        let rs = json!([{"type":"function","name":"bash","description":"run","parameters":{"type":"object"}},
                        {"type":"function","name":"edit","parameters":{"type":"object"}}]);
        assert_eq!(tools_sha256(Family::OpenAiResponses, Some(&rs)).0, ha);
        let custom = json!([{"type":"custom","name":"x","format":{"type":"grammar"}}]);
        let n = normalize_tool(Family::OpenAiResponses, &custom[0]);
        assert_eq!(
            n,
            json!({"description":"","input_schema":{"custom":{"type":"grammar"}},"name":"x"})
        );
        assert_eq!(tools_sha256(Family::OpenAiChat, None).0, tagged(b"[]"));
    }
}
