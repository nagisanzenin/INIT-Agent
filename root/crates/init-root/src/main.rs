//! `init-root`: the immutable Root (spec v0.3 Part II, docs/root/root.md).
//! Budget: about 5,000 non-blank, non-comment lines in src/, tests excluded (root/root.md §4.8 as raised
//! by the orchestrator on 2026-10-20 from 1,500).
#![deny(unsafe_code)] // only `sys.rs` (FFI) may carry #[allow(unsafe_code)] (root/root.md §4.8)

mod access; // ADR-009 workspace, full mode, snapshots and /undo (SBX-009)
mod benchmark; // owner-controlled experiment freeze and external gateway binding
mod charter; // R5 (WP20)
#[cfg(target_os = "linux")]
mod cgroup; // LINUX: per-workload cgroup v2 memory.max under a delegated subtree (the memwatch backstop)
mod cli; // the CLI (WP20; exec-clean: WP21)
mod doctor; // `doctor --sweep-dry-run` (R-SAFE, ADR-008): the D1 selection, read-only
mod egress; // R2/R3 (WP22; C11 validation: WP20)
mod extend; // ADR-011 §5 attribution: usage ledger, cap table, caps.json (X5)
mod goal; // ADR-013 GBAL: owner goals, predictions, observation cards, outcomes and calibration
mod grant; // ADR-011 §7.4 owner grants: requests, /grant and /deny, the overlay with expiry (X7)
mod install; // the homes, the layout checks and install (WP20)
mod ledger; // R7 (WP25; the level fold: WP20)
mod lineage; // R4 (WP23; record format, append, verify: WP20)
mod link; // the owner channel and the Root state (WP20; owner.in delivery: WP21)
mod memwatch; // MEM-RUNAWAY: the per-workload memory watchdog and the workload disk floor
mod panic; // R1 panic and resume (WP20; SIGUSR1, held-instance kills, resume rollback: WP21)
mod providers; // UX-KEYMODEL: the provider catalog, `providers add`, `model use`, `keys.set` over owner.sock
mod sealed; // R6 (WP24; C12 validation: WP20)
#[path = "extend/select.rs"]
mod select; // ADR-011 §6 selection (X6): decay, archive and quarantine debts, Fisher-tail correlation, bloat
mod sentinel; // ADR-011 §3 (X3): the harm sentinel, turns, groups, H1–H12, rollback
mod snapshot; // ADR-011 §4 (X4): system snapshots, R2 restore, the A4 rollback probe, `settled`
mod sup; // R1 supervision and the kernel control link (WP21)
mod sweep; // R1: the D1 membership rule (WP21; the live scan only with `live-sweep`; the dry-run: ADR-008)
mod sys; // FFI
#[cfg(feature = "test-fixtures")]
mod testfix; // fake clock and test ledgers (root/root.md §3.16), test builds only
mod tuple; // R1: tuples, the blob store, pointers, rollback targets (WP21)
mod twin; // ADR-011 Evaluation plan (X9): the frozen twin, the nightly shadow replay, twin_report, twin.json
mod upgrade; // lane UPGRADE (v0.5.0): `init-root upgrade`, the owner's stop-then-replace upgrade to a new release
mod util;
mod zone; // R1 spawn broker (WP21)

use std::process::ExitCode;

fn main() -> ExitCode {
    let _ = (egress::AREA, sealed::AREA, lineage::AREA); // module ids
    select::register(); // X6: archive/quarantine debts and bloat (A5) before X4's A4; turn and direct-rollback hooks
    sup::kprobe::register(); // X8: the Root conformance probe of a kernel change, before X4's snapshot
    snapshot::register(); // X4: the A4/snapshot admission hook and R2 (sentinel::Hooks)
    goal::register(); // ADR-013: the cards' closed-turn hook (sentinel::Hooks)
    twin::register(); // X9: the replay store's closed-turn hook (sentinel::Hooks)
    ExitCode::from(cli::run(std::env::args().skip(1).collect()))
}
