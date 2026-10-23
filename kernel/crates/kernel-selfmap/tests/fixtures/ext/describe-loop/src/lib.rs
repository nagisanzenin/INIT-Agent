//! Fixture: `describe` never returns (selfmap.md S-09: fuel runs out).
#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({ path: "../../../../../../../wit/init-ext/v1", world: "init:ext/tool" });

    struct C;
    impl Guest for C {
        fn describe() -> ToolInfo {
            let mut i: u64 = 0;
            loop {
                i = std::hint::black_box(i.wrapping_add(1));
            }
        }
        fn call(input: String) -> Result<String, ToolError> {
            Ok(input)
        }
    }
    export!(C);
}
