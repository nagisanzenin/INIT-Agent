//! `describe()` of an extension component (selfmap.md §3.6 step 2; ext/host.md §3.15; ext/wit.md §3.7).
//!
//! The call never runs in the kernel process. [`DescribeRunner`] starts a child,
//! `init-kernel selfmap --describe <wasm>`, whose [`child_main`] hosts the component with Wasmtime
//! =48.0.6 and prints one ICJ line `{"api","info"}`. A Wasm sandbox escape then reaches only that child.
//!
//! The child's host links exactly what the body gives `describe` (ext/host.md §3.15): all of WASI p2
//! with an **empty** `WasiCtx` (no preopens, env, args or stdio; network uses denied) and
//! `init:ext/process@1.0.0` with every function returning `denied("describe has no capabilities")`.
//! Engine: fuel (200,000,000), Cranelift `OptLevel::None`, memory ≤ 64 MiB. `describe` is called twice
//! on the same instance and the two results must be identical.
//!
//! The parent validates and canonicalizes the result ([`validate`]): name, description, schema, version.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::icj::to_icj_string;
use crate::manifest::run_bounded;

/// The `bindgen!` module for `init:ext@1` (one module per supported major; ext/wit.md §3.6).
mod v1 {
    wasmtime::component::bindgen!({
        path: "../../../wit/init-ext/v1",
        world: "init:ext/tool",
    });
}

use v1::init::ext::process;

/// Fuel for instantiation plus both calls.
pub const FUEL: u64 = 200_000_000;
/// Linear-memory cap.
pub const MEMORY_MAX: usize = 64 << 20;
/// The text every `process` function returns inside `describe`.
pub const DENIED: &str = "describe has no capabilities";

/// The raw `tool-info` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: String,
    pub version: String,
    pub timeout_ms: Option<u32>,
}

impl RawToolInfo {
    pub fn to_value(&self) -> Value {
        json!({
            "description": self.description,
            "input_schema": self.input_schema,
            "name": self.name,
            "timeout_ms": self.timeout_ms,
            "version": self.version,
        })
    }

    pub fn from_value(v: &Value) -> Result<Self, String> {
        let s = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("describe child output lacks `{k}`"))
        };
        let timeout_ms = match v.get("timeout_ms") {
            None | Some(Value::Null) => None,
            Some(x) => Some(
                x.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or("describe child output: bad `timeout_ms`")?,
            ),
        };
        Ok(RawToolInfo {
            name: s("name")?,
            description: s("description")?,
            input_schema: s("input_schema")?,
            version: s("version")?,
            timeout_ms,
        })
    }
}

struct Host {
    wasi: wasmtime_wasi::WasiCtx,
    table: wasmtime::component::ResourceTable,
    limits: wasmtime::StoreLimits,
}

impl wasmtime_wasi::WasiView for Host {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

fn denied() -> process::ProcessError {
    process::ProcessError::Denied(DENIED.to_string())
}

impl process::Host for Host {
    fn run(
        &mut self,
        _req: process::RunRequest,
    ) -> Result<process::RunOutput, process::ProcessError> {
        Err(denied())
    }
    fn spawn(&mut self, _req: process::SpawnRequest) -> Result<u32, process::ProcessError> {
        Err(denied())
    }
    fn write(&mut self, _session: u32, _data: Vec<u8>) -> Result<(), process::ProcessError> {
        Err(denied())
    }
    fn read(
        &mut self,
        _session: u32,
        _wait_ms: u32,
        _until: Option<String>,
    ) -> Result<process::ReadResult, process::ProcessError> {
        Err(denied())
    }
    fn close(&mut self, _session: u32) -> Result<(), process::ProcessError> {
        Err(denied())
    }
    fn sessions(&mut self) -> Vec<u32> {
        Vec::new()
    }
}

fn to_raw(i: v1::ToolInfo) -> RawToolInfo {
    RawToolInfo {
        name: i.name,
        description: i.description,
        input_schema: i.input_schema,
        version: i.version,
        timeout_ms: i.timeout_ms,
    }
}

/// In-process `describe` (called only by [`child_main`]): returns the API (`init:ext@1`) and the info.
pub fn describe_in_process(bytes: &[u8]) -> Result<(String, RawToolInfo), String> {
    use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
    use wasmtime::{Config, Engine, OptLevel, Store, StoreLimitsBuilder};

    fn e(what: &'static str) -> impl Fn(wasmtime::Error) -> String {
        move |err| format!("{what}: {err:#}")
    }
    let mut cfg = Config::new();
    cfg.consume_fuel(true).cranelift_opt_level(OptLevel::None);
    let engine = Engine::new(&cfg).map_err(e("engine"))?;
    let component =
        Component::from_binary(&engine, bytes).map_err(e("the component does not compile"))?;
    let mut linker: Linker<Host> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(e("linker"))?;
    v1::Tool::add_to_linker::<Host, HasSelf<Host>>(&mut linker, |h: &mut Host| h)
        .map_err(e("linker"))?;
    let host = Host {
        wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
        table: ResourceTable::new(),
        limits: StoreLimitsBuilder::new().memory_size(MEMORY_MAX).build(),
    };
    let mut store = Store::new(&engine, host);
    store.limiter(|h| &mut h.limits);
    store.set_fuel(FUEL).map_err(e("fuel"))?;
    let tool = v1::Tool::instantiate(&mut store, &component, &linker)
        .map_err(e("instantiation failed"))?;
    let a = tool
        .call_describe(&mut store)
        .map_err(e("describe trapped"))?;
    let b = tool
        .call_describe(&mut store)
        .map_err(e("second describe trapped"))?;
    let (a, b) = (to_raw(a), to_raw(b));
    if a != b {
        return Err("describe is not deterministic: two calls returned different values".into());
    }
    Ok(("init:ext@1".to_string(), a))
}

/// Child mode (`init-kernel selfmap --describe <wasm>`): prints `{"api","info"}` as one ICJ line and
/// returns 0, or prints an `ext_contract` diagnostic to stderr and returns 4.
pub fn child_main(wasm: &Path) -> i32 {
    let result = std::fs::read(wasm)
        .map_err(|e| format!("cannot read {}: {e}", wasm.display()))
        .and_then(|b| describe_in_process(&b));
    match result {
        Ok((api, info)) => {
            println!(
                "{}",
                to_icj_string(&json!({"api": api, "info": info.to_value()}))
            );
            0
        }
        Err(e) => {
            let d = crate::diag::Diag::new(crate::diag::EXT_CONTRACT, e);
            eprintln!("{}", d.json_line());
            4
        }
    }
}

/// How the parent starts the describe child: `cmd[0] cmd[1..] --describe <wasm>`.
#[derive(Debug, Clone)]
pub struct DescribeRunner {
    pub cmd: Vec<OsString>,
    pub timeout: Duration,
}

impl DescribeRunner {
    pub fn new(cmd: Vec<OsString>) -> Self {
        DescribeRunner {
            cmd,
            timeout: Duration::from_secs(120),
        }
    }

    /// Runs the child with an empty environment in `cwd` and parses its answer.
    pub fn run(&self, wasm: &Path, cwd: &Path) -> Result<(String, RawToolInfo), String> {
        let (prog, args) = self.cmd.split_first().ok_or("empty describe command")?;
        let mut c = Command::new(prog);
        c.args(args)
            .arg("--describe")
            .arg(wasm)
            .env_clear()
            .current_dir(cwd);
        let o =
            run_bounded(c, self.timeout, 4 << 20).map_err(|e| format!("describe child: {e}"))?;
        if !o.status.success() {
            let err = String::from_utf8_lossy(&o.stderr);
            let msg = err
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .find_map(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| {
                    format!("describe child exited with {}: {}", o.status, err.trim())
                });
            return Err(msg);
        }
        let v: Value = serde_json::from_slice(&o.stdout)
            .map_err(|e| format!("describe child printed no JSON: {e}"))?;
        let api = v
            .get("api")
            .and_then(Value::as_str)
            .ok_or("describe child output lacks `api`")?
            .to_string();
        let info = RawToolInfo::from_value(v.get("info").unwrap_or(&Value::Null))?;
        Ok((api, info))
    }
}

/// Tool names an extension may not take (body/tools.md §4.1).
pub const CORE_TOOLS: [&str; 5] = ["bash", "edit", "ext", "read", "write"];

/// `^[a-z][a-z0-9-]{0,31}$`, no `--`, no trailing `-` (ext/wit.md §3.7).
pub fn valid_ext_name(n: &str) -> bool {
    let b = n.as_bytes();
    (1..=32).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !n.contains("--")
        && !n.ends_with('-')
}

const SCHEMA_KEYWORDS: [&str; 15] = [
    "type",
    "properties",
    "required",
    "items",
    "enum",
    "const",
    "description",
    "additionalProperties",
    "default",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
];

fn check_schema_node(v: &Value, ptr: &str) -> Result<(), String> {
    let Value::Object(o) = v else {
        return Err(format!("{ptr}: a schema must be an object"));
    };
    for (k, x) in o {
        if !SCHEMA_KEYWORDS.contains(&k.as_str()) {
            return Err(format!(
                "{ptr}: keyword `{k}` is not allowed (ext/wit.md §3.7)"
            ));
        }
        match k.as_str() {
            "properties" => {
                let Value::Object(props) = x else {
                    return Err(format!("{ptr}/properties: must be an object"));
                };
                for (p, s) in props {
                    check_schema_node(s, &format!("{ptr}/properties/{p}"))?;
                }
            }
            "items" => check_schema_node(x, &format!("{ptr}/items"))?,
            "additionalProperties" if !x.is_boolean() => {
                check_schema_node(x, &format!("{ptr}/additionalProperties"))?
            }
            "required" if !x.as_array().is_some_and(|a| a.iter().all(Value::is_string)) => {
                return Err(format!("{ptr}/required: must be an array of strings"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Validates and canonicalizes an input schema (ext/wit.md §3.7): ≤ 16 KiB, JSON, root object with
/// `"type": "object"`, allowed keywords only, integers only. Returns the parsed value (its ICJ form is
/// the canonical form).
pub fn canonical_schema(text: &str) -> Result<Value, String> {
    if text.len() > 16 * 1024 {
        return Err(format!("input-schema is {} bytes (max 16384)", text.len()));
    }
    let v: Value =
        serde_json::from_str(text).map_err(|e| format!("input-schema is not JSON: {e}"))?;
    if v.get("type").and_then(Value::as_str) != Some("object") {
        return Err("input-schema root must be an object schema with \"type\": \"object\"".into());
    }
    check_schema_node(&v, "")?;
    if let Some(p) = crate::icj::find_float(&v) {
        return Err(format!("input-schema has a non-integer number at {p}"));
    }
    Ok(v)
}

/// Validates a `describe` result for the extension in directory `dir_name` and returns the
/// `describe` object of `self.json` (selfmap.md §4.2). `timeout_ms` is kept as declared (null = default).
pub fn validate(info: &RawToolInfo, dir_name: &str) -> Result<Value, String> {
    if !valid_ext_name(&info.name) {
        return Err(format!(
            "name `{}` breaks ^[a-z][a-z0-9-]{{0,31}}$ (no --, no trailing -)",
            info.name
        ));
    }
    if CORE_TOOLS.contains(&info.name.as_str()) {
        return Err(format!("name `{}` is a core tool name", info.name));
    }
    if info.name != dir_name {
        return Err(format!(
            "name `{}` differs from the directory name `{dir_name}`",
            info.name
        ));
    }
    if !(1..=1024).contains(&info.description.len()) {
        return Err(format!(
            "description is {} bytes (1-1024)",
            info.description.len()
        ));
    }
    let schema = canonical_schema(&info.input_schema)?;
    let parts: Vec<&str> = info.version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(format!(
            "version `{}` is not MAJOR.MINOR.PATCH",
            info.version
        ));
    }
    let mut o = Map::new();
    o.insert(
        "description".into(),
        Value::String(info.description.clone()),
    );
    o.insert("input_schema".into(), schema);
    o.insert("name".into(), Value::String(info.name.clone()));
    o.insert(
        "timeout_ms".into(),
        info.timeout_ms.map(Value::from).unwrap_or(Value::Null),
    );
    o.insert("version".into(), Value::String(info.version.clone()));
    Ok(Value::Object(o))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, schema: &str) -> RawToolInfo {
        RawToolInfo {
            name: name.into(),
            description: "d".into(),
            input_schema: schema.into(),
            version: "0.1.0".into(),
            timeout_ms: None,
        }
    }

    #[test]
    fn validation_rules() {
        let ok = r#"{"type":"object","properties":{"a":{"type":"string","description":"x"}},"required":["a"],"additionalProperties":false}"#;
        let v = validate(&info("count-lines", ok), "count-lines").unwrap();
        assert_eq!(
            to_icj_string(&v["input_schema"]),
            r#"{"additionalProperties":false,"properties":{"a":{"description":"x","type":"string"}},"required":["a"],"type":"object"}"#
        );
        assert_eq!(v["timeout_ms"], Value::Null);
        assert!(validate(&info("count-lines", ok), "other").is_err());
        assert!(validate(&info("bash", ok), "bash").is_err());
        assert!(validate(&info("a--b", ok), "a--b").is_err());
        assert!(validate(&info("ab-", ok), "ab-").is_err());
        assert!(validate(&info("x", r##"{"type":"object","$ref":"#"}"##), "x").is_err());
        assert!(
            validate(
                &info("x", r#"{"type":"object","properties":{"a":{"oneOf":[]}}}"#),
                "x"
            )
            .is_err()
        );
        assert!(validate(&info("x", r#"{"type":"array"}"#), "x").is_err());
        assert!(
            validate(
                &info(
                    "x",
                    r#"{"type":"object","properties":{"a":{"maximum":1.5}}}"#
                ),
                "x"
            )
            .is_err()
        );
        assert!(
            validate(
                &info(
                    "x",
                    &format!(
                        "{{\"type\":\"object\",\"description\":\"{}\"}}",
                        "y".repeat(16400)
                    )
                ),
                "x"
            )
            .is_err()
        );
        // Property names are not keywords.
        assert!(
            validate(
                &info(
                    "x",
                    r#"{"type":"object","properties":{"oneOf":{"type":"string"}}}"#
                ),
                "x"
            )
            .is_ok()
        );
        let mut bad = info("x", ok);
        bad.version = "1.0".into();
        assert!(validate(&bad, "x").is_err());
        bad.version = "1.0.0".into();
        bad.description = String::new();
        assert!(validate(&bad, "x").is_err());
    }
}
