//! `init-sandbox-profiles`: the sandbox profile texts that `init-root` compiles in, the Root's
//! composition of them, the parameter checks applied to kernel-supplied values, and the A6
//! sandbox self-test (WP6a).
//!
//! Sources (the docs are the spec):
//! - kernel/sandbox.md §3.3.1 (PRELUDE + `body` SECTION), §3.4 (`build`/`check`), §3.5 (`git-read`),
//!   §3.12 (class rules), §8 (the test suite);
//! - root/root.md §3.3.1 (composition, ROOT_ALLOWS), §3.3.2 (zone tail Z1–Z7, with Z2 per D9),
//!   §3.3.3 (`kernel` SECTION), §3.3.4 (V1–V6, with the `LINK_*` exemption of D8).
//!
//! Platform boundary: the SBPL texts, their composition and the self-test exist only on macOS
//! (`cfg(target_os = "macos")`). Linux has a fail-closed namespace/mount renderer; broad home/workspace grants remain unsupported.
//! [`params`] (V1–V5 and the class rules) is platform-neutral.
//!
//! Live-system safety (CONVENTIONS §1, D1): nothing in this crate sweeps, pattern-kills or signals
//! a process it did not spawn. The self-test signals only PIDs it holds.
#![deny(unsafe_code)] // only `sys` (FFI) carries #[allow(unsafe_code)]

pub mod access;
pub mod params;

#[cfg(unix)]
pub mod entry;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(unix)]
pub mod probe;

#[cfg(target_os = "macos")]
pub mod seatbelt;

#[cfg(target_os = "macos")]
pub mod selftest;

#[cfg(unix)]
mod sys;

/// A profile id, as named by the kernel's `spawn` request (root/root.md §4.1.3) or chosen by the Root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProfileId {
    /// `init-kernel supervise` and its helpers; Root-only (V5).
    Kernel,
    /// Live bodies, safe mode, probes and fleet members.
    Body,
    /// Candidate and stub builds.
    Build,
    /// Gate acceptance checks: the `build` text with different parameters.
    Check,
    /// Sandboxed `git-upload-pack` on agent repos.
    GitRead,
}

impl ProfileId {
    pub const ALL: [ProfileId; 5] = [ProfileId::Kernel, ProfileId::Body, ProfileId::Build, ProfileId::Check, ProfileId::GitRead];

    /// The wire id (kernel/sandbox.md §4.1).
    pub fn wire(self) -> &'static str {
        match self {
            ProfileId::Kernel => "kernel",
            ProfileId::Body => "body",
            ProfileId::Build => "build",
            ProfileId::Check => "check",
            ProfileId::GitRead => "git-read",
        }
    }

    pub fn from_wire(s: &str) -> Option<ProfileId> {
        ProfileId::ALL.into_iter().find(|p| p.wire() == s)
    }
}
