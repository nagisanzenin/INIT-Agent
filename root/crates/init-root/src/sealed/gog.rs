//! §3.3 the mutation suite M and the gate-of-gates. M is data shipped with the Root release: a control base
//! B (`sealed/M/base/`) and `sealed/M/suite.json`, one generator spec per class (M1–M7 must be rejected,
//! the controls C1–C3 accepted). A generator copies B into a fresh tree in the run root and applies its one
//! known-bad change as exact-text edits (M5's answer key is filled from this run's pool). The candidate
//! gate runs as `<gate> gate judge` (kernel/gate.md §4.1a) in one sealed run root holding its home copy and
//! a sealed lane, and is admitted only if every verdict is as expected and it agrees with the reference
//! evaluator on the LKG arm of C1.

use super::run::{self, Env, RunRoot};
use super::stats::{Rng, seed};
use super::task::{self, Task};
use crate::charter::musd;
use crate::link::Root;
use crate::util::{self, icj};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn cp(src: &Path, dst: &Path) -> Result<(), String> {
    let ok = Command::new("/bin/cp").arg("-R").arg(src).arg(dst).status().is_ok_and(|s| s.success());
    ok.then_some(()).ok_or(format!("copy {} failed", src.display()))
}

/// One generator: B copied to `out`, then each `{file?, find, with}` edit applied to every occurrence
/// (`{{ANSWERS}}` in `with` is the answer key). A missing anchor is an error: the spec no longer fits B.
pub fn generate(base: &Path, ops: &[Value], answers: &Value, out: &Path) -> Result<(), String> {
    cp(base, out)?;
    for op in ops {
        let name = op["file"].as_str().unwrap_or("init-body");
        let (Some(find), Some(with)) = (op["find"].as_str(), op["with"].as_str()) else { return Err("a generator op needs find and with".into()) };
        if name.split('/').any(|c| c == ".." || c.is_empty()) {
            return Err(format!("bad generator file {name:?}"));
        }
        let f = out.join(name);
        let text = fs::read_to_string(&f).map_err(|e| format!("{name}: {e}"))?;
        if !text.contains(find) {
            return Err(format!("generator anchor not found in {name}: {:?}", find.chars().take(60).collect::<String>()));
        }
        fs::write(&f, text.replace(find, &with.replace("{{ANSWERS}}", &icj(answers).unwrap_or_default()))).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The answer key M5 ships: every file of each pool task's `solution.tar`, by task id.
fn answers(run: &RunRoot, pool: &[Task]) -> Result<Value, String> {
    let mut key = Map::new();
    for t in pool {
        let d = run.dir.join("tmp/answers").join(&t.id);
        task::extract(&t.dir.join("solution.tar"), &d)?;
        let mut files = Map::new();
        for e in walk(&d) {
            let rel = e.strip_prefix(&d).map_err(|e| e.to_string())?.to_string_lossy().to_string();
            files.insert(rel, Value::String(fs::read_to_string(&e).map_err(|e| e.to_string())?));
        }
        key.insert(t.id.clone(), Value::Object(files));
    }
    Ok(Value::Object(key))
}

fn walk(d: &Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(d).into_iter().flatten().flatten().flat_map(|e| if e.path().is_dir() { walk(&e.path()) } else { vec![e.path()] }).collect()
}

#[allow(clippy::too_many_arguments)]
/// `<gate> [args…] gate judge --home H --candidate C --running P --lkg P --level L --out F` in the run root's
/// sandbox on the lane; the verdict counts only if the exit code agrees with it (0 accept, 1 reject, 2, 3).
fn judge(env: &Env, run: &RunRoot, lane: &Path, gate: &[OsString], home: &Path, cand: &Path, parent: &Path, name: &str) -> Result<Value, String> {
    let out = run.dir.join("out").join(format!("{name}.json"));
    let mut argv = gate.to_vec();
    for (k, v) in [("--home", home), ("--candidate", cand), ("--running", parent), ("--lkg", parent), ("--out", &out)] {
        argv.extend([OsString::from(k), v.into()]);
    }
    argv.extend(["--level".into(), env.level.to_string().into()]);
    let mut c = run::sandboxed(env, run, Some(lane), &argv);
    c.current_dir(&run.dir);
    let (code, _, _) = run::supervise(c, run, &crate::memwatch::Limits::of(&env.ch), Duration::from_secs(3600), None, || None)?;
    let v: Value = fs::read_to_string(&out).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
    let want = ["accept", "reject", "inconclusive", "error"].get(code.unwrap_or(3).clamp(0, 3) as usize).copied();
    Ok(if v["verdict"].as_str() == want { v } else { json!({"verdict": "error", "reason": "judge_exit_mismatch"}) })
}

/// §3.3's sample sizes from `n` S tasks: (holdout, pool) = (⌊n/3⌋ clamped to 1–4, the rest up to 8). From
/// 12 tasks on this is the procedure's 4 + 8, the gate's own minimums (kernel/gate.md §3.7.2).
fn split(n: usize) -> (usize, usize) {
    let nh = (n / 3).clamp(1, 4).min(n);
    (nh, (n - nh).min(8))
}

/// `sealed::gog`: the procedure of §3.3 on the stand-in or shipped M; appends `mutation{…}`. An S below
/// `S_MIN` tasks plus a canary (after a leak drop: import refuses one) is a measurement gap, not a broken gate:
/// nothing runs, and the record is `mutation{pass:null, underpowered:true}`, which R7 neither counts as a run
/// nor treats as a trigger, plus the `gog_underpowered` alert (provisional owner decision 2026-10-20, §3.15).
pub fn gog(root: &Root, gate: &Path, args: &[String], source: &str) -> Result<Value, String> {
    let env = Env::of(root)?;
    let _op = run::begin(&env.rh);
    task::leak_scan(root);
    let m = env.rh.join("sealed/M");
    let suite: Value = fs::read_to_string(m.join("suite.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).ok_or("sealed/M/suite.json is missing")?;
    let (canaries, mut tasks): (Vec<Task>, Vec<Task>) = task::store(&env.rh).into_iter().partition(|t| t.canary);
    let gate_sha = fs::read(gate).map(|b| util::sha256_hex(&b)[..12].to_string()).unwrap_or_default();
    if tasks.len() < super::S_MIN || canaries.is_empty() {
        let data = json!({"controls": 0, "controls_ok": 0, "disagreements": 0, "failed": [], "gate": gate_sha, "pass": null, "rejected": 0,
            "sample_n": tasks.len(), "source": source, "spent_musd": 0, "total": 0, "tuple": null, "underpowered": true});
        root.append("mutation", data.clone());
        root.alert_from("sealed", "gog_underpowered", &format!("[root] the gate-of-gates did not run: S holds {} tasks and {} canaries, \
            below the {} tasks plus the canary it needs; add tasks with {} (recorded as underpowered, no demotion)",
            tasks.len(), canaries.len(), super::S_MIN, util::root_cmd(&env.rh, "sealed import <dir>")));
        return Ok(json!({"mutation": data, "verdicts": {}}));
    }
    let id = util::hex(&util::random::<6>().map_err(|e| e.to_string())?);
    Rng(seed(&id, "gog:sample")).shuffle(&mut tasks);
    let (nh, np) = split(tasks.len());
    let (holdout, pool) = (&tasks[..nh], &tasks[nh..nh + np]);
    let run = RunRoot::new(&env.rh)?;
    let home = run.dir.join("ghome");
    for (set, ts) in [("vault/gate/pool", pool), ("vault/holdout", holdout), ("vault/canaries", &canaries[..])] {
        fs::create_dir_all(home.join(set)).map_err(|e| e.to_string())?;
        for t in ts {
            cp(&t.dir, &home.join(set).join(&t.id))?;
            if t.canary {
                let c = json!({"kind": t.prov["canary"].as_str().unwrap_or("leak"), "v": 1});
                fs::write(home.join(set).join(&t.id).join("canary.json"), icj(&c).unwrap_or_default()).map_err(|e| e.to_string())?;
            }
        }
    }
    let bin = run.dir.join("bin/gate");
    fs::copy(gate, &bin).and_then(|_| fs::set_permissions(&bin, fs::Permissions::from_mode(0o500))).map_err(|e| format!("gate: {e}"))?;
    let mut argv: Vec<OsString> = vec![bin.into()];
    argv.extend(args.iter().map(OsString::from).chain(["gate".into(), "judge".into()]));
    let key = answers(&run, pool)?;
    let budget = musd(&env.ch.sealed.gog_usd).unwrap_or(0);
    let lane = crate::egress::sealed_lane(&env.rh, &env.ch, &run.id, &run::lane_sock(&env.rh, &run), budget, env.level).map_err(|e| e.to_string())?;
    let (base, mut failed, mut verdicts) = (run.dir.join("m/base"), Vec::new(), Vec::new());
    fs::create_dir_all(run.dir.join("m")).map_err(|e| e.to_string())?;
    generate(&m.join("base"), &[], &key, &base)?;
    for c in suite["classes"].as_array().into_iter().flatten() {
        let cid = c["id"].as_str().filter(|s| s.bytes().all(|b| b.is_ascii_alphanumeric())).ok_or("suite: bad class id")?;
        let ops = |k: &str| c[k].as_array().cloned().unwrap_or_default();
        let cand = run.dir.join("m").join(cid);
        generate(&m.join("base"), &ops("ops"), &key, &cand)?;
        let parent = if c.get("parent_ops").is_some() {
            let p = run.dir.join("m").join(format!("{cid}-parent"));
            generate(&m.join("base"), &ops("parent_ops"), &key, &p)?;
            p
        } else {
            base.clone()
        };
        let out = judge(&env, &run, &lane.sock, &argv, &home, &cand, &parent, cid)?;
        if out["verdict"] != c["expect"] {
            failed.push(cid.to_string());
        }
        verdicts.push((cid.to_string(), c["expect"].clone(), out, parent));
    }
    let mut spent = lane.close()["spent_musd"].as_u64().unwrap_or(0);
    // agreement (§3.3): the LKG arm of C1's record against the reference evaluator, task by task (all-R pass)
    let mut gate_pass = std::collections::BTreeMap::<String, (bool, u32)>::new();
    let c1 = verdicts.iter().find(|v| v.0 == "C1").ok_or("suite: no C1")?;
    for r in c1.2["record"]["runs"].as_array().into_iter().flatten().filter(|r| r["arm"] == "p") {
        let e = gate_pass.entry(r["task"].as_str().unwrap_or("").to_string()).or_insert((true, 0));
        *e = (e.0 && r["score_permille"] == 1000, e.1.max(r["repeat"].as_u64().unwrap_or(1) as u32));
    }
    let mut disagree = 0;
    for (tid, (pass, reps)) in &gate_pass {
        let Some(t) = pool.iter().chain(holdout).find(|t| &t.id == tid) else { continue };
        let mut ok = true;
        for r in 1..=*reps {
            let o = run::run_task(&env, &c1.3.join("init-body"), t, r, budget.saturating_sub(spent))?;
            spent += o["spent_musd"].as_u64().unwrap_or(0);
            ok &= o["score_permille"] == 1000;
        }
        disagree += i64::from(ok != *pass);
    }
    let n_m = verdicts.iter().filter(|v| v.1 == "reject").count();
    let rejected = verdicts.iter().filter(|v| v.1 == "reject" && v.2["verdict"] == "reject").count();
    let controls_ok = verdicts.iter().filter(|v| v.1 == "accept" && v.2["verdict"] == "accept").count();
    let pass = failed.is_empty() && !gate_pass.is_empty() && disagree <= env.ch.sealed.admit_disagree_max && !run::cancelled();
    let data = json!({"controls": verdicts.len() - n_m, "controls_ok": controls_ok, "disagreements": disagree, "failed": failed,
        "gate": gate_sha, "pass": pass, "rejected": rejected, "sample_n": pool.len() + holdout.len(), "source": source,
        "spent_musd": spent, "total": n_m, "tuple": null, "underpowered": false});
    root.append("mutation", data.clone());
    if !pass {
        root.alert_from("sealed", "gog_fail", &format!("[root] a candidate gate failed the gate-of-gates (failed: {failed:?}, disagreements {disagree})"));
    }
    // the owner's answer adds each class's verdict and reason (class ids only: no S or mutant content)
    let per: Map<String, Value> = verdicts.iter().map(|v| (v.0.clone(), json!(format!("{}/{}", v.2["verdict"].as_str().unwrap_or("?"),
        v.2["reason"].as_str().unwrap_or("?"))))).collect();
    Ok(json!({"mutation": data, "verdicts": per}))
}

#[cfg(test)]
mod tests {
    #[test]
    fn split_reaches_four_plus_eight_at_twelve_tasks() {
        assert_eq!(super::split(5), (1, 4));
        assert_eq!(super::split(8), (2, 6));
        assert_eq!(super::split(12), (4, 8)); // the stand-in S (FIX5): the gate's minimums, no overrides
        assert_eq!(super::split(40), (4, 8));
        assert_eq!(super::split(2), (1, 1));
    }
}
