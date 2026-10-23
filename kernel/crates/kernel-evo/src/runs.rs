//! Visible pool runs as the miner reads them (evolution-v2 §3.4.1 row 2): RunRecords v3 from the
//! pool cache (`vault/gate/cache/<tuple12>/<key64>.jsonl`, incumbent-arm runs, GATE-V2b) and from
//! the gate jobs (`vault/gate/jobs/<job>/runs.jsonl`), only `set:"pool"`; and the retained session
//! logs of the failing ones (`vault/gate/jobs/<job>/sessions/<run_id>.jsonl`).
//!
//! Holdout, canary, capability and sealed runs are never read (`set` filter), and their logs are
//! never retained by the gate.

use crate::session::Instance;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One pool RunRecord, the fields the miner uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolRun {
    pub gate_id: String,
    pub run_id: String,
    pub task: String,
    pub labels: Vec<String>,
    pub family: Option<String>,
    pub score: u64,
    pub outcome: String,
    pub caps_hit: Option<String>,
    pub cost_ref_musd: Option<u64>,
    pub turns: Option<u64>,
    /// In the pool cache (screen-eligible incumbent runs, m7 §3.2.7).
    pub cached: bool,
}

impl PoolRun {
    /// `run:<gate_id>/<run_id>`.
    pub fn reference(&self) -> String {
        format!("run:{}/{}", self.gate_id, self.run_id)
    }
    /// score < 1000.
    pub fn failing(&self) -> bool {
        self.score < 1000
    }
    /// The first label, or `live` style fallback `pool`.
    pub fn label(&self) -> String {
        self.labels.first().cloned().unwrap_or_else(|| "pool".into())
    }
}

fn from_record(r: &Value, cached: bool) -> Option<PoolRun> {
    if r.get("set").and_then(Value::as_str).unwrap_or("pool") != "pool" {
        return None;
    }
    // Candidate-arm runs (G4, screens) measure bodies that are not running; only the incumbent's
    // runs describe the current body's frictions.
    if r.get("arm").and_then(Value::as_str) == Some("c") {
        return None;
    }
    let s = |k: &str| r.get(k).and_then(Value::as_str).map(str::to_string);
    Some(PoolRun {
        gate_id: s("gate_id")?,
        run_id: s("run_id")?,
        task: s("task")?,
        labels: r.get("labels").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
        family: s("family"),
        score: r.get("score_permille").and_then(Value::as_u64).unwrap_or(0),
        outcome: s("outcome").unwrap_or_default(),
        caps_hit: s("caps_hit"),
        cost_ref_musd: r.get("cost_ref_musd").and_then(Value::as_u64),
        turns: r.get("turns").and_then(Value::as_u64),
        cached,
    })
}

fn jsonl(path: &Path) -> Vec<Value> {
    let Ok(Some(b)) = crate::fsx::read_capped(path, 64 << 20) else { return vec![] };
    String::from_utf8_lossy(&b).split('\n').filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn dirs(p: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(p).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// Every pool run the kernel recorded: cache entries first, then job records, de-duplicated by
/// `(gate_id, run_id)`. Sorted by `(gate_id, run_id)`.
pub fn pool_runs(home: &Path) -> Vec<PoolRun> {
    let mut by: BTreeMap<(String, String), PoolRun> = BTreeMap::new();
    for tuple in dirs(&home.join("vault/gate/cache")) {
        for f in dirs(&tuple) {
            if f.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            for e in jsonl(&f) {
                if let Some(r) = e.get("run").and_then(|r| from_record(r, true)) {
                    by.entry((r.gate_id.clone(), r.run_id.clone())).or_insert(r);
                }
            }
        }
    }
    for job in dirs(&home.join("vault/gate/jobs")) {
        for e in jsonl(&job.join("runs.jsonl")) {
            if let Some(r) = from_record(&e, false) {
                by.entry((r.gate_id.clone(), r.run_id.clone())).or_insert(r);
            }
        }
    }
    by.into_values().collect()
}

/// The retained log of a run, if the gate kept it.
pub fn retained_log(home: &Path, run: &PoolRun) -> PathBuf {
    home.join("vault/gate/jobs").join(&run.gate_id).join("sessions").join(format!("{}.jsonl", run.run_id))
}

/// The run-level instances (§3.4.2): `task.fail`, `cap.hit:<cap>`, `cost.spike`, `turns.spike`.
pub fn run_instances(runs: &[PoolRun]) -> Vec<Instance> {
    let mut out = Vec::new();
    // Rolling medians per label over the previous 20 runs (record order).
    let mut hist: BTreeMap<String, (Vec<u64>, Vec<u64>)> = BTreeMap::new();
    for r in runs {
        let label = r.label();
        let mk = |signal: &str, detail: String| Instance {
            signal: signal.to_string(),
            weight: crate::classify::weight(signal),
            reference: r.reference(),
            refs: vec![r.reference()],
            session: format!("run:{}/{}", r.gate_id, r.run_id),
            task: Some(r.task.clone()),
            label: Some(label.clone()),
            family: r.family.clone(),
            run: Some(format!("{}/{}", r.gate_id, r.run_id)),
            ts: String::new(),
            detail,
            failure: r.failing(),
            fatal: r.failing(),
            uid: format!("run:{}/{}/{signal}", r.gate_id, r.run_id),
        };
        if r.failing() {
            out.push(mk(&format!("task.fail:{label}"), format!("{} scored {} ({})", r.task, r.score, r.outcome)));
        }
        if let Some(c) = &r.caps_hit {
            out.push(mk(&format!("cap.hit:{c}"), format!("{} hit the {c} cap", r.task)));
        }
        let h = hist.entry(label.clone()).or_default();
        if let Some(c) = r.cost_ref_musd {
            if let Some(m) = median(&h.0)
                && h.0.len() >= 3
                && c > 3 * m
            {
                out.push(mk("cost.spike", format!("{} cost {c} µUSD > 3× median {m}", r.task)));
            }
            h.0.push(c);
            if h.0.len() > 20 {
                h.0.remove(0);
            }
        }
        if let Some(t) = r.turns {
            if let Some(m) = median(&h.1)
                && h.1.len() >= 3
                && t > 3 * m
            {
                out.push(mk("turns.spike", format!("{} took {t} turns > 3× median {m}", r.task)));
            }
            h.1.push(t);
            if h.1.len() > 20 {
                h.1.remove(0);
            }
        }
    }
    out
}

fn median(v: &[u64]) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_unstable();
    Some(s[s.len() / 2])
}

/// Tasks with at least one passing and one failing run (the candidates for divergence pairs).
pub fn mixed_tasks(runs: &[PoolRun]) -> BTreeSet<String> {
    let mut pass = BTreeSet::new();
    let mut fail = BTreeSet::new();
    for r in runs {
        if r.failing() { fail.insert(r.task.clone()) } else { pass.insert(r.task.clone()) };
    }
    pass.intersection(&fail).cloned().collect()
}
