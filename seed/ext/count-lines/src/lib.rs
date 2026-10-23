//! count-lines: count the lines of a workspace file that contain a substring.
//!
//! Template for INIT T1 extensions (WIT init:ext@1, world `tool`).
//! Copy this directory to body/ext/<name>/, rename Cargo.toml.template to Cargo.toml,
//! and change the two lines marked CHANGE.
//! Logic lives in plain functions (testable with `cargo test` on the host);
//! the bindings exist only when building for wasm32.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use serde::Deserialize;
use serde_json::{Value, json};

const NAME: &str = "count-lines"; // CHANGE 2 of 2: must equal the directory name
const VERSION: &str = env!("CARGO_PKG_VERSION");

const DESCRIPTION: &str = "Count the lines of a workspace file that contain a substring. \
Returns {\"count\": n, \"lines\": [line numbers of the first 20 matches]}.";

const SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "path":   {"type": "string", "description": "File path relative to the workspace, e.g. \"src/main.rs\""},
    "needle": {"type": "string", "description": "Substring to look for, e.g. \"fn \""}
  },
  "required": ["path", "needle"],
  "additionalProperties": false
}"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: String,
    needle: String,
}

/// Pure logic: no I/O, so it is unit-testable natively.
fn count(text: &str, needle: &str) -> Value {
    let mut lines = Vec::new();
    let mut count: u64 = 0;
    for (i, line) in text.lines().enumerate() {
        if line.contains(needle) {
            count += 1;
            if lines.len() < 20 {
                lines.push(i + 1);
            }
        }
    }
    json!({"count": count, "lines": lines})
}

/// The tool: parse input, read the file under /work, run the logic.
fn run(input: &str) -> Result<String, Fail> {
    let input: Input = serde_json::from_str(input).map_err(|e| Fail::Input(e.to_string()))?;
    if input.path.starts_with('/') || input.path.split('/').any(|c| c == "..") {
        return Err(Fail::Input(format!("path must be relative to the workspace: {}", input.path)));
    }
    let full = format!("/work/{}", input.path);
    let text = std::fs::read_to_string(&full).map_err(|e| Fail::Run(format!("{full}: {e}")))?;
    Ok(count(&text, &input.needle).to_string())
}

enum Fail {
    Input(String),
    Run(String),
}

#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({
        path: "../../wit/init-ext/v1",
        world: "init:ext/tool",
    });

    use super::{DESCRIPTION, Fail, NAME, SCHEMA, VERSION};

    struct Component;

    impl Guest for Component {
        fn describe() -> ToolInfo {
            // Must be constant: no I/O, same value every time.
            ToolInfo {
                name: NAME.to_string(),
                description: DESCRIPTION.to_string(),
                input_schema: SCHEMA.to_string(),
                version: VERSION.to_string(),
                timeout_ms: None, // 30 s default; Some(ms) up to 600_000
            }
        }

        fn call(input: String) -> Result<String, ToolError> {
            super::run(&input).map_err(|f| match f {
                Fail::Input(m) => ToolError::InvalidInput(m),
                Fail::Run(m) => ToolError::Failed(m),
            })
        }
    }

    export!(Component);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_matching_lines() {
        let v = count("fn a\nlet x\nfn b\n", "fn ");
        assert_eq!(v, json!({"count": 2, "lines": [1, 3]}));
    }

    #[test]
    fn schema_is_an_object_schema_and_matches_input() {
        let s: Value = serde_json::from_str(SCHEMA).expect("SCHEMA is JSON");
        assert_eq!(s["type"], "object");
        // An input with exactly the schema's required properties must deserialize.
        // Adapt: if a required property is not a string, give it a valid example value.
        let mut example = serde_json::Map::new();
        for k in s["required"].as_array().unwrap() {
            example.insert(k.as_str().unwrap().to_string(), Value::String("x".into()));
        }
        let _: Input = serde_json::from_value(Value::Object(example)).expect("schema/Input drift");
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(matches!(run(r#"{"path":"a","needle":"b","extra":1}"#), Err(Fail::Input(_))));
    }
}
