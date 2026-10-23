//! Fixture: `describe` reads its schema from a file (selfmap.md S-09: no preopen, so it traps).
#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({ path: "../../../../../../../wit/init-ext/v1", world: "init:ext/tool" });

    struct C;
    impl Guest for C {
        fn describe() -> ToolInfo {
            let schema = std::fs::read_to_string("/work/schema.json").expect("read /work/schema.json");
            ToolInfo {
                name: "describe-reads-file".into(),
                description: "Reads a file in describe.".into(),
                input_schema: schema,
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
