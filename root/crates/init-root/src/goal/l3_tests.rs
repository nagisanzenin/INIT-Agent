//! L3 (v0.5.3) tests of the GBAL statistics, written against the API main has (`reading`, `calibration`,
//! `checkin_rounds`, the control link and owner lines in a test home), so the same file runs on main, where D1–D3's
//! tests fail, and after (release-protocol §4.1). D1: the exact permutation test of a reading; D2: `met` spends α over
//! the goal's rounds; D3: the check-in freedom needs Brier skill against the base-rate reference; and the GBAL mock
//! flow's verdicts on the same readings. Then two v0.5.3 fixes found on the way: every live change in a goal spends a
//! round, and a reading on a change the sentinel rolled back ends.

use super::*;

/// L3 D3 (fails on main, where the freedom read the Brier score alone): an agent that files `same` at p = 0.9 on
/// changes that do nothing, which read `same` (noise) 9 times in 10, scores Brier 0.09 and used to earn 6 rounds
/// between check-ins. The base-rate reference (the class `same` came true 90 % of the time) scores 0.09 too: no skill,
/// the default 3. An agent whose forecasts separate hits from misses keeps its freedom. Written against the API main
/// has (`calibration`, `checkin_rounds`), so the same test runs on both.
#[test]
fn d3_freedom_needs_skill_against_the_base_rate() {
    let rec = |i: usize, expect: &str, p: u64, o: u64| {
        let id = format!("P-{i:06}");
        [json!({"by": "root", "kind": "goal_predict", "data": {"id": id, "expect": expect, "goal_kind": "card:cost_turn", "p_permille": p}}),
            json!({"by": "root", "kind": "goal_outcome", "data": {"goal_kind": "card:cost_turn", "p_permille": p, "o": o, "prediction": id}})]
    };
    let same: Vec<Value> = (0..10).flat_map(|i| rec(i, "same", 900, u64::from(i != 4))).collect();
    let cal = calibration(&same);
    assert_eq!(checkin_rounds(cal.get("card:cost_turn").copied()), 3, "no skill: no freedom ({:?})", cal.get("card:cost_turn"));
    // a forecaster with resolution: 0.9 on the 9 that came true, 0.2 on the one that did not (Brier 0.013 < 0.09)
    let sharp: Vec<Value> = (0..10).flat_map(|i| rec(i, "same", if i == 4 { 200 } else { 900 }, u64::from(i != 4))).collect();
    assert_eq!(checkin_rounds(calibration(&sharp).get("card:cost_turn").copied()), 12);
}

/// L3 D3 (fails on main): every live change in a card goal is forecast. A change that goes live in the goal with no
/// prediction gets the goal card's reading, scored at the Root's abstention forecast (500‰ that the card moves toward
/// the threshold, Brier 0.25 whatever happens), so the agent no longer picks which changes its calibration covers.
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn d3_every_live_change_in_a_card_goal_is_forecast() {
    let (dir, root) = crate::grant::tests::test_home("gd3u", "");
    assert!(line(&root, r#"/goal "fewer requests" done=card:requests_turn<=2"#));
    for i in 0..5 {
        turn_closed(&root, &Turn { seq: 1 + i, kind: "owner".into(), end: 1000 + i as i64, tuple: Some("t0".into()), requests: 3, ..Turn::default() });
    }
    let sha = "b".repeat(40);
    let rb = root.append("tuple", json!({"id": "tB", "tuple": {"body": {"sha": sha}}})).unwrap();
    let e = crate::util::parse_rfc3339(rb["at"].as_str().unwrap()).unwrap();
    for i in 0..5 {
        turn_closed(&root, &Turn { seq: 10 + i, kind: "owner".into(), end: e + 1 + i as i64, tuple: Some("tB".into()), requests: 3, ..Turn::default() });
    }
    let oc: Vec<Value> = crate::lineage::records(&root.p.rh).unwrap().into_iter().filter(|r| r["kind"] == "goal_outcome").map(|r| r["data"].clone()).collect();
    assert_eq!(oc.iter().map(|o| (o["source"].as_str(), o["prediction"].is_null(), o["p_permille"].as_u64(), o["expect"].as_str(), o["o"].as_u64(), o["brier_ppm"].as_u64()))
        .collect::<Vec<_>>(), [(Some("unforecast"), true, Some(500), Some("better"), Some(0), Some(250_000))], "{oc:?}");
    assert_eq!(kernel(&root, "goal.status", &json!({})).unwrap()["calibration"]["card:requests_turn"]["n"], 1);
    let _ = std::fs::remove_dir_all(&dir); // exact path, made by test_home
}

/// L3 D1 (fails on main): Welch's z against a fixed 2 is a large-sample rule. On the corrections card (0 or 1000 per
/// turn) 5 clean turns after a change, against a baseline with 4 corrections in 40, read z = +2.08 and `better`, yet a
/// change that does nothing gives 5 clean turns with probability 0.9⁵ = 59 %. The exact permutation p is the chance
/// that 5 turns drawn from the 45 hold none of the 4: C(41, 5)/C(45, 5) = 0.613, noise. Fully separated turns stay
/// beyond noise at 5 against 5 (1/252) and at 5 against 6 (1/462), each way.
#[test]
fn d1_verdicts_come_from_the_exact_permutation_test() {
    let corr = card("corrections").unwrap();
    let before: Vec<i64> = (0..40).map(|i| if i % 10 == 3 { 1000 } else { 0 }).collect();
    let r = reading(corr, &before, &[0; 5]);
    assert!(r["z_permille"].as_i64().unwrap() >= 2000, "Welch's z called this better: {r}");
    assert_eq!(r["verdict"], "noise", "{r}");
    assert_eq!((r["p_better_ppm"].as_u64(), r["p_worse_ppm"].as_u64(), r["test"].as_str()), (Some(613_377), Some(1_000_000), Some("permutation")), "{r}");
    let cost = card("cost_turn").unwrap();
    let r = reading(cost, &[100, 104, 96, 102, 98, 100], &[50, 52, 48, 51, 49]);
    assert_eq!((r["verdict"].as_str(), r["p_better_ppm"].as_u64()), (Some("better"), Some(2_165)), "{r}");
    let r = reading(cost, &[5; 5], &[9; 5]);
    assert_eq!((r["verdict"].as_str(), r["p_worse_ppm"].as_u64()), (Some("worse"), Some(3_969)), "{r}");
    // near the boundary: 4/252 = 15,874 ppm is beyond noise (≤ 22,750), 10/252 = 39,683 is not
    let req = card("requests_turn").unwrap();
    assert_eq!(reading(req, &[3, 3, 2, 2, 2], &[1, 1, 1, 1, 2])["verdict"], "better");
    assert_eq!(reading(req, &[3, 3, 2, 2, 2], &[1, 1, 1, 2, 2])["verdict"], "noise");
}

/// L3 D2 (fails on main): `met` spends 5 % over the goal's rounds. A registered reading beyond noise at p = 4/252
/// (15,874 ppm, `better`) attributes a goal of 3 rounds (≤ 50,000/3 = 16,666 ppm) but not one of the default 10 (≤
/// 5,000 ppm): with 10 rounds of changes that do nothing, readings that good arise by chance in about 15 % of goals.
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn d2_met_spends_alpha_over_the_rounds() {
    for (rounds, want) in [(10, "met_unattributed"), (3, "met")] {
        let (dir, root) = crate::grant::tests::test_home(if rounds == 3 { "gd2a" } else { "gd2b" }, "");
        let k = |cmd: &str, a: Value| kernel(&root, cmd, &a);
        assert!(line(&root, &format!(r#"/goal "fewer requests" done=card:requests_turn<=2 rounds={rounds}"#)));
        for (i, r) in [3, 3, 2, 2, 2].iter().enumerate() {
            turn_closed(&root, &Turn { seq: 1 + i as u64, kind: "owner".into(), end: 1000 + i as i64, tuple: Some("t0".into()), requests: *r, ..Turn::default() });
        }
        let sha = "a".repeat(40);
        k("goal.predict", json!({"sha": sha, "mode": "A", "card": "requests_turn", "expect": "better", "p_permille": 700})).unwrap();
        let ra = root.append("tuple", json!({"id": "tA", "tuple": {"body": {"sha": sha}}})).unwrap();
        let e = crate::util::parse_rfc3339(ra["at"].as_str().unwrap()).unwrap();
        for (i, r) in [1, 1, 1, 1, 2].iter().enumerate() {
            turn_closed(&root, &Turn { seq: 10 + i as u64, kind: "owner".into(), end: e + 1 + i as i64, tuple: Some("tA".into()), requests: *r, ..Turn::default() });
        }
        let rd = crate::lineage::records(&root.p.rh).unwrap().into_iter().find(|r| r["kind"] == "card_reading").unwrap();
        assert_eq!((rd["data"]["verdict"].as_str(), rd["data"]["registered"].as_bool()), (Some("better"), Some(true)), "{rd}");
        let s = k("goal.stop", json!({"reason": "met"})).unwrap();
        assert_eq!((s["reason"].as_str(), s["verified"].as_bool()), (Some(want), Some(want == "met")), "rounds={rounds}: {s}");
        if want == "met" {
            assert_eq!((s["attributed"]["p_ppm"].as_u64(), s["attributed"]["alpha_ppm"].as_u64()), (Some(15_874), Some(16_666)), "{s}");
        }
        let _ = std::fs::remove_dir_all(&dir); // exact path, made by test_home
    }
}

/// L3 gate: the GBAL mock flow on the macOS bundle (REL-040, build/status/gbal-bundle-mock.json) keeps its verdicts on
/// the same readings. That flow needs a release bundle and a test HOME and runs at release; this reproduces its
/// decisions through the Root's goal code in a test home: the goal `requests_turn<=2`, rounds 8, guardrails
/// `cost_turn:300,provider_errors`; 5 baseline turns (2 requests, 780 µUSD, no errors); P-000001 (better, 700‰) halves
/// requests and cost; the owner checks in; P-000002 (worse, 800‰) quadruples both. Verdicts: P-000001 `better`,
/// registered, o = 1 (Brier 0.09); P-000002 `worse` on requests (o = 1, Brier 0.065 over 2) and rolled back by the
/// cost guardrail; the stop is `met`, verified, attributed to P-000001.
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn gbal_bundle_mock_verdicts_on_the_same_readings() {
    let (dir, root) = crate::grant::tests::test_home("gbmk", "");
    let k = |cmd: &str, a: Value| kernel(&root, cmd, &a);
    let recs = |kind: &str| crate::lineage::records(&root.p.rh).unwrap().into_iter().filter(|r| r["kind"] == kind).map(|r| r["data"].clone()).collect::<Vec<_>>();
    let mut seq = 0;
    let mut turns = |tuple: &str, from: i64, req: u32, cost: u64| {
        for i in 0..5 {
            seq += 1;
            turn_closed(&root, &Turn { seq, kind: "owner".into(), end: from + 1 + i, tuple: Some(tuple.into()), requests: req, cost_musd: cost, ..Turn::default() });
        }
    };
    turns("tg", 1000, 2, 780);
    assert!(line(&root, r#"/goal "answer short questions about this folder with fewer model requests per turn" done=card:requests_turn<=2 usd=0.40 wall=3h rounds=8 guard=cost_turn:300,provider_errors"#));
    let live = |sha: &str, tid: &str| crate::util::parse_rfc3339(root.append("tuple", json!({"id": tid, "tuple": {"body": {"sha": sha}}})).unwrap()["at"].as_str().unwrap()).unwrap();
    let (s1, s2) = ("9f1db00e".repeat(5), "a55c3463".repeat(5));
    k("goal.predict", json!({"sha": s1, "mode": "A", "card": "requests_turn", "expect": "better", "p_permille": 700})).unwrap();
    let t1 = live(&s1, "t1");
    turns("t1", t1, 1, 390);
    assert!(line(&root, "/goal continue"));
    k("goal.predict", json!({"sha": s2, "mode": "A", "card": "requests_turn", "expect": "worse", "p_permille": 800})).unwrap();
    let t2 = live(&s2, "t2");
    turns("t2", t2, 4, 1560);
    let rd = recs("card_reading");
    let get = |sha: &str, card: &str| rd.iter().find(|r| r["sha"] == sha && r["card"] == card).unwrap_or_else(|| panic!("{card} on {sha}: {rd:?}")).clone();
    let v = |r: &Value| (r["verdict"].as_str().unwrap().to_string(), r["action"].as_str().unwrap().to_string(), r["registered"].as_bool().unwrap());
    let yes = |a: &str, b: &str| (a.to_string(), b.to_string(), true);
    assert_eq!([v(&get(&s1, "requests_turn")), v(&get(&s1, "cost_turn")), v(&get(&s1, "provider_errors"))], [yes("better", "none"), yes("better", "none"), yes("noise", "none")]);
    assert_eq!([v(&get(&s2, "requests_turn")), v(&get(&s2, "cost_turn")), v(&get(&s2, "provider_errors"))], [yes("worse", "none"), yes("worse", "rollback"), yes("noise", "none")]);
    let oc = recs("goal_outcome");
    assert_eq!(oc.iter().map(|o| (o["prediction"].as_str(), o["o"].as_u64(), o["brier_ppm"].as_u64(), o["n"].as_u64())).collect::<Vec<_>>(),
        [(Some("P-000001"), Some(1), Some(90_000), Some(1)), (Some("P-000002"), Some(1), Some(65_000), Some(2))]);
    assert_eq!(oc[1]["harm"], json!(["guard:cost_turn"]));
    let s = k("goal.stop", json!({"reason": "met"})).unwrap();
    assert_eq!((s["reason"].as_str(), s["verified"].as_bool(), s["attributed"]["prediction"].as_str()), (Some("met"), Some(true), Some("P-000001")), "{s}");
    let _ = std::fs::remove_dir_all(&dir); // exact path, made by test_home
}

/// v0.5.3, the budget gap (fails before it): every change that goes live while a goal is active spends one of its
/// rounds and a check-in step, predicted or not. A predicted change spends its round once, when it is filed; a change
/// made while the goal is paused is not the goal's. Once the rounds are spent, the next prediction is refused.
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn every_live_change_in_a_goal_spends_a_round() {
    let (dir, root) = crate::grant::tests::test_home("grnd", "");
    let k = |cmd: &str, a: Value| kernel(&root, cmd, &a);
    let spent = || k("goal.status", json!({})).unwrap()["goal"].clone();
    assert!(line(&root, r#"/goal "fewer requests" done=card:requests_turn<=2 rounds=2"#));
    let mut seq = 0;
    let mut turn = |tid: &str, end: i64| {
        seq += 1;
        turn_closed(&root, &Turn { seq, kind: "owner".into(), end, tuple: Some(tid.into()), requests: 3, ..Turn::default() });
    };
    for i in 0..5 {
        turn("t0", 1000 + i);
    }
    let live = |sha: &str, tid: &str| crate::util::parse_rfc3339(root.append("tuple", json!({"id": tid, "tuple": {"body": {"sha": sha}}})).unwrap()["at"].as_str().unwrap()).unwrap();
    // an unpredicted change goes live: one round and one check-in step
    let e = live(&"1".repeat(40), "tU");
    turn("tU", e + 1);
    let g = spent();
    assert_eq!((g["spent"]["rounds"].as_u64(), g["rounds_before_checkin"].as_u64()), (Some(1), Some(2)), "{g}");
    // a change made while the goal is paused is not the goal's
    assert!(line(&root, "/goal pause"));
    let e = live(&"2".repeat(40), "tV");
    assert!(line(&root, "/goal resume"));
    turn("tV", e + 1);
    assert_eq!(spent()["spent"]["rounds"], 1);
    // a predicted change spends its round when it is filed, and not again when it goes live
    let p = k("goal.predict", json!({"sha": "3".repeat(40), "mode": "A", "card": "requests_turn", "expect": "better", "p_permille": 600})).unwrap();
    assert_eq!(p["round"], 2, "{p}");
    let e = live(&"3".repeat(40), "tP");
    turn("tP", e + 1);
    assert_eq!(spent()["spent"]["rounds"], 2);
    let no = k("goal.predict", json!({"sha": "4".repeat(40), "mode": "A", "card": "requests_turn", "expect": "better", "p_permille": 600})).unwrap_err();
    assert_eq!(no.0, "goal.budget", "{}", no.1);
    let _ = std::fs::remove_dir_all(&dir); // exact path, made by test_home
}

/// v0.5.3 (fails before it): a reading on a change the sentinel rolled back used to stay pending for ever (no more
/// turns come on its tuple). With fewer than 3 turns it now ends `closed`, verdict `rolled_back`, and its prediction
/// resolves with no outcome; with 3 or more it is read on the turns it had, as after a guardrail's rollback. Either
/// way the undone change never counts for `met`.
#[test]
fn readings_on_a_change_the_sentinel_rolled_back_end_and_never_count_for_met() {
    let sha = "a".repeat(40);
    let goal = Goal { id: "G-000001".into(), kind: "card:cost_turn".into(), done: json!({"card": "cost_turn", "kind": "card", "op": "<=", "value": 1_000_000}),
        rounds: 10, state: "active".into(), ..Goal::default() };
    let pred = Pred { id: "P-000001".into(), goal: goal.id.clone(), sha: sha.clone(), card: Some("cost_turn".into()), expect: "better".into(), p: 600, seq: 5,
        kind: goal.kind.clone(), ..Pred::default() };
    let at = |ms: i64| crate::util::rfc3339(ms);
    let live = json!({"by": "root", "kind": "tuple", "seq": 9, "at": at(100), "data": {"id": "tA", "tuple": {"body": {"sha": sha}}}});
    let opened = json!({"by": "root", "kind": "change_open", "seq": 10, "at": at(101), "data": {"group": 7, "sha": sha, "tuple": "tA"}});
    let closed = json!({"by": "root", "kind": "change_close", "seq": 30, "at": at(140), "data": {"group": 7, "outcome": "rolled_back", "attribution": "direct"}});
    let run = |after: usize, recs: &[Value]| {
        let mut st = State { goals: vec![goal.clone()], preds: vec![pred.clone()], ..State::default() };
        measure(&mut st, "cost_turn", &sha, None, false, None).unwrap();
        // 40 baseline turns, so 3 clearly cheaper turns are beyond the goal's spent level too (p = 1/C(43, 3))
        st.obs = (0..40).map(|i| Obs { seq: i, end: 10 + i as i64, tuple: "t0".into(), cost: 1000 + i, ..Obs::default() })
            .chain((0..after).map(|i| Obs { seq: 50 + i as u64, end: 110 + i as i64, tuple: "tA".into(), cost: 10, ..Obs::default() })).collect();
        let mut out = Vec::new();
        complete(&mut st, recs, &mut out);
        let appended: Vec<(&str, Value)> = out.into_iter().filter_map(|o| if let Out::Append(k, v) = o { Some((k, v)) } else { None }).collect();
        (st, appended)
    };
    // 2 turns, then the sentinel rolled the group back: closed with no verdict, the prediction resolved, no outcome
    let (st, ap) = run(2, &[live.clone(), opened.clone(), closed.clone()]);
    assert_eq!(ap.len(), 1, "{ap:?}");
    assert_eq!((ap[0].0, ap[0].1["action"].as_str(), ap[0].1["verdict"].as_str(), ap[0].1["n_after"].as_u64()), ("card_reading", Some("closed"), Some("rolled_back"), Some(2)));
    assert!(st.measures.is_empty() && st.preds[0].resolved);
    // still pending while the change is live
    let (st, ap) = run(2, &[live.clone(), opened.clone()]);
    assert!(ap.is_empty() && st.measures.len() == 1);
    // 3 turns: read on what it had (`better`), but `met` does not count a change the sentinel undid
    let recs = [live.clone(), opened.clone(), closed.clone()];
    let (st, ap) = run(3, &recs);
    let rd = ap.iter().find(|(k, _)| *k == "card_reading").expect("a reading").1.clone();
    assert_eq!((rd["verdict"].as_str(), rd["n_after"].as_u64(), rd["registered"].as_bool()), (Some("better"), Some(3), Some(true)), "{rd}");
    let with_reading = |mut r: Vec<Value>| {
        r.push(json!({"by": "root", "kind": "card_reading", "seq": 31, "data": rd}));
        r
    };
    assert_eq!(met_check(&goal, &with_reading(recs.to_vec()), &st.preds).0, false, "an undone change is not the agent's doing");
    assert_eq!(met_check(&goal, &with_reading(vec![live, opened]), &st.preds).0, true, "the same reading on a change that stayed live attributes");
}

/// v0.5.3, the owner's decision on guardrails (fails before it): a guardrail on a discrete or count card reads 15 owner
/// turns of the change; one on a continuous card (cost, wall) still reads 5; a guardrail that joins a pending reading
/// of its card raises it to 15.
#[test]
fn guardrails_on_discrete_cards_read_15_turns() {
    let sha = "a".repeat(40);
    let mut st = State::default();
    measure(&mut st, "provider_errors", &sha, None, true, Some(0)).unwrap();
    measure(&mut st, "cost_turn", &sha, None, true, Some(300)).unwrap();
    measure(&mut st, "requests_turn", &sha, None, false, None).unwrap();
    measure(&mut st, "requests_turn", &sha, None, true, Some(0)).unwrap();
    let n = |c: &str| st.measures.iter().find(|m| m.card == c).unwrap().n;
    assert_eq!((n("provider_errors"), n("cost_turn"), n("requests_turn")), (15, 5, 15));
    // 40 baseline turns with no errors, then a change whose turns all fail a request: the cost guardrail decides at
    // 5 turns, the error guardrail at 15
    let recs = vec![json!({"by": "root", "kind": "tuple", "seq": 9, "at": crate::util::rfc3339(100), "data": {"id": "tA", "tuple": {"body": {"sha": sha}}}})];
    st.obs = (0..40).map(|i| Obs { seq: i, end: 10 + i as i64, tuple: "t0".into(), cost: 100, req: 2, ..Obs::default() }).collect();
    let after = |st: &mut State, from: u64, k: u64| st.obs.extend((from..from + k).map(|i| Obs { seq: 100 + i, end: 110 + i as i64, tuple: "tA".into(), cost: 100, req: 2, perr: 1, ..Obs::default() }));
    after(&mut st, 0, 5);
    let mut out = Vec::new();
    complete(&mut st, &recs, &mut out);
    let read = |out: &[Out]| out.iter().filter_map(|o| if let Out::Append("card_reading", v) = o { Some(v["card"].as_str().unwrap().to_string()) } else { None }).collect::<Vec<_>>();
    assert_eq!(read(&out), ["cost_turn"], "at 5 turns only the continuous card's guardrail reads");
    after(&mut st, 5, 10);
    let mut out = Vec::new();
    complete(&mut st, &recs, &mut out);
    assert_eq!(read(&out), ["provider_errors", "requests_turn"]);
    let e = out.iter().find_map(|o| if let Out::Append("card_reading", v) = o { (v["card"] == "provider_errors").then(|| v.clone()) } else { None }).unwrap();
    assert_eq!((e["n_after"].as_u64(), e["verdict"].as_str(), e["action"].as_str()), (Some(15), Some("worse"), Some("rollback")), "{e}");
}

/// v0.5.3 (fails before it): a reading whose change a newer change replaced, which then served an owner turn, gets
/// no more turns; it is read on the turns it had (at least 3), or ends `closed`, verdict `replaced`. A turn on the
/// parent that ends after the change went live (it started before) is no replacement.
#[test]
fn a_reading_on_a_replaced_change_is_read_on_the_turns_it_had() {
    let sha = "a".repeat(40);
    let at = |ms: i64| crate::util::rfc3339(ms);
    let recs = vec![json!({"by": "root", "kind": "tuple", "seq": 9, "at": at(100), "data": {"id": "tA", "tuple": {"body": {"sha": sha}}}}),
        json!({"by": "root", "kind": "tuple", "seq": 20, "at": at(200), "data": {"id": "tB", "tuple": {"body": {"sha": "b".repeat(40)}}}})];
    let run = |a_turns: u64, straddle: bool, newer: bool| {
        let mut st = State::default();
        measure(&mut st, "requests_turn", &sha, None, true, Some(0)).unwrap();
        st.obs = (0..10).map(|i| Obs { seq: i, end: 10 + i as i64, tuple: "t0".into(), req: 2, ..Obs::default() }).collect();
        if straddle {
            st.obs.push(Obs { seq: 50, end: 105, tuple: "t0".into(), req: 2, ..Obs::default() });
        }
        st.obs.extend((0..a_turns).map(|i| Obs { seq: 60 + i, end: 110 + i as i64, tuple: "tA".into(), req: 2, ..Obs::default() }));
        if newer {
            st.obs.push(Obs { seq: 90, end: 210, tuple: "tB".into(), req: 2, ..Obs::default() });
        }
        let mut out = Vec::new();
        complete(&mut st, &recs, &mut out);
        (st.measures.len(), out.into_iter().find_map(|o| if let Out::Append("card_reading", v) = o { Some(v) } else { None }))
    };
    let (pending, r) = run(6, false, true);
    let r = r.expect("read on the 6 turns it had");
    assert_eq!((pending, r["n_after"].as_u64(), r["verdict"].as_str()), (0, Some(6), Some("noise")), "{r}");
    let (_, r) = run(2, false, true);
    assert_eq!(r.map(|r| (r["action"].clone(), r["verdict"].clone(), r["n_after"].clone())), Some((json!("closed"), json!("replaced"), json!(2))));
    assert_eq!(run(6, true, false), (1, None), "no newer change: still pending, the straddling parent turn is no replacement");
}

/// v0.5.3 (fails before it): a forecast's own reading waits for the change's guardrails, so a guardrail's rollback is
/// on its outcome as `harm` (G4): with a guardrail on provider errors, the predicted cost reading reads 15 turns.
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn a_forecast_waits_for_the_change_s_guardrails() {
    let (dir, root) = crate::grant::tests::test_home("gwai", "");
    assert!(line(&root, r#"/goal "cheaper turns" done=card:cost_turn<=60 guard=provider_errors,wall_turn"#));
    let sha = "a".repeat(40);
    kernel(&root, "goal.predict", &json!({"sha": sha, "mode": "A", "card": "cost_turn", "expect": "better", "p_permille": 700})).unwrap();
    let st = kernel(&root, "goal.status", &json!({})).unwrap();
    let n = |c: &str| st["pending"].as_array().unwrap().iter().find(|m| m["card"] == c && m["sha"] == sha.as_str()).map(|m| m["n"].as_u64());
    assert_eq!((n("cost_turn"), n("provider_errors"), n("wall_turn")), (Some(Some(15)), Some(Some(15)), Some(Some(5))), "{st}");
    let _ = std::fs::remove_dir_all(&dir); // exact path, made by test_home
}

/// V9 (fails on c4797cc, where every kept reading counted): `card.measure` can read the same live change again once a
/// reading of it completed. A first reading within noise, then a re-read of the same change that happens to read
/// `better` beyond the goal's level and at the threshold, does not make `met`: only the first completed reading of each
/// change counts. The same `better` reading as the change's first one does.
#[test]
fn a_reread_of_the_same_change_never_makes_met() {
    let sha = "b".repeat(40);
    let goal = Goal { id: "G-000001".into(), kind: "card:cost_turn".into(), done: json!({"card": "cost_turn", "kind": "card", "op": "<=", "value": 100}),
        rounds: 10, state: "active".into(), ..Goal::default() };
    let pred = Pred { id: "P-000001".into(), goal: goal.id.clone(), sha: sha.clone(), card: Some("cost_turn".into()), expect: "better".into(), p: 600, seq: 5,
        kind: goal.kind.clone(), ..Pred::default() };
    let rd = |seq: u64, verdict: &str, mean: i64, p: u64| json!({"by": "root", "kind": "card_reading", "seq": seq, "data": {"action": "none",
        "card": "cost_turn", "delta_milli": mean - 900_000, "goal": goal.id, "mean_after_milli": mean, "mean_before_milli": 900_000, "measure": format!("M-{seq:06}"),
        "p_better_ppm": p, "p_worse_ppm": 1_000_000 - p, "prediction": "P-000001", "registered": true, "sha": sha, "verdict": verdict, "z_permille": 0}});
    let live = json!({"by": "root", "kind": "tuple", "seq": 9, "data": {"id": "tB", "tuple": {"body": {"sha": sha}}}});
    let noise = rd(20, "noise", 880_000, 300_000);
    let better = rd(30, "better", 90_000, 10);
    let preds = vec![pred];
    assert_eq!(met_check(&goal, &[live.clone(), better.clone()], &preds).0, true, "the change's first reading attributes");
    let (verified, attributed) = met_check(&goal, &[live, noise, better], &preds);
    assert_eq!(verified, false, "a re-read is not a new round");
    assert!(attributed.as_ref().is_none_or(Value::is_null), "{attributed:?}");
}

/// V11 (fails on c4797cc, which said nothing): an owner guardrail that ends without checking its change alerts the owner,
/// naming the change and the card. (a) A newer change went live and served an owner turn after 2 turns: the guardrail's
/// reading closes `replaced`. (b) The change's parent tuple had 2 owner turns: the reading is `no_noise_floor`. Neither
/// rolls back; a guardrail that did check its change raises no such alert.
#[test]
fn a_guardrail_that_checks_nothing_alerts_the_owner() {
    let (sa, sb) = ("a".repeat(40), "c".repeat(40));
    let at = crate::util::rfc3339;
    let tuple = |seq: u64, id: &str, sha: &str, ms: i64| json!({"by": "root", "kind": "tuple", "seq": seq, "at": at(ms), "data": {"id": id, "tuple": {"body": {"sha": sha}}}});
    let obs = |seq: u64, end: i64, t: &str| Obs { seq, end, tuple: t.into(), cost: 100, ..Obs::default() };
    let run = |st: &mut State, recs: &[Value]| {
        let mut out = Vec::new();
        complete(st, recs, &mut out);
        out.into_iter().filter_map(|o| if let Out::Alert(c, m) = o { Some((c, m)) } else { None }).collect::<Vec<_>>()
    };
    // (a) replaced before 3 turns
    let mut st = State::default();
    measure(&mut st, "cost_turn", &sa, None, true, Some(0)).unwrap();
    st.obs = (0..10).map(|i| obs(i, 10 + i as i64, "t0")).chain((0..2).map(|i| obs(20 + i, 110 + i as i64, "tA"))).chain([obs(30, 210, "tB")]).collect();
    let alerts = run(&mut st, &[tuple(9, "tA", &sa, 100), tuple(19, "tB", &sb, 200)]);
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert_eq!(alerts[0].0, "goal_guard_unchecked");
    assert!(alerts[0].1.contains("card:cost_turn") && alerts[0].1.contains(&sa[..12]) && alerts[0].1.contains("replaced"), "{}", alerts[0].1);
    assert!(st.measures.is_empty());
    // (b) no noise floor: the parent tuple had 2 owner turns
    let mut st = State::default();
    measure(&mut st, "cost_turn", &sa, None, true, Some(0)).unwrap();
    st.obs = (0..2).map(|i| obs(i, 10 + i as i64, "t0")).chain((0..5).map(|i| obs(20 + i, 110 + i as i64, "tA"))).collect();
    let alerts = run(&mut st, &[tuple(9, "tA", &sa, 100)]);
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert!(alerts[0].0 == "goal_guard_unchecked" && alerts[0].1.contains("t0 had 2 owner turns") && alerts[0].1.contains(&sa[..12]), "{}", alerts[0].1);
    // a guardrail with a noise floor that passes: no alert
    let mut st = State::default();
    measure(&mut st, "cost_turn", &sa, None, true, Some(0)).unwrap();
    st.obs = (0..10).map(|i| obs(i, 10 + i as i64, "t0")).chain((0..5).map(|i| obs(20 + i, 110 + i as i64, "tA"))).collect();
    assert!(run(&mut st, &[tuple(9, "tA", &sa, 100)]).is_empty());
}
