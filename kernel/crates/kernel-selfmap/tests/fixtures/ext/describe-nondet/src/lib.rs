//! Fixture: `describe` returns different bytes on the second call (selfmap.md S-09).
#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({ path: "../../../../../../../wit/init-ext/v1", world: "init:ext/tool" });

    use std::sync::atomic::{AtomicU32, Ordering};
    static CALLS: AtomicU32 = AtomicU32::new(0);
    struct C;
    impl Guest for C {
        fn describe() -> ToolInfo {
            let n = CALLS.fetch_add(1, Ordering::SeqCst);
            ToolInfo {
                name: "describe-nondet".into(),
                description: format!("Call number {n}."),
                input_schema: r#"{"type":"object","properties":{"text":{"type":"string","description":"Text."}},"required":["text"],"additionalProperties":false}"#.into(),
                version: "0.1.0".into(),
                timeout_ms: None,
            }
        }
        fn call(input: String) -> Result<String, ToolError> {
            Ok(input)
        }
    }
    export!(C);
}
