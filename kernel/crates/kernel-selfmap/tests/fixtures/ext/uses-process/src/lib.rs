//! Fixture: a tool whose `call` runs a process, so it imports `init:ext/process` (ext/wit.md W-3).
#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({ path: "../../../../../../../wit/init-ext/v1", world: "init:ext/tool" });

    use init::ext::process;
    struct C;
    impl Guest for C {
        fn describe() -> ToolInfo {
            ToolInfo {
                name: "uses-process".into(),
                description: "Run echo through the host.".into(),
                input_schema: r#"{"type":"object","properties":{"text":{"type":"string","description":"Text."}},"required":["text"],"additionalProperties":false}"#.into(),
                version: "0.2.0".into(),
                timeout_ms: None,
            }
        }
        fn call(input: String) -> Result<String, ToolError> {
            let req = process::RunRequest {
                argv: vec!["echo".into(), input],
                cwd: "/work".into(),
                env: vec![],
                stdin: vec![],
                timeout_ms: 1000,
            };
            match process::run(&req) {
                Ok(o) => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
                Err(_) => Err(ToolError::Failed("run failed".into())),
            }
        }
    }
    export!(C);
}
