//! `init-testkit`: the $0 test kit: mock LLM, fake body, fake kernel, fake Root, scenario runner, measurements (testing.md §3.3-§3.8, §3.11). publish = false.
//!
//! | Module | What | Doc |
//! |---|---|---|
//! | [`mockllm`] | the deterministic mock model (`init-mockllm`) | testing.md §3.4 (WP2) |
//! | [`fakebody`] | `init-fakebody`: the kernel-facing contract of a body | testing.md §3.5; kernel/supervise.md §4.1, §4.4, §4.5 |
//! | [`fakekernel`] | `init-fakekernel`: the Root-facing contract of a kernel | testing.md §3.5a; root/root.md §4.1 |
//! | [`fakeroot`] | `init-fakeroot`: the kernel-facing contract of the Root, with the mock egress | testing.md §3.5b; root/root.md §3.5, §4.1, §4.2; root/egress.md §4 |
//! | [`scenario`] | the scenario runner and format, including Root steps | testing.md §3.6 |
//! | [`fault`] | the `INIT_FAULT` mechanism | testing.md §3.8.1 |
//! | [`guard`] | the live/disposable guard (sentinel file) | testing.md §3.1a, v03-decisions D1 |
//! | [`link`] | both ends of the control link (fd 3 lines, fd 4 fds) | root/root.md §4.1.1 |
//! | [`sys`] | the libc wrappers (the only `unsafe`) | — |
//! | [`watchdog`] | bounded test watchdog, leak-safe spawn/output for tests (K-FLAKE) | — |
//!
//! Every wire message goes through the `init-proto` types (WP1).

pub mod fakebody;
pub mod fakekernel;
pub mod fakeroot;
pub mod fault;
pub mod guard;
pub mod link;
pub mod matcher;
pub mod mockllm;
pub mod scenario;
pub mod sys;
pub mod util;
pub mod watchdog;

/// This crate's name, for diagnostics.
pub const CRATE: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_builds() {
        assert_eq!(super::CRATE, "init-testkit");
    }
}
