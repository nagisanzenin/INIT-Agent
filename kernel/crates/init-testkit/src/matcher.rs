//! Partial JSON matching for scenario expectations (testing.md §3.6 rules).
//!
//! - An object matcher matches when every key it names is present in the actual object and
//!   matches; other keys are ignored.
//! - A string starting with `~` means "contains": the actual string (or the JSON text of a
//!   non-string value) contains the rest.
//! - `$NAME` refers to a capture: a matcher string that is exactly `$NAME` is replaced by the
//!   captured value; inside a longer string it is replaced textually. Substitution happens before
//!   matching ([`substitute`]).
//! - Arrays match element-wise and must have the same length.
//! - Everything else matches by equality.

use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Captured values (`"capture": {"NEW_SHA": "sha"}`).
pub type Captures = BTreeMap<String, Value>;

/// Replace `$NAME` references in every string of `m`.
pub fn substitute(m: &Value, caps: &Captures) -> Value {
    match m {
        Value::String(s) => {
            if let Some(name) = s.strip_prefix('$')
                && let Some(v) = caps.get(name)
            {
                return v.clone();
            }
            if !s.contains('$') {
                return m.clone();
            }
            let mut out = s.clone();
            // Longest names first, so `$A` does not clobber `$AB`.
            let mut names: Vec<&String> = caps.keys().collect();
            names.sort_by_key(|n| std::cmp::Reverse(n.len()));
            for n in names {
                let rep = match &caps[n] {
                    Value::String(x) => x.clone(),
                    other => other.to_string(),
                };
                out = out.replace(&format!("${n}"), &rep);
            }
            Value::String(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, caps)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), substitute(v, caps)))
                .collect::<Map<_, _>>(),
        ),
        _ => m.clone(),
    }
}

/// True when `actual` matches the (already substituted) matcher `m`.
pub fn matches(m: &Value, actual: &Value) -> bool {
    match m {
        Value::String(s) if s.starts_with('~') => {
            let needle = &s[1..];
            match actual {
                Value::String(a) => a.contains(needle),
                Value::Null => false,
                other => other.to_string().contains(needle),
            }
        }
        Value::Object(mo) => match actual {
            Value::Object(ao) => mo
                .iter()
                .all(|(k, mv)| ao.get(k).is_some_and(|av| matches(mv, av))),
            _ => false,
        },
        Value::Array(ma) => match actual {
            Value::Array(aa) => {
                ma.len() == aa.len() && ma.iter().zip(aa).all(|(x, y)| matches(x, y))
            }
            _ => false,
        },
        other => other == actual,
    }
}

/// The value at a dotted path (`"a.b.0.c"`).
pub fn get_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for p in path.split('.').filter(|p| !p.is_empty()) {
        cur = match cur {
            Value::Object(o) => o.get(p)?,
            Value::Array(a) => a.get(p.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Record captures from a matched value: `spec` maps a capture name to a dotted path.
pub fn capture(spec: &Value, actual: &Value, caps: &mut Captures) -> Result<(), String> {
    let Some(o) = spec.as_object() else {
        return Ok(());
    };
    for (name, path) in o {
        let p = path
            .as_str()
            .ok_or_else(|| format!("capture {name}: the path must be a string"))?;
        let v = get_path(actual, p)
            .ok_or_else(|| format!("capture {name}: no `{p}` in the matched value"))?;
        caps.insert(name.clone(), v.clone());
    }
    Ok(())
}

/// Ordered-subsequence match: each matcher must match a later item than the previous one.
/// Returns `Err(index of the first matcher not found)`.
pub fn in_order(matchers: &[Value], items: &[Value], caps: &Captures) -> Result<(), usize> {
    let mut pos = 0;
    for (i, m) in matchers.iter().enumerate() {
        let m = substitute(m, caps);
        match items[pos..].iter().position(|it| matches(&m, it)) {
            Some(j) => pos += j + 1,
            None => return Err(i),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn partial_contains_and_captures() {
        let ev = json!({"type": "boot", "reason": "promote", "sha": "abc123", "n": 3, "content": [{"text": "hello world"}]});
        assert!(matches(&json!({"type": "boot"}), &ev));
        assert!(!matches(&json!({"type": "boot", "reason": "cold"}), &ev));
        assert!(
            matches(&json!({"content": "~hello"}), &ev),
            "contains over JSON text of a non-string"
        );
        assert!(matches(&json!({"sha": "~c12"}), &ev));
        assert!(!matches(&json!({"missing": null}), &ev));
        let mut caps = Captures::new();
        capture(
            &json!({"NEW_SHA": "sha", "T": "content.0.text"}),
            &ev,
            &mut caps,
        )
        .unwrap();
        assert_eq!(caps["NEW_SHA"], json!("abc123"));
        assert_eq!(
            substitute(&json!({"sha": "$NEW_SHA", "r": "refs/x/$NEW_SHA"}), &caps),
            json!({"sha": "abc123", "r": "refs/x/abc123"})
        );
        assert!(capture(&json!({"X": "nope"}), &ev, &mut caps).is_err());
    }

    #[test]
    fn ordered_subsequence() {
        let items = vec![
            json!({"t": "a"}),
            json!({"t": "b"}),
            json!({"t": "a", "k": 1}),
            json!({"t": "c"}),
        ];
        let caps = Captures::new();
        assert_eq!(
            in_order(&[json!({"t": "a"}), json!({"t": "c"})], &items, &caps),
            Ok(())
        );
        assert_eq!(
            in_order(
                &[json!({"t": "b"}), json!({"k": 1}), json!({"t": "c"})],
                &items,
                &caps
            ),
            Ok(())
        );
        assert_eq!(
            in_order(&[json!({"t": "c"}), json!({"t": "a"})], &items, &caps),
            Err(1)
        );
        assert!(matches(
            &json!([1, {"a": 2}]),
            &json!([1, {"a": 2, "b": 3}])
        ));
        assert!(!matches(&json!([1]), &json!([1, 2])));
    }
}
