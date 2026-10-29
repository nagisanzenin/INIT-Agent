//! §3.5 the sealed audit: the current tuple T_c against audited-good T_a on a rotating seeded sample of S,
//! R repeats each, interleaved by task, Root canaries first on T_c; δ_S from T_a's repeats (smoothed with
//! `sealed/noise.json`), the margin, the paired cluster bootstrap; `audit{…}`, and `audited` (plus `lkg` when
//! T_c is newer) only after a complete audit with no regression, and only while T_c is still `current` and no change
//! group that admitted it closed rolled back (v0.5.3, `lkg_cas`). R7 (ledger.rs) turns a regression into the
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

/// L2b (INV-5): the audit's `lkg` move as a compare-and-set against `lkg` as it stands now, read under the lineage lock
/// (the audit ran for minutes, and a window close, a gated promotion or a rollback may have moved it): `moved` when the
/// audited tuple `tc` is newer along the lineage than `lkg` and no open change group admitted it; else the refusal
/// (`already`, `behind`, `open_group`), which the `audit` record names as `lkg_cas`.
/// v0.5.3 (V1): also refused when `tc` is no longer `current` (`not_current`: the sentinel or the owner rolled it back,
/// or a newer tuple replaced it, while the audit ran) or when a change group that admitted `tc` closed `rolled_back` or
/// `collateral` (`rolled_back`). These two refusals also keep `audited` where it was ([`marks_audited`]).
fn lkg_cas(recs: &[Value], tc: &str, current: Option<&str>, lkg: Option<&str>) -> &'static str {
    let (mut open, mut undone) = (std::collections::BTreeMap::<u64, bool>::new(), false); // group → one of its changes admitted `tc`
    for r in recs.iter().filter(|r| r["by"] == "root") {
        match (r["kind"].as_str(), r["data"]["group"].as_u64()) {
            (Some("change_open"), Some(g)) => *open.entry(g).or_default() |= r["data"]["tuple"] == tc,
            (Some("change_close"), Some(g)) => {
                let admitted = open.remove(&g).unwrap_or(false);
                undone |= admitted && matches!(r["data"]["outcome"].as_str(), Some("rolled_back" | "collateral"));
            }
            _ => {}
        }
    }
    match lkg {
        _ if current != Some(tc) => "not_current",
        _ if undone => "rolled_back",
        Some(lk) if lk == tc => "already",
        Some(lk) if formed(recs, tc) <= formed(recs, lk) => "behind",
        _ if open.values().any(|&t| t) => "open_group",
        _ => "moved",
    }
}

/// V1: whether a complete audit with no regression may record `tc` as `audited`: not when the `lkg` compare-and-set found
/// it rolled back or no longer current, since audited-good is where a demotion rolls back to.
fn marks_audited(cas: &str) -> bool {
    !matches!(cas, "not_current" | "rolled_back")
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
    // `audit`, `audited` and the `lkg` compare-and-set under one lineage lock, so nothing moves `lkg` in between (INV-5)
    let mut data = data;
    let mut lg = lock(&root.lin);
    let ok = complete && !regression;
    if ok {
        let recs = crate::lineage::records(&env.rh).map_err(|e| format!("lineage: {e}"))?;
        let ptrs = lock(&root.sup.ptrs).clone();
        data["lkg_cas"] = json!(lkg_cas(&recs, &tc, ptrs.current.as_deref(), ptrs.lkg.as_deref()));
    }
    let lin = |e: std::io::Error| format!("lineage: {e}");
    let mut recs = vec![root.append_locked(&mut lg, "root", "audit", data.clone()).map_err(lin)?];
    if ok && data["lkg_cas"].as_str().is_some_and(marks_audited) {
        recs.push(root.append_locked(&mut lg, "root", "audited", json!({"tuple": tc})).map_err(lin)?);
        if data["lkg_cas"] == "moved" {
            recs.push(root.append_locked(&mut lg, "root", "lkg", json!({"tuple": tc})).map_err(lin)?);
        }
    }
    drop(lg);
    recs.iter().for_each(|r| root.after(r));
    Ok(json!({"audit": data, "seq": recs[0]["seq"]}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L2b (INV-5): the audit's `lkg` compare-and-set, against `lkg` as it stands at the end.
    #[test]
    fn the_audits_lkg_move_is_a_compare_and_set() {
        let r = |seq: u64, kind: &str, data: Value| json!({"by": "root", "data": data, "kind": kind, "seq": seq});
        let formed = |seq: u64, id: &str| r(seq, "tuple", json!({"id": id}));
        let base = vec![formed(1, "tA"), formed(2, "tC"), formed(3, "tN")];
        assert_eq!(lkg_cas(&base, "tC", Some("tC"), Some("tA")), "moved");
        assert_eq!(lkg_cas(&base, "tC", Some("tC"), None), "moved");
        assert_eq!(lkg_cas(&base, "tC", Some("tC"), Some("tC")), "already");
        assert_eq!(lkg_cas(&base, "tC", Some("tC"), Some("tN")), "behind", "lkg moved on while the audit ran");
        let mut open = base.clone();
        open.push(r(4, "change_open", json!({"group": 7, "tuple": "tB"})));
        open.push(r(5, "change_open", json!({"group": 7, "tuple": "tC"})));
        assert_eq!(lkg_cas(&open, "tC", Some("tC"), Some("tA")), "open_group");
        let mut kernel = open.clone();
        kernel[4]["by"] = json!("kernel:000000000000");
        assert_eq!(lkg_cas(&kernel, "tC", Some("tC"), Some("tA")), "moved", "only the Root's own change_open counts");
        open.push(r(6, "change_close", json!({"group": 7, "outcome": "clean"})));
        assert_eq!(lkg_cas(&open, "tC", Some("tC"), Some("tA")), "moved", "closed");
        let other = vec![formed(1, "tA"), formed(2, "tC"), r(3, "change_open", json!({"group": 8, "tuple": "tD"}))];
        assert_eq!(lkg_cas(&other, "tC", Some("tC"), Some("tA")), "moved", "an open group of another tuple");
    }

    /// V1 (fails on c4797cc, where only a group still open refused): the sentinel rolled the audited tuple's group back
    /// while the audit ran. `tuple tA, lkg tA, change_open{group 7, tuple tB}, tuple tB`; the audit starts with T_c = tB;
    /// then `change_close{group 7, rolled_back, direct}` and `rollback{to tA}`: neither `lkg` nor `audited` may move onto tB.
    #[test]
    fn the_audit_never_marks_a_tuple_rolled_back_during_it() {
        let r = |seq: u64, kind: &str, data: Value| json!({"by": "root", "data": data, "kind": kind, "seq": seq});
        let mut recs = vec![r(1, "tuple", json!({"id": "tA"})), r(2, "lkg", json!({"tuple": "tA"})),
            r(3, "change_open", json!({"group": 7, "tuple": "tB"})), r(4, "tuple", json!({"id": "tB"}))];
        assert_eq!(lkg_cas(&recs, "tB", Some("tB"), Some("tA")), "open_group", "at the audit's start");
        recs.push(r(5, "change_close", json!({"attribution": "direct", "group": 7, "outcome": "rolled_back"})));
        assert_eq!(lkg_cas(&recs, "tB", Some("tB"), Some("tA")), "rolled_back", "closed rolled_back, even with current still on tB");
        recs.push(r(6, "rollback", json!({"to": "tA"})));
        let cas = lkg_cas(&recs, "tB", Some("tA"), Some("tA"));
        assert_eq!(cas, "not_current");
        assert!(!marks_audited(cas) && !marks_audited("rolled_back"), "no `audited` either");
        recs[4]["data"]["outcome"] = json!("collateral");
        assert_eq!(lkg_cas(&recs[..5], "tB", Some("tB"), Some("tA")), "rolled_back", "a collateral close undid it too");
        // a clean close keeps the move, and the refusals of an lkg already there or ahead still mark `audited`
        recs[4]["data"]["outcome"] = json!("clean");
        assert_eq!(lkg_cas(&recs[..5], "tB", Some("tB"), Some("tA")), "moved");
        assert!(["moved", "already", "behind", "open_group"].iter().all(|c| marks_audited(c)));
    }
}
