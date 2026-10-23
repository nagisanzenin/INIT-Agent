//! Test-only: a small JSON Schema (draft 2020-12) validator covering exactly the keywords that
//! log-context.md §4.2's schema uses, and the schema itself, extracted verbatim from the doc into
//! `session-event-1.schema.json`. `pattern` supports the six patterns the schema contains; any other
//! pattern fails the check loudly, so a schema change cannot silently weaken the test.

use serde_json::Value;

pub const SCHEMA_TEXT: &str = include_str!("session-event-1.schema.json");

pub fn schema() -> Value {
    serde_json::from_str(SCHEMA_TEXT).expect("schema parses")
}

fn pattern_matches(pat: &str, s: &str) -> bool {
    let all = |s: &str, f: fn(u8) -> bool| s.bytes().all(f);
    let lhex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
    match pat {
        "^[0-9a-f]{16}$" => s.len() == 16 && all(s, lhex),
        "^sha256:[0-9a-f]{64}$" => s.len() == 71 && s.starts_with("sha256:") && all(&s[7..], lhex),
        "^([0-9a-f]{40}|[0-9a-f]{64})$" => (s.len() == 40 || s.len() == 64) && all(s, lhex),
        "^[a-z][a-z0-9_]{0,31}$" => crate::log::event::valid_type_tag(s),
        "^[0-9A-Za-z._-]{1,64}$" => crate::log::clock::valid_session_id(s),
        "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\\.[0-9]{3}Z$" => {
            crate::log::event::valid_ts(s)
        }
        other => panic!("schema_check: unsupported pattern {other}"),
    }
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        other => panic!("schema_check: unsupported type {other}"),
    }
}

pub struct Validator {
    root: Value,
}

impl Validator {
    pub fn new() -> Self {
        Validator { root: schema() }
    }

    /// Validates `v`; returns the list of violations (empty = valid).
    pub fn validate(&self, v: &Value) -> Vec<String> {
        let mut errs = Vec::new();
        self.check(&self.root, v, "", &mut errs);
        errs
    }

    fn resolve<'a>(&'a self, r: &str) -> &'a Value {
        let name = r
            .strip_prefix("#/$defs/")
            .unwrap_or_else(|| panic!("schema_check: unsupported $ref {r}"));
        self.root["$defs"]
            .get(name)
            .unwrap_or_else(|| panic!("schema_check: missing $def {name}"))
    }

    fn ok(&self, s: &Value, v: &Value) -> bool {
        let mut e = Vec::new();
        self.check(s, v, "", &mut e);
        e.is_empty()
    }

    fn check(&self, s: &Value, v: &Value, at: &str, errs: &mut Vec<String>) {
        let Some(s) = s.as_object() else { return };
        if let Some(r) = s.get("$ref").and_then(Value::as_str) {
            self.check(self.resolve(r), v, at, errs);
        }
        if let Some(t) = s.get("type") {
            let ok = match t {
                Value::String(t) => type_ok(t, v),
                Value::Array(ts) => ts.iter().any(|t| type_ok(t.as_str().unwrap(), v)),
                _ => true,
            };
            if !ok {
                errs.push(format!("{at}: type {t} expected, got {v}"));
            }
        }
        if let Some(c) = s.get("const")
            && c != v
        {
            errs.push(format!("{at}: const {c} expected, got {v}"));
        }
        if let Some(Value::Array(e)) = s.get("enum")
            && !e.contains(v)
        {
            errs.push(format!("{at}: {v} not in enum {e:?}"));
        }
        if let (Some(p), Some(st)) = (s.get("pattern").and_then(Value::as_str), v.as_str())
            && !pattern_matches(p, st)
        {
            errs.push(format!("{at}: {st:?} does not match {p}"));
        }
        if let Some(st) = v.as_str() {
            let n = st.chars().count() as u64;
            if let Some(m) = s.get("minLength").and_then(Value::as_u64)
                && n < m
            {
                errs.push(format!("{at}: shorter than {m}"));
            }
            if let Some(m) = s.get("maxLength").and_then(Value::as_u64)
                && n > m
            {
                errs.push(format!("{at}: longer than {m}"));
            }
        }
        if v.is_i64() || v.is_u64() {
            let x = v
                .as_i64()
                .map(|x| x as i128)
                .unwrap_or_else(|| v.as_u64().unwrap() as i128);
            if let Some(m) = s.get("minimum").and_then(Value::as_i64)
                && x < m as i128
            {
                errs.push(format!("{at}: {x} < minimum {m}"));
            }
            if let Some(m) = s.get("maximum").and_then(Value::as_i64)
                && x > m as i128
            {
                errs.push(format!("{at}: {x} > maximum {m}"));
            }
        }
        if let Some(n) = s.get("not")
            && self.ok(n, v)
        {
            errs.push(format!("{at}: matches a `not` schema"));
        }
        if let Some(Value::Array(all)) = s.get("allOf") {
            for sub in all {
                self.check(sub, v, at, errs);
            }
        }
        if let Some(Value::Array(one)) = s.get("oneOf") {
            let n = one.iter().filter(|sub| self.ok(sub, v)).count();
            if n != 1 {
                errs.push(format!("{at}: oneOf matched {n} schemas for {v}"));
            }
        }
        if let Some(i) = s.get("if")
            && self.ok(i, v)
            && let Some(t) = s.get("then")
        {
            self.check(t, v, at, errs);
        }
        if let Some(o) = v.as_object() {
            if let Some(Value::Array(req)) = s.get("required") {
                for r in req {
                    if !o.contains_key(r.as_str().unwrap()) {
                        errs.push(format!("{at}: missing required {r}"));
                    }
                }
            }
            if let Some(Value::Object(props)) = s.get("properties") {
                for (k, ps) in props {
                    if let Some(x) = o.get(k) {
                        self.check(ps, x, &format!("{at}/{k}"), errs);
                    }
                }
            }
        }
        if let (Some(items), Some(a)) = (s.get("items"), v.as_array()) {
            for (i, x) in a.iter().enumerate() {
                self.check(items, x, &format!("{at}/{i}"), errs);
            }
        }
    }
}

/// Asserts that every line of a session file validates.
pub fn assert_file_valid(path: &std::path::Path) {
    let val = Validator::new();
    let text = std::fs::read(path).unwrap();
    for (i, line) in text.split(|b| *b == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            continue;
        }; // corrupt lines are allowed
        let errs = val.validate(&v);
        assert!(
            errs.is_empty(),
            "line {i} invalid: {errs:?}\n{}",
            String::from_utf8_lossy(line)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validator_catches_violations() {
        let v = Validator::new();
        let good = json!({"format":1,"id":"0b6f1c2d3e4f5061","origin":{"reason":"cold"},"parent_id":null,
            "session_id":"20261019-203015-9f3a2c","sha":"3f2a1c0e3f2a1c0e3f2a1c0e3f2a1c0e3f2a1c0e","ts":"2026-10-19T20:30:15.123Z","type":"session"});
        assert_eq!(v.validate(&good), Vec::<String>::new());
        let mut bad = good.clone();
        bad["origin"]["reason"] = json!("warm");
        assert!(!v.validate(&bad).is_empty());
        let mut bad = good.clone();
        bad["id"] = json!("0000000000000000");
        assert!(!v.validate(&bad).is_empty());
        let mut bad = good.clone();
        bad.as_object_mut().unwrap().remove("sha");
        assert!(!v.validate(&bad).is_empty());
        // Owner messages need input_id; blocks are checked through oneOf.
        let msg = json!({"content":[{"type":"text","text":"hi"}],"id":"0b6f1c2d3e4f5062","parent_id":"0b6f1c2d3e4f5061",
            "role":"user","source":"owner","ts":"2026-10-19T20:30:15.123Z","type":"message"});
        assert!(!v.validate(&msg).is_empty());
        let mut msg2 = msg.clone();
        msg2["input_id"] = json!("00000000000000a1");
        assert_eq!(v.validate(&msg2), Vec::<String>::new());
        msg2["content"][0] = json!({"type":"text"});
        assert!(!v.validate(&msg2).is_empty());
    }
}
