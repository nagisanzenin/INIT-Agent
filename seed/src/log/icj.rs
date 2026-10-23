//! INIT canonical JSON, ICJ v1 (log-context.md §3.8.1).
//!
//! 1. UTF-8, no BOM, no insignificant whitespace.
//! 2. Object keys unique and sorted byte-wise (`serde_json::Map` is a `BTreeMap` without
//!    `preserve_order`, which the body never enables; T-CTX-3 checks it).
//! 3. Strings escape `"` `\` and U+0000–U+001F only (`\b \t \n \f \r`, others `\u00xx` lowercase);
//!    `/`, U+007F and non-ASCII are raw. This is exactly serde_json 1.0.154's `to_vec`.
//! 4. Integers only: floats are rejected.
//! 5. `true`, `false`, `null` literals.
//!
//! Always build a `serde_json::Value` first; never derive `Serialize` straight to bytes (a derive emits
//! declaration order, not sorted order).

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IcjError {
    /// A float was found at this JSON pointer.
    Float(String),
    Serialize(String),
}

impl std::fmt::Display for IcjError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IcjError::Float(p) => write!(f, "ICJ: float at {p} (integers only)"),
            IcjError::Serialize(m) => write!(f, "ICJ: {m}"),
        }
    }
}

impl std::error::Error for IcjError {}

impl From<IcjError> for std::io::Error {
    fn from(e: IcjError) -> Self {
        std::io::Error::other(e.to_string())
    }
}

/// Returns the JSON pointer of the first float in `v`, if any.
pub fn find_float(v: &Value) -> Option<String> {
    fn walk(v: &Value, path: &mut String) -> bool {
        match v {
            Value::Number(n) => !(n.is_i64() || n.is_u64()),
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    let len = path.len();
                    path.push('/');
                    path.push_str(&i.to_string());
                    if walk(x, path) {
                        return true;
                    }
                    path.truncate(len);
                }
                false
            }
            Value::Object(m) => {
                for (k, x) in m {
                    let len = path.len();
                    path.push('/');
                    path.push_str(&k.replace('~', "~0").replace('/', "~1"));
                    if walk(x, path) {
                        return true;
                    }
                    path.truncate(len);
                }
                false
            }
            _ => false,
        }
    }
    let mut p = String::new();
    if walk(v, &mut p) {
        Some(if p.is_empty() { "/".into() } else { p })
    } else {
        None
    }
}

/// ICJ bytes of a value. Rejects floats (rule 4).
pub fn to_vec(v: &Value) -> Result<Vec<u8>, IcjError> {
    if let Some(p) = find_float(v) {
        return Err(IcjError::Float(p));
    }
    serde_json::to_vec(v).map_err(|e| IcjError::Serialize(e.to_string()))
}

/// ICJ string of a value.
pub fn to_string(v: &Value) -> Result<String, IcjError> {
    to_vec(v).map(|b| String::from_utf8(b).expect("serde_json emits UTF-8"))
}

/// Typed struct → `Value` (sorted keys come from `Map`, not from the derive).
pub fn to_value<T: Serialize>(x: &T) -> Result<Value, IcjError> {
    serde_json::to_value(x).map_err(|e| IcjError::Serialize(e.to_string()))
}

/// True when `bytes` is exactly the ICJ form of the JSON it holds.
pub fn is_canonical(bytes: &[u8]) -> bool {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(v) => to_vec(&v).map(|b| b == bytes).unwrap_or(false),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// T-CTX-3: golden vectors.
    #[test]
    fn golden_vectors() {
        // Control characters: the five short escapes, everything else \u00xx lowercase.
        let s: String = (0u32..0x20).map(|c| char::from_u32(c).unwrap()).collect();
        let got = to_string(&json!(s)).unwrap();
        assert_eq!(
            got,
            "\"\\u0000\\u0001\\u0002\\u0003\\u0004\\u0005\\u0006\\u0007\\b\\t\\n\\u000b\\f\\r\\u000e\\u000f\
             \\u0010\\u0011\\u0012\\u0013\\u0014\\u0015\\u0016\\u0017\\u0018\\u0019\\u001a\\u001b\\u001c\\u001d\\u001e\\u001f\""
        );
        // `"` and `\` escaped; `/`, U+007F and non-ASCII raw.
        assert_eq!(
            to_string(&json!("a\"b\\c/d\u{7f}é✓😀")).unwrap(),
            "\"a\\\"b\\\\c/d\u{7f}é✓😀\""
        );
        // Nested key order is byte-wise at every level (uppercase < lowercase, "a" < "aa" < "b").
        let v = json!({"b": {"z": 1, "Z": 2, "aa": [ {"y": true, "x": null} ], "a": false}, "a": -5, "é": 0, "e": 18446744073709551615u64});
        assert_eq!(
            to_string(&v).unwrap(),
            r#"{"a":-5,"b":{"Z":2,"a":false,"aa":[{"x":null,"y":true}],"z":1},"e":18446744073709551615,"é":0}"#
        );
        // No insignificant whitespace.
        assert_eq!(
            to_string(&json!([1, {"k": []}])).unwrap(),
            r#"[1,{"k":[]}]"#
        );
    }

    #[test]
    fn floats_rejected() {
        assert_eq!(
            to_vec(&json!({"a": [1, {"b": 1.5}]})),
            Err(IcjError::Float("/a/1/b".into()))
        );
        assert_eq!(to_vec(&json!(2.0)), Err(IcjError::Float("/".into())));
        assert!(to_vec(&json!({"a": i64::MIN, "b": u64::MAX})).is_ok());
    }

    #[test]
    fn canonical_check() {
        assert!(is_canonical(br#"{"a":1,"b":"x"}"#));
        assert!(!is_canonical(br#"{"b":1,"a":2}"#));
        assert!(!is_canonical(br#"{"a": 1}"#));
        assert!(!is_canonical(b"\"\\u00e9\"")); // non-ASCII must be raw
        assert!(!is_canonical(br#""\/""#));
    }

    /// T-CTX-3, second half: `serde_json` is built without `preserve_order`.
    #[test]
    fn no_preserve_order_feature() {
        // Behavioral check: insertion order is not kept.
        let mut m = serde_json::Map::new();
        m.insert("z".into(), json!(1));
        m.insert("a".into(), json!(2));
        assert_eq!(serde_json::to_string(&m).unwrap(), r#"{"a":2,"z":1}"#);
        // Build-graph check, when cargo is available (it is under `cargo test`).
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        let out = std::process::Command::new(cargo)
            .args([
                "tree",
                "--offline",
                "-e",
                "features",
                "-i",
                "serde_json",
                "--manifest-path",
                manifest,
            ])
            .output();
        if let Ok(out) = out
            && out.status.success()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(
                text.contains("serde_json"),
                "cargo tree printed no serde_json:\n{text}"
            );
            assert!(
                !text.contains("preserve_order"),
                "serde_json has preserve_order:\n{text}"
            );
        }
    }
}
