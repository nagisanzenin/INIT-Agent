//! Canonical JSON output: sorted keys at every level, compact, no whitespace.
//!
//! The mock's response bytes must not depend on whether some other crate in the workspace build
//! turns on serde_json's `preserve_order` (feature unification), so keys are sorted here explicitly
//! instead of relying on `Map` being a `BTreeMap`.

use serde_json::Value;

/// Serialize `v` with sorted keys, compactly.
pub fn to_string(v: &Value) -> String {
    let mut out = String::new();
    write(&mut out, v);
    out
}

/// Serialize `v` with sorted keys into `out`.
pub fn write(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_str(out, s),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(out, x);
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(out, k);
                out.push(':');
                write(out, &m[k.as_str()]);
            }
            out.push('}');
        }
    }
}

/// A JSON string literal (serde_json's escaping: `"`, `\`, control characters; UTF-8 kept).
pub fn write_str(out: &mut String, s: &str) {
    match serde_json::to_string(s) {
        Ok(lit) => out.push_str(&lit),
        Err(_) => out.push_str("\"\""), // unreachable for &str
    }
}

/// A JSON string literal as an owned `String`.
pub fn str_lit(s: &str) -> String {
    let mut out = String::new();
    write_str(&mut out, s);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorts_keys_recursively() {
        let v = json!({"b": 1, "a": {"z": [true, null], "c": "x\"y"}});
        assert_eq!(to_string(&v), r#"{"a":{"c":"x\"y","z":[true,null]},"b":1}"#);
    }

    #[test]
    fn keeps_utf8_and_escapes_controls() {
        assert_eq!(str_lit("é\n\u{1}"), "\"é\\n\\u0001\"");
    }
}
