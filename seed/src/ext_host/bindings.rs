//! `bindgen!` for `init:ext@1` and the per-major linker (ext/host.md §3.5).
//!
//! H0: the crate is `#![forbid(unsafe_code)]`; the macro's expansion contains `unsafe` blocks,
//! which rustc does not report for macros from other crates. This compiles (verified at WP14).

use wasmtime::Engine;
use wasmtime::component::{HasSelf, Linker};

use super::state::CallState;

pub mod v1 {
    wasmtime::component::bindgen!({
        path: "wit/init-ext/v1",
        world: "init:ext/tool",
    });
}

/// All of WASI p2 (sync) plus `init:ext/process@1.0.0`. `define_unknown_imports_as_traps` is not
/// used: a component that imports anything else (e.g. `wasi:http`) fails at load, naming it.
pub(crate) fn new_linker_v1(engine: &Engine) -> wasmtime::Result<Linker<CallState>> {
    let mut l: Linker<CallState> = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut l)?;
    v1::Tool::add_to_linker::<CallState, HasSelf<CallState>>(&mut l, |s| s)?;
    Ok(l)
}
