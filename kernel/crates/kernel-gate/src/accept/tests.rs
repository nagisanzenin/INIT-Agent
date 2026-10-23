//! G-22: one synthetic case per branch of the accept rule (gate.md §3.9) and of §3.9.2; plus G-43,
//! G-49 and the unit half of G-44 (mechanical inconclusive resolution).

use super::*;
use crate::stats::data::{RunValues, Source};
use crate::stats::noise::NoiseHistory;

const R: usize = 2;

/// A task with R = 2. `d`: +1 the candidate passes and LKG fails, 0 both pass, −1 the reverse.
fn t(id: &str, set: TaskSet, source: Source, d: i8) -> TaskRuns {
    let (c, p) = match d {
        1 => (1000, 0),
        0 => (1000, 1000),
        _ => (0, 1000),
    };
    let run = |s: u32| RunValues {
        score_permille: s,
        cost_ref_musd: 20_000,
        wall_ms: 100_000,
        turns: 10,
        tokens_in: 10_000,
        tokens_out: 500,
        gap_ms_p50: 2000,
        cache_read: 9000,
        input_total: 10_000,
    };
    TaskRuns {
        id: id.into(),
        set,
        source,
        family: None,
        c: vec![run(c); R],
        p: vec![run(p); R],
    }
}

/// 10 maintainer pool tasks and 6 holdout tasks; `pool_d[i]` / `holdout_d[i]` give the per-task outcome
/// (missing entries tie).
fn data(pool_d: &[i8], holdout_d: &[i8]) -> G4Data {
    let mut tasks = Vec::new();
    for i in 0..10 {
        tasks.push(t(
            &format!("pool-{i}"),
            TaskSet::Pool,
            Source::Maintainer,
            *pool_d.get(i).unwrap_or(&0),
        ));
    }
    for i in 0..6 {
        tasks.push(t(
            &format!("hold-{i}"),
            TaskSet::Holdout,
            Source::Maintainer,
            *holdout_d.get(i).unwrap_or(&0),
        ));
    }
    G4Data {
        gate_id: "j-000022".into(),
        change_type: ChangeType::Feat,
        capability_labels: vec![],
        tasks,
        canaries: vec![0, 0],
        capability: vec![],
        repro: vec![],
        body_tokens_c: 21_000,
        body_tokens_p: 21_000,
        budget: CostBudget {
            soft_day_musd: 4_000_000,
            evolution_share_permille: 600,
            tasks_per_day: 10.0,
        },
        top_up_allowed: false,
        config: G4Config::default(),
    }
}

/// δ: 0.05 on the pass rate (m = 0.10 at n = 16), 0.05 on every log metric.
fn deltas() -> Deltas {
    Deltas::uniform(0.05, 0.05)
}

fn scale_c_cost(g: &mut G4Data, factor: f64) {
    for task in &mut g.tasks {
        for r in &mut task.c {
            r.cost_ref_musd = (r.cost_ref_musd as f64 * factor).round() as u64;
        }
    }
}

fn verdict(g: &G4Data, d: &Deltas) -> Verdict {
    accept(g, d)
        .decision
        .verdict()
        .expect("a verdict, not a top-up")
}

use AcceptReason as A;
use InconclusiveReason as I;
use RejectReason as Rj;

#[test]
fn canary_trip_rejects_first() {
    let mut g = data(&[1, 1, 1], &[1]);
    g.canaries = vec![0, 1000];
    let e = accept(&g, &deltas());
    assert_eq!(e.decision, Decision::Verdict(Verdict::Reject(Rj::Canary)));
    assert_eq!(e.stats.canary_max_permille, 1000);
    assert_eq!(
        Verdict::Reject(Rj::Canary).code().as_deref(),
        Some("gate.g4.canary")
    );
}

#[test]
fn below_minimums_is_insufficient() {
    let mut g = data(&[], &[]);
    g.tasks
        .retain(|t| t.set == TaskSet::Holdout || t.id.as_str() < "pool-5");
    assert_eq!(
        verdict(&g, &deltas()),
        Verdict::Inconclusive(I::Insufficient)
    );
    let mut g = data(&[], &[]);
    g.canaries.clear();
    assert_eq!(
        verdict(&g, &deltas()),
        Verdict::Inconclusive(I::Insufficient)
    );
}

#[test]
fn margin_above_max_tops_up_once_then_is_underpowered() {
    let g = data(&[], &[]);
    let wide = Deltas::uniform(0.10, 0.05); // m = 0.20 > 0.15
    assert_eq!(verdict(&g, &wide), Verdict::Inconclusive(I::Underpowered));
    let mut g2 = g.clone();
    g2.top_up_allowed = true;
    match accept(&g2, &wide).decision {
        Decision::TopUp { margin, n_needed } => {
            assert!((margin - 0.20).abs() < 1e-12);
            assert_eq!(n_needed, 29); // 16 · (0.20 / 0.15)² = 28.4
        }
        other => panic!("expected a top-up, got {other:?}"),
    }
}

#[test]
fn clear_score_regression_is_inferior() {
    let g = data(&[-1, -1, -1, -1], &[-1, -1]);
    assert_eq!(verdict(&g, &deltas()), Verdict::Reject(Rj::ScoreInferior));
}

#[test]
fn split_guards_reject_pool_and_holdout_losses() {
    // One lost task: overall non-inferior with a wide margin, but beyond 2·δ on its split. D14 floors
    // only the divergence guard: δ_p = 0.02 is below its floor (0.05 on 10 pool tasks) and the split
    // guard still uses it, so one lost task (d_pool = −0.1) trips it.
    let mut d = Deltas::uniform(0.12, 0.05);
    d.pass_pool = 0.02;
    d.pass_holdout = 0.02;
    let mut g = data(&[-1], &[]);
    g.config.margin_max = 0.30;
    let e = accept(&g, &d);
    assert_eq!(
        e.decision,
        Decision::Verdict(Verdict::Reject(Rj::PoolInferior))
    );
    let s = e.stats.score.unwrap();
    assert_eq!(
        (s.pool.delta, s.pool.delta_min, s.pool.delta_floored),
        (0.02, 0.05, 0.05)
    );
    let mut g = data(&[], &[-1]);
    g.config.margin_max = 0.30;
    assert_eq!(verdict(&g, &d), Verdict::Reject(Rj::HoldoutInferior));
}

/// D14, G-22 style, the gate-of-gates' C1 (WP8c finding 1): a true gain that the 1-task holdout does not
/// exercise, measured by a deterministic model (δ = 0 everywhere). Without the floor the divergence
/// guard's threshold is 2·√(0 + 0) = 0 and any pool gain is `pool_holdout_divergence`; with δ_p ≥ 1/8
/// and δ_h ≥ 1/2 the threshold is 2·√(1/64 + 1/4) ≈ 1.03, so the gain passes every guard and the NI
/// test (L > 0). A 1-task holdout cannot show that it generalizes, so D14's holdout rule makes it
/// `inconclusive: underpowered:holdout`, not `better`. The split guards keep the raw δ, so a loss on the
/// holdout is still caught.
#[test]
fn d14_c1_gain_unexercised_by_a_one_task_holdout_is_not_divergence() {
    let mut g = data(&[1, 1, 1, 0], &[0]);
    g.tasks
        .retain(|t| ["pool-0", "pool-1", "pool-2", "pool-3", "hold-0"].contains(&t.id.as_str()));
    g.config.min_pool = 4;
    g.config.min_holdout = 1;
    let zero = Deltas::uniform(0.0, 0.0);
    let e = accept(&g, &zero);
    assert_eq!(
        e.decision,
        Decision::Verdict(Verdict::Inconclusive(I::UnderpoweredHoldout))
    );
    assert!(!e.audit_required);
    // Without the holdout rule the same data is `better` (the rule proper).
    assert_eq!(
        accept_rule(&g, &zero, None).0,
        Decision::Verdict(Verdict::Accept(A::Better))
    );
    let s = e.stats.score.unwrap();
    assert!(s.l > 0.0);
    assert_eq!((s.pool.d, s.holdout.d), (0.75, 0.0));
    assert_eq!((s.delta, s.delta_min, s.margin), (0.0, 0.1, 0.1));
    assert_eq!(
        (s.pool.delta, s.pool.delta_min, s.pool.delta_floored),
        (0.0, 0.125, 0.125)
    );
    assert_eq!(
        (
            s.holdout.delta,
            s.holdout.delta_min,
            s.holdout.delta_floored
        ),
        (0.0, 0.5, 0.5)
    );
    // The pre-D14 rule (guards on the raw δ = 0) rejected exactly this case.
    assert!(s.pool.d - s.holdout.d > 2.0 * (s.pool.delta.powi(2) + s.holdout.delta.powi(2)).sqrt());
    // The floor does not hide divergence a real holdout shows: G-49's data with δ = 0 still diverges
    // (threshold 2·√(0.05² + 0.083²) ≈ 0.19 < 0.4).
    let g = data(&[1, 1, 1, 1], &[]);
    assert_eq!(
        verdict(&g, &zero),
        Verdict::Reject(Rj::PoolHoldoutDivergence)
    );
    // With a 4-task holdout the rule stands aside: a gain the holdout shows is `better`; the same pool
    // gain with an unexercised 4-task holdout is divergence (0.75 > 2·√(0.125² + 0.125²) ≈ 0.35).
    let mut g = data(&[1, 1, 1, 0], &[1, 1, 0, 0]);
    g.tasks
        .retain(|t| t.set == TaskSet::Pool && t.id.as_str() < "pool-4" || t.id.as_str() < "hold-4");
    g.config.min_pool = 4;
    assert_eq!(g.count(TaskSet::Holdout), 4);
    assert_eq!(verdict(&g, &zero), Verdict::Accept(A::Better));
    let mut g = data(&[1, 1, 1, 0], &[]);
    g.tasks
        .retain(|t| t.set == TaskSet::Pool && t.id.as_str() < "pool-4" || t.id.as_str() < "hold-4");
    g.config.min_pool = 4;
    assert_eq!(
        verdict(&g, &zero),
        Verdict::Reject(Rj::PoolHoldoutDivergence)
    );
    // Nor does it admit a regression: the same 5 tasks with one loss are `score_inferior` (m = 0.1).
    let mut g = data(&[1, 1, -1, 0], &[0]);
    g.tasks
        .retain(|t| ["pool-0", "pool-1", "pool-2", "pool-3", "hold-0"].contains(&t.id.as_str()));
    g.config.min_pool = 4;
    g.config.min_holdout = 1;
    assert_ne!(verdict(&g, &zero), Verdict::Accept(A::Better));
    // A loss on the 1-task holdout trips the (unfloored) holdout guard, as before D14 (a wide margin
    // keeps the NI test from rejecting it first; δ_h = 0 stays below its floor of 0.5).
    let mut g = data(&[1, 1, 1, 0], &[-1]);
    g.tasks
        .retain(|t| ["pool-0", "pool-1", "pool-2", "pool-3", "hold-0"].contains(&t.id.as_str()));
    g.config.min_pool = 4;
    g.config.min_holdout = 1;
    g.config.margin_max = 1.0;
    let mut wide = zero.clone();
    wide.pass_all = 0.4;
    assert_eq!(verdict(&g, &wide), Verdict::Reject(Rj::HoldoutInferior));
}

/// Keeps the first `k` holdout tasks (and every pool task), with `min_holdout = k`.
fn holdout_of(mut g: G4Data, k: usize) -> G4Data {
    let keep: Vec<String> = (0..k).map(|i| format!("hold-{i}")).collect();
    g.tasks
        .retain(|t| t.set == TaskSet::Pool || keep.contains(&t.id));
    g.config.min_holdout = k;
    g
}

/// D14's holdout rule, G-22 style: an accept that rests on a score gain (`better`, `capability_gain`,
/// `repro_fixed`) on a holdout of fewer than 4 tasks is `inconclusive: underpowered:holdout`; at 4 it
/// stands. The tie-break accepts (`cheaper`, `smaller`) and `perf_gain` are unaffected, and a reject
/// stays a reject.
#[test]
fn d14_score_gain_needs_a_holdout_of_four() {
    let uh = Verdict::Inconclusive(I::UnderpoweredHoldout);
    assert_eq!(GAIN_MIN_HOLDOUT, 4);
    assert_eq!(uh.kind(), "inconclusive");
    assert_eq!(uh.reason(), "underpowered:holdout");
    assert_eq!(uh.code().as_deref(), Some("gate.g4.underpowered:holdout"));
    assert_eq!(
        serde_json::to_value(uh).unwrap(),
        serde_json::json!({"verdict": "inconclusive", "reason": "underpowered:holdout"})
    );
    // better: a gain on pool and holdout alike.
    let g = data(&[1, 1, 1, 1], &[1, 1, 1, 0]);
    assert_eq!(
        verdict(&holdout_of(g.clone(), 4), &deltas()),
        Verdict::Accept(A::Better)
    );
    for k in 1..4 {
        let e = accept(&holdout_of(g.clone(), k), &deltas());
        assert_eq!(e.decision, Decision::Verdict(uh), "holdout {k}");
        assert!(!e.audit_required && e.stats.score.is_some());
    }
    // capability_gain.
    let mut g = data(&[], &[]);
    g.capability_labels = vec!["interactive".into()];
    g.capability = vec![
        cap_family("fam-a", 2, 1000, 0),
        cap_family("fam-b", 2, 1000, 0),
    ];
    assert_eq!(
        verdict(&holdout_of(g.clone(), 4), &deltas()),
        Verdict::Accept(A::CapabilityGain)
    );
    assert_eq!(verdict(&holdout_of(g, 1), &deltas()), uh);
    // repro_fixed.
    let mut g = data(&[], &[]);
    g.repro = vec![ReproEvidence::Frozen {
        reference: "task:2026-10-20-03".into(),
        c: vec![1000, 1000],
        p: vec![0, 0],
    }];
    assert_eq!(
        verdict(&holdout_of(g.clone(), 4), &deltas()),
        Verdict::Accept(A::ReproFixed)
    );
    assert_eq!(verdict(&holdout_of(g, 3), &deltas()), uh);
    // Unaffected on a 1-task holdout: cheaper, smaller, perf_gain, and the rejects.
    let mut g = holdout_of(data(&[], &[]), 1);
    scale_c_cost(&mut g, 0.5);
    assert_eq!(verdict(&g, &deltas()), Verdict::Accept(A::Cheaper));
    let mut g = holdout_of(data(&[], &[]), 1);
    g.body_tokens_c = 1;
    assert_eq!(verdict(&g, &deltas()), Verdict::Accept(A::Smaller));
    let mut g = holdout_of(data(&[], &[]), 1);
    g.change_type = ChangeType::Perf;
    scale_c_cost(&mut g, 0.5);
    assert_eq!(verdict(&g, &deltas()), Verdict::Accept(A::PerfGain));
    let g = holdout_of(data(&[], &[]), 1);
    assert_eq!(verdict(&g, &deltas()), Verdict::Reject(Rj::NoGain));
    let mut g = holdout_of(data(&[1, 1, 1, 1], &[1]), 1);
    scale_c_cost(&mut g, 3.0);
    g.budget.tasks_per_day = 1e6;
    assert_eq!(
        verdict(&g, &deltas()),
        Verdict::Reject(Rj::CostlierThanBudget)
    );
}

/// G-49: pool +0.4, holdout 0, δ_p = δ_h = 0.08 → `pool_holdout_divergence` (testing.md MS-5).
#[test]
fn g49_divergence_guard() {
    let mut d = Deltas::uniform(0.05, 0.05);
    d.pass_pool = 0.08;
    d.pass_holdout = 0.08;
    let g = data(&[1, 1, 1, 1], &[]);
    let e = accept(&g, &d);
    assert_eq!(
        e.decision,
        Decision::Verdict(Verdict::Reject(Rj::PoolHoldoutDivergence))
    );
    let s = e.stats.score.unwrap();
    assert!((s.pool.d - 0.4).abs() < 1e-12 && s.holdout.d == 0.0);
    // The same pool gain mirrored on the holdout is not divergence.
    let g = data(&[1, 1, 1, 1], &[1, 1, 1]);
    assert_ne!(verdict(&g, &d), Verdict::Reject(Rj::PoolHoldoutDivergence));
}

#[test]
fn perf_gain_and_perf_no_gain() {
    let mut g = data(&[], &[]);
    g.change_type = ChangeType::Perf;
    assert_eq!(verdict(&g, &deltas()), Verdict::Reject(Rj::PerfNoGain));
    scale_c_cost(&mut g, 0.5);
    assert_eq!(verdict(&g, &deltas()), Verdict::Accept(A::PerfGain));
}

#[test]
fn better_within_rho_max() {
    let mut g = data(&[1, 1, 1, 1], &[1, 1]);
    assert_eq!(verdict(&g, &deltas()), Verdict::Accept(A::Better));
    scale_c_cost(&mut g, 1.4);
    assert_eq!(verdict(&g, &deltas()), Verdict::Accept(A::Better));
}

#[test]
fn better_above_rho_max_is_judged_against_the_task_part_of_the_soft_day() {
    let mut g = data(&[1, 1, 1, 1], &[1, 1]);
    scale_c_cost(&mut g, 2.0); // T_cost(c) = 40,000 µUSD
    // 10 tasks/day × 40,000 = 400,000 ≤ 4,000,000 × 0.4 = 1,600,000
    assert_eq!(verdict(&g, &deltas()), Verdict::Accept(A::Better));
    // 100 tasks/day × 40,000 = 4,000,000 > 1,600,000
    g.budget.tasks_per_day = 100.0;
    assert_eq!(
        verdict(&g, &deltas()),
        Verdict::Reject(Rj::CostlierThanBudget)
    );
}

fn cap_family(name: &str, instances: usize, c: u32, p: u32) -> CapFamily {
    CapFamily::from_instances(name, &vec![(vec![c; R], vec![p; R]); instances])
}

#[test]
fn capability_gain_on_the_sealed_half() {
    let mut g = data(&[], &[]);
    g.capability_labels = vec!["interactive".into()];
    g.capability = vec![
        cap_family("fam-a", 2, 1000, 0),
        cap_family("fam-b", 2, 1000, 0),
    ];
    let e = accept(&g, &deltas());
    assert_eq!(
        e.decision,
        Decision::Verdict(Verdict::Accept(A::CapabilityGain))
    );
    assert!(
        e.audit_required,
        "capability_gain always rests on agent tasks"
    );
    let cap = e.stats.capability.unwrap();
    assert_eq!((cap.n, cap.n_families), (4, 2));
}

/// G-43 and "a capability claim with no eligible tasks": the path does not apply; tie-breaks decide.
#[test]
fn g43_capability_claim_without_enough_tasks_falls_through() {
    let mut g = data(&[], &[]);
    g.capability_labels = vec!["interactive".into()];
    g.capability = vec![
        cap_family("fam-a", 2, 1000, 0),
        cap_family("fam-b", 1, 1000, 0),
    ];
    let e = accept(&g, &deltas());
    assert_eq!(e.decision, Decision::Verdict(Verdict::Reject(Rj::NoGain)));
    assert_eq!(
        e.stats.capability.unwrap().status,
        crate::stats::boot::CapStatus::Insufficient
    );
    g.capability.clear();
    assert_eq!(verdict(&g, &deltas()), Verdict::Reject(Rj::NoGain));
    // Without a claim A is ignored even if it would show a gain.
    let mut g = data(&[], &[]);
    g.capability = vec![
        cap_family("fam-a", 2, 1000, 0),
        cap_family("fam-b", 2, 1000, 0),
    ];
    assert_eq!(verdict(&g, &deltas()), Verdict::Reject(Rj::NoGain));
}

#[test]
fn capability_gain_dearer_than_budget_is_rejected() {
    let mut g = data(&[], &[]);
    g.capability_labels = vec!["interactive".into()];
    g.capability = vec![
        cap_family("fam-a", 2, 1000, 0),
        cap_family("fam-b", 2, 1000, 0),
    ];
    scale_c_cost(&mut g, 2.0);
    g.budget.tasks_per_day = 100.0;
    assert_eq!(
        verdict(&g, &deltas()),
        Verdict::Reject(Rj::CostlierThanBudget)
    );
}

#[test]
fn tie_breaks_cheaper_costlier_smaller_no_gain() {
    let mut g = data(&[], &[]);
    assert_eq!(verdict(&g, &deltas()), Verdict::Reject(Rj::NoGain));
    g.body_tokens_c = 20_000;
    assert_eq!(verdict(&g, &deltas()), Verdict::Accept(A::Smaller));
    g.body_tokens_c = 22_000;
    assert_eq!(verdict(&g, &deltas()), Verdict::Reject(Rj::NoGain));
    let mut cheap = data(&[], &[]);
    scale_c_cost(&mut cheap, 0.5);
    assert_eq!(verdict(&cheap, &deltas()), Verdict::Accept(A::Cheaper));
    let mut dear = data(&[], &[]);
    scale_c_cost(&mut dear, 2.0);
    assert_eq!(verdict(&dear, &deltas()), Verdict::Reject(Rj::Costlier));
    // A cheaper candidate that is also bigger still wins on cost × time.
    cheap.body_tokens_c = 30_000;
    assert_eq!(verdict(&cheap, &deltas()), Verdict::Accept(A::Cheaper));
}

#[test]
fn ct_ignores_wall_when_configured() {
    let mut g = data(&[], &[]);
    for task in &mut g.tasks {
        for r in &mut task.c {
            r.wall_ms /= 2;
        }
    }
    assert_eq!(verdict(&g, &deltas()), Verdict::Accept(A::Cheaper));
    g.config.ct_include_wall = false; // mock-model runs: wall clock is scheduler jitter
    assert_eq!(verdict(&g, &deltas()), Verdict::Reject(Rj::NoGain));
}

#[test]
fn repro_fixed_on_a_tie_and_on_a_modest_cost_rise() {
    let mut g = data(&[], &[]);
    g.repro = vec![ReproEvidence::Frozen {
        reference: "task:2026-10-20-03".into(),
        c: vec![1000, 1000],
        p: vec![0, 0],
    }];
    let e = accept(&g, &deltas());
    assert_eq!(
        e.decision,
        Decision::Verdict(Verdict::Accept(A::ReproFixed))
    );
    assert!(!e.audit_required, "a maintainer repro needs no audit");
    scale_c_cost(&mut g, 1.3);
    assert_eq!(verdict(&g, &deltas()), Verdict::Accept(A::ReproFixed));
    scale_c_cost(&mut g, 2.0); // 2.6× now: above ρ_max
    assert_eq!(verdict(&g, &deltas()), Verdict::Reject(Rj::Costlier));
    // A repro that LKG passes once is not a fix (needs all R failed).
    let mut g = data(&[], &[]);
    g.repro = vec![ReproEvidence::Frozen {
        reference: "task:x".into(),
        c: vec![1000, 1000],
        p: vec![0, 1000],
    }];
    assert_eq!(verdict(&g, &deltas()), Verdict::Reject(Rj::NoGain));
}

#[test]
fn agent_family_repro_counts_only_eligible_sealed_flips_and_needs_audit() {
    let mut g = data(&[], &[]);
    let fam = |eligible: bool, sealed: Vec<(Vec<u32>, Vec<u32>)>| ReproEvidence::AgentFamily {
        reference: "task:fam-a".into(),
        eligible,
        sealed,
    };
    g.repro = vec![fam(
        true,
        vec![
            (vec![1000, 1000], vec![0, 0]),
            (vec![1000, 1000], vec![0, 0]),
        ],
    )];
    let e = accept(&g, &deltas());
    assert_eq!(
        e.decision,
        Decision::Verdict(Verdict::Accept(A::ReproFixed))
    );
    assert!(e.audit_required);
    g.repro = vec![fam(false, vec![(vec![1000, 1000], vec![0, 0])])];
    assert_eq!(verdict(&g, &deltas()), Verdict::Reject(Rj::NoGain));
    g.repro = vec![fam(
        true,
        vec![
            (vec![1000, 1000], vec![0, 0]),
            (vec![1000, 1000], vec![1000, 1000]),
        ],
    )];
    assert_eq!(verdict(&g, &deltas()), Verdict::Reject(Rj::NoGain));
}

/// §3.9.2: `better` whose recompute without agent tasks is `no_gain` → flagged.
#[test]
fn audit_required_when_better_rests_on_agent_tasks() {
    let mut g = data(&[], &[]);
    // 4 visible agent instances in the stratum, all gains; the 10 maintainer pool tasks tie.
    for i in 0..4 {
        let mut task = t(&format!("fam-{i}.a"), TaskSet::Pool, Source::Agent, 1);
        task.family = Some(format!("fam-{i}"));
        g.tasks.push(task);
    }
    let mut d = deltas();
    d.pass_pool = 0.15; // keep the divergence guard out of this case: 2·√2·0.15 = 0.42 > 4/14
    d.pass_holdout = 0.15;
    let e = accept(&g, &d);
    assert_eq!(e.decision, Decision::Verdict(Verdict::Accept(A::Better)));
    assert!(e.audit_required);
    let (without, _) = accept_inner(
        &g.without_agent_tasks(),
        &d,
        Some(e.stats.score.unwrap().margin),
    );
    assert_eq!(without, Decision::Verdict(Verdict::Reject(Rj::NoGain)));
}

/// §3.9.2: `better` that the maintainer tasks alone support → not flagged, even with agent tasks present.
#[test]
fn no_audit_when_maintainer_tasks_alone_give_better() {
    let mut g = data(&[1, 1, 1, 1], &[1, 1]);
    for i in 0..2 {
        let mut task = t(&format!("fam-{i}.a"), TaskSet::Pool, Source::Agent, 0);
        task.family = Some(format!("fam-{i}"));
        g.tasks.push(task);
    }
    let e = accept(&g, &deltas());
    assert_eq!(e.decision, Decision::Verdict(Verdict::Accept(A::Better)));
    assert!(!e.audit_required);
    assert_eq!(e.stats.n.agent, 2);
}

#[test]
fn no_audit_for_cheaper_or_smaller() {
    let mut g = data(&[], &[]);
    scale_c_cost(&mut g, 0.5);
    let e = accept(&g, &deltas());
    assert_eq!(e.decision, Decision::Verdict(Verdict::Accept(A::Cheaper)));
    assert!(!e.audit_required);
    let mut g = data(&[], &[]);
    g.body_tokens_c = 1;
    let e = accept(&g, &deltas());
    assert_eq!(e.decision, Decision::Verdict(Verdict::Accept(A::Smaller)));
    assert!(!e.audit_required);
}

#[test]
fn accept_is_reproducible_bit_for_bit() {
    let g = data(&[1, 0, -1, 1], &[1, 0]);
    assert_eq!(accept(&g, &deltas()), accept(&g, &deltas()));
}

#[test]
fn verdict_vocabulary_and_codes() {
    assert_eq!(Verdict::Accept(A::CapabilityGain).kind(), "accept");
    assert_eq!(
        Verdict::Accept(A::CapabilityGain).reason(),
        "capability_gain"
    );
    assert_eq!(Verdict::Accept(A::Cheaper).code(), None);
    assert_eq!(
        Verdict::Reject(Rj::PoolHoldoutDivergence).code().unwrap(),
        "gate.g4.pool_holdout_divergence"
    );
    assert_eq!(
        Verdict::Reject(Rj::InconclusiveFinal).code().unwrap(),
        "gate.g4.inconclusive_final"
    );
    assert_eq!(
        Verdict::Inconclusive(I::NoNoiseEstimate).code().unwrap(),
        "gate.g4.no_noise_estimate"
    );
    assert_eq!(Verdict::Error(ErrorReason::Cancelled).kind(), "error");
    let j = serde_json::to_string(&Verdict::Reject(Rj::CostlierThanBudget)).unwrap();
    assert_eq!(j, r#"{"verdict":"reject","reason":"costlier_than_budget"}"#);
    assert!(
        Verdict::Accept(A::ReproFixed).adds_capability()
            && !Verdict::Accept(A::Smaller).adds_capability()
    );
}

/// G-44 (unit half): the first inconclusive escalates once (R = 4); a still-inconclusive escalated
/// re-run is `reject: inconclusive_final`; decisive verdicts pass through; F16 after 24 h.
#[test]
fn g44_inconclusive_is_resolved_mechanically() {
    let inc = Verdict::Inconclusive(I::Underpowered);
    assert_eq!(resolve(inc, false, 4), Resolution::Escalate { repeats: 4 });
    assert_eq!(
        resolve(inc, true, 4),
        Resolution::Final(Verdict::Reject(Rj::InconclusiveFinal))
    );
    let acc = Verdict::Accept(A::Better);
    assert_eq!(resolve(acc, true, 4), Resolution::Final(acc));
    let err = Verdict::Error(ErrorReason::Infra);
    assert_eq!(resolve(err, false, 4), Resolution::Final(err));
    assert!(!provisional_expired(86_400, 86_400) && provisional_expired(86_401, 86_400));
}

#[test]
fn futility_stop_maps_to_score_inferior_early() {
    let look = crate::stats::boot::futility("j", &[-0.5; 8], Some(0.08), 2.0, 2_000, 0.01);
    assert_eq!(
        futility_verdict(look),
        Some(Verdict::Reject(Rj::ScoreInferiorEarly))
    );
    assert_eq!(futility_verdict(None), None);
}

#[test]
fn judge_measures_noise_or_reports_no_estimate() {
    // R = 1 and no history: inconclusive (no_noise_estimate), and nothing is stored.
    let mut g = data(&[1, 1], &[1]);
    for task in &mut g.tasks {
        task.c.truncate(1);
        task.p.truncate(1);
    }
    let (e, d, store) = judge(&g, None, "2026-10-20T03:00:00.000Z");
    assert_eq!(
        e.decision,
        Decision::Verdict(Verdict::Inconclusive(I::NoNoiseEstimate))
    );
    assert!(d.is_none() && store.is_empty());
    // With a recent history entry for every required key, R = 1 uses δ_hist alone.
    let mut h = NoiseHistory::default();
    let model = h.models.entry("mock/mock".into()).or_default();
    for key in ["pass_rate", "pass_rate@pool", "pass_rate@holdout"] {
        model.insert(
            key.into(),
            NoiseEntry {
                delta_log_ppm: None,
                delta_permille: Some(50),
                n: 16,
                repeats: 2,
                at: "2026-10-19T00:00:00.000Z".into(),
            },
        );
    }
    for key in ["cost_usd", "wall_s", "cost_x_wall"] {
        model.insert(
            key.into(),
            NoiseEntry {
                delta_log_ppm: Some(50_000),
                delta_permille: None,
                n: 16,
                repeats: 2,
                at: "2026-10-19T00:00:00.000Z".into(),
            },
        );
    }
    let now = crate::stats::noise::parse_rfc3339("2026-10-20T03:00:00.000Z").unwrap();
    let ctx = HistoryCtx {
        history: &h,
        model: "mock/mock",
        now_unix: now,
        max_age_days: 30,
    };
    let (e, d, store) = judge(&g, Some(&ctx), "2026-10-20T03:00:00.000Z");
    assert!(e.decision.verdict().is_some());
    let d = d.unwrap();
    assert!((d.pass_pool - 0.05 * (16f64 / 10.0).sqrt()).abs() < 1e-9);
    assert!(store.contains_key("pass_rate@holdout"));
    // A canary trip needs no δ at all.
    g.canaries = vec![1000];
    let (e, d, _) = judge(&g, None, "2026-10-20T03:00:00.000Z");
    assert_eq!(e.decision, Decision::Verdict(Verdict::Reject(Rj::Canary)));
    assert!(d.is_none());
}

#[test]
fn judge_end_to_end_with_measured_noise() {
    // R = 2 with some disagreement in the LKG arm: δ is measured, then the rule runs.
    let mut g = data(&[1, 1, 1, 1], &[1, 1]);
    g.tasks[9].p[1].score_permille = 0; // one LKG disagreement: δ ≈ 0.044, m ≈ 0.088
    g.config.noise_b = 500;
    let (e, d, store) = judge(&g, None, "2026-10-20T03:00:00.000Z");
    assert!(d.unwrap().pass_all > 0.0);
    assert_eq!(e.decision, Decision::Verdict(Verdict::Accept(A::Better)));
    assert!(store.contains_key("pass_rate") && store.contains_key("cost_x_wall"));
}

/// D14 quality guard (FIX2): G-21's design (a quarter of the tasks flaky at p = 0.5, the rest
/// deterministic; δ measured per gate from the LKG repeats, no history) run through the **whole** accept
/// rule, split into pool and holdout. Three rules on the same data: pre-D14 (raw δ in the divergence
/// guard, no holdout rule), the divergence floor alone, and D14 as committed (floor + holdout rule).
/// Both D14 changes only turn rejects into accepts (the floor) or accepts into inconclusives (the rule),
/// so the other two are exact counterfactuals of the committed verdict. `margin_max` is not applied, as
/// in G-21. Scenarios:
/// - `0`, `-2δ` (every task), `hold-2δ` (the same mean loss on the holdout only): the candidate is smaller,
///   so a non-inferior tie is accepted (`smaller`), and an accept at −2δ is a false accept;
/// - `c1`: the LKG fails half of each split's tasks (deterministically) and the candidate fixes all of
///   them; the gain generalizes, so an accept is right (the gog's C1);
/// - `pool-fit`: the same, but the candidate fixes only the pool's half (the MS-5 answer-key
///   signature); the gain does not generalize, so an accept is a false accept.
///
///   In `c1` and `pool-fit` the candidate has the LKG's size and cost, so only `better` accepts.
#[test]
#[ignore = "D14 full-rule simulation; run with --release --ignored --nocapture"]
fn d14_full_rule_false_accept_with_and_without_the_floor() {
    use crate::stats::noise::noise;
    use crate::stats::rng::SplitMix64;
    use crate::stats::sim::{analytic_delta, design};
    let gates = 2_000;
    for (np, nh) in [(4usize, 1usize), (4, 4), (8, 4), (10, 6), (18, 12)] {
        let n = np + nh;
        // Holdout slots spread evenly over the design (flaky tasks come first in `design`).
        let hold: Vec<bool> = (0..n).map(|i| (i + 1) * nh / n > i * nh / n).collect();
        // Half of each split, by position within the split.
        let marked: Vec<bool> = (0..n)
            .map(|i| (0..i).filter(|&j| hold[j] == hold[i]).count() % 2 == 0)
            .collect();
        for r in [2usize, 3] {
            let delta = analytic_delta(n, r);
            for sc in ["0", "-2δ", "hold-2δ", "c1", "pool-fit"] {
                let (mut pre, mut floor, mut committed) = (0usize, 0usize, 0usize);
                for gi in 0..gates {
                    let id = format!("d14-{np}+{nh}-r{r}-{sc}-{gi}");
                    let mut rng = SplitMix64::for_gate(&id, "sim-scores");
                    let mut bern = |p: f64| {
                        let u = (rng.next() >> 11) as f64 / (1u64 << 53) as f64;
                        RunValues {
                            score_permille: if u < p { 1000 } else { 0 },
                            cost_ref_musd: 20_000,
                            wall_ms: 100_000,
                            ..RunValues::default()
                        }
                    };
                    let tasks: Vec<TaskRuns> = design(n)
                        .iter()
                        .enumerate()
                        .map(|(i, &p)| {
                            let (p, q) = match sc {
                                "-2δ" => (p, p - 2.0 * delta),
                                "hold-2δ" if hold[i] => {
                                    (p, p - 2.0 * delta * n as f64 / nh as f64)
                                }
                                "c1" if marked[i] => (0.0, 1.0),
                                "pool-fit" if marked[i] => (0.0, if hold[i] { 0.0 } else { 1.0 }),
                                _ => (p, p),
                            };
                            let q = q.clamp(0.0, 1.0);
                            let c = (0..r).map(|_| bern(q)).collect();
                            let lkg = (0..r).map(|_| bern(p)).collect();
                            TaskRuns {
                                id: format!("t{i}"),
                                set: if hold[i] {
                                    TaskSet::Holdout
                                } else {
                                    TaskSet::Pool
                                },
                                source: Source::Maintainer,
                                family: None,
                                c,
                                p: lkg,
                            }
                        })
                        .collect();
                    let refs: Vec<&TaskRuns> = tasks.iter().collect();
                    let split = |s: TaskSet| -> Vec<&TaskRuns> {
                        tasks.iter().filter(|t| t.set == s).collect()
                    };
                    let nz = |k: &str, v: &[&TaskRuns]| {
                        noise(&id, k, Metric::PassRate, v, 400).unwrap_or(0.0)
                    };
                    let mut d = Deltas::uniform(0.0, 0.05);
                    d.pass_all = nz("pass_rate", &refs);
                    d.pass_pool = nz("pass_rate@pool", &split(TaskSet::Pool));
                    d.pass_holdout = nz("pass_rate@holdout", &split(TaskSet::Holdout));
                    let mut g = data(&[], &[]);
                    g.gate_id = id.clone();
                    g.tasks = tasks.clone();
                    if !matches!(sc, "c1" | "pool-fit") {
                        g.body_tokens_c = 20_000;
                    }
                    g.config.min_pool = np;
                    g.config.min_holdout = nh;
                    g.config.margin_max = f64::INFINITY;
                    g.config.bootstrap_b = 2_000;
                    let e = accept(&g, &d);
                    let v = e.decision.verdict();
                    let acc = matches!(v, Some(Verdict::Accept(_)));
                    let acc_floor = acc || v == Some(Verdict::Inconclusive(I::UnderpoweredHoldout));
                    if !acc_floor {
                        continue;
                    }
                    let s = e.stats.score.unwrap();
                    let (p, h) = (s.pool, s.holdout);
                    let div_raw = p.d - h.d > 2.0 * (p.delta.powi(2) + h.delta.powi(2)).sqrt();
                    pre += usize::from(!div_raw);
                    floor += 1;
                    committed += usize::from(acc);
                }
                assert!(pre <= floor && committed <= floor);
                let f = |k: usize| k as f64 / gates as f64;
                eprintln!(
                    "D14 pool={np} holdout={nh} R={r} δ={delta:.3} {sc:>8}: accept pre-D14 {:.4}, floor only {:.4}, D14 {:.4}",
                    f(pre),
                    f(floor),
                    f(committed)
                );
            }
        }
    }
}
