//! The registry of loaded extensions and the `tool-info` validation rules (ext/host.md §3.7,
//! ext/wit.md §3.7).

use std::collections::BTreeMap;
use std::hash::Hasher;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::Value;

use super::bindings::v1;
use super::state::CallState;
use super::{CALL_TIMEOUT_DEFAULT_MS, CALL_TIMEOUT_MAX_MS, CALL_TIMEOUT_MIN_MS};

/// One registry generation. Readers clone the `Arc` under the host's mutex; a commit replaces it.
#[derive(Default)]
pub struct Registry {
    /// +1 on every commit.
    pub generation: u64,
    /// name → active version.
    pub exts: BTreeMap<String, Arc<ExtVersion>>,
    /// name → last load error (may coexist with an active older version).
    pub failed: BTreeMap<String, String>,
    /// Crates in `body/ext` with no artifact.
    pub not_built: Vec<String>,
}

/// One loaded, validated component. Shared through `Arc`: a call holds its version for its whole
/// life, so an old version is freed only when its last call ends (the drain, §3.7).
#[derive(Clone)]
pub struct ExtVersion {
    pub name: String,
    pub info: ToolInfoV,
    /// `init:ext` major (1).
    pub api: u32,
    pub origin: Origin,
    pub artifact: PathBuf,
    pub fp: Fingerprint,
    pub compile_ms: u32,
    pub loaded_at: SystemTime,
    pub(crate) pre: Pre,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// `bin/ext` (kernel-published, gate-built).
    Kept,
    /// `work/target` (agent-built in this session).
    Trial,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Kept => "kept",
            Origin::Trial => "trial",
        }
    }
}

/// One variant per supported `init:ext` major (`V2(..)` when that major ships).
#[derive(Clone)]
pub(crate) enum Pre {
    V1(v1::ToolPre<CallState>),
}

/// A validated, canonical `tool-info`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolInfoV {
    pub name: String,
    pub description: String,
    /// Parsed; serialized canonically (ICJ: sorted keys, compact, integers only).
    pub input_schema: Value,
    pub version: String,
    /// After defaulting and clamping.
    pub timeout_ms: u32,
}

impl ToolInfoV {
    pub fn schema_text(&self) -> String {
        self.input_schema.to_string()
    }
}

/// Change detection inside one process (§3.7): SipHash with fixed keys, no `sha2` dependency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub len: u64,
    pub mtime_ns: u128,
    pub hash: u64,
}

pub(crate) fn hash_bytes(b: &[u8]) -> u64 {
    // std's DefaultHasher::new(): fixed keys (0, 0) on purpose, stable within one process.
    let mut h = std::hash::DefaultHasher::new();
    h.write(b);
    h.finish()
}

/// The raw `tool-info` fields, as the guest returned them (`v1::ToolInfo` is not `PartialEq`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawInfo {
    pub name: String,
    pub description: String,
    pub input_schema: String,
    pub version: String,
    pub timeout_ms: Option<u32>,
}

impl From<v1::ToolInfo> for RawInfo {
    fn from(t: v1::ToolInfo) -> Self {
        RawInfo {
            name: t.name,
            description: t.description,
            input_schema: t.input_schema,
            version: t.version,
            timeout_ms: t.timeout_ms,
        }
    }
}

/// Names the body's own tools use; an extension may not take one.
pub const RESERVED_NAMES: &[&str] = &["ask_owner", "bash", "delegate", "edit", "ext", "read", "write"];
pub const DESCRIPTION_MAX: usize = 1024;
pub const SCHEMA_MAX: usize = 16_384;

/// JSON Schema keywords allowed in `input-schema` (ext/wit.md §3.7).
pub const SCHEMA_KEYWORDS: &[&str] = &[
    "additionalProperties",
    "const",
    "default",
    "description",
    "enum",
    "items",
    "maxItems",
    "maxLength",
    "maximum",
    "minItems",
    "minLength",
    "minimum",
    "properties",
    "required",
    "type",
];

/// `^[a-z][a-z0-9-]{0,31}$`, no `--`, no trailing `-`.
pub fn valid_name(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && !n.contains("--")
        && !n.ends_with('-')
}

/// `^[0-9]+\.[0-9]+\.[0-9]+$`
pub fn valid_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
}

/// ext/wit.md §3.7. `dir_name: None` skips the name-equals-directory check (`init-body ext
/// describe`). Returns the canonical info and notes for the reload report (timeout clamping).
pub(crate) fn validate(
    dir_name: Option<&str>,
    raw: &RawInfo,
) -> Result<(ToolInfoV, Vec<String>), String> {
    let n = &raw.name;
    if !valid_name(n) {
        return Err(format!(
            "tool-info.name {n:?} is invalid: must match ^[a-z][a-z0-9-]{{0,31}}$ with no \"--\" and no trailing \"-\""
        ));
    }
    if RESERVED_NAMES.contains(&n.as_str()) {
        return Err(format!(
            "tool-info.name {n:?} is reserved (read, write, edit, bash, ext)"
        ));
    }
    if let Some(d) = dir_name
        && d != n
    {
        return Err(format!(
            "tool-info.name {n:?} does not match directory {d:?}"
        ));
    }
    let dl = raw.description.len();
    if dl == 0 || dl > DESCRIPTION_MAX {
        return Err(format!(
            "tool-info.description is {dl} bytes; it must be 1 to {DESCRIPTION_MAX}"
        ));
    }
    let schema = check_schema_text(&raw.input_schema)?;
    if !valid_version(&raw.version) {
        return Err(format!(
            "tool-info.version {:?} is invalid: must be MAJOR.MINOR.PATCH",
            raw.version
        ));
    }
    let mut notes = Vec::new();
    let timeout_ms = match raw.timeout_ms {
        None => CALL_TIMEOUT_DEFAULT_MS,
        Some(t) => {
            let c = t.clamp(CALL_TIMEOUT_MIN_MS, CALL_TIMEOUT_MAX_MS);
            if c != t {
                notes.push(format!(
                    "timeout_ms {t} clamped to {c} (allowed {CALL_TIMEOUT_MIN_MS}..={CALL_TIMEOUT_MAX_MS})"
                ));
            }
            c
        }
    };
    Ok((
        ToolInfoV {
            name: n.clone(),
            description: raw.description.clone(),
            input_schema: schema,
            version: raw.version.clone(),
            timeout_ms,
        },
        notes,
    ))
}

/// Size, JSON, object root with `"type": "object"`, allowed keywords only, integers only.
pub(crate) fn check_schema_text(text: &str) -> Result<Value, String> {
    if text.len() > SCHEMA_MAX {
        return Err(format!(
            "tool-info.input-schema is {} bytes; the limit is {SCHEMA_MAX}",
            text.len()
        ));
    }
    let v: Value = serde_json::from_str(text)
        .map_err(|e| format!("tool-info.input-schema is not JSON: {e}"))?;
    match &v {
        Value::Object(m) if m.get("type") == Some(&Value::String("object".into())) => {}
        _ => {
            return Err(
                "tool-info.input-schema: the root must be an object schema with \"type\": \"object\""
                    .into(),
            );
        }
    }
    if let Some(p) = first_non_integer(&v, "") {
        return Err(format!(
            "tool-info.input-schema: number at {p} is not an integer (ICJ: integers only)"
        ));
    }
    check_schema_node(&v, "")?;
    Ok(v)
}

fn first_non_integer(v: &Value, at: &str) -> Option<String> {
    match v {
        Value::Number(n) if !(n.is_i64() || n.is_u64()) => {
            Some(if at.is_empty() { "/".into() } else { at.into() })
        }
        Value::Array(a) => a
            .iter()
            .enumerate()
            .find_map(|(i, x)| first_non_integer(x, &format!("{at}/{i}"))),
        Value::Object(m) => m
            .iter()
            .find_map(|(k, x)| first_non_integer(x, &format!("{at}/{k}"))),
        _ => None,
    }
}

/// Walks schema positions only: `properties` values, `items`, and `additionalProperties` when it
/// is a schema. Values of `enum`, `const` and `default` are data, not schemas.
fn check_schema_node(v: &Value, at: &str) -> Result<(), String> {
    let Value::Object(m) = v else {
        return Err(format!(
            "tool-info.input-schema: {} must be a schema object",
            if at.is_empty() { "/" } else { at }
        ));
    };
    for (k, x) in m {
        if !SCHEMA_KEYWORDS.contains(&k.as_str()) {
            return Err(format!(
                "tool-info.input-schema: keyword {k:?} at {} is not allowed (allowed: {})",
                if at.is_empty() { "/" } else { at },
                SCHEMA_KEYWORDS.join(", ")
            ));
        }
        let here = format!("{at}/{k}");
        match k.as_str() {
            "properties" => {
                let Value::Object(props) = x else {
                    return Err(format!("tool-info.input-schema: {here} must be an object"));
                };
                for (p, s) in props {
                    check_schema_node(s, &format!("{here}/{p}"))?;
                }
            }
            "items" => check_schema_node(x, &here)?,
            "additionalProperties" if !x.is_boolean() => check_schema_node(x, &here)?,
            "required" => {
                let ok = x.as_array().is_some_and(|a| a.iter().all(Value::is_string));
                if !ok {
                    return Err(format!(
                        "tool-info.input-schema: {here} must be an array of strings"
                    ));
                }
            }
            "type" => {
                let ok = x.is_string()
                    || x.as_array()
                        .is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_string));
                if !ok {
                    return Err(format!(
                        "tool-info.input-schema: {here} must be a string or an array of strings"
                    ));
                }
            }
            "description" if !x.is_string() => {
                return Err(format!("tool-info.input-schema: {here} must be a string"));
            }
            "enum" if !x.is_array() => {
                return Err(format!("tool-info.input-schema: {here} must be an array"));
            }
            "minimum" | "maximum" | "minLength" | "maxLength" | "minItems" | "maxItems"
                if !x.is_number() =>
            {
                return Err(format!("tool-info.input-schema: {here} must be an integer"));
            }
            _ => {}
        }
    }
    Ok(())
}
