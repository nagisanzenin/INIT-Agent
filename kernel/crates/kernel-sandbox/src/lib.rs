//! `kernel-sandbox`: Seatbelt (macOS) and bwrap/Landlock/seccomp (Linux) profiles, launcher and self-test (kernel/sandbox.md).
//!
//! WP0 skeleton. WP6a (macOS), WP6b (Linux) fills it in; its dependencies are already declared and vendored.

/// This crate's name, for diagnostics.
pub const CRATE: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_builds() {
        assert_eq!(super::CRATE, "kernel-sandbox");
    }
}
