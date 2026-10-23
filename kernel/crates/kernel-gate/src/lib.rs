#![forbid(unsafe_code)]
//! `kernel-gate`: the evolution gate (kernel/gate.md) and the commit validator (evolution.md §3.8–§3.10).
//!
//! WP8 core (lane K2). What lives here needs no running body:
//! - [`scope`]: the scope decision from the changed paths and the lockfile dependency rule (gate.md §3.1);
//! - [`g0`]: the G0 command list, its environment, the build-config policy and the shape checks (gate.md §3.3);
//! - [`g1`]: the G1 drift comparison of committed fields (selfmap.md §3.8, v03-decisions D12) and the
//!   WIT copy drift check (gate.md §3.4 item 3);
//! - [`stats`]: the noise floor δ(m), the paired cluster-bootstrap non-inferiority test, the futility look
//!   and the capability test, all seeded deterministically (gate.md §3.8);
//! - [`tasks`]: agent-authored task families, their eligibility (D3, K = 5 promotions) and the G4 task
//!   selection with the sealed half (D2) (gate.md §3.7.2, §3.7.5);
//! - [`accept`]: the v0.3 accept rule, `cost_guard`, `audit_required` and the mechanical resolution of
//!   inconclusive verdicts (gate.md §3.9, §3.12; v03-decisions D4);
//! - [`evo`]: the `evo` commit validator (evolution.md §3.8–§3.10.1).
//!
//! - [`g2`]: the G2 pass criteria on what the two probes and the proxy observed (gate.md §3.5);
//! - [`g3`]: the G3 instruction and the verification of the fetched self-edit (gate.md §3.6);
//! - [`g4`]: the G4 run driver: schedule, futility look, infra re-run, top-up (gate.md §3.7);
//! - [`record`]: the gate record's outward copies: Root subset, note, report (gate.md §3.13).
//! - [`g4v2`]: G4 v2's kernel half (M7, lean): tiers and pre-flight, the `sealed.g4_slice` wire types,
//!   the screen summary, `accept_v2` and the G4 record v3 keys (m7-design.md §3.2, §4).
//! - [`judge`]: `init-kernel gate judge` (gate.md §4.1a): a full gate on an isolated home copy, for the
//!   Root's gate-of-gates; no traces, deterministic, verdict JSON to `--out` (WP8c).
//!
//! The process side (probes, the build slot, the sandboxed fetch, the handoff) is
//! `kernel-supervise::promote` (WP8b).

pub mod accept;
pub mod caps;
pub mod evo;
pub mod g0;
pub mod g1;
pub mod g2;
pub mod g3;
pub mod g4;
pub mod g4v2;
pub mod judge;
pub mod pool_cache;
pub mod record;
pub mod retain;
pub mod scope;
pub mod screen;
pub mod stats;
pub mod tasks;

/// This crate's name, for diagnostics.
pub const CRATE: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh, empty directory under `kernel/target/test-homes/WP8/` (BUILD_AGENT_BRIEF: tests never
    /// touch anything outside the repo). The caller removes it with [`remove_home`] by exact path.
    pub fn test_home(name: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-homes/WP8")
            .join(format!("{name}-{}-{n}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("remove stale test home");
        }
        std::fs::create_dir_all(&dir).expect("create test home");
        dir
    }

    /// Removes a directory created by [`test_home`], by its exact path.
    pub fn remove_home(dir: &std::path::Path) {
        assert!(dir.to_string_lossy().contains("/target/test-homes/WP8/"));
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name() {
        assert_eq!(super::CRATE, "kernel-gate");
    }
}
