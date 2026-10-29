//! v0.5.3 (the owner's level semantics, 2026-10-29): what a level certifies. `harm_certified` is an anytime-valid
//! test of "the rate of exercised groups ending in a direct harm rollback is at least X"; a level is reached only
//! while it rejects that at 95 %. These tests pin its exact boundaries, its validity in simulation with checks after
//! every group, the fold's counts against an independent recount (INV-7), and `ledger --verify` recomputing it.

use super::*;

/// The exercised groups each level needs with 0, 1 and 2 direct harms: one fewer is not certified, these are.
/// STATS-053 shows the same table.
const NEEDED: [(u64, [u64; 3]); 4] = [(250, [15, 25, 32]), (200, [19, 32, 41]), (100, [41, 66, 85]), (50, [83, 135, 174])];

#[test]
fn the_certified_boundaries_per_level() {
    for (x, need) in NEEDED {
        for (h, &n) in need.iter().enumerate() {
            let h = h as u64;
            assert!(!harm_certified(n - 1, h, x) && harm_certified(n, h, x), "{x}‰, {h} harms: {n} groups");
            assert!(harm_bound(n, h) <= x && harm_bound(n - 1, h) > x, "{x}‰, {h} harms: the bound {} at {n}", harm_bound(n, h));
        }
    }
    // the levels' rates in NEED are these
    assert_eq!(NEED.iter().map(|n| n.harm_permille).collect::<Vec<_>>(), NEEDED.iter().map(|n| n.0).collect::<Vec<_>>());
    // a fixed-n test would certify 25 % after 11 clean groups (P(0 of 11 | 25 %) = 0.042 ≤ 0.05; 10: 0.056), but it
    // is valid for one look only; checked after every group, the anytime-valid e-process needs 15
    let p0 = |n: u64| crate::sentinel::stats::binom_tail(n, n, 750, 1000); // P(no harm in n) at 25 %
    assert!(!p0(10).le((1, 20)) && p0(11).le((1, 20)));
    assert!(!harm_certified(11, 0, 250) && !harm_certified(14, 0, 250) && harm_certified(15, 0, 250));
    // degenerate inputs certify nothing
    assert!(!harm_certified(0, 0, 250) && !harm_certified(5, 6, 250) && !harm_certified(100, 0, 0));
    assert_eq!((harm_bound(0, 0), harm_bound(3, 3)), (1000, 1000));
}

/// The bound is monotone where the criterion relies on it: more harms or a lower rate never certify more, and the
/// criterion is met exactly when the shown bound is at most the level's rate.
#[test]
fn the_bound_is_monotone_and_consistent_with_the_criterion() {
    for n in [1u64, 7, 20, 41, 90] {
        for h in 0..=n.min(12) {
            let b = harm_bound(n, h);
            for x in [50u64, 100, 200, 250, 500, 999] {
                assert_eq!(harm_certified(n, h, x), b <= x, "n {n} h {h} x {x} bound {b}");
            }
            if h > 0 {
                assert!(harm_bound(n, h) >= harm_bound(n, h - 1), "a harm never lowers the bound (n {n}, h {h})");
            }
        }
    }
}

/// SplitMix64 for the simulated harm records (synthetic data only).
fn next(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// For n = 1..=max, the most direct harms with which `harm_certified` still certifies x‰ (None: not even 0). It is
/// non-decreasing in n, so each n starts from the last answer.
fn thresholds(x: u64, max: u64) -> Vec<Option<u64>> {
    let mut out = vec![None];
    let mut h: Option<u64> = None;
    for n in 1..=max {
        let mut cur = h;
        while harm_certified(n, cur.map_or(0, |c| c + 1), x) {
            cur = Some(cur.map_or(0, |c| c + 1));
        }
        out.push(cur);
        h = cur;
    }
    out
}

/// Runs of `groups` exercised groups, each a direct harm with probability `p` (permille), checked after every group:
/// the share of runs that are ever certified below x‰.
fn climb_rate(x: u64, p: u64, groups: u64, runs: u64, seed: u64) -> f64 {
    let th = thresholds(x, groups);
    let mut s = seed;
    let mut climbed = 0;
    for _ in 0..runs {
        let mut h = 0;
        for n in 1..=groups {
            h += u64::from(next(&mut s) % 1000 < p);
            if th[n as usize].is_some_and(|t| h <= t) {
                climbed += 1;
                break;
            }
        }
    }
    climbed as f64 / runs as f64
}

/// The property the owner asked for: at a true harm rate equal to the level's X, a record checked after every group
/// climbs in at most 5 % of long runs (Ville's inequality bounds it for any length). At L1's 25 %, 300 groups a run,
/// 2,000 seeded runs, with two Monte Carlo standard errors of slack; at a quarter of X it climbs in most runs. The
/// full table, every level and 600 groups, is `harm_bound_null_table` (ignored).
#[test]
fn at_the_level_s_rate_a_run_climbs_at_most_5_percent_of_the_time() {
    let null = climb_rate(250, 250, 300, 2_000, 0x1e7e_1053_0000_0001);
    let se = (0.05f64 * 0.95 / 2_000.0).sqrt();
    assert!(null <= 0.05 + 2.0 * se, "a 25 % harm rate certified below 25 % in {:.2} % of runs", null * 100.0);
    let good = climb_rate(250, 62, 300, 500, 0x1e7e_1053_0000_0002);
    assert!(good >= 0.9, "a 6.2 % harm rate is certified below 25 % in {:.1} % of runs", good * 100.0);
}

/// The full null table for STATS-053: at each level's rate X (and at X/2), runs of 600 exercised groups, checked after
/// every group. `cargo test --offline -p init-root --bin init-root ledger::harm_tests -- --ignored --nocapture`.
#[test]
#[ignore = "full table: cargo test --offline -p init-root --bin init-root ledger::harm_tests -- --ignored --nocapture"]
fn harm_bound_null_table() {
    for (i, (x, need)) in NEEDED.iter().enumerate() {
        let null = climb_rate(*x, *x, 600, 10_000, 0x1e7e_1053_0000_0100 + i as u64);
        let half = climb_rate(*x, *x / 2, 600, 2_000, 0x1e7e_1053_0000_0200 + i as u64);
        eprintln!("HARM L{} X={x}‰ needed (0,1,2 harms) {need:?}: climbs at rate X {:.2} % (10,000 runs × 600 groups); at X/2 {:.1} % (2,000 runs)",
            i + 1, null * 100.0, half * 100.0);
        assert!(null <= 0.05 + 2.0 * (0.05f64 * 0.95 / 10_000.0).sqrt());
    }
}

/// INV-7, differentially: on seeded random lineages (ladder records, groups opened and closed with random outcomes,
/// window turns and attributions, kernel-signed copies and unrelated records), the fold's (exercised, direct) equals
/// an independent recount from the records since the newest `ladder` or genesis, after every record, and the
/// criterion's `met` is `harm_certified` of the recount.
#[test]
fn the_fold_agrees_with_an_independent_recount() {
    let mut s = 0x1e7e_1053_0000_0003u64;
    for run in 0..40 {
        let mut recs = vec![json!({"at": rfc3339(1_893_456_000_000), "by": "root", "data": {}, "kind": "genesis", "seq": 0})];
        for i in 1..400u64 {
            let at = rfc3339(1_893_456_000_000 + i as i64 * 60_000);
            let r = next(&mut s) % 100;
            // the kernel cannot append Root kinds (kinds.rs); here its copies of groups and turns must be ignored
            let by = if r > 2 && next(&mut s).is_multiple_of(10) { "kernel:000000000000" } else { "root" };
            let (kind, data) = match r {
                0..=2 => ("ladder", json!({"cause": "unlock", "direction": "up", "from": 0, "to": 1, "trigger": {"kind": "test_fixture", "seq": 0}})),
                3..=45 => ("change_open", json!({"group": i, "parent_tuple": "p", "tuple": format!("t{i}")})),
                46..=90 => {
                    let attribution = [Value::Null, json!("direct"), json!("suspect"), json!("collateral")][(next(&mut s) % 4) as usize].clone();
                    let outcome = ["clean", "quiet", "flagged", "rolled_back", "collateral"][(next(&mut s) % 5) as usize];
                    ("change_close", json!({"attribution": attribution, "group": next(&mut s) % 400, "outcome": outcome, "turns": next(&mut s) % 12}))
                }
                _ => ("turns", json!({"owner": 3, "start": at})),
            };
            recs.push(json!({"at": at, "by": by, "data": data, "kind": kind, "seq": i}));
            let l = Level::fold(&recs);
            let since = recs.iter().rposition(|r| (r["kind"] == "ladder" || r["kind"] == "genesis") && r["by"] == "root").unwrap();
            let closes: Vec<&Value> = recs[since..].iter().filter(|r| r["kind"] == "change_close" && r["by"] == "root").map(|r| &r["data"]).collect();
            let is_direct = |d: &Value| d["attribution"] == "direct";
            let undone = |d: &Value| d["outcome"] == "rolled_back" || d["outcome"] == "collateral";
            let n = closes.iter().filter(|d| is_direct(d) || (!undone(d) && d["turns"].as_u64().unwrap_or(0) >= CLEAN_TURNS)).count() as u64;
            let h = closes.iter().filter(|d| is_direct(d)).count() as u64;
            assert_eq!((l.exercised, l.direct), (n, h), "run {run}, record {i}");
            if i % 50 == 0 {
                let c = l.criteria(1_893_456_000_000 + i as i64 * 60_000).into_iter().find(|c| c["id"] == "harm_rate_bound").unwrap();
                let x = NEED[(l.level + 1).clamp(1, 4) as usize - 1].harm_permille;
                assert_eq!((c["met"].as_bool(), c["groups"].as_u64(), c["direct"].as_u64(), c["need"].as_u64()), (Some(harm_certified(n, h, x)), Some(n), Some(h), Some(x)));
            }
        }
    }
}

/// `ledger --verify` recomputes the bound from the lineage: a climb recorded before the record certified it is a
/// problem; the same climb after the 15th exercised group is not.
#[test]
fn verify_recomputes_the_harm_bound() {
    let at = |m: i64| rfc3339(1_893_456_000_000 + m * 60_000);
    let mut recs = vec![json!({"at": at(0), "by": "root", "data": {}, "kind": "genesis", "seq": 0}),
        json!({"at": at(1), "by": "root", "data": {"extend": {"mode": "free"}}, "kind": "charter", "seq": 1})];
    for d in 0..5 {
        let start = rfc3339(1_893_456_000_000 + d * DAY);
        recs.push(json!({"at": rfc3339(1_893_456_000_000 + (d + 1) * DAY), "by": "root", "data": {"owner": 10, "start": start}, "kind": "turns", "seq": recs.len()}));
    }
    let groups = |recs: &mut Vec<Value>, n: usize| {
        for _ in 0..n {
            let g = recs.len() as u64;
            recs.push(json!({"at": rfc3339(1_893_456_000_000 + 6 * DAY), "by": "root", "data": {"group": g, "parent_tuple": format!("p{g}"), "tuple": format!("t{g}")}, "kind": "change_open", "seq": g}));
            recs.push(json!({"at": rfc3339(1_893_456_000_000 + 6 * DAY), "by": "root", "data": {"group": g, "outcome": "clean", "turns": 5}, "kind": "change_close", "seq": g + 1}));
        }
    };
    let climb = |recs: &[Value]| json!({"at": rfc3339(1_893_456_000_000 + 8 * DAY), "by": "root", "data": {"cause": "unlock", "direction": "up", "from": 0, "to": 1,
        "trigger": {"kind": "unlock", "seq": recs.len() - 1}}, "kind": "ladder", "seq": recs.len()});
    groups(&mut recs, 14);
    let mut early = recs.clone();
    early.push(climb(&early));
    assert_eq!(verify(&early).len(), 1, "14 clean groups do not certify 25 %: {:?}", verify(&early));
    assert!(verify(&early)[0].contains("harm_rate_bound"), "{:?}", verify(&early));
    groups(&mut recs, 1);
    recs.push(climb(&recs));
    assert_eq!(verify(&recs), Vec::<String>::new());
}

/// V5 (fails on c4797cc, where any close with 5 or more turns was an exercise): a rollback attributed `suspect`,
/// `collateral` or nothing is neither a harm nor an exercise, so crash rollbacks with two groups open (every one
/// `suspect`) cannot raise n without h. A direct harm rollback still counts in both, and a clean or flagged close with
/// 5 turns counts in n.
#[test]
fn rollbacks_not_attributed_direct_are_not_exercises() {
    let at = |m: i64| rfc3339(1_893_456_000_000 + m * 60_000);
    let close = |seq: u64, outcome: &str, attribution: Value, turns: u64| json!({"at": at(seq as i64), "by": "root", "kind": "change_close", "seq": seq,
        "data": {"attribution": attribution, "group": seq, "outcome": outcome, "turns": turns}});
    let mut recs = vec![json!({"at": at(0), "by": "root", "data": {}, "kind": "genesis", "seq": 0})];
    for (i, (outcome, attribution)) in [("rolled_back", json!("suspect")), ("collateral", Value::Null), ("rolled_back", json!("collateral")),
        ("rolled_back", Value::Null), ("collateral", json!("suspect"))].into_iter().enumerate()
    {
        recs.push(close(1 + i as u64, outcome, attribution, 9));
    }
    let l = Level::fold(&recs);
    assert_eq!((l.exercised, l.direct), (0, 0), "five undone groups with 9 turns each, none direct");
    recs.push(close(10, "rolled_back", json!("direct"), 0));
    recs.push(close(11, "clean", Value::Null, 5));
    recs.push(close(12, "flagged", Value::Null, 7));
    recs.push(close(13, "quiet", Value::Null, 4));
    let l = Level::fold(&recs);
    assert_eq!((l.exercised, l.direct), (3, 1), "the direct harm, the clean and the flagged close");
}
