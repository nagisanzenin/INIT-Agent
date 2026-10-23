//! G4 statistics (kernel/gate.md §3.8): noise floor δ(m), paired cluster-bootstrap non-inferiority,
//! futility look, capability test, the G-21 calibration simulation, and ADR-011's policy simulator (X10).

pub mod boot;
pub mod data;
pub mod extend_sim;
pub mod noise;
pub mod rng;
pub mod sim;

pub use boot::{
    CapFamily, CapResult, CapStatus, FutilityLook, Interval, bootstrap_ni, capability_test,
    futility, margin,
};
pub use data::{Arm, Metric, RunValues, Source, TaskRuns, TaskSet};
pub use noise::{Deltas, NoiseEntry, NoiseHistory, compute_deltas};
pub use rng::{SplitMix64, seed_u64};
