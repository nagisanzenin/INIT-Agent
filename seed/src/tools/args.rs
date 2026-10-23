//! Typed argument parsing (tools.md §3.1): field by field, unknown fields rejected with a
//! "did you mean" hint, integers accept `120.0` but not `1.5`.

use serde_json::{Map, Value};

use super::ToolError;

/// Common habits from other harnesses, mapped to this registry's names.
const ALIASES: &[(&str, &str)] = &[
    ("file_path", "path"),
    ("filepath", "path"),
    ("file", "path"),
    ("filename", "path"),
    ("file_name", "path"),
    ("old_string", "old_text"),
    ("oldText", "old_text"),
    ("old", "old_text"),
    ("search", "old_text"),
    ("new_string", "new_text"),
    ("newText", "new_text"),
    ("new", "new_text"),
    ("replace", "new_text"),
    ("replacement", "new_text"),
    ("cmd", "command"),
    ("script", "command"),
    ("text", "content"),
    ("contents", "content"),
    ("data", "content"),
    ("body", "content"),
    ("lines", "limit"),
    ("count", "limit"),
    ("max_lines", "limit"),
    ("start", "offset"),
    ("line", "offset"),
    ("start_line", "offset"),
    ("from", "offset"),
    ("timeout_ms", "timeout"),
    ("timeout_s", "timeout"),
    ("time_limit", "timeout"),
    ("args", "input"),
    ("arguments", "input"),
    ("params", "input"),
    ("parameters", "input"),
    ("tool", "name"),
    ("extension", "name"),
    ("edit", "edits"),
    ("changes", "edits"),
    ("replacements", "edits"),
];

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn field_name(prefix: &str, f: &str) -> String {
    if prefix.is_empty() {
        f.to_string()
    } else {
        format!("{prefix}.{f}")
    }
}

/// Every key must be one of `allowed`.
pub fn check_fields(
    m: &Map<String, Value>,
    allowed: &[&str],
    prefix: &str,
) -> Result<(), ToolError> {
    for k in m.keys() {
        if allowed.contains(&k.as_str()) {
            continue;
        }
        let shown = if prefix.is_empty() {
            format!("unknown field \"{k}\"")
        } else {
            format!("{prefix}: unknown field \"{k}\"")
        };
        let lower = k.to_ascii_lowercase();
        let guess = ALIASES
            .iter()
            .find(|(a, t)| (*a == k || a.eq_ignore_ascii_case(&lower)) && allowed.contains(t))
            .map(|(_, t)| *t)
            .or_else(|| {
                allowed
                    .iter()
                    .copied()
                    .find(|a| lower.len() >= 3 && (lower.contains(a) || a.contains(lower.as_str())))
            });
        let e = ToolError::invalid(shown);
        return Err(match guess {
            Some(g) => e.hint(format!("did you mean \"{g}\"?")),
            None => e.hint(format!("allowed fields: {}", allowed.join(", "))),
        });
    }
    Ok(())
}

pub fn req<'a>(m: &'a Map<String, Value>, f: &str, prefix: &str) -> Result<&'a Value, ToolError> {
    m.get(f).ok_or_else(|| {
        ToolError::invalid(format!("{}: missing required field", field_name(prefix, f)))
    })
}

pub fn req_str(m: &Map<String, Value>, f: &str, prefix: &str) -> Result<String, ToolError> {
    match req(m, f, prefix)? {
        Value::String(s) => Ok(s.clone()),
        other => Err(ToolError::invalid(format!(
            "{}: expected a string, found {}",
            field_name(prefix, f),
            json_type(other)
        ))),
    }
}

/// A JSON integer, or a number with no fractional part (`120.0` reads as 120).
pub fn as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.fract() == 0.0 && f.abs() < 9.0e15)
                .map(|f| f as i64)
        }),
        _ => None,
    }
}

pub fn opt_int(m: &Map<String, Value>, f: &str, prefix: &str) -> Result<Option<i64>, ToolError> {
    match m.get(f) {
        None => Ok(None),
        Some(v) => as_int(v).map(Some).ok_or_else(|| {
            ToolError::invalid(format!(
                "{}: expected an integer, found {}",
                field_name(prefix, f),
                match v {
                    Value::Number(_) => "a fractional number",
                    other => json_type(other),
                }
            ))
        }),
    }
}

pub fn type_error(field: &str, expected: &str, v: &Value) -> ToolError {
    ToolError::invalid(format!(
        "{field}: expected {expected}, found {}",
        json_type(v)
    ))
}
