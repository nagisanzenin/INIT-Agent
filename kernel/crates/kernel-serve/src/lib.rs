//! `kernel-serve`: the initctl server (kernel/initctl.md v0.3).
//!
//! | Module | Contract |
//! |---|---|
//! | [`server`] | JSON lines over `run/kernel.sock` and the member and probe lane sockets (§3.1), the envelope (§3.2), one request at a time per connection, progress frames, the 1 MiB response cap (§5) |
//! | [`peer`] | peer credentials and lane attribution (§3.3) |
//! | [`limits`] | 8 connections, 10 req/s (burst 20) and a 30 s idle timeout per lane (§3.1) |
//! | [`gating`] | the lane matrix (§3.4) and kernel state gating (§3.5), pure |
//! | [`jobs`] | the job model, records, milestones and `wait` (§3.6, §4.3, §4.4) |
//! | [`quota`] | `vendor_day`, `proposals_day`, `task_submits_day`, `fleet_spawns_day` (§4.2) |
//! | [`paths`] | path-argument hardening (§7) |
//! | [`propose`] | `kernel propose` files (§4.1) |
//! | [`vendor`] | `vendor` manifest steps 1–2 (§4.1) |
//! | [`backend`] | the [`Backend`] trait kernel-supervise and kernel-gate implement |
//!
//! Wiring (kernel-supervise):
//! ```ignore
//! let early = EarlyLane::bind(LaneSpec::live(&home), Limits::default())?; // right after run/kernel.lock:
//!                                                             // E_BUSY{reason:"starting"} until adopted
//! let srv = kernel_serve::start(ServeConfig::new(&home, VERSION), backend, Arc::new(SandboxPolicy { canary }))?;
//! srv.adopt_lane(early)?;                                     // same socket, now served
//! // (or srv.add_lane(LaneSpec::live(&home))? without an early lane)
//! srv.add_lane(LaneSpec::probe(&home, "p7", ProbePurpose::Probe))?; // per probe instance
//! srv.remove_lane("probe:p7")?;
//! srv.shutdown();                                             // Stopping / exit
//! ```

#![allow(clippy::result_large_err)]

pub mod backend;
pub mod clock;
pub mod fit;
pub mod fsx;
pub mod gating;
pub mod jobs;
pub mod limits;
pub mod paths;
pub mod peer;
pub mod propose;
pub mod quota;
pub mod server;
pub mod vendor;

pub use backend::{Backend, Begun, BodyCount, BusyInfo, KernelView, LaneCtx, LaneSpec, LedgerSnapshot, not_implemented};
pub use jobs::{JobConfig, JobCtl, JobGone, JobStore};
pub use limits::Limits;
pub use peer::{Peer, PeerPolicy, SameUidPolicy, SandboxPolicy, peer_of};
pub use quota::QuotaLimits;
pub use server::{EarlyLane, STARTING, ServeConfig, ServerHandle, start, starting_error};

/// This crate's name, for diagnostics.
pub const CRATE: &str = env!("CARGO_PKG_NAME");
