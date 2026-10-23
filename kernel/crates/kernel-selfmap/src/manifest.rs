//! Tools from `init-body --manifest` (selfmap.md §3.5, body/boot.md §4.3) and the `init-tools-v1` hash
//! (kernel/proxy.md §3.12.2).
//!
//! The binary is run as `env -i <binary> --manifest`: no environment, an empty scratch directory as cwd,
//! stdin closed, a 10 s timeout and at most 1 MiB of stdout. It must exit 0 and print exactly one line
//! of JSON. Until WP13 ships the real `--manifest`, callers may pass the manifest bytes directly
//! ([`ManifestSource::Json`], CLI `--manifest-json`); the same parse and checks apply.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::icj::{sha256_tag, to_icj, to_icj_string};

/// Where the manifest comes from.
#[derive(Debug, Clone)]
pub enum ManifestSource {
    /// Run `env -i <binary> --manifest`.
    Binary(PathBuf),
    /// The manifest's stdout bytes, given directly (stub until WP13; also used by tests).
    Json(Vec<u8>),
}

/// `--manifest` limits (selfmap.md §3.5).
pub const MANIFEST_TIMEOUT: Duration = Duration::from_secs(10);
pub const MANIFEST_MAX_STDOUT: usize = 1 << 20;

/// Output of a bounded child run.
pub struct ChildOutput {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs `cmd` with stdin closed, stdout/stderr piped, killing it (by its own PID) at `timeout` or when
/// stdout passes `max_stdout`. Only the child spawned here is ever signalled.
pub fn run_bounded(
    mut cmd: Command,
    timeout: Duration,
    max_stdout: usize,
) -> Result<ChildOutput, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("cannot start: {e}"))?;
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let cap = max_stdout as u64 + 1;
    let out_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = (&mut out).take(cap).read_to_end(&mut v);
        // Drain the rest so the child never blocks on a full pipe; it is killed below if too large.
        let _ = std::io::copy(&mut out, &mut std::io::sink());
        v
    });
    let err_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = (&mut err).take(64 * 1024).read_to_end(&mut v);
        let _ = std::io::copy(&mut err, &mut std::io::sink());
        v
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(format!("wait failed: {e}")),
        }
    };
    let stdout = out_t.join().unwrap_or_default();
    let stderr = err_t.join().unwrap_or_default();
    let Some(status) = status else {
        return Err(format!("timed out after {} ms", timeout.as_millis()));
    };
    if stdout.len() > max_stdout {
        return Err(format!("stdout is larger than {max_stdout} bytes"));
    }
    Ok(ChildOutput {
        status,
        stdout,
        stderr,
    })
}

/// Runs `env -i <binary> --manifest` in the empty directory `cwd` and returns its stdout.
pub fn run_manifest(binary: &Path, cwd: &Path) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new(binary);
    cmd.arg("--manifest").env_clear().current_dir(cwd);
    let o = run_bounded(cmd, MANIFEST_TIMEOUT, MANIFEST_MAX_STDOUT)
        .map_err(|e| format!("{} --manifest: {e}", binary.display()))?;
    if !o.status.success() {
        return Err(format!(
            "{} --manifest exited with {}: {}",
            binary.display(),
            o.status,
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    Ok(o.stdout)
}

/// Parses the manifest: exactly one line of JSON (an object with a `tools` array and an integer
/// `manifest`), optionally followed by `\n`.
pub fn parse_manifest(stdout: &[u8]) -> Result<Map<String, Value>, String> {
    let body = stdout.strip_suffix(b"\n").unwrap_or(stdout);
    if body.contains(&b'\n') {
        return Err("--manifest printed more than one line".into());
    }
    let v: Value =
        serde_json::from_slice(body).map_err(|e| format!("--manifest is not JSON: {e}"))?;
    let Value::Object(m) = v else {
        return Err("--manifest is not a JSON object".into());
    };
    if !m.get("manifest").is_some_and(|x| x.is_u64()) {
        return Err("--manifest lacks the integer field `manifest`".into());
    }
    if !m.get("tools").is_some_and(Value::is_array) {
        return Err("--manifest lacks the array field `tools`".into());
    }
    Ok(m)
}

/// Normalizes one registry tool to `{"description","input_schema","name"}` (proxy.md §3.12.2: a missing
/// description is `""`, a missing schema is `{}`; other keys are dropped).
pub fn normalize_tool(name: &str, description: Option<&Value>, schema: Option<&Value>) -> Value {
    json!({
        "description": description.cloned().unwrap_or(Value::String(String::new())),
        "input_schema": schema.cloned().unwrap_or(json!({})),
        "name": name,
    })
}

/// The registry tools of a manifest, normalized and sorted byte-wise by name.
pub fn manifest_tools(m: &Map<String, Value>) -> Result<Vec<Value>, String> {
    let mut v = Vec::new();
    for (i, t) in m["tools"].as_array().expect("checked").iter().enumerate() {
        let name = t
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("manifest tools[{i}] has no string `name`"))?;
        v.push(normalize_tool(
            name,
            t.get("description"),
            t.get("input_schema"),
        ));
    }
    sort_by_name(&mut v);
    Ok(v)
}

fn sort_by_name(v: &mut [Value]) {
    v.sort_by(|a, b| {
        let n = |x: &Value| x["name"].as_str().unwrap_or("").to_string();
        n(a).cmp(&n(b))
    });
}

/// `init-tools-v1`: `"sha256:" + hex(SHA-256(ICJ([normalized tools sorted byte-wise by name])))`.
pub fn tools_sha256(normalized_sorted: &[Value]) -> String {
    sha256_tag(&to_icj(&Value::Array(normalized_sorted.to_vec())))
}

/// `schema_sha256(tool) = "sha256:" + hex(SHA-256(ICJ(tool)))`.
pub fn schema_sha256(tool: &Value) -> String {
    sha256_tag(&to_icj(tool))
}

/// A provider rendering of the tool list in a model request (kernel/proxy.md §3.12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    OpenAiChat,
    Anthropic,
    OpenAiResponses,
}

/// Normalizes the `tools` array of a request to the registry shape, sorted by name, so that
/// [`tools_sha256`] of the result is comparable with the manifest's (shared vectors with proxy P-13).
pub fn normalize_request_tools(family: Family, tools: &Value) -> Result<Vec<Value>, String> {
    let arr = tools.as_array().ok_or("`tools` is not an array")?;
    let mut v = Vec::new();
    for (i, t) in arr.iter().enumerate() {
        let name_of = |o: &Value| {
            o.get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("tools[{i}] has no string `name`"))
        };
        let n = match family {
            Family::OpenAiChat => {
                let f = t
                    .get("function")
                    .ok_or_else(|| format!("tools[{i}] has no `function`"))?;
                normalize_tool(&name_of(f)?, f.get("description"), f.get("parameters"))
            }
            Family::Anthropic => {
                normalize_tool(&name_of(t)?, t.get("description"), t.get("input_schema"))
            }
            Family::OpenAiResponses => match t.get("type").and_then(Value::as_str) {
                Some("custom") => {
                    let schema = json!({"custom": t.get("format").cloned().unwrap_or(Value::Null)});
                    normalize_tool(&name_of(t)?, t.get("description"), Some(&schema))
                }
                _ => normalize_tool(&name_of(t)?, t.get("description"), t.get("parameters")),
            },
        };
        v.push(n);
    }
    sort_by_name(&mut v);
    Ok(v)
}

/// The `tools` object of `self.json` (`{sha256, list}`) from normalized, sorted tools; each entry adds
/// `schema_sha256` and `tokens` (tokens of the tool's ICJ text, what it costs in the prefix).
pub fn tools_object(tools: &[Value], tok: &crate::tokens::Tokenizer) -> Value {
    let list: Vec<Value> = tools
        .iter()
        .map(|t| {
            let mut o = t.as_object().expect("normalized").clone();
            o.insert("schema_sha256".into(), Value::String(schema_sha256(t)));
            o.insert("tokens".into(), Value::from(tok.count(&to_icj_string(t))));
            Value::Object(o)
        })
        .collect();
    json!({"list": list, "sha256": tools_sha256(tools)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parse_rules() {
        assert!(parse_manifest(b"{\"manifest\":1,\"tools\":[]}\n").is_ok());
        assert!(parse_manifest(b"{\"manifest\":1,\"tools\":[]}").is_ok());
        assert!(parse_manifest(b"log line\n{\"manifest\":1,\"tools\":[]}\n").is_err());
        assert!(parse_manifest(b"{\"manifest\":1,\"tools\":[]}\n\n").is_err());
        assert!(parse_manifest(b"{\"tools\":[]}\n").is_err());
        assert!(parse_manifest(b"{\"manifest\":1}\n").is_err());
        assert!(parse_manifest(b"[]\n").is_err());
    }

    #[test]
    fn normalization_defaults_and_order() {
        let m = parse_manifest(
            br#"{"manifest":1,"tools":[{"name":"b","extra":1},{"name":"a","description":"d","input_schema":{"type":"object"}}]}"#,
        )
        .unwrap();
        let t = manifest_tools(&m).unwrap();
        assert_eq!(
            to_icj_string(&Value::Array(t.clone())),
            r#"[{"description":"d","input_schema":{"type":"object"},"name":"a"},{"description":"","input_schema":{},"name":"b"}]"#
        );
        assert_eq!(
            tools_sha256(&t),
            sha256_tag(to_icj_string(&Value::Array(t.clone())).as_bytes())
        );
    }
}
