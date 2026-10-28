//! §3.5 the sealed audit: the current tuple T_c against audited-good T_a on a rotating seeded sample of S,
//! R repeats each, interleaved by task, Root canaries first on T_c; δ_S from T_a's repeats (smoothed with
//! `sealed/noise.json`), the margin, the paired cluster bootstrap; `audit{…}`, and `audited` (plus `lkg` when
//! T_c is newer) only after a complete audit with no regression. R7 (ledger.rs) turns a regression into the
//! demotion and the rollback to audited-good.

use super::run::{self, Env};
use super::stats::{Rng, lower, noise, seed, smooth};
use super::task::{self, Task};
use crate::charter::musd;
use crate::link::{Root, lock};
use crate::util::{self, icj, now_ms, rfc3339, write_atomic};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn body(rh: &Path, id: &str) -> Result<PathBuf, String> {
    let t = crate::tuple::load(rh, id).filter(|t| crate::tuple::verify(rh, t)).ok_or(format!("tuple {id} is missing or does not verify"))?;
    let sha = t["body"]["binary"].as_str().unwrap_or("");
    crate::tuple::blob(rh, sha).filter(|p| p.is_file()).ok_or(format!("tuple {id} has no body binary in the store"))
}

/// The seq of the `tuple` record that formed `id` (to tell which of two tuples is newer).
fn formed(recs: &[Value], id: &str) -> u64 {
    recs.iter().rev().filter(|r| r["kind"] == "tuple" && r["data"]["id"] == id).find_map(|r| r["seq"].as_u64()).unwrap_or(0)
}

fn pm(x: f64) -> i64 {
    (x * 1000.0).round() as i64
}

/// `init-root audit run [--reason …]`.
pub fn audit(root: &Root, reason: &str) -> Result<Value, String> {
    let env = Env::of(root)?;
    let _op = run::begin(&env.rh);
    task::leak_scan(root);
    let p = lock(&root.sup.ptrs).clone();
    let tc = p.current.clone().ok_or("no current tuple")?;
    let ta = p.audited.clone().unwrap_or_else(|| tc.clone());
    let (bc, ba) = (body(&env.rh, &tc)?, body(&env.rh, &ta)?);
    let s = &env.ch.sealed;
    let state_p = env.rh.join("sealed/noise.json");
    let state: Value = std::fs::read_to_string(&state_p).ok().and_then(|x| serde_json::from_str(&x).ok()).unwrap_or(json!({}));
    let (canaries, mut tasks): (Vec<Task>, Vec<Task>) = task::store(&env.rh).into_iter().partition(|t| t.canary);
    if tasks.is_empty() {
        return Err(format!("S is empty: {}", util::root_cmd(&env.rh, "sealed import <dir>")));
    }
    // the sample rotates over a fixed order, so every task runs at least once per ⌈|S|/n⌉ audits
    let want = (s.audit_n.max(1) as usize) * if state["double"] == true { 2 } else { 1 };
    let (total, cursor) = (tasks.len(), state["cursor"].as_u64().unwrap_or(0) as usize % tasks.len());
    let n = want.min(total);
    tasks.rotate_left(cursor);
    tasks.truncate(n);
    let id = util::hex(&util::random::<6>().map_err(|e| e.to_string())?);
    Rng(seed(&id, "audit:order")).shuffle(&mut tasks);
    let (budget, r) = (musd(&s.audit_usd).unwrap_or(0), s.audit_repeats.max(1) as u32);
    let (mut spent, mut partial, mut canary) = (0u64, false, false);
    let mut go = |body: &Path, t: &Task, rep: u32| -> Result<Option<i64>, String> {
        if run::cancelled() {
            return Ok(None);
        }
        let o = run::run_task(&env, body, t, rep, budget)?;
        spent += o["spent_musd"].as_u64().unwrap_or(0);
        Ok((o["outcome"] != "cost_capped").then(|| o["score_permille"].as_i64().unwrap_or(0)))
    };
    for t in &canaries {
        canary |= go(&bc, t, 1)? == Some(1000);
    }
    let (mut sc, mut sa) = (Vec::new(), Vec::new());
    'tasks: for t in &tasks {
        let (mut c, mut a) = (Vec::new(), Vec::new());
        for rep in 1..=r {
            match (go(&bc, t, rep)?, go(&ba, t, rep)?) {
                (Some(x), Some(y)) => (c.push(x as f64 / 1000.0), a.push(y as f64 / 1000.0)),
                _ => {
                    partial = true;
                    break 'tasks;
                }
            };
        }
        sc.push(c);
        sa.push(a);
    }
    if run::cancelled() {
        return Err("the audit was cancelled (panic or stop)".into());
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let (mc, ma): (Vec<f64>, Vec<f64>) = (sc.iter().map(|v| mean(v)).collect(), sa.iter().map(|v| mean(v)).collect());
    let d: Vec<f64> = mc.iter().zip(&ma).map(|(c, a)| c - a).collect();
    let k = d.len();
    let prev = state["delta_permille"].as_i64().zip(state["n"].as_u64()).map(|(x, n)| (x as f64 / 1000.0, n as usize));
    let delta = noise(&sa, &id).map(|x| smooth(x, k, prev)).or(prev.map(|p| p.0)).unwrap_or(0.0);
    let margin = (2.0 * delta).max(0.5 / k.max(1) as f64);
    let underpowered = k == 0 || margin > s.margin_max;
    let l = lower(&d, &id, "ni");
    let ni_fail = k > 0 && !underpowered && l <= -margin;
    let regression = canary || ni_fail;
    let complete = !partial && !underpowered;
    let data = json!({"baseline": ta, "baseline_permille": pm(mean(&ma)), "d_permille": pm(mean(&d)), "delta_s_permille": pm(delta),
        "l_permille": pm(l), "margin_permille": pm(margin), "n": k, "partial": partial || underpowered, "reason": reason,
        "regression": regression, "repeats": r, "score_permille": pm(mean(&mc)), "spent_musd": spent,
        "trigger": if canary { "canary" } else if ni_fail { "ni_fail" } else { "none" }, "tuple": tc, "underpowered": underpowered});
    let st = json!({"at": rfc3339(now_ms()), "cursor": (cursor + n) % total, "delta_permille": pm(delta),
        "double": underpowered, "n": k, "repeats": r, "s1": state["s1"], "v": 1}); // `s1`: M7 calibration's δ_S
    let _ = write_atomic(&state_p, format!("{}\n", icj(&st).unwrap_or_default()).as_bytes(), 0o600);
    let rec = root.append("audit", data.clone()).ok_or("the lineage is unavailable")?;
    if complete && !regression {
        root.append("audited", json!({"tuple": tc}));
        let recs = crate::lineage::records(&env.rh).unwrap_or_default();
        if p.lkg.as_ref().is_none_or(|lk| lk != &tc && formed(&recs, &tc) > formed(&recs, lk)) {
            root.append("lkg", json!({"tuple": tc}));
        }
    }
    Ok(json!({"audit": data, "seq": rec["seq"]}))
}
