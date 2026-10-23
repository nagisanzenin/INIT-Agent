//! `kernel-evo`: the evolution engine's Reflect stage and minimal propose path (lane EVO-LITE,
//! build/specs/PROOF-LEAN.md D-L5; docs/evolution-v2.md §3.4, §3.5.1, §3.5.3, §4).
//!
//! | Module | Contract |
//! |---|---|
//! | [`classify`] | the per-tool-result extractors of §3.4.2 (pure) |
//! | [`session`] | signals from one session log: tool results, `finish.*`, `owner.*`, `retry.*`, `loop.*`, red-then-green |
//! | [`trace`] | EV-TRACE: the `trace:` reader, the failure-bearing predicate, the validator's lookup |
//! | [`runs`] | visible pool runs (RunRecords, retained failing-run logs), `task.fail`, `cap.hit`, spikes |
//! | [`mine`] | `evo::mine`: cursors, merge, clusters, attribution, s_c, priority, eligibility, `frictions.jsonl`, the digest, the boot summary |
//! | [`proposal`] | proposal records, evidence rules, `evo propose / status / abandon` |
//! | [`variant`] | kernel-built variant commits under `refs/init/evo/<p>/v<k>` |
//! | [`screen`] | the screen driver and the go/no-go (est ≥ 80‰) |
//!
//! New modules start behind the `wip` feature, so the kernel workspace builds at every commit.

pub mod fsx;
pub mod classify;
pub mod mine;
pub mod proposal;
pub mod runs;
pub mod screen;
pub mod session;
pub mod trace;
pub mod variant;
