//! Fixture: a well-behaved tool that echoes its input.
#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({ path: "../../../../../../../wit/init-ext/v1", world: "init:ext/tool" });

    struct C;
    impl Guest for C {
        fn describe() -> ToolInfo {
            ToolInfo {
                name: "echo".into(),
                description: "Return the input text unchanged.".into(),
                input_schema: r#"{"type":"object","properties":{"text":{"type":"string","description":"Text."}},"required":["text"],"additionalProperties":false}"#.into(),
                version: "0.1.0".into(),
                timeout_ms: Some(5000),
            }
        }
        fn call(input: String) -> Result<String, ToolError> {
            Ok(input)
        }
    }
    export!(C);
}
