//! The engine side of the screen and the go/no-go (evolution-v2 §3.5.2–§3.5.3, D-L5 lean: E0
//! and E3 only). The screen is reject-only: a pass only lets the estimate run, and the estimate
//! only decides whether the gate is asked; nothing here can accept a change.
//!
//! The screen itself is a [`Screener`]: the kernel's is E0 (G0 + G1 build of the variant) then
//! GATE-V2b's E3 mini-run on the pool cache (`kernel-supervise` `promote::evo_screen`); tests use
//! a scripted one. The estimate is kernel-gate's `screen::est_permille` (one copy of the formula).

use crate::mine::{self, Cluster};
use crate::proposal::{self, codes, Chosen, Problem, Proposal, Variant};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;

/// q = 1 − p̄_S ≈ 0.5 (§3.5.3).
pub const Q_PERMILLE: u32 = 500;
/// T = 0.8 until refitted (EV-COMPOSE, cut by D-L5).
pub const TRANSFER_PERMILLE: u32 = 800;

/// One variant's screen result (E0 + E3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub pass: bool,
    /// `screen.e0:<code>`, `screen.tamper`, `screen.mini_inferior`, `screen.costlier`,
    /// `screen.grid`, `screen.budget`.
    pub reason: Option<String>,
    pub targeted_fixed: u32,
    pub targeted_total: u32,
    pub controls_lost: u32,
    pub cost_musd: u64,
    /// Stage records, for `vault/lineage.jsonl` (`screen`, §4.7).
    #[serde(default)]
    pub stages: Vec<Value>,
}

/// Screens one variant.
pub trait Screener {
    fn screen(&self, p: &Proposal, v: &Variant) -> Result<Outcome, String>;
}

/// The proposal's s_c (§3.5.3): fatal pool runs of its clusters (union) / failing pool runs.
pub fn s_c_permille(home: &Path, p: &Proposal) -> u32 {
    let idx: Vec<Cluster> = mine::read_index(home);
    let failing = mine::failing_runs(home);
    if failing == 0 {
        return 0;
    }
    let fatal: BTreeSet<&String> = idx.iter().filter(|c| p.frictions.contains(&c.id)).flat_map(|c| c.fatal.iter()).collect();
    ((fatal.len() as u64 * 1000 / failing).min(1000)) as u32
}

/// The screen's result for the caller.
#[derive(Clone, Debug)]
pub struct Screened {
    pub proposal: Proposal,
    /// Kernel records: one `screen` per variant, then the `proposal` transition.
    pub records: Vec<(String, Value)>,
}

/// `evo screen <p>`: screens every variant (cheapest first, stops a variant at its first
/// reject), ranks survivors (E3 fixes, then the smaller diff), estimates, decides.
pub fn run(home: &Path, id: &str, screener: &dyn Screener, budget_musd: u64) -> Result<Screened, Problem> {
    let mut p = proposal::load(home, id).map_err(|e| Problem::new(codes::IO, e.to_string()))?.ok_or_else(|| Problem::new(codes::STATE, format!("{id}: no such proposal")))?;
    if p.state != "variants" {
        return Err(Problem::new(codes::STATE, format!("{id} is {}; screen it after `evo variant`", p.state)));
    }
    let mut records = Vec::new();
    let mut spent = 0u64;
    let mut survivors: Vec<(u32, u32, usize)> = Vec::new(); // (fixed, diff size, k)
    let variants = p.variants.clone();
    for v in &variants {
        let o = if spent >= budget_musd {
            Outcome { pass: false, reason: Some("screen.budget".into()), targeted_fixed: 0, targeted_total: 0, controls_lost: 0, cost_musd: 0, stages: vec![] }
        } else {
            match screener.screen(&p, v) {
                Ok(o) => o,
                Err(e) => Outcome { pass: false, reason: Some(format!("screen.e0:{}", crate::classify::cap(&e, 120))), targeted_fixed: 0, targeted_total: 0, controls_lost: 0, cost_musd: 0, stages: vec![] },
            }
        };
        spent += o.cost_musd;
        records.push(("screen".to_string(), json!({"proposal": p.id, "k": v.k, "sha": v.sha, "pass": o.pass, "reason": o.reason, "targeted_fixed": o.targeted_fixed, "targeted_total": o.targeted_total, "controls_lost": o.controls_lost, "cost_musd": o.cost_musd, "stages": o.stages})));
        if o.pass {
            let size = v.diff_stat.split_whitespace().filter_map(|w| w.parse::<u32>().ok()).skip(1).sum::<u32>();
            survivors.push((o.targeted_fixed, size, v.k as usize));
        }
        if let Some(x) = p.variants.iter_mut().find(|x| x.k == v.k) {
            x.screen = Some(serde_json::to_value(&o).unwrap_or(Value::Null));
        }
    }
    survivors.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let s_c = s_c_permille(home, &p);
    let note;
    let state;
    if let Some(&(_, _, k)) = survivors.first() {
        let v = p.variants.iter().find(|x| x.k as usize == k).cloned().ok_or_else(|| Problem::new(codes::IO, "survivor vanished"))?;
        let o: Outcome = v.screen.clone().and_then(|s| serde_json::from_value(s).ok()).ok_or_else(|| Problem::new(codes::IO, "screen result missing"))?;
        let est = kernel_gate::screen::est_permille(s_c, o.targeted_fixed, o.targeted_total, Q_PERMILLE, TRANSFER_PERMILLE);
        let go = kernel_gate::screen::go(est);
        p.est = Some(est);
        p.go = Some(go);
        note = json!({"survivors": survivors.len(), "best": k, "est": est, "s_c_permille": s_c, "fixed": o.targeted_fixed, "targeted": o.targeted_total, "spent_musd": spent, "s_measurable": v.s_measurable});
        if go {
            p.chosen = Some(Chosen { k: v.k, sha: v.sha.clone(), aim: kernel_gate::screen::aim(est).to_string(), s_measurable: v.s_measurable });
            state = "screened".to_string();
        } else {
            state = "abandoned:below_resolution".to_string();
        }
    } else {
        note = json!({"survivors": 0, "spent_musd": spent});
        state = "abandoned:screen".to_string();
    }
    let rec = p.transition(&state, note);
    records.push(("proposal".into(), rec));
    proposal::store(home, &p).map_err(|e| Problem::new(codes::IO, e.to_string()))?;
    Ok(Screened { proposal: p, records })
}

/// Marks a screened proposal `promoted` when its chosen variant entered the pipeline.
pub fn mark_promoted(home: &Path, id: &str, job: &str, sha: &str) -> Option<Value> {
    let mut p = proposal::load(home, id).ok().flatten()?;
    if p.state != "screened" || p.chosen.as_ref().map(|c| c.sha.as_str()) != Some(sha) {
        return None;
    }
    let rec = p.transition("promoted", json!({"job": job, "sha": sha}));
    proposal::store(home, &p).ok()?;
    Some(rec)
}
