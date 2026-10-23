// WP14 test fixture: imports an interface the host does not provide (a stand-in for
// `wasi:http`), so it must fail at load, naming the import (host.md H10, ext/wit.md §3.4).
#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({
        inline: r#"
package fx:badimport@1.0.0;

interface outgoing-handler {
    handle: func(x: u32) -> u32;
}

world tool {
    import outgoing-handler;
    record tool-info {
        name: string,
        description: string,
        input-schema: string,
        version: string,
        timeout-ms: option<u32>,
    }
    variant tool-error {
        invalid-input(string),
        failed(string),
    }
    export describe: func() -> tool-info;
    export call: func(input: string) -> result<string, tool-error>;
}
"#,
        world: "fx:badimport/tool",
    });

    struct Component;

    impl Guest for Component {
        fn describe() -> ToolInfo {
            ToolInfo {
                name: "fx-badimport".into(),
                description: "imports fx:badimport/outgoing-handler".into(),
                input_schema: r#"{"type":"object"}"#.into(),
                version: "0.1.0".into(),
                timeout_ms: None,
            }
        }
        fn call(_input: String) -> Result<String, ToolError> {
            Ok(format!("{}", fx::badimport::outgoing_handler::handle(1)))
        }
    }

    export!(Component);
}
