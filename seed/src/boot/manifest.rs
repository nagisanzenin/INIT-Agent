//! `init-body --manifest` (body/boot.md §4.3): the body's static self-description for
//! kernel/selfmap.md, one ICJ line. It reads no env var and touches no file or socket, so its
//! output depends only on the binary.

use std::io::Write;
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::provider::models::SEED_MODELS;

/// `ext_api_majors`: the `init:ext` WIT majors the built-in host accepts (ext/wit.md §3.6).
pub const EXT_API_MAJORS: [u32; 1] = [1];

/// The env vars the body reads (§4.1), sorted.
pub const ENV: [&str; 12] = [
    "HOME",
    "INIT_BOOT_ID",
    "INIT_BOOT_REASON",
    "INIT_HOME",
    "INIT_KERNEL_SOCK",
    "INIT_KERNEL_VERSION",
    "INIT_MODE",
    "INIT_PROXY_SOCK",
    "INIT_SESSION",
    "INIT_SHA",
    "INIT_UI",
    "PATH",
];

pub const SUBCOMMANDS: [&str; 5] = [
    "--manifest",
    "--version",
    "ext call",
    "ext describe",
    "ext inspect",
];

/// The registry objects `{description, input_schema, name}`, sorted byte-wise by name.
pub fn tools() -> Vec<Value> {
    let mut t = crate::tools::spec_values();
    t.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .as_bytes()
            .cmp(b["name"].as_str().unwrap_or("").as_bytes())
    });
    t
}

/// `init-tools-v1` (kernel/proxy.md §3.12.2): `"sha256:" + hex(SHA-256(ICJ(tools sorted by name)))`.
pub fn tools_sha256(tools: &[Value]) -> String {
    let bytes = crate::log::icj::to_vec(&Value::Array(tools.to_vec()))
        .expect("the registry is ICJ (integers only)");
    crate::sha256::tagged(&bytes)
}

pub fn manifest() -> Value {
    let mut models: Vec<(&str, &str, &str)> = SEED_MODELS
        .iter()
        .map(|m| (m.api.as_str(), m.provider, m.id))
        .collect();
    models.sort();
    let tools = tools();
    json!({
        "body_version": env!("CARGO_PKG_VERSION"),
        "control_channel": super::env::UI,
        "env": ENV,
        "ext_api_majors": EXT_API_MAJORS,
        "manifest": 1,
        "modes": super::env::MODES,
        "models": models.iter().map(|(api, p, id)| json!({"api": api, "id": id, "provider": p})).collect::<Vec<_>>(),
        "subcommands": SUBCOMMANDS,
        "tools_sha256": tools_sha256(&tools),
        "tools": tools,
    })
}

pub fn print() -> ExitCode {
    let line = match crate::log::icj::to_string(&manifest()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}", json!({"type": "boot_error", "code": 70, "step": "manifest", "message": e.to_string()}));
            return ExitCode::from(70);
        }
    };
    let mut out = std::io::stdout().lock();
    if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
        return ExitCode::from(70);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §8 item 11: `tools` is tools.md §4.1 byte for byte (the registry file is the golden: WP12's
    /// copy of tools.md §4.1 in context/testdata) and `tools_sha256` is init-tools-v1 over it.
    #[test]
    fn manifest_tools_golden_and_hash() {
        let m = manifest();
        let golden: Value =
            serde_json::from_str(include_str!("../context/testdata/tools.json")).unwrap();
        assert_eq!(m["tools"], golden);
        let icj = crate::log::icj::to_string(&golden).unwrap();
        assert_eq!(icj.len(), 4_986, "tools.md §4.1 is 4,986 bytes minified");
        assert_eq!(
            m["tools_sha256"].as_str().unwrap(),
            crate::sha256::tagged(icj.as_bytes())
        );
        let names: Vec<&str> = m["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["bash", "delegate", "edit", "ext", "read", "write"]);
        // Sorted keys, one line.
        let line = crate::log::icj::to_string(&m).unwrap();
        assert!(!line.contains('\n'));
        assert!(crate::log::icj::is_canonical(line.as_bytes()));
        assert_eq!(m["models"].as_array().unwrap().len(), 7);
        assert_eq!(m["models"][0], json!({"api": "chat_completions", "id": "deepseek-flash", "provider": "deepseek"}));
        assert_eq!(m["models"][2], json!({"api": "chat_completions", "id": "mock-frontier-1", "provider": "mock"}));
        assert_eq!(m["models"][6], json!({"api": "responses", "id": "gpt-6.1-sol", "provider": "openai"}));
        assert_eq!(m["env"].as_array().unwrap().len(), 12);
        assert_eq!(m["ext_api_majors"], json!([1]));
    }
}
