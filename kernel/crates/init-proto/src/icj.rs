//! INIT canonical JSON v1 (body/log-context.md §3.8.1).
//!
//! 1. UTF-8, no BOM, no insignificant whitespace.
//! 2. Object keys unique and sorted byte-wise (`serde_json::Map` without `preserve_order`).
//! 3. serde_json 1.0.154's escaping (`/`, U+007F and non-ASCII raw; control chars `\u00xx` lowercase).
//! 4. Integers only: no floats anywhere.
//! 5. `true`, `false`, `null` literals.
//!
//! Every encoder here goes through `serde_json::Value`, because `#[derive(Serialize)]` emits fields
//! in declaration order, not sorted.

use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::fmt;

/// JSON nesting cap for requests (kernel/initctl.md §7 "JSON depth is capped at 32").
pub const MAX_DEPTH: usize = 32;

/// Why a value is not ICJ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IcjError {
    /// The value could not be serialized or parsed at all.
    Json(String),
    /// A float was found at this JSON pointer.
    Float(String),
    /// Nesting deeper than the cap.
    TooDeep(usize),
}

impl fmt::Display for IcjError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IcjError::Json(e) => write!(f, "json: {e}"),
            IcjError::Float(p) => write!(f, "float at {p} (ICJ: integers only)"),
            IcjError::TooDeep(d) => write!(f, "nesting depth {d} exceeds {MAX_DEPTH}"),
        }
    }
}

impl std::error::Error for IcjError {}

impl From<serde_json::Error> for IcjError {
    fn from(e: serde_json::Error) -> Self {
        IcjError::Json(e.to_string())
    }
}

/// Serialize to a `Value` (keys sorted) and check it is integers-only.
pub fn to_value<T: Serialize + ?Sized>(x: &T) -> Result<Value, IcjError> {
    let v = serde_json::to_value(x)?;
    check_integers_only(&v)?;
    Ok(v)
}

/// ICJ bytes of `x` (no trailing newline).
pub fn to_vec<T: Serialize + ?Sized>(x: &T) -> Result<Vec<u8>, IcjError> {
    let v = to_value(x)?;
    Ok(serde_json::to_vec(&v)?)
}

/// ICJ text of `x` (no trailing newline).
pub fn to_string<T: Serialize + ?Sized>(x: &T) -> Result<String, IcjError> {
    let v = to_value(x)?;
    Ok(serde_json::to_string(&v)?)
}

/// ICJ bytes of `x` followed by `\n`: one JSON-lines frame.
pub fn to_line<T: Serialize + ?Sized>(x: &T) -> Result<Vec<u8>, IcjError> {
    let mut b = to_vec(x)?;
    b.push(b'\n');
    Ok(b)
}

/// Parse any JSON text and re-serialize it as ICJ (fails on floats).
pub fn canonicalize(bytes: &[u8]) -> Result<Vec<u8>, IcjError> {
    let v: Value = serde_json::from_slice(bytes)?;
    check_integers_only(&v)?;
    Ok(serde_json::to_vec(&v)?)
}

/// True when `bytes` is exactly its own ICJ serialization.
pub fn is_canonical(bytes: &[u8]) -> bool {
    matches!(canonicalize(bytes), Ok(c) if c == bytes)
}

/// Parse `bytes` (one line, without the newline) into `T`, checking ICJ rule 4 (integers only)
/// and the nesting cap first.
pub fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, IcjError> {
    let v: Value = serde_json::from_slice(bytes)?;
    let d = depth(&v);
    if d > MAX_DEPTH {
        return Err(IcjError::TooDeep(d));
    }
    check_integers_only(&v)?;
    Ok(serde_json::from_value(v)?)
}

/// Fail on the first float (ICJ rule 4), reporting its JSON pointer.
pub fn check_integers_only(v: &Value) -> Result<(), IcjError> {
    fn walk(v: &Value, path: &mut String) -> Result<(), IcjError> {
        match v {
            Value::Number(n) if !(n.is_i64() || n.is_u64()) => Err(IcjError::Float(if path.is_empty() {
                "/".to_string()
            } else {
                path.clone()
            })),
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    let len = path.len();
                    path.push('/');
                    path.push_str(&i.to_string());
                    walk(x, path)?;
                    path.truncate(len);
                }
                Ok(())
            }
            Value::Object(m) => {
                for (k, x) in m {
                    let len = path.len();
                    path.push('/');
                    path.push_str(&k.replace('~', "~0").replace('/', "~1"));
                    walk(x, path)?;
                    path.truncate(len);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    walk(v, &mut String::new())
}

/// Nesting depth: a scalar is 0, `{}` / `[]` is 1.
pub fn depth(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(m) => 1 + m.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// The `"v":1` marker carried by every frame and every kernel state file.
///
/// Serializes as `1`; deserializing anything else fails with an error whose text starts with
/// `unsupported version` (testing.md §3.2: "`v` mismatch error"). A struct with `pub v: V1`
/// therefore cannot be built from, or written as, another version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct V1;

/// The text every version error starts with.
pub const UNSUPPORTED_VERSION: &str = "unsupported version";

impl Serialize for V1 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(1)
    }
}

impl<'de> Deserialize<'de> for V1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        match v.as_u64() {
            Some(1) => Ok(V1),
            _ => Err(D::Error::custom(format!("{UNSUPPORTED_VERSION}: v={v}, expected 1"))),
        }
    }
}

/// A frame body plus `"v":1`, for tagged frame enums that do not carry `v` themselves
/// (the stdio frames, control-link events). Serializing adds `"v":1` to the object;
/// deserializing requires it and strips it before handing the rest to `T`.
#[derive(Clone, Debug, PartialEq)]
pub struct Versioned<T>(pub T);

impl<T: Serialize> Serialize for Versioned<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error as _;
        let mut v = serde_json::to_value(&self.0).map_err(S::Error::custom)?;
        match v.as_object_mut() {
            Some(m) => {
                m.insert("v".to_string(), Value::from(1u32));
            }
            None => return Err(S::Error::custom("a versioned frame must be a JSON object")),
        }
        v.serialize(s)
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Versioned<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut v = Value::deserialize(d)?;
        let m = v
            .as_object_mut()
            .ok_or_else(|| D::Error::custom("a versioned frame must be a JSON object"))?;
        match m.remove("v") {
            Some(x) if x.as_u64() == Some(1) => {}
            Some(x) => return Err(D::Error::custom(format!("{UNSUPPORTED_VERSION}: v={x}, expected 1"))),
            None => return Err(D::Error::custom("missing field `v`")),
        }
        T::deserialize(v).map(Versioned).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorted_keys_and_escapes() {
        // T-CTX-3-style golden vectors: control chars, '/', U+007F, non-ASCII, nested key order.
        let v = json!({"z": 1, "a": {"y": "\u{0001}\u{0008}\t\n\u{000c}\r\u{001f}", "b": "/\u{007f}é😀\"\\"}, "M": [true, false, null]});
        let b = to_vec(&v).unwrap();
        assert_eq!(
            String::from_utf8(b).unwrap(),
            "{\"M\":[true,false,null],\"a\":{\"b\":\"/\u{7f}é😀\\\"\\\\\",\"y\":\"\\u0001\\b\\t\\n\\f\\r\\u001f\"},\"z\":1}"
        );
    }

    #[test]
    fn struct_fields_come_out_sorted() {
        #[derive(Serialize)]
        struct S {
            zeta: u8,
            alpha: u8,
        }
        assert_eq!(to_string(&S { zeta: 1, alpha: 2 }).unwrap(), r#"{"alpha":2,"zeta":1}"#);
    }

    #[test]
    fn floats_rejected() {
        assert_eq!(
            to_vec(&json!({"a": [1, {"b": 1.5}]})).unwrap_err(),
            IcjError::Float("/a/1/b".into())
        );
        assert!(canonicalize(b"{\"a\":1.0}").is_err());
        assert!(to_vec(&json!({"a": u64::MAX, "b": i64::MIN})).is_ok());
    }

    #[test]
    fn canonical_detection() {
        assert!(is_canonical(br#"{"a":1,"b":[1,2]}"#));
        assert!(!is_canonical(br#"{"b":1,"a":2}"#));
        assert!(!is_canonical(br#"{"a": 1}"#));
    }

    #[test]
    fn depth_cap() {
        let mut s = String::new();
        for _ in 0..33 {
            s.push('[');
        }
        for _ in 0..33 {
            s.push(']');
        }
        assert_eq!(from_slice::<Value>(s.as_bytes()).unwrap_err(), IcjError::TooDeep(33));
        assert_eq!(depth(&json!(1)), 0);
        assert_eq!(depth(&json!({"a": [1]})), 2);
    }

    #[test]
    fn v1_marker() {
        assert_eq!(to_string(&V1).unwrap(), "1");
        assert!(serde_json::from_str::<V1>("1").is_ok());
        let e = serde_json::from_str::<V1>("2").unwrap_err().to_string();
        assert!(e.starts_with(UNSUPPORTED_VERSION), "{e}");
    }

    #[test]
    fn versioned_wrapper() {
        #[derive(Serialize, Deserialize, Debug, PartialEq)]
        #[serde(tag = "t", rename_all = "snake_case")]
        enum F {
            Ack { seq: u64 },
        }
        let s = to_string(&Versioned(F::Ack { seq: 3 })).unwrap();
        assert_eq!(s, r#"{"seq":3,"t":"ack","v":1}"#);
        let back: Versioned<F> = serde_json::from_str(&s).unwrap();
        assert_eq!(back.0, F::Ack { seq: 3 });
        let e = serde_json::from_str::<Versioned<F>>(r#"{"seq":3,"t":"ack","v":2}"#).unwrap_err();
        assert!(e.to_string().starts_with(UNSUPPORTED_VERSION));
        assert!(serde_json::from_str::<Versioned<F>>(r#"{"seq":3,"t":"ack"}"#).is_err());
    }
}
