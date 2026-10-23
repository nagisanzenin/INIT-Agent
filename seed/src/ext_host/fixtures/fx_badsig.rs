// WP14 test fixture: exports `call` with the wrong signature (u32 instead of string), so it
// cannot match init:ext@1 (host.md H10).
#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({
        inline: r#"
package fx:badsig@1.0.0;

world tool {
    record tool-info {
        name: string,
        description: string,
        input-schema: string,
        version: string,
        timeout-ms: option<u32>,
    }
    export describe: func() -> tool-info;
    export call: func(input: u32) -> string;
}
"#,
        world: "fx:badsig/tool",
    });

    struct Component;

    impl Guest for Component {
        fn describe() -> ToolInfo {
            ToolInfo {
                name: "fx-badsig".into(),
                description: "wrong call signature".into(),
                input_schema: r#"{"type":"object"}"#.into(),
                version: "0.1.0".into(),
                timeout_ms: None,
            }
        }
        fn call(input: u32) -> String {
            format!("{input}")
        }
    }

    export!(Component);
}
