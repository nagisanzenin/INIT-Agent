//! R6: minimal sealed evaluation (root/sealed-eval.md v0.2). WP20: the `[sealed]` charter section and
//! `validate` (charter rule C12, §4.3). WP24: the S store (task.rs); sealed runs and the reference
//! evaluator on sealed egress lanes (run.rs, egress/lane.rs; D5); the mutation suite and the gate-of-gates
//! (gog.rs); the manual audit (audit.rs); the Root's copy of the gate statistics (stats.rs); the family
//! admission rules (admit.rs; D2, D3). Owner commands: `sealed import|list|gog`, `audit run`.
//! M7 lean (ROOT-S1a; PROOF-LEAN.md): S v1 from pack `s1` (pack.rs), the G4 slice, calibration, the cost probe
//! and the milestone check (slice.rs) on ROOT-S1b's exact statistics (exact.rs); `sealed draw|show|calibrate|probe|
//! milestone|slice`, and `sealed.g4_slice` on the control link.

mod admit;
mod audit;
mod exact; // G4 v2 statistics (M7, lane ROOT-S1b)
mod gog;
mod pack; // M7 S v1: pack install, draw, partitions (lane ROOT-S1a)
mod run;
mod slice; // M7: the G4 slice, calibration, the cost probe, the milestone check (lane ROOT-S1a)
mod stats;
mod task;

use crate::charter::{Finding, err, musd};
use crate::link::{Refusal, Root};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;

pub use run::{cancel_all, clean_stale};
pub use slice::g4_slice;
pub use task::leak_check;
pub(crate) use run::{Env, RunRoot, sandboxed, supervise, try_begin}; // ADR-011 A4 probe roots (X4, snapshot.rs)
pub(crate) use task::extract; // X4: a tuple's t0 blob into a probe root
pub(crate) use run::{cancelled, lane_sock}; // X9: the twin's replay arms (twin.rs)

pub const AREA: &str = "sealed";

/// sealed-eval.md §4.3. `margin_max` is a ratio in TOML; ICJ is integers only, so it is published
/// (and digested) as `margin_max_permille`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Sealed {
    pub audit_schedule: String,
    pub audit_weekday: String,
    pub audit_n: i64,
    pub audit_repeats: i64,
    pub audit_usd: String,
    pub gog_usd: String,
    pub reserve_usd: String,
    pub bisect_usd: String,
    pub parallel: i64,
    #[serde(rename(serialize = "margin_max_permille"), serialize_with = "permille")]
    pub margin_max: f64,
    pub audit_required_within_h: i64,
    pub admit_repeats: i64,
    pub admit_noop_max_permille: i64,
    pub admit_k_promotions: i64,
    pub family_min_instances: i64,
    pub admit_disagree_max: i64,
    /// M7 (m7-design.md §4.3, lean subset; PROOF-LEAN D-L2 cuts the rotation and look keys).
    pub pack: String,
    pub ratchet_k: i64,
    pub milestone_repeats: i64,
    pub cache_max_age_h: i64,
    pub sentinel_permille: i64,
}

impl Default for Sealed {
    fn default() -> Self {
        let s = |x: &str| x.to_string();
        Sealed { audit_schedule: s("weekly"), audit_weekday: s("sat"), audit_n: 24, audit_repeats: 2, audit_usd: s("3.00"),
            gog_usd: s("5.00"), reserve_usd: s("3.00"), bisect_usd: s("0.00"), parallel: 2, margin_max: 0.25,
            audit_required_within_h: 48, admit_repeats: 3, admit_noop_max_permille: 500, admit_k_promotions: 5,
            family_min_instances: 2, admit_disagree_max: 1, pack: s("s1"), ratchet_k: 4, milestone_repeats: 3, cache_max_age_h: 72,
            sentinel_permille: 200 }
    }
}

fn permille<S: serde::Serializer>(x: &f64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_i64((x * 1000.0).round() as i64)
}

/// C12. `total_musd` is the charter's `money.total_usd` (None when it failed C5).
pub fn validate(s: &Sealed, _total_musd: Option<u64>, max_instances: i64) -> Vec<Finding> {
    let mut e = Vec::new();
    let mut m = |k: &str, ok: bool, msg: &str| if !ok { e.push(err(&format!("sealed.{k}"), "C12", msg)) };
    let money = [("audit_usd", &s.audit_usd), ("gog_usd", &s.gog_usd), ("reserve_usd", &s.reserve_usd), ("bisect_usd", &s.bisect_usd)];
    for (k, v) in money {
        m(k, musd(v).is_some(), "must be a decimal string like \"3.00\"");
    }
    m("audit_schedule", ["weekly", "daily", "manual"].contains(&s.audit_schedule.as_str()), "weekly, daily or manual");
    m("audit_weekday", ["mon", "tue", "wed", "thu", "fri", "sat", "sun"].contains(&s.audit_weekday.as_str()), "mon…sun");
    m("audit_n", (8..=200).contains(&s.audit_n), "must be 8–200");
    m("audit_repeats", (2..=5).contains(&s.audit_repeats), "must be 2–5");
    m("parallel", (1..=4).contains(&s.parallel) && s.parallel <= max_instances - 2, "must be 1–4 and ≤ quotas.max_instances − 2");
    m("margin_max", (0.0..=1.0).contains(&s.margin_max), "must be 0–1");
    m("admit_k_promotions", s.admit_k_promotions >= 5, "must be ≥ 5 (D3)");
    m("family_min_instances", s.family_min_instances >= 2, "must be ≥ 2 (D2)");
    m("pack", !s.pack.is_empty() && s.pack.len() <= 32 && s.pack.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)), "a pack name");
    m("ratchet_k", (1..=8).contains(&s.ratchet_k), "must be 1–8");
    m("milestone_repeats", (2..=5).contains(&s.milestone_repeats), "must be 2–5");
    m("cache_max_age_h", (0..=168).contains(&s.cache_max_age_h), "must be 0–168");
    m("sentinel_permille", (100..=1000).contains(&s.sentinel_permille), "must be 100–1000");
    let nonneg = [s.audit_required_within_h, s.admit_repeats, s.admit_noop_max_permille, s.admit_disagree_max];
    m("admit_repeats", nonneg.iter().all(|x| *x >= 0), "counts must be non-negative");
    e
}

/// Owner-socket `audit.run{reason}` and `sealed.gog{gate, args, source}` (the CLI runs the same code under the
/// Root lock when no Root is up). Both are long: the CLI waits up to two hours.
pub fn request(root: &Root, cmd: &str, a: &Value) -> Result<Value, Refusal> {
    let r = match cmd {
        "audit.run" => audit::audit(root, a["reason"].as_str().unwrap_or("manual")),
        // M7 (lane ROOT-S1a): owner commands on S v1
        "sealed.draw" => pack::draw(root, a),
        "sealed.calibrate" => slice::calibrate(root, a),
        "sealed.probe" => slice::probe(root, a),
        "sealed.milestone" => slice::milestone(root, a),
        "sealed.slice" => return slice::g4_slice(root, a).map_err(|(c, m, _)| (c, m)),
        _ => match a["gate"].as_str() {
            Some(g) => {
                let args: Vec<String> = a["args"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
                gog::gog(root, Path::new(g), &args, a["source"].as_str().unwrap_or("maintainer"))
            }
            None => Err("sealed.gog needs a gate executable".into()),
        },
    };
    r.map_err(|m| ("root.sealed", m))
}

/// The smallest S the gate-of-gates runs on: 12 tasks plus the canary, so its split draws the 8 + 4 sample whose
/// holdout of 4 the gate's D14 rule needs (§3.3). Provisional owner decision 2026-10-20 (build-deviations §3.15).
pub const S_MIN: usize = 12;

/// (tasks, canaries) in `tasks`.
fn sizes<'a>(tasks: impl IntoIterator<Item = &'a task::Task>) -> (usize, usize) {
    tasks.into_iter().fold((0, 0), |(n, c), t| if t.canary { (n, c + 1) } else { (n + 1, c) })
}

fn too_small((n, c): (usize, usize)) -> Option<String> {
    (n < S_MIN || c == 0).then(|| format!("S would hold {n} tasks and {c} canaries: sealed import needs at least {S_MIN} tasks plus \
        the canary, the gate-of-gates' 8 + 4 sample (root/sealed-eval.md §3.3); nothing was imported"))
}

/// `init-root sealed import <dir>`: an M suite (`suite.json` + `base/`), one task, or a directory of tasks. The S that
/// results must hold at least `S_MIN` tasks plus a canary (§3.15): a smaller set is refused before any check runs,
/// and a set that falls below it because some tasks fail validation is taken back out by exact path.
pub fn import(p: &crate::install::Paths, src: &Path) -> Result<Value, String> {
    let env = run::Env::load(p)?;
    if src.join("suite.json").is_file() {
        let dst = env.rh.join("sealed/M");
        let _ = std::fs::remove_dir_all(&dst); // exact path: the previous suite, replaced as a whole
        let ok = std::process::Command::new("/bin/cp").arg("-R").arg(src).arg(&dst).status().is_ok_and(|s| s.success());
        return if ok { Ok(json!({"suite": "imported"})) } else { Err("copying the suite failed".into()) };
    }
    let single = src.join("task.json").is_file();
    let mut dirs: Vec<_> = if single {
        vec![src.to_path_buf()]
    } else {
        std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.join("task.json").is_file()).collect()
    };
    dirs.sort();
    let have = task::store(&env.rh);
    let new: Vec<task::Task> = dirs.iter().filter_map(|d| task::load(d).ok()).filter(|t| !have.iter().any(|h| h.id == t.id)).collect();
    if let Some(m) = too_small(sizes(have.iter().chain(&new))) {
        return Err(m);
    }
    let (mut ok, mut refused) = (Vec::new(), Vec::new());
    for d in dirs {
        match task::import(&env, &d) {
            Ok(v) => ok.push(v),
            Err(e) if single => return Err(e),
            Err(e) => refused.push(json!({"dir": d.file_name().map(|n| n.to_string_lossy().to_string()), "error": e})),
        }
    }
    if let Some(m) = too_small(sizes(&task::store(&env.rh))) {
        for v in &ok {
            if let Some(id) = v["id"].as_str() {
                let _ = std::fs::remove_dir_all(env.rh.join("sealed/S").join(id)); // exact path, imported just above
            }
        }
        return Err(format!("{m}; refused: {}", crate::util::icj(&json!(refused)).unwrap_or_default()));
    }
    Ok(json!({"imported": ok, "refused": refused}))
}

pub fn list(rh: &Path) -> Value {
    task::list(rh)
}

/// `init-root sealed show [--g4 <gate_id>|--partition|--templates]` (owner-only; never task content).
pub fn show(rh: &Path, a: &Value) -> Result<Value, String> {
    slice::show(rh, a)
}

#[cfg(test)]
mod tests {
    #[test]
    fn defaults_are_valid_and_digest_as_integers() {
        let s = super::Sealed::default();
        assert!(super::validate(&s, Some(25_000_000), 12).is_empty());
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["margin_max_permille"], 250);
        assert!(crate::util::icj(&v).is_ok());
        let tight = super::Sealed { parallel: 4, ..s };
        assert_eq!(super::validate(&tight, Some(25_000_000), 5)[0].key, "sealed.parallel");
    }
}
