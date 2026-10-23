//! INIT canonical JSON (body/log-context.md §3.8.1) and SHA-256 helpers.
//!
//! ICJ is `serde_json::to_vec` of a `Value` (no `preserve_order`, so maps are sorted byte-wise) with
//! integers only. `self.json` is written in the pretty form of selfmap.md §4.1: the same keys and
//! escapes, 2-space indent, `\n` line ends and a final newline.

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in d.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// `"sha256:" + hex(SHA-256(bytes))`.
pub fn sha256_tag(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

/// The first float found in `v` (ICJ rule 4: integers only), as a JSON Pointer.
pub fn find_float(v: &Value) -> Option<String> {
    fn walk(v: &Value, ptr: &mut String) -> bool {
        match v {
            Value::Number(n) => !(n.is_i64() || n.is_u64()),
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    let len = ptr.len();
                    ptr.push_str(&format!("/{i}"));
                    if walk(x, ptr) {
                        return true;
                    }
                    ptr.truncate(len);
                }
                false
            }
            Value::Object(o) => {
                for (k, x) in o {
                    let len = ptr.len();
                    ptr.push('/');
                    ptr.push_str(&k.replace('~', "~0").replace('/', "~1"));
                    if walk(x, ptr) {
                        return true;
                    }
                    ptr.truncate(len);
                }
                false
            }
            _ => false,
        }
    }
    let mut p = String::new();
    walk(v, &mut p).then_some(p)
}

/// Compact ICJ bytes. Panics on a float: every caller builds its values from integers.
pub fn to_icj(v: &Value) -> Vec<u8> {
    if let Some(p) = find_float(v) {
        panic!("ICJ: float at {p}");
    }
    serde_json::to_vec(v).expect("a Value always serializes")
}

/// Compact ICJ text.
pub fn to_icj_string(v: &Value) -> String {
    String::from_utf8(to_icj(v)).expect("serde_json emits UTF-8")
}

/// Pretty ICJ (selfmap.md §4.1): sorted keys, 2-space indent, `\n`, final newline, integers only.
pub fn to_pretty(v: &Value) -> Vec<u8> {
    if let Some(p) = find_float(v) {
        panic!("ICJ: float at {p}");
    }
    let mut out = serde_json::to_vec_pretty(v).expect("a Value always serializes");
    out.push(b'\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sha_of_empty() {
        assert_eq!(
            sha256_tag(b""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn icj_sorts_keys_and_escapes_per_rule_3() {
        let v = json!({"b": 1, "a": "x/y\u{7f}é\u{1}\n"});
        assert_eq!(
            to_icj_string(&v),
            "{\"a\":\"x/y\u{7f}é\\u0001\\n\",\"b\":1}"
        );
    }

    #[test]
    fn pretty_form() {
        let v = json!({"b": [], "a": {"c": [1, 2]}});
        let s = String::from_utf8(to_pretty(&v)).unwrap();
        assert_eq!(
            s,
            "{\n  \"a\": {\n    \"c\": [\n      1,\n      2\n    ]\n  },\n  \"b\": []\n}\n"
        );
    }

    #[test]
    fn floats_are_found() {
        assert_eq!(
            find_float(&json!({"a": [1, {"b/c": 1.5}]})),
            Some("/a/1/b~1c".into())
        );
        assert_eq!(find_float(&json!({"a": [1, -2, u64::MAX]})), None);
    }
}
