//! `init-proto`: kernel-side shared types (serde) for the initctl wire protocol, the kernel JSON
//! files, the body stdio protocol and the Root↔kernel control link.
//!
//! Never a dependency of the body (CONVENTIONS.md §2). Dependencies: `serde` and `serde_json`
//! only (never with `preserve_order`), so the Root workspace may also link it by path.
//!
//! | Module | Contract | Owner doc |
//! |---|---|---|
//! | [`icj`] | INIT canonical JSON v1: sorted keys, integers only; the `V1` version marker | body/log-context.md §3.8.1 |
//! | [`envelope`] | request / response / progress / event frames, line limits | kernel/initctl.md §3.1–§3.2, root/root.md §4.1.1 |
//! | [`errors`] | the `E_*` registry, CLI exit codes, typed `error.data` | kernel/initctl.md §4.2, §4.5 |
//! | [`initctl`] | every command's args and result, the lane matrix, state gating, the job model | kernel/initctl.md §3.4–§4.4 |
//! | [`files`] | `boot.json`, `lkg.json`, `update-failed.json`, `supervisor.json`, `ready`, `heartbeat`, `vault/lineage.jsonl` | kernel/supervise.md §4.3, §4.4, §4.7 |
//! | [`stdio`] | body stdio-jsonl/1 frames | kernel/supervise.md §4.5 |
//! | [`root_link`] | Root↔kernel control link messages, boot block, `root.*` codes | root/root.md §4.1, §4.5; root/egress.md §4.2 |
//!
//! Serialization rule: anything written to a wire, a file or a hash goes through
//! [`icj::to_vec`] (which builds a `serde_json::Value` first, so keys come out sorted).
//! Never `serde_json::to_vec` a derived struct directly: derive emits declaration order.

// Protocol error values (`ErrorBody`, `RequestReject`) are cold-path objects returned once
// per refused request; boxing them would only obscure the API.
#![allow(clippy::result_large_err)]

#[macro_use]
mod macros;

pub mod envelope;
pub mod errors;
pub mod files;
pub mod icj;
pub mod initctl;
pub mod root_link;
pub mod stdio;
pub mod validate;

pub use envelope::{ErrorBody, ErrorDetail, Frame, FrameError, ProgressFrame, Request, RequestReject, Response};
pub use errors::ErrorCode;
pub use icj::V1;

/// Re-exported so `initctl` stays "std + init-proto only" (CONVENTIONS §2) and uses the same
/// serde_json build.
pub use serde_json;

/// This crate's name, for diagnostics.
pub const CRATE: &str = env!("CARGO_PKG_NAME");

/// The initctl protocol version (`"v":1` on every frame; `proto` in `ping`).
pub const PROTO: u32 = 1;
/// The control-link protocol version (`hello.proto`).
pub const ROOT_PROTO: u32 = 1;
/// The body stdio protocol (`INIT_UI`).
pub const UI_PROTO: &str = "stdio-jsonl/1";

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name() {
        assert_eq!(super::CRATE, "init-proto");
    }
}
