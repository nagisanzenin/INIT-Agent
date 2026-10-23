//! `kernel-supervise`: the body supervisor under the Root (kernel/supervise.md v0.3).
//!
//! | Module | Contract |
//! |---|---|
//! | [`link`] | the control link, kernel side (§3.13; root/root.md §4.1) |
//! | [`instance`] | sandbox instances and the [`instance::Spawner`] seam (§3.4) |
//! | [`machine`] | the pure body-slot state machine (§3.3, §3.7) |
//! | [`supervisor`] | the driver: spawning, relay, readiness, heartbeats, handoff (§3.4–§3.10) |
//! | [`outbox`] | durable `lineage.append` with `idem` (§3.13.3, §4.7) |
//! | [`inbox`] | the durable owner inbox and kernel notices (§3.10) |
//! | [`bodyio`] | the three body pipes (§3.4 step 4, §3.10 stdout discipline) |
//! | [`binstore`] | `bin/` renames (§3.6.5) |
//! | [`extset`] | kept-component sets `bin/ext.d/<set>` and the `bin/ext*` links (§3.6.5, §3.6.8) |
//! | [`git`] | the hardened git prefix and lineage refs (§3.11) |
//! | [`build`] | the build slot and `cache/deps/` (§3.6.2) |
//! | [`proberoot`], [`probe`] | the staging probe (§3.6.3) |
//! | [`recovery`] | cold start from the Root's tuple (§3.9) |
//! | [`fleet`], [`kupgrade`] | L2 and L3 hooks (stubs, §3.14, §3.15) |
//! | [`promote`] | the T2 pipeline: G0–G4 driven through the gate core, handoff, LKG (§3.6; WP8b) |

// Nested `if let` reads closer to the doc's step lists than let-chains.
#![allow(clippy::collapsible_if)]

pub mod binstore;
pub mod bodyio;
pub mod build;
pub mod clock;
pub mod config;
pub mod extset;
pub mod fault;
pub mod fleet;
pub mod fsx;
pub mod git;
pub mod inbox;
pub mod install;
pub mod instance;
pub mod klog;
pub mod kupgrade;
pub mod layout;
pub mod lineage;
pub mod link;
pub mod machine;
pub mod outbox;
pub mod ownerline;
pub mod probe;
pub mod proberoot;
pub mod promote;
pub mod proxyctl;
pub mod recovery;
pub mod spawnspec;
pub mod supervisor;
pub mod sys;

#[cfg(test)]
pub(crate) mod testutil;

/// This crate's name, for diagnostics.
pub const CRATE: &str = env!("CARGO_PKG_NAME");

pub use supervisor::{main_under_root, main_under_root_with};
