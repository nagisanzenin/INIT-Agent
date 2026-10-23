//! G4 v2 kernel tests ($0, no model): the lean subset of M7-G01…G15 (build/status/GATE-V2a.md).

use super::*;
use crate::accept::CostBudget;

fn inputs() -> V2Inputs {
    V2Inputs {
        change_type: ChangeType::Feat,
        predicts_sealed_gain: false,
        body_tokens_c: 100,
        body_tokens_p: 100,
        rho_max: 1.5,
        budget: CostBudget { soft_day_musd: 4_000_000, evolution_share_permille: 600, tasks_per_day: 10.0 },
    }
}

/// The pinned answer of build/status/ROOT-S1a.md with another status.
fn answer(status: &str, d: i64, m: i64, cpp: i64) -> SliceAnswer {
    let text = format!(
        r#"{{"seq":123,"status":"{status}","ratchet_signal":false,"unconfirmed":0,"ratchet_k":4,
            "tier":"standard","n":32,"repeats":3,"d_permille":{d},"m_permille":{m},"cpp_ratio_permille":{cpp},
            "looks_left":null,"s_version":"s1.0","dup":false,"spent_musd":371000}}"#
    );
    serde_json::from_str(&text).unwrap()
}

fn screen() -> ScreenSummary {
    ScreenSummary { n: 12, c_pass: 6, p_pass: 6, net: 0, d_permille: 0, ..Default::default() }
}

fn ct(class: CtClass) -> Option<CtStats> {
    let effect = match class {
        CtClass::Cheaper => -0.5,
        CtClass::Costlier => 0.5,
        CtClass::Same => 0.0,
    };
    Some(CtStats { metric: Metric::CostXWall, effect, delta: 0.1, class })
}

fn v(s: &ScreenSummary, a: &SliceAnswer, x: &V2Inputs) -> Verdict {
    accept_v2(s, a, x).verdict
}

/// M7-G06: tier auto against $0.30, $0.70, $1.20, $0.10 (planning $0.0018 per fresh run, 2 gate canaries).
#[test]
fn m7_g06_tier_auto_and_preflight() {
    let pick = |b: u64| preflight(TierCfg::Auto, None, b, 1_800, 2).map(|p| p.tier);
    assert_eq!(pick(300_000), Ok(Tier::Lean));
    assert_eq!(pick(700_000), Ok(Tier::Standard));
    assert_eq!(pick(1_200_000), Ok(Tier::Strong));
    assert_eq!(pick(100_000), Err(InconclusiveReason::BudgetInsufficient));
    // m7 §3.2.6's fresh run counts.
    assert_eq!([Tier::Lean, Tier::Standard, Tier::Strong].map(|t| t.runs_fresh(2)), [127, 223, 351]);
    // A pinned tier is used only if it fits; the escalated re-run's floor is the next tier up.
    assert_eq!(preflight(TierCfg::Fixed(Tier::Standard), None, 1_200_000, 1_800, 2).map(|p| p.tier), Ok(Tier::Standard));
    assert_eq!(preflight(TierCfg::Fixed(Tier::Strong), None, 700_000, 1_800, 2).map(|p| p.tier), Err(InconclusiveReason::BudgetInsufficient));
    assert_eq!(preflight(TierCfg::Auto, Some(Tier::Lean.up()), 300_000, 1_800, 2).map(|p| p.tier), Err(InconclusiveReason::BudgetInsufficient));
    assert_eq!(preflight(TierCfg::Auto, Some(Tier::Standard), 700_000, 1_800, 2).map(|p| p.tier), Ok(Tier::Standard));
    // No estimate known: $0.005 per run.
    let p = preflight(TierCfg::Auto, None, 10_000_000, 0, 2).unwrap();
    assert_eq!((p.tier, p.estimate_musd), (Tier::Strong, 351 * 5_000));
    assert_eq!(TierCfg::parse("auto"), Some(TierCfg::Auto));
    assert_eq!(TierCfg::parse("lean"), Some(TierCfg::Fixed(Tier::Lean)));
    assert_eq!(TierCfg::parse("huge"), None);
}

/// The request and result shapes ROOT-S1a pinned (455b246).
#[test]
fn slice_wire_format_is_the_pinned_one() {
    let a = SliceArgs {
        gate_id: "j-000051".into(),
        candidate: SliceCandidate { body: "ab".repeat(32), ext: SliceExt::default(), t0: None },
        incumbent: "t9a07c2e91d04".into(),
        tier: Tier::Standard.into(),
        budget_musd: 3_000_000,
        wall_s: 5400,
    };
    let got = serde_json::to_string(&a).unwrap();
    let want = format!(
        r#"{{"budget_musd":3000000,"candidate":{{"body":"{}","ext":{{"components":[],"set":null}},"t0":null}},"gate_id":"j-000051","incumbent":"t9a07c2e91d04","tier":{{"n":32,"name":"standard","repeats":3}},"wall_s":5400}}"#,
        "ab".repeat(32)
    );
    let canon = |s: &str| serde_json::to_string(&serde_json::from_str::<Value>(s).unwrap()).unwrap();
    assert_eq!(canon(&got), canon(&want));
    let r = answer("superior", 200, 220, 950);
    assert_eq!(r.parsed(), Some(SliceStatus::Superior));
    assert_eq!((r.looks_left, r.dup, r.s_version.as_deref()), (None, false, Some("s1.0")));
    // A valid test without its statistics is malformed: `error: sealed`, never a verdict on the candidate.
    let mut bad = r.clone();
    bad.d_permille = None;
    assert_eq!(bad.parsed(), None);
    assert_eq!(v(&screen(), &bad, &inputs()), Verdict::Error(ErrorReason::Sealed));
    let unknown = answer("great", 0, 0, 0);
    assert_eq!(v(&screen(), &unknown, &inputs()), Verdict::Error(ErrorReason::Sealed));
    // Tripwires and errors may carry 0s.
    assert_eq!(answer("error:harness_health", 0, 0, 0).parsed(), Some(SliceStatus::Error("harness_health".into())));
    assert_eq!(SliceStatus::parse("error:"), None);
}

/// M7-R13's kernel side: the agent view rounds d to 50, m to 10, cpp to 50 and carries no ids.
#[test]
fn agent_view_is_rounded() {
    let r = answer("tie", 74, 216, 1074);
    let a = r.agent_view();
    assert_eq!((a["d_permille"].as_i64(), a["m_permille"].as_i64(), a["cpp_ratio_permille"].as_i64()), (Some(50), Some(220), Some(1050)));
    assert_eq!(round_to(-75, 50), -100);
    assert_eq!(round_to(-74, 50), -50);
    assert_eq!(round_to(25, 50), 50);
    let keys: Vec<&String> = a.as_object().unwrap().keys().collect();
    for k in keys {
        assert!(["seq", "status", "n", "repeats", "tier", "d_permille", "m_permille", "cpp_ratio_permille", "looks_left", "s_version", "dup", "spent_musd"].contains(&k.as_str()), "{k}");
    }
}

/// Root refusals and `error:<cause>` (orchestrator-approved mapping, build/status/GATE-V2a.md).
#[test]
fn refusals_and_errors_are_never_candidate_verdicts() {
    assert_eq!(SliceRefusal::from_code("root.sealed_none", "").verdict(), None);
    assert_eq!(SliceRefusal::from_code("root.unknown_cmd", "").verdict(), None);
    assert_eq!(SliceRefusal::from_code("root.busy", "").verdict(), Some(Verdict::Inconclusive(InconclusiveReason::SealedUnavailable)));
    assert_eq!(SliceRefusal::from_code("root.mode", "").verdict(), Some(Verdict::Inconclusive(InconclusiveReason::SealedUnavailable)));
    assert_eq!(SliceRefusal::from_code("root.sealed_quota", "").verdict(), Some(Verdict::Inconclusive(InconclusiveReason::SealedQuota)));
    assert_eq!(SliceRefusal::from_code("root.budget", "").verdict(), Some(Verdict::Inconclusive(InconclusiveReason::BudgetInsufficient)));
    assert_eq!(SliceRefusal::from_code("root.store", "").verdict(), Some(Verdict::Error(ErrorReason::Infra)));
    assert_eq!(SliceRefusal::from_code("root.args", "").verdict(), Some(Verdict::Error(ErrorReason::Sealed)));
    let e = |c: &str| v(&screen(), &answer(&format!("error:{c}"), 0, 0, 0), &inputs());
    // M7-G13 (kernel side): the incumbent failed `health` → error, retried once like infra.
    assert_eq!(e("harness_health"), Verdict::Error(ErrorReason::HarnessHealth));
    assert!(ErrorReason::HarnessHealth.retryable() && ErrorReason::CheckerTamper.retryable());
    assert_eq!(e("checker_tamper"), Verdict::Error(ErrorReason::CheckerTamper));
    assert_eq!(e("budget"), Verdict::Inconclusive(InconclusiveReason::Budget));
    assert_eq!(e("wall"), Verdict::Inconclusive(InconclusiveReason::Timeout));
    assert_eq!(e("runs_failed:cost_capped"), Verdict::Error(ErrorReason::RunsFailedCostCapped));
    assert_eq!(e("internal"), Verdict::Error(ErrorReason::Sealed));
    assert_eq!(e("cancelled"), Verdict::Error(ErrorReason::Cancelled));
}

/// M7-G02: one synthetic case per branch of m7 §3.2.5 (lean: no capability path).
#[test]
fn m7_g02_accept_v2_branches() {
    let x = inputs();
    let s = screen();
    // Screen tripwires first (M7-G01's reject order is GATE-V2b's; here: they win over any slice).
    let mut k = s.clone();
    k.canary_trip = true;
    assert_eq!(v(&k, &answer("superior", 300, 200, 900), &x), Verdict::Reject(RejectReason::Canary));
    for (r, want) in [
        (ScreenReject::Tamper, RejectReason::Tamper),
        (ScreenReject::ScreenErrors, RejectReason::ScreenErrors),
        (ScreenReject::ScreenInferior, RejectReason::ScreenInferior),
        (ScreenReject::Canary, RejectReason::Canary),
    ] {
        let mut t = s.clone();
        t.reject = Some(r);
        assert_eq!(v(&t, &answer("superior", 300, 200, 900), &x), Verdict::Reject(want));
    }
    // Slice tripwires: the candidate's fault → reject (M7-G13: candidate fails health → reject: health).
    for (st, want) in [("canary", RejectReason::Canary), ("tamper", RejectReason::Tamper), ("leak", RejectReason::Leak), ("health", RejectReason::Health)] {
        assert_eq!(v(&s, &answer(st, 0, 0, 0), &x), Verdict::Reject(want), "{st}");
    }
    assert_eq!(v(&s, &answer("inferior", -300, 200, 1000), &x), Verdict::Reject(RejectReason::ScoreInferior));
    assert_eq!(v(&s, &answer("inferior_early", -400, 200, 1000), &x), Verdict::Reject(RejectReason::ScoreInferiorEarly));
    // Superior → better (cost per pass within 1.5).
    let d = accept_v2(&s, &answer("superior", 200, 220, 950), &x);
    assert_eq!(d.verdict, Verdict::Accept(AcceptReason::Better));
    assert!(!d.milestone_required);
    assert_eq!(accept_class(&d.verdict).as_deref(), Some("better"));
    // Divergence: the screen far above the slice.
    let mut hi = s.clone();
    hi.d_permille = 1000;
    assert_eq!(v(&hi, &answer("tie", 0, 200, 1000), &x), Verdict::Reject(RejectReason::PoolHoldoutDivergence));
    // No screen (n = 0): no divergence guard.
    let none = ScreenSummary::default();
    assert_eq!(accept_v2(&none, &answer("tie", 0, 200, 1000), &x).divergence, None);
    // perf: tie or superior plus a cost or wall gain.
    let perf = V2Inputs { change_type: ChangeType::Perf, ..x.clone() };
    let mut pg = s.clone();
    pg.perf_gain = Some(true);
    assert_eq!(v(&pg, &answer("tie", 0, 200, 900), &perf), Verdict::Accept(AcceptReason::PerfGain));
    assert_eq!(accept_class(&Verdict::Accept(AcceptReason::PerfGain)).as_deref(), Some("tie_break:perf_gain"));
    assert_eq!(v(&s, &answer("tie", 0, 200, 900), &perf), Verdict::Reject(RejectReason::PerfNoGain));
    assert_eq!(v(&pg, &answer("tie_negative", -50, 200, 900), &perf), Verdict::Reject(RejectReason::PerfNoGain));
    // Tie-breaks on a tie: cheaper, costlier, smaller, no_gain.
    let mut ch = s.clone();
    ch.ct = ct(CtClass::Cheaper);
    let a = v(&ch, &answer("tie", 0, 200, 900), &x);
    assert_eq!(a, Verdict::Accept(AcceptReason::Cheaper));
    assert_eq!(accept_class(&a).as_deref(), Some("tie_break:cheaper"));
    let mut co = s.clone();
    co.ct = ct(CtClass::Costlier);
    assert_eq!(v(&co, &answer("tie", 0, 200, 900), &x), Verdict::Reject(RejectReason::Costlier));
    let small = V2Inputs { body_tokens_c: 90, ..x.clone() };
    assert_eq!(v(&s, &answer("tie", 0, 200, 900), &small), Verdict::Accept(AcceptReason::Smaller));
    assert_eq!(v(&s, &answer("tie", 0, 200, 900), &x), Verdict::Reject(RejectReason::NoGain));
}

/// M7-G03: a cheaper candidate with D < 0, not significant → `no_gain` (the tie guard).
#[test]
fn m7_g03_tie_guard() {
    let mut ch = screen();
    ch.ct = ct(CtClass::Cheaper);
    assert_eq!(v(&ch, &answer("tie_negative", -50, 200, 800), &inputs()), Verdict::Reject(RejectReason::NoGain));
    let small = V2Inputs { body_tokens_c: 10, ..inputs() };
    assert_eq!(v(&screen(), &answer("tie_negative", -50, 200, 800), &small), Verdict::Reject(RejectReason::NoGain));
}

/// M7-G04: superior with a cost-per-pass ratio of 1.6 → `costlier_per_pass`; 1.5 passes.
#[test]
fn m7_g04_costlier_per_pass() {
    assert_eq!(v(&screen(), &answer("superior", 250, 200, 1600), &inputs()), Verdict::Reject(RejectReason::CostlierPerPass));
    assert_eq!(v(&screen(), &answer("superior", 250, 200, 1500), &inputs()), Verdict::Accept(AcceptReason::Better));
    // cost_guard on top: dearer than ρ_max on the screen and the day does not fit.
    let mut dear = screen();
    dear.ct = Some(CtStats { metric: Metric::CostXWall, effect: 1.0, delta: 0.1, class: CtClass::Costlier });
    dear.c_cost_musd = Some(1_000_000);
    assert_eq!(v(&dear, &answer("superior", 250, 200, 1000), &inputs()), Verdict::Reject(RejectReason::CostlierThanBudget));
    dear.c_cost_musd = Some(1_000);
    assert_eq!(v(&dear, &answer("superior", 250, 200, 1000), &inputs()), Verdict::Accept(AcceptReason::Better));
}

/// M7-G05: an underpowered tie that a tie-break would accept → `inconclusive: underpowered` (the
/// escalated re-run goes one tier up; the Root makes no second superiority look); otherwise `no_gain`.
#[test]
fn m7_g05_underpowered() {
    let mut ch = screen();
    ch.ct = ct(CtClass::Cheaper);
    assert_eq!(v(&ch, &answer("underpowered", 0, 300, 900), &inputs()), Verdict::Inconclusive(InconclusiveReason::Underpowered));
    assert_eq!(v(&screen(), &answer("underpowered", 0, 300, 900), &inputs()), Verdict::Reject(RejectReason::NoGain));
    assert_eq!(crate::accept::resolve(Verdict::Inconclusive(InconclusiveReason::Underpowered), false, 4), crate::accept::Resolution::Escalate { repeats: 4 });
    assert_eq!(Tier::Standard.up(), Tier::Strong);
}

/// M7-G15 (kernel side): the ratchet.
#[test]
fn m7_g15_ratchet() {
    let claim = V2Inputs { predicts_sealed_gain: true, ..inputs() };
    let mut a = answer("tie", 50, 200, 1000);
    a.ratchet_signal = Some(true);
    a.unconfirmed = Some(1);
    a.ratchet_k = Some(4);
    let d = accept_v2(&screen(), &a, &claim);
    assert_eq!(d.verdict, Verdict::Accept(AcceptReason::LineageGain));
    assert!(d.milestone_required);
    assert_eq!(accept_class(&d.verdict).as_deref(), Some("lineage_gain"));
    // Without a pre-registered sealed claim: no_gain.
    assert_eq!(v(&screen(), &a, &inputs()), Verdict::Reject(RejectReason::NoGain));
    // Costlier per pass than RHO_RATCHET (1.2): no ratchet → the tie-breaks → no_gain.
    let mut dear = a.clone();
    dear.cpp_ratio_permille = Some(1300);
    assert_eq!(v(&screen(), &dear, &claim), Verdict::Reject(RejectReason::NoGain));
    // Costlier on cost × wall (screen): no ratchet.
    let mut co = screen();
    co.ct = ct(CtClass::Costlier);
    assert_eq!(v(&co, &a, &claim), Verdict::Reject(RejectReason::Costlier));
    // The 5th in a span of 4: ratchet_full.
    let mut full = a.clone();
    full.unconfirmed = Some(4);
    assert_eq!(v(&screen(), &full, &claim), Verdict::Reject(RejectReason::RatchetFull));
    // No signal, or a Root without the span fields: no ratchet.
    let mut nosig = a.clone();
    nosig.ratchet_signal = Some(false);
    assert_eq!(v(&screen(), &nosig, &claim), Verdict::Reject(RejectReason::NoGain));
    let mut nospan = a.clone();
    nospan.ratchet_k = None;
    assert_eq!(v(&screen(), &nospan, &claim), Verdict::Reject(RejectReason::NoGain));
    // Only a tie: superior is `better`, tie_negative never ratchets.
    let mut neg = a.clone();
    neg.status = "tie_negative".into();
    assert_eq!(v(&screen(), &neg, &claim), Verdict::Reject(RejectReason::NoGain));
}

/// `Predicts: pass_rate@sealed …` (evolution.md §3.10.1 scope `sealed`).
#[test]
fn sealed_predicts_scope() {
    use crate::evo::predicts::{parse_clause, predicts_sealed_gain};
    let c = parse_clause("pass_rate@sealed >= parent+15%").unwrap();
    assert!(c.is_sealed_gain());
    assert_eq!(c.to_string(), "pass_rate@sealed >= parent+15%");
    assert!(!parse_clause("pass_rate@sealed >= parent").unwrap().is_sealed_gain());
    assert!(!parse_clause("pass_rate@sealed >= parent+0%").unwrap().is_sealed_gain());
    assert!(parse_clause("pass_rate@sealed > parent+5%").unwrap().is_sealed_gain());
    for bad in ["cost_usd@sealed <= parent", "pass_rate@sealed <= parent", "pass_rate@sealed >= parent-5%", "pass_rate@sealed >= 0.5"] {
        assert!(parse_clause(bad).is_err(), "{bad}");
    }
    // The m7 §3.7 spelling does not parse (PROOF-LEAN rules).
    assert!(parse_clause("pass_rate >= parent+0.15 sealed").is_err());
    let all = crate::evo::predicts::parse_predicts("pass_rate@sealed >= parent+15%; cost_usd <= parent+30%");
    assert!(all.parse_errors.is_empty(), "{:?}", all.parse_errors);
    assert!(predicts_sealed_gain(&all.clauses));
}

/// The screen statistics from paired runs (GATE-V2b's runner calls this).
#[test]
fn screen_stats_from_runs() {
    let run = |task: &str, arm: &str, score: u32, cost: u64, requests: u64| RunRecord {
        v: crate::g4::RUN_RECORD_V,
        task: task.into(),
        arm: arm.into(),
        set: "pool".into(),
        repeat: 1,
        outcome: "done".into(),
        score_permille: score,
        cost_ref_musd: cost,
        wall_ms: 10_000,
        requests,
        ..Default::default()
    };
    let runs = vec![
        run("a", "c", 1000, 100, 1),
        run("a", "p", 0, 200, 1),
        run("b", "c", 0, 100, 0), // a candidate run error: 0 at the incumbent's values
        run("b", "p", 1000, 200, 1),
        run("c", "c", 1000, 100, 1),
        run("c", "p", 1000, 200, 0), // an incumbent run error: the task is left out
    ];
    let noise = ScreenNoise { pass_pool: Some(0.05), cost_x_wall: Some(0.1), cost_usd: Some(0.1), wall_s: Some(0.1) };
    let st = screen_stats(&runs, &noise, true);
    assert_eq!(st.n, 2);
    assert_eq!(st.d_permille, 0);
    assert_eq!(st.delta_permille, Some(50));
    let ct = st.ct.unwrap();
    assert_eq!(ct.class, CtClass::Cheaper, "{ct:?}");
    assert_eq!(st.perf_gain, Some(true));
    let none = screen_stats(&runs, &ScreenNoise::default(), true);
    assert_eq!((none.ct, none.perf_gain), (None, None));
    assert_eq!(screen_stats(&[], &noise, true), ScreenStats::default());
}

/// M7-G12: the G4 record v3 keys; no existing key changes; `sealed: null` on the v0.3 path.
#[test]
fn m7_g12_record_v3() {
    let rec = V2Record {
        tier: Some(Tier::Standard),
        preflight: Some(Preflight { tier: Tier::Standard, runs: 223, estimate_musd: 401_400 }),
        screen: Some(screen()),
        sealed: Some(answer("superior", 210, 216, 960)),
        sealed_refusal: None,
        milestone_required: false,
        accept_class: Some("better".into()),
        divergence: Some((0.1, 0.3)),
    };
    let mut v = json!({"status": "accept", "reason": "better", "spent_musd": 1000, "runs": 31});
    rec.extend(&mut v);
    assert_eq!(v["tier"], "standard");
    assert_eq!(v["screen"], json!({"n": 12, "c_pass": 6, "p_pass": 6, "net": 0, "verdict": "continue"}));
    assert_eq!(v["sealed"]["d_permille"], 200);
    assert_eq!(v["sealed"]["m_permille"], 220);
    assert_eq!(v["sealed"]["seq"], 123);
    assert_eq!(v["cpp"], json!({"ratio_permille": 950}));
    assert_eq!(v["milestone_required"], false);
    assert_eq!(v["accept_class"], "better");
    assert_eq!(v["spent_musd"], 372_000, "the slice's spend is part of G4's");
    assert_eq!((v["status"].as_str(), v["reason"].as_str(), v["runs"].as_u64()), (Some("accept"), Some("better"), Some(31)));
    let status = rec.status_view("j-000051", &Verdict::Accept(AcceptReason::Better));
    assert_eq!(status["accept_class"], "better");
    assert!(status.to_string().len() < 1000);
}
