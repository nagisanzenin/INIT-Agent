//! `init-root doctor` (ADR-008, owner-machine mode). `--sweep-dry-run` evaluates the D1 sweep (root/root.md
//! §3.5.4) over the real process table and returns the set a live sweep would take, with every other
//! process counted by the condition it fails. It is the owner-machine evidence that the predicate selects
//! only INIT's own instances. This module only reads: the process table through `sys::LiveTable`
//! (`proc_listpids`, `sandbox_check`, `proc_pidpath`), and `INIT_HOME/run/mark` and `state/root.json`. It
//! sends no signal, and tests/rsafe.rs fails if it ever gains a call that could.

use crate::install::Paths;
use crate::sweep::{ProcTable, Verdict, classify};
use crate::util::{now_ms, rfc3339};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The marks a sweep of this zone would use: each instance mark `INIT_HOME/run/mark/<12 hex>` present
/// (sorted), then the zone mark of the generic sweep.
fn marks(ih: &Path) -> Vec<(String, PathBuf)> {
    let dir = ih.join("run/mark");
    let mut v: Vec<(String, PathBuf)> = fs::read_dir(&dir).into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.len() == 12 && n.bytes().all(|c| c.is_ascii_hexdigit()))
        .map(|n| (n.clone(), dir.join(n))).collect();
    v.sort();
    v.push(("zone-mark".into(), ih.join("run/zone-mark")));
    v
}

/// The instances the Root records (`state/root.json`, rewritten before every spawn answer and after every
/// exit): pid → (instance, kind). While a Root runs these are exactly the pids it holds; with none running
/// the file is a previous Root's record. Read only to label the result; a recorded pid is never acted on.
fn recorded(rh: &Path) -> BTreeMap<i32, (String, String)> {
    let v: Value = fs::read_to_string(rh.join("state/root.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    v["instances"].as_array().into_iter().flatten().filter_map(|i| {
        let pid = i32::try_from(i["pid"].as_i64()?).ok()?;
        Some((pid, (i["id"].as_str()?.to_string(), i["kind"].as_str().unwrap_or("").to_string())))
    }).collect()
}

/// The pure evaluation: every pid of `t` except `me`, classified against `marks` with the sweep's own rule.
pub fn evaluate(t: &dyn ProcTable, marks: &[(String, PathBuf)], decoy: &Path, canary: &Path,
    recorded: &BTreeMap<i32, (String, String)>, me: i32, exe: &dyn Fn(i32) -> Option<String>) -> Value {
    let degraded = t.sandboxed(me).is_none();
    let paths: Vec<&Path> = marks.iter().map(|(_, p)| p.as_path()).collect();
    let mut hits: Vec<Vec<i32>> = vec![Vec::new(); marks.len()];
    let (mut would, mut rejected, mut evaluated) = (Vec::new(), BTreeMap::<&str, u64>::new(), 0u64);
    for name in ["not_sandboxed", "mark_unreadable", "reads_decoy_or_canary", "unknown"] {
        rejected.insert(name, 0);
    }
    // degraded (no `sandbox_check`, RF13): a live sweep never scans, so the dry-run selects nothing either
    for pid in t.pids().into_iter().filter(|&p| p != me && !degraded) {
        evaluated += 1;
        let why = match classify(t, pid, &paths, decoy, canary) {
            Verdict::Member(ix) => {
                ix.iter().for_each(|&i| hits[i].push(pid));
                let h = recorded.get(&pid);
                would.push(json!({"pid": pid, "recorded": h.is_some(), "instance": h.map(|x| &x.0), "kind": h.map(|x| &x.1),
                    "marks": ix.iter().map(|&i| &marks[i].0).collect::<Vec<_>>(), "exe": exe(pid)}));
                continue;
            }
            Verdict::NotSandboxed => "not_sandboxed",
            Verdict::MarkUnreadable => "mark_unreadable",
            Verdict::ReadsDecoyOrCanary => "reads_decoy_or_canary",
            Verdict::Unknown => "unknown",
        };
        *rejected.entry(why).or_default() += 1;
    }
    let total: u64 = rejected.values().sum();
    let unrecorded = would.iter().filter(|w| w["recorded"] == false).count();
    json!({
        "v": 1, "dry_run": true, "signals_sent": 0, "at": rfc3339(now_ms()),
        "build": {"live_sweep": cfg!(feature = "live-sweep"), "sweep": if cfg!(feature = "live-sweep") { "d1" } else { "held_only" }},
        "rule": "D1: sandboxed AND may read the mark AND denied both the decoy and the Root canary",
        "decoy": decoy, "canary": canary, "degraded": degraded,
        "marks": marks.iter().zip(&hits).map(|((n, p), h)| json!({"name": n, "path": p, "members": h})).collect::<Vec<_>>(),
        "evaluated": evaluated, "would_kill": would, "unrecorded_members": unrecorded,
        "rejected": {"total": total, "not_sandboxed": rejected["not_sandboxed"], "mark_unreadable": rejected["mark_unreadable"],
            "reads_decoy_or_canary": rejected["reads_decoy_or_canary"], "unknown": rejected["unknown"]},
    })
}

/// `init-root doctor --sweep-dry-run` over this machine's process table.
#[cfg(target_os = "macos")]
pub fn sweep_dry_run(p: &Paths) -> Result<Value, String> {
    let ih_marks = p.i("run/mark");
    if !ih_marks.is_dir() {
        return Err(format!("{} is missing: run init-root install first", ih_marks.display()));
    }
    let (decoy, canary) = (p.i("run/mark/decoy"), p.r("run/canary"));
    let mut v = evaluate(&crate::sys::LiveTable, &marks(&p.ih), &decoy, &canary, &recorded(&p.rh), std::process::id() as i32,
        &crate::sys::LiveTable::path);
    v["init_home"] = json!(p.ih);
    v["root_home"] = json!(p.rh);
    Ok(v)
}

#[cfg(not(target_os = "macos"))]
pub fn sweep_dry_run(_: &Paths) -> Result<Value, String> {
    Err("doctor --sweep-dry-run is macOS-only: the Linux zone confirms by pid namespace (WP6b), not by a predicate".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake table: pid → (sandboxed, readable paths).
    struct T(BTreeMap<i32, (Option<bool>, Vec<PathBuf>)>);
    impl ProcTable for T {
        fn pids(&self) -> Vec<i32> {
            self.0.keys().copied().collect()
        }
        fn sandboxed(&self, pid: i32) -> Option<bool> {
            self.0.get(&pid).and_then(|x| x.0)
        }
        fn may_read(&self, pid: i32, path: &Path) -> Option<bool> {
            self.0.get(&pid).map(|x| x.1.iter().any(|r| r == path))
        }
    }

    #[test]
    fn evaluate_reports_members_recorded_or_not_and_counts_rejects() {
        let (ih, rh) = (PathBuf::from("/i"), PathBuf::from("/r"));
        let m = vec![("a3f09c1e5b7d".to_string(), ih.join("run/mark/a3f09c1e5b7d")), ("zone-mark".into(), ih.join("run/zone-mark"))];
        let (decoy, canary) = (ih.join("run/mark/decoy"), rh.join("run/canary"));
        let r = |v: &[&PathBuf]| v.iter().map(|p| (*p).clone()).collect::<Vec<_>>();
        let t = T([
            (1, (Some(false), vec![])),                                     // us
            (10, (Some(true), r(&[&m[0].1, &m[1].1]))),                     // the instance
            (11, (Some(true), r(&[&m[1].1]))),                              // an escapee of a reaped instance (zone mark)
            (20, (Some(false), r(&[&m[0].1]))),                             // the owner's shell
            (21, (Some(true), r(&[&m[0].1, &m[1].1, &decoy, &canary]))),     // a permissive foreign sandbox
            (22, (Some(true), vec![])),                                     // an app sandbox that reads nothing of ours
            (23, (None, vec![])),
        ].into_iter().collect());
        let rec: BTreeMap<i32, (String, String)> = [(10, ("a3f09c1e5b7d".to_string(), "body".to_string()))].into_iter().collect();
        let v = evaluate(&t, &m, &decoy, &canary, &rec, 1, &|_| None);
        assert_eq!((v["dry_run"].as_bool(), v["signals_sent"].as_i64(), v["evaluated"].as_i64()), (Some(true), Some(0), Some(6)));
        let pids: Vec<i64> = v["would_kill"].as_array().unwrap().iter().map(|w| w["pid"].as_i64().unwrap()).collect();
        assert_eq!(pids, [10, 11]);
        assert_eq!((v["would_kill"][0]["recorded"].as_bool(), v["would_kill"][1]["recorded"].as_bool(), v["unrecorded_members"].as_i64()),
            (Some(true), Some(false), Some(1)));
        assert_eq!(v["marks"][0]["members"], json!([10]));
        assert_eq!(v["marks"][1]["members"], json!([10, 11]));
        assert_eq!(v["rejected"], json!({"total": 4, "not_sandboxed": 1, "mark_unreadable": 1, "reads_decoy_or_canary": 1, "unknown": 1}));
        assert_eq!(v["build"]["sweep"], if cfg!(feature = "live-sweep") { "d1" } else { "held_only" });
        // degraded: no sandbox_check for our own pid → nothing is evaluated, nothing selected
        let t = T([(1, (None, vec![])), (10, (Some(true), r(&[&m[0].1])))].into_iter().collect());
        let v = evaluate(&t, &m, &decoy, &canary, &rec, 1, &|_| None);
        assert_eq!((v["degraded"].as_bool(), v["evaluated"].as_i64(), v["would_kill"].as_array().map(Vec::len)), (Some(true), Some(0), Some(0)));
    }
}
