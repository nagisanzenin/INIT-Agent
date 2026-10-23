use super::*;
use crate::g4::{CheckSpec, Limits, SourceRef, TaskJson, WorkspaceRef};
use crate::pool_cache::{ArmKey, HARNESS_V};
use crate::stats::data::Source;
use crate::testutil::{remove_home, test_home};
use std::path::Path;

fn rec(arm: &str, task: &str, score: u32) -> RunRecord {
    RunRecord {
        v: 3,
        gate_id: "j-000040".into(),
        run_id: format!("{arm}-{task}-r1"),
        arm: arm.into(),
        task: task.into(),
        set: "pool".into(),
        repeat: 1,
        outcome: "done".into(),
        score_permille: score,
        requests: 3,
        cost_ref_musd: 1000,
        model_served: Some("m-1".into()),
        ..Default::default()
    }
}

fn pairs(spec: &[(u32, u32)]) -> Vec<(RunRecord, RunRecord)> {
    spec.iter().enumerate().map(|(i, (c, p))| (rec("c", &format!("t{i}"), *c), rec("p", &format!("t{i}"), *p))).collect()
}

/// M7-G01: a canary at 1000, a tamper, 3 candidate errors, net −4 reject; net −3 continues.
#[test]
fn m7_g01_screen_rejects() {
    let even = pairs(&[(1000, 1000); 12]);
    let ok = screen_decide(&[], &even, SCREEN_REJECT_ERRORS, SCREEN_REJECT_NET);
    assert_eq!((ok.n, ok.c_pass, ok.p_pass, ok.net, ok.reject), (12, 12, 12, 0, None));
    let mut canary = rec("c", "k1", 1000);
    canary.set = "canary".into();
    assert_eq!(screen_decide(&[canary], &even, 3, 4).reject, Some(ScreenRejectCode::Canary));
    let mut t = even.clone();
    t[5].0.tamper = true;
    t[5].0.score_permille = 0;
    assert_eq!(screen_decide(&[], &t, 3, 4).reject, Some(ScreenRejectCode::Tamper));
    let mut e = even.clone();
    for p in e.iter_mut().take(3) {
        p.0.requests = 0;
        p.0.outcome = "crash".into();
    }
    let o = screen_decide(&[], &e, 3, 4);
    assert_eq!((o.c_errors, o.reject), (3, Some(ScreenRejectCode::ScreenErrors)));
    e[2].0.requests = 1;
    e[2].0.score_permille = 1000;
    assert_eq!(screen_decide(&[], &e, 3, 4).reject, None, "2 errors, net −2: continue");
    let mut spec = vec![(1000, 1000); 12];
    for s in spec.iter_mut().take(4) {
        *s = (0, 1000);
    }
    let o = screen_decide(&[], &pairs(&spec), 3, 4);
    assert_eq!((o.net, o.reject, o.d_permille), (-4, Some(ScreenRejectCode::ScreenInferior), -333));
    spec[3] = (1000, 1000);
    let o = screen_decide(&[], &pairs(&spec), 3, 4);
    assert_eq!((o.net, o.reject), (-3, None));
    // An incumbent error leaves its task out of n and net.
    let mut d = pairs(&spec);
    d[0].1.requests = 0;
    let o = screen_decide(&[], &d, 3, 4);
    assert_eq!((o.n, o.net), (11, -2));
}

fn e3(targeted: bool, c: u32, p: u32) -> E3Task {
    let t = if targeted { "tg" } else { "ct" };
    E3Task { targeted, c: rec("c", t, c), p: rec("p", t, p) }
}

/// EV-21: 0 fixes and 1 control lost; 2 controls lost; a tamper; 2.5× cost.
#[test]
fn ev21_e3_rejects() {
    let pass = e3_decide(&[e3(true, 1000, 0), e3(true, 0, 0), e3(false, 1000, 1000), e3(false, 1000, 1000)], false);
    assert!(pass.pass, "{pass:?}");
    assert_eq!((pass.targeted_fixed, pass.targeted_total, pass.controls_lost, pass.controls_total), (1, 2, 0, 2));
    let v = e3_decide(&[e3(true, 0, 0), e3(false, 0, 1000), e3(false, 1000, 1000)], false);
    assert_eq!(v.reason.as_deref(), Some("screen.mini_inferior"));
    let v = e3_decide(&[e3(true, 1000, 0), e3(false, 0, 1000), e3(false, 0, 1000)], false);
    assert_eq!(v.reason.as_deref(), Some("screen.mini_inferior"), "2 controls lost even with a fix");
    let v = e3_decide(&[e3(true, 1000, 0), e3(false, 0, 1000), e3(false, 1000, 1000)], false);
    assert!(v.pass, "1 fix and 1 control lost passes");
    let mut t = e3(true, 1000, 0);
    t.c.tamper = true;
    assert_eq!(e3_decide(&[t, e3(false, 1000, 1000)], false).reason.as_deref(), Some("screen.tamper"));
    assert_eq!(e3_decide(&[e3(true, 1000, 0)], true).reason.as_deref(), Some("screen.tamper"), "a canary trip");
    let mut a = e3(false, 1000, 1000);
    let mut b = e3(false, 1000, 1000);
    a.c.cost_ref_musd = 2500;
    b.c.cost_ref_musd = 2500;
    let v = e3_decide(&[e3(true, 1000, 0), a.clone(), b.clone()], false);
    assert_eq!((v.reason.as_deref(), v.cand_cost_ref_musd, v.inc_cost_ref_musd), (Some("screen.costlier"), 5000, 2000));
    a.c.cost_ref_musd = 2000;
    b.c.cost_ref_musd = 2000;
    assert!(e3_decide(&[e3(true, 1000, 0), a, b], false).pass, "exactly 2× is not costlier");
}

/// EV-23: one duplicated run, one missing run.
#[test]
fn ev23_grid() {
    let exp = vec![("a".to_string(), "c"), ("a".to_string(), "p"), ("b".to_string(), "c"), ("b".to_string(), "p")];
    let ok = vec![rec("c", "a", 0), rec("p", "a", 0), rec("c", "b", 0), rec("p", "b", 0)];
    assert_eq!(assert_grid(&exp, &ok), Ok(()));
    let mut dup = ok.clone();
    dup.push(rec("c", "a", 0));
    assert!(assert_grid(&exp, &dup).unwrap_err().starts_with("duplicate run c/a"));
    assert!(assert_grid(&exp, &ok[..3]).unwrap_err().starts_with("missing run p/b"));
    let mut extra = ok.clone();
    extra.push(rec("c", "z", 0));
    assert!(assert_grid(&exp, &extra).unwrap_err().starts_with("unexpected run c/z"));
}

/// EV-25 (lean go/no-go): est 160 → go, aim `better`; 120 → go, aim `ratchet`; 60 → no go.
#[test]
fn ev25_lean_go_no_go() {
    assert_eq!((go(160), aim(160)), (true, "better"));
    assert_eq!((go(120), aim(120)), (true, "ratchet"));
    assert_eq!((go(80), go(79)), (true, false));
    assert_eq!((go(60), aim(60)), (false, "below_resolution"));
    // est = round(1000 · 0.5 · s_c · (f+1)/(t+2) · 0.8).
    assert_eq!(est_permille(1000, 4, 4, 500, 800), 333);
    assert_eq!(est_permille(500, 3, 4, 500, 800), 133);
    assert_eq!(est_permille(250, 0, 4, 500, 800), 17);
    assert_eq!(est_permille(0, 4, 4, 500, 800), 0);
    assert_eq!(est_permille(1000, 0, 0, 1000, 1000), 500);
}

fn task(home: &Path, id: &str, labels: &[&str]) -> TaskDef {
    let dir = home.join("pool").join(id);
    std::fs::create_dir_all(dir.join("check")).unwrap();
    std::fs::write(dir.join("task.json"), format!("{{\"v\":3,\"id\":\"{id}\"}}")).unwrap();
    std::fs::write(dir.join("check/run"), b"#!/bin/sh\nexit 0\n").unwrap();
    TaskDef {
        id: id.into(),
        set: RunSet::Pool,
        source: Source::Maintainer,
        labels: labels.iter().map(|s| s.to_string()).collect(),
        family: None,
        dir,
        task: TaskJson {
            v: 3,
            id: id.into(),
            title: None,
            prompt: "do it".into(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
            workspace: WorkspaceRef { file: "workspace.tar".into(), sha256: "0".repeat(64), bytes: 1 },
            check: CheckSpec { argv: vec!["check/run".into()], timeout_s: 60 },
            limits: Limits { wall_s: 600, max_cost_musd: 100_000, max_turns: None, max_output_tokens: None, max_input_tokens: None },
            source: SourceRef { kind: Source::Maintainer, session: None, event_id: None, author_sha: None },
            accepted: None,
            created: "2026-10-20T00:00:00Z".into(),
            family: None,
            template: None,
            difficulty: None,
        },
    }
}

/// A deterministic executor: scores per arm from a table (default 1000), a served model string, and a
/// count of the runs it made.
struct Fake {
    c: BTreeMap<String, u32>,
    p: Mutex<BTreeMap<String, u32>>,
    served: Mutex<String>,
    made: Mutex<Vec<(String, String)>>,
}

impl Fake {
    fn new(c: &[(&str, u32)], p: &[(&str, u32)]) -> Fake {
        Fake {
            c: c.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            p: Mutex::new(p.iter().map(|(k, v)| (k.to_string(), *v)).collect()),
            served: Mutex::new("m-1".into()),
            made: Mutex::new(Vec::new()),
        }
    }
    fn made(&self, arm: &str) -> usize {
        self.made.lock().unwrap().iter().filter(|(a, _)| a == arm).count()
    }
}

impl RunExecutor for Fake {
    fn run(&self, arm: Arm, t: &TaskDef, r: u32, id: &str) -> RunRecord {
        let a = if arm == Arm::C { "c" } else { "p" };
        self.made.lock().unwrap().push((a.into(), t.id.clone()));
        let score = if arm == Arm::C { self.c.get(&t.id).copied() } else { self.p.lock().unwrap().get(&t.id).copied() }.unwrap_or(1000);
        let mut x = rec(a, &t.id, score);
        x.run_id = id.into();
        x.repeat = r;
        x.set = if t.set == RunSet::Canary { "canary".into() } else { "pool".into() };
        x.model_served = Some(self.served.lock().unwrap().clone());
        x
    }
}

fn query() -> CacheQuery {
    CacheQuery {
        arm: ArmKey { body_sha256: "b".repeat(64), ext_set: String::new(), t0_sha256: "c".repeat(40) },
        routed: "deepseek/deepseek-flash".into(),
        routing: "L0".into(),
        runner: HARNESS_V.into(),
        effort: "high".into(),
        reserve_output_tokens: 65_536,
    }
}

/// The runner across gates (M7-G08 on the process path, M7-R09 on the pool): gate 1 fills the cache
/// (incumbent fresh, back to back); gate 2 reuses it with a ⌈20%⌉ sentinel; a served-model change
/// finds nothing; an injected drift invalidates the tuple, alerts and reruns the incumbent fresh.
#[test]
fn cached_pairs_across_gates() {
    let home = test_home("screen-cache");
    let tasks: Vec<TaskDef> = (0..15).map(|i| task(&home, &format!("t{i}"), &["exercise"])).collect();
    let cache = PoolCache::open(&home);
    let q = query();
    let now = 1_800_000_000;
    let cu = CacheUse { cache: &cache, query: &q, now_unix: now, max_age_s: 259_200, sentinel_permille: 200 };
    // Incumbent passes t0..t12, fails t13, t14.
    let fails = [("t13", 0), ("t14", 0)];
    let ex = Fake::new(&[], &fails);
    let g1 = run_cached_pairs("j-000001", &ex, &tasks, Some(&cu), 3);
    assert_eq!((ex.made("c"), ex.made("p"), g1.reused, g1.sentinel), (15, 15, 0, None));
    assert!(g1.pairs.iter().all(|(c, p)| c.task == p.task && c.cache.as_deref() == Some("fresh") && p.cache.as_deref() == Some("fresh")));
    assert_eq!(cache.all_runs().len(), 15);
    // Gate 2: reuse + sentinel ⌈0.2 × 15⌉ = 3.
    let ex2 = Fake::new(&[], &fails);
    let g2 = run_cached_pairs("j-000002", &ex2, &tasks, Some(&cu), 3);
    let s = g2.sentinel.unwrap();
    assert_eq!((ex2.made("c"), ex2.made("p"), g2.reused, s.tasks, s.drift), (15, 3, 12, 3, false));
    assert_eq!(g2.pairs.iter().filter(|(_, p)| p.cache.as_deref() == Some("reused")).count(), 12);
    assert_eq!(g2.pairs.iter().filter(|(_, p)| p.cache.as_deref() == Some("sentinel")).count(), 3);
    assert!(g2.alerts.is_empty());
    let mut ids: Vec<&str> = g2.pairs.iter().map(|(_, p)| p.task.as_str()).collect();
    ids.dedup();
    assert_eq!(ids.len(), 15, "pairs stay in task order");
    assert_eq!(run_cached_pairs("j-000002", &Fake::new(&[], &fails), &tasks, Some(&cu), 1).pairs.iter().map(|(_, p)| p.cache.clone()).collect::<Vec<_>>(),
               g2.pairs.iter().map(|(_, p)| p.cache.clone()).collect::<Vec<_>>(), "seeded: the same sentinel tasks");
    // A served-model change: nothing reusable, the incumbent runs fresh everywhere.
    let ex3 = Fake::new(&[], &[]);
    *ex3.served.lock().unwrap() = "m-2".into();
    let g3 = run_cached_pairs("j-000003", &ex3, &tasks, Some(&cu), 3);
    assert_eq!((ex3.made("p"), g3.reused), (15, 0));
    // Drift: the incumbent now fails everything it passed; on 13 tasks the sentinel runs 3, all
    // cached as passes (Σ p̂ = 3, bound 2.5 · √1) and fails all 3: drift.
    let ids: Vec<String> = (0..15).map(|i| format!("t{i}")).collect();
    let all_fail: Vec<(&str, u32)> = ids.iter().map(|s| (s.as_str(), 0)).collect();
    let ex4 = Fake::new(&[], &all_fail);
    let g4 = run_cached_pairs("j-000004", &ex4, &tasks[..13], Some(&cu), 3);
    let s = g4.sentinel.unwrap();
    assert!(s.drift && s.tasks == 3 && s.passes == 0, "{s:?}");
    assert_eq!(g4.alerts, ["incumbent_drift"]);
    assert_eq!((ex4.made("p"), g4.reused), (13, 0), "sentinel 3 + 10 fresh reruns");
    assert!(g4.pairs.iter().all(|(_, p)| p.score_permille == 0 && p.cache.as_deref() != Some("reused")));
    // After invalidation only the new fresh/sentinel runs remain for this tuple.
    assert!(cache.all_runs().iter().all(|e| e.run.score_permille == 0 || e.run.model_served.as_deref() == Some("m-2")));
    // Without a cache: always fresh.
    let ex5 = Fake::new(&[], &[]);
    let g5 = run_cached_pairs("j-000005", &ex5, &tasks[..3], None, 2);
    assert_eq!((ex5.made("p"), g5.reused), (3, 0));
    remove_home(&home);
}

/// Pool calibration (REAL-M7, M7 line C): incumbent-only, R fresh runs per task with distinct run ids, all
/// cached; a later screen then reuses them (sentinel only) instead of running the incumbent fresh.
#[test]
fn pool_calibration_fills_the_cache() {
    let home = test_home("screen-calib");
    let tasks: Vec<TaskDef> = (0..5).map(|i| task(&home, &format!("t{i}"), &["exercise"])).collect();
    let cache = PoolCache::open(&home);
    let q = query();
    let cu = CacheUse { cache: &cache, query: &q, now_unix: 1_800_000_000, max_age_s: 259_200, sentinel_permille: 200 };
    let ex = Fake::new(&[], &[("t4", 0)]);
    let runs = run_pool_calibration("j-000010", &ex, &tasks, 2, Some(&cu), 3);
    assert_eq!((runs.len(), ex.made("p"), ex.made("c")), (10, 10, 0));
    assert!(runs.iter().all(|r| r.arm == "p" && r.cache.as_deref() == Some("fresh")));
    let mut ids: Vec<&str> = runs.iter().map(|r| r.run_id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 10, "one run id per task and repeat");
    assert_eq!(runs.iter().filter(|r| r.task == "t4" && r.score_permille == 0).count(), 2);
    assert_eq!(cache.all_runs().len(), 10);
    // The next screen pairs every candidate run with a cached incumbent run (plus the sentinel).
    let ex2 = Fake::new(&[], &[("t4", 0)]);
    let g = run_cached_pairs("j-000011", &ex2, &tasks, Some(&cu), 3);
    assert_eq!((ex2.made("c"), ex2.made("p"), g.reused), (5, 1, 4));
    remove_home(&home);
}

/// The G4 screen and E3 drivers end to end on the fake executor: selection, canaries first, the grid.
#[test]
fn drivers_end_to_end() {
    let home = test_home("screen-drivers");
    let mut pool: Vec<TaskDef> = (0..8).map(|i| task(&home, &format!("ex{i}"), &["exercise"])).collect();
    pool.extend((0..6).map(|i| task(&home, &format!("rp{i}"), &["repair"])));
    pool.extend((0..3).map(|i| task(&home, &format!("sm{i}"), &["smoke"])));
    let sel = select_screen("j-000050", &pool, SCREEN_K);
    assert_eq!(sel.len(), 12);
    assert!(sel.iter().all(|id| !id.starts_with("sm")), "smoke never in the screen");
    assert_eq!(sel, select_screen("j-000050", &pool, SCREEN_K));
    assert_ne!(sel, select_screen("j-000051", &pool, SCREEN_K));
    assert!(sel.iter().filter(|i| i.starts_with("rp")).count() == 6, "round-robin over labels");
    let chosen: Vec<TaskDef> = pool.iter().filter(|t| sel.contains(&t.id)).cloned().collect();
    let mut k = task(&home, "k-leak", &[]);
    k.set = RunSet::Canary;
    // A broken candidate: fails 5 tasks the incumbent passes → screen_inferior.
    let mut bad: Vec<(&str, u32)> = sel.iter().take(5).map(|s| (s.as_str(), 0)).collect();
    bad.push(("k-leak", 0));
    let ex = Fake::new(&bad, &[]);
    let (o, runs, _) = run_g4_screen("j-000050", &ex, &chosen, std::slice::from_ref(&k), None, 4);
    assert_eq!((o.n, o.net, o.reject), (12, -5, Some(ScreenRejectCode::ScreenInferior)));
    assert_eq!(runs.len(), 25);
    assert_eq!(runs[0].set, "canary");
    // A canary at 1000 stops before any pool run.
    let ex = Fake::new(&[("k-leak", 1000)], &[]);
    let (o, runs, _) = run_g4_screen("j-000052", &ex, &chosen, std::slice::from_ref(&k), None, 4);
    assert_eq!((o.reject, runs.len()), (Some(ScreenRejectCode::Canary), 1));
    // E3: 2 targeted tasks the incumbent fails, the candidate fixes one; 2 controls.
    let targeted: Vec<TaskDef> = pool[..2].to_vec();
    let passed_ids: BTreeSet<String> = ["rp0", "rp1", "rp2"].iter().map(|s| s.to_string()).collect();
    let ctl_ids = select_controls("j-000060", &pool, &["ex0".into(), "ex1".into()], &passed_ids, E3_CONTROLS);
    assert_eq!(ctl_ids.len(), 2);
    assert!(ctl_ids.iter().all(|c| passed_ids.contains(c)), "controls the incumbent passed come first");
    let controls: Vec<TaskDef> = pool.iter().filter(|t| ctl_ids.contains(&t.id)).cloned().collect();
    let ex = Fake::new(&[("ex1", 0)], &[("ex0", 0), ("ex1", 0)]);
    let (v, runs, _) = run_e3("j-000060", &ex, &targeted, &controls, &[], None, 2);
    assert!(v.pass, "{v:?}");
    assert_eq!((v.targeted_fixed, v.targeted_total, v.controls_lost, v.controls_total, runs.len()), (1, 2, 0, 2, 8));
    let est = est_permille(500, v.targeted_fixed, v.targeted_total, DEFAULT_Q_PERMILLE, DEFAULT_TRANSFER_PERMILLE);
    assert_eq!((est, go(est)), (100, true));
    remove_home(&home);
}
