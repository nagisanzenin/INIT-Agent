//! ADR-013 (GBAL) unit tests ($0): card statistics (σ, se, z, verdicts), Brier and the calibration fold, the
//! check-in freedom, the owner's `/goal` grammar, the prediction-before-measure ordering (registered vs
//! unregistered), per-goal budget metering, guardrail and gate rollbacks, and `met` verification.

use super::*;

fn obs(seq: u64, end: i64, tuple: &str, cost: u64) -> Obs {
    Obs { seq, end, tuple: tuple.into(), cost, wall: 1000, req: 2, input: 1000, cache: 500, ..Obs::default() }
}

#[test]
fn stats_are_exact_integers_in_milli_units() {
    assert_eq!(stats(&[]), (0, 0, 0));
    assert_eq!(stats(&[7]), (1, 7000, 0));
    // 2, 4, 4, 4, 5, 5, 7, 9: mean 5, sample sd sqrt(32/7) = 2.138…
    assert_eq!(stats(&[2, 4, 4, 4, 5, 5, 7, 9]), (8, 5000, 2138));
    assert_eq!(isqrt(0), 0);
    assert_eq!(isqrt(15), 3);
    assert_eq!(isqrt(16), 4);
    assert_eq!(isqrt(u128::from(u64::MAX)), 4_294_967_295);
}

#[test]
fn readings_compare_against_the_noise_floor() {
    let cost = card("cost_turn").unwrap();
    let base = [100, 110, 90, 105, 95, 100];
    // a clear drop in cost: better (lower is better), z positive
    let r = reading(cost, &base, &[50, 55, 45, 50, 52]);
    assert_eq!(r["verdict"], "better", "{r}");
    assert!(r["z_permille"].as_i64().unwrap() >= 2000);
    assert_eq!(r["n_before"], 6);
    assert!(r["delta_milli"].as_i64().unwrap() < 0);
    // a rise: worse
    assert_eq!(reading(cost, &base, &[160, 170, 150, 165, 158])["verdict"], "worse");
    // inside the noise
    assert_eq!(reading(cost, &base, &[102, 98, 104, 96, 100])["verdict"], "noise");
    // fewer than 5 baseline turns: no noise floor, whatever the delta (the instrument_gap case)
    assert_eq!(reading(cost, &base[..4], &[1, 1, 1, 1, 1])["verdict"], "no_noise_floor");
    // higher is better for cache reads
    let cache = card("cache_read").unwrap();
    assert_eq!(reading(cache, &[300, 310, 290, 305, 295], &[600, 610, 590, 605, 595])["verdict"], "better");
    // zero noise on both arms: the sign decides
    assert_eq!(reading(cost, &[5; 5], &[4; 5])["verdict"], "better");
    assert_eq!(reading(cost, &[5; 5], &[5; 5])["verdict"], "noise");
    // per-turn card values
    let o = Obs { input: 2000, cache: 500, corr: true, ..Obs::default() };
    assert_eq!(((cache.value)(&o), (card("corrections").unwrap().value)(&o)), (Some(250), Some(1000)));
    assert_eq!((cache.value)(&Obs::default()), None, "no input tokens: no cache-read value");
}

#[test]
fn brier_calibration_and_checkin_freedom() {
    assert_eq!(brier_ppm(&[]), 0);
    assert_eq!(brier_ppm(&[(1000, 1), (0, 0)]), 0, "perfect");
    assert_eq!(brier_ppm(&[(1000, 0)]), 1_000_000, "confidently wrong");
    assert_eq!(brier_ppm(&[(700, 1), (700, 0)]), (90_000 + 490_000) / 2);
    let out = |kind: &str, p: u64, o: u64, by: &str| json!({"by": by, "kind": "goal_outcome", "data": {"goal_kind": kind, "p_permille": p, "o": o}});
    let recs = vec![out("card:cost_turn", 900, 1, "root"), out("card:cost_turn", 800, 1, "root"), out("owner", 600, 0, "root"),
        out("card:cost_turn", 0, 1, "kernel:abc"), json!({"by": "root", "kind": "goal_predict", "data": {"goal_kind": "owner", "p_permille": 1, "o": 1}})];
    let cal = calibration(&recs);
    // both cost outcomes came true: the class's base rate is 1 and the reference scores 0 (L3 D3)
    assert_eq!(cal["card:cost_turn"], Cal { n: 2, brier_ppm: (10_000 + 40_000) / 2, ref_ppm: 0 }, "Root records only: the kernel's forged outcome is ignored");
    assert_eq!(cal["owner"], Cal { n: 1, brier_ppm: 360_000, ref_ppm: 0 });
    let c = |n, brier_ppm, ref_ppm| Some(Cal { n, brier_ppm, ref_ppm });
    assert_eq!(checkin_rounds(None), 3);
    assert_eq!(checkin_rounds(c(4, 0, 250_000)), 3, "too few outcomes to earn anything");
    assert_eq!(checkin_rounds(c(5, 150_000, 250_000)), 6);
    assert_eq!(checkin_rounds(c(10, 80_000, 250_000)), 12);
    assert_eq!(checkin_rounds(c(5, 300_000, 250_000)), 1, "badly calibrated: a short leash");
    // L3 D3: no skill against the base-rate reference, no freedom, whatever the Brier score
    assert_eq!(checkin_rounds(c(10, 80_000, 80_000)), 3, "a tie with the reference is no skill");
    assert_eq!(checkin_rounds(c(5, 150_000, 100_000)), 3);
    assert_eq!(checkin_rounds(c(10, 70_000, 75_000)), 12);
}

#[test]
fn the_owner_goal_grammar() {
    let (i, g) = parse(r#""cheaper turns on big repos" done=card:cost_turn<=8000 usd=0.50 wall=2h rounds=6 guard=cache_read:50,provider_errors"#).unwrap();
    assert_eq!(i, "cheaper turns on big repos");
    assert_eq!((g.kind.as_str(), g.usd_musd, g.wall_s, g.rounds), ("card:cost_turn", 500_000, 7200, 6));
    assert_eq!(g.done, json!({"card": "cost_turn", "kind": "card", "op": "<=", "value": 8000}));
    assert_eq!(g.guard, vec![("cache_read".to_string(), 50), ("provider_errors".to_string(), 0)]);
    let (_, g) = parse(r#""make onboarding clearer""#).unwrap();
    assert_eq!((g.kind.as_str(), g.usd_musd, g.rounds), ("owner", 1_000_000, 10));
    let (_, g) = parse(&format!("\"x\" done=exemplar:{}", "AB".repeat(32))).unwrap();
    assert_eq!(g.done["sha256"], "ab".repeat(32));
    for bad in ["no quotes", r#""" "#, r#""x" done=card:nope<=1"#, r#""x" guard=tool_errors"#, r#""x" usd=-1"#, r#""x" wall=5d"#, r#""x" rounds=0"#, r#""x" colour=red"#] {
        assert!(parse(bad).is_err(), "{bad}");
    }
}

/// The goal's Root path through a booted test Root (no daemon): owner set → agent prediction → the change goes live
/// (its `tuple` record) → owner turns on both tuples → the reading, registered, with its outcome and calibration.
/// A second change regresses the owner's guardrail and is rolled back; an unpredicted change is `registered:false`;
/// the budget meter stops the goal; `met` is verified against the card.
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn predict_measure_outcome_guardrail_budget_and_stop() {
    let (dir, root) = crate::grant::tests::test_home("gbal", "");
    let recs = |kind: &str| crate::lineage::records(&root.p.rh).unwrap().into_iter().filter(|r| r["kind"] == kind).map(|r| r["data"].clone()).collect::<Vec<_>>();
    // the agent cannot set a goal, and predicting with no goal is refused
    let k = |cmd: &str, a: Value| kernel(&root, cmd, &a);
    assert_eq!(k("goal.predict", json!({"sha": "a".repeat(40), "mode": "A", "card": "cost_turn", "expect": "better", "p_permille": 700})).unwrap_err().0, "goal.none");
    assert_eq!(k("goal.set", json!({})).unwrap_err().0, "root.unknown_cmd");
    assert!(k("goal.propose", json!({"intent": "use fewer tokens"})).unwrap()["id"] == "gp-000001");
    assert!(line(&root, r#"/goal "cheaper turns" done=card:cost_turn<=60 usd=0.50 rounds=6 guard=requests_turn"#));
    let set = &recs("goal")[0];
    assert_eq!((set["op"].as_str(), set["by"].as_str(), set["kind"].as_str()), (Some("set"), Some("owner"), Some("card:cost_turn")));
    assert_eq!(set["intent_sha256"], sha256_hex(b"cheaper turns"));
    assert_eq!(k("goal.status", json!({})).unwrap()["goal"]["intent"], "cheaper turns");
    // mode A needs a card
    assert_eq!(k("goal.predict", json!({"sha": "a".repeat(40), "mode": "A", "expect": "better", "p_permille": 700})).unwrap_err().0, "root.args");

    // change A: predicted first, then live (its tuple record), then 6 baseline + 5 after owner turns
    let (sa, sb) = ("a".repeat(40), "b".repeat(40));
    let p = k("goal.predict", json!({"sha": sa, "mode": "A", "card": "cost_turn", "expect": "better", "p_permille": 800})).unwrap();
    assert_eq!((p["id"].as_str(), p["round"].as_u64()), (Some("P-000001"), Some(1)));
    let base = [100, 104, 96, 102, 98, 100];
    for (i, c) in base.iter().enumerate() {
        turn_closed(&root, &Turn { seq: i as u64 + 1, kind: "owner".into(), end: 1000 + i as i64, tuple: Some("t0".into()), cost_musd: *c, requests: 2, ..Turn::default() });
    }
    let at = |sha: &str, tid: &str| root.append("tuple", json!({"id": tid, "tuple": {"body": {"sha": sha}}})).unwrap();
    let ra = at(&sa, "tA");
    assert!(ra["seq"].as_u64().unwrap() > p["seq"].as_u64().unwrap(), "the prediction precedes the change");
    // G3 (real-model proof): a prediction for a change already live is refused and spends no round; an abbreviated
    // sha is refused (it could never match the tuple's full sha)
    let late = k("goal.predict", json!({"sha": sa, "mode": "A", "card": "cost_turn", "expect": "better", "p_permille": 800})).unwrap_err();
    assert_eq!(late.0, "goal.late", "{}", late.1);
    assert_eq!(k("goal.predict", json!({"sha": "c".repeat(12), "mode": "A", "card": "cost_turn", "expect": "better", "p_permille": 800})).unwrap_err().0, "root.args");
    assert_eq!(k("goal.status", json!({})).unwrap()["goal"]["spent"]["rounds"], 1, "the refused predictions spent no round");
    let end0 = crate::util::parse_rfc3339(ra["at"].as_str().unwrap()).unwrap();
    // v0.5.3: the owner's guardrail is on a discrete card (requests), so it reads 15 turns, and the forecast's own
    // reading waits for it
    for (i, c) in [50, 52, 48, 51, 49].iter().cycle().take(15).enumerate() {
        turn_closed(&root, &Turn { seq: 20 + i as u64, kind: "owner".into(), end: end0 + 1 + i as i64, tuple: Some("tA".into()), cost_musd: *c, requests: 2, ..Turn::default() });
    }
    let rd = recs("card_reading");
    let cost = rd.iter().find(|r| r["card"] == "cost_turn" && r["sha"] == sa.as_str()).expect("the cost reading");
    assert_eq!((cost["verdict"].as_str(), cost["registered"].as_bool(), cost["prediction"].as_str()), (Some("better"), Some(true), Some("P-000001")), "{cost}");
    assert_eq!((cost["n_before"].as_u64(), cost["n_after"].as_u64(), cost["parent_tuple"].as_str()), (Some(6), Some(15), Some("t0")));
    assert!(cost["sd_milli"].as_i64().unwrap() > 0 && cost["se_milli"].as_i64().unwrap() > 0);
    let guard = rd.iter().find(|r| r["card"] == "requests_turn" && r["sha"] == sa.as_str()).expect("the guardrail reading");
    assert_eq!((guard["action"].as_str(), guard["guard"].as_i64()), (Some("none"), Some(0)), "requests unchanged: the guard holds");
    let oc = &recs("goal_outcome")[0];
    assert_eq!((oc["o"].as_u64(), oc["source"].as_str(), oc["n"].as_u64(), oc["brier_ppm"].as_u64()), (Some(1), Some("card"), Some(1), Some(40_000)));
    assert_eq!(k("goal.status", json!({})).unwrap()["calibration"]["card:cost_turn"], json!({"brier_ppm": 40_000, "n": 1, "ref_brier_ppm": 0, "skill": false}));

    // change B: predicted, but it doubles the requests per turn: the owner's guardrail rolls it back
    k("goal.predict", json!({"sha": sb, "mode": "hybrid", "card": "cost_turn", "expect": "better", "p_permille": 600})).unwrap();
    let rb = at(&sb, "tB");
    let end1 = crate::util::parse_rfc3339(rb["at"].as_str().unwrap()).unwrap();
    for i in 0..15 {
        turn_closed(&root, &Turn { seq: 40 + i, kind: "owner".into(), end: end1 + 1 + i as i64, tuple: Some("tB".into()), cost_musd: 50, requests: 5, ..Turn::default() });
    }
    let rd = recs("card_reading");
    let g = rd.iter().find(|r| r["card"] == "requests_turn" && r["sha"] == sb.as_str()).expect("B's guardrail reading");
    assert_eq!((g["verdict"].as_str(), g["action"].as_str(), g["parent_tuple"].as_str()), (Some("worse"), Some("rollback"), Some("tA")), "{g}");
    let ob = recs("goal_outcome").into_iter().find(|o| o["prediction"] == "P-000002").expect("B's outcome");
    // G4: scored on B's own card (cost_turn, expected better, read as noise: a miss), the rollback recorded alongside
    assert_eq!((ob["o"].as_u64(), ob["source"].as_str(), ob["reading"]["verdict"].as_str()), (Some(0), Some("card"), Some("noise")), "{ob}");
    assert_eq!(ob["harm"], json!(["guard:requests_turn"]));
    assert!(std::fs::read_to_string(root.p.r("alerts.jsonl")).unwrap().contains("goal_guard"));

    // an unpredicted change measured by the agent: registered:false
    let sc = "c".repeat(40);
    k("card.measure", json!({"card": "wall_turn", "sha": sc, "n": 3})).unwrap();
    let rc = at(&sc, "tC");
    let end2 = crate::util::parse_rfc3339(rc["at"].as_str().unwrap()).unwrap();
    for i in 0..3 {
        turn_closed(&root, &Turn { seq: 60 + i, kind: "owner".into(), end: end2 + 1 + i as i64, tuple: Some("tC".into()), cost_musd: 50, wall_ms: 900, requests: 2, ..Turn::default() });
    }
    let w = recs("card_reading").into_iter().find(|r| r["card"] == "wall_turn").expect("C's reading");
    assert_eq!((w["registered"].as_bool(), w["prediction"].clone()), (Some(false), Value::Null), "{w}");
    // a prediction filed after the change went live is refused (G3) and spends no round; a reading the agent asks for
    // then is unregistered
    let sd = "d".repeat(40);
    let rdd = at(&sd, "tD");
    assert_eq!(k("goal.predict", json!({"sha": sd, "mode": "A", "card": "cost_turn", "expect": "better", "p_permille": 500})).unwrap_err().0, "goal.late");
    k("card.measure", json!({"card": "cost_turn", "sha": sd, "n": 5})).unwrap();
    // v0.5.3: C and D went live unpredicted and each spent a round and a check-in step (4 of 6 used, 4 since the
    // check-in): the check-in comes first (3 rounds at no calibration), then /goal continue
    let p9 = json!({"sha": "9".repeat(40), "mode": "A", "card": "cost_turn", "expect": "better", "p_permille": 500});
    assert_eq!(k("goal.predict", p9.clone()).unwrap_err().0, "goal.checkin");
    assert_eq!(k("goal.status", json!({})).unwrap()["goal"]["spent"]["rounds"], 4);
    assert!(line(&root, "/goal continue"));
    // round 5: a prediction for a change not yet live, which then goes live (a later prediction is a new round)
    assert_eq!(k("goal.predict", p9).unwrap()["round"], 5);
    at(&"9".repeat(40), "t9");
    let end3 = crate::util::parse_rfc3339(rdd["at"].as_str().unwrap()).unwrap();
    for i in 0..5 {
        turn_closed(&root, &Turn { seq: 80 + i, kind: "owner".into(), end: end3 + 1 + i as i64, tuple: Some("tD".into()), cost_musd: 50, requests: 2, ..Turn::default() });
    }
    let late = recs("card_reading").into_iter().find(|r| r["card"] == "cost_turn" && r["sha"] == sd.as_str()).expect("D's reading");
    assert_eq!(late["registered"], false, "a prediction after the change is not a prediction: {late}");

    // the budget: 6 rounds, 5 used; a predicted change spends no second round when it goes live (t9)
    assert_eq!(k("goal.predict", json!({"sha": "e".repeat(40), "mode": "B", "expect": "better", "p_permille": 500})).unwrap()["round"], 6);
    at(&"e".repeat(40), "tE");
    assert_eq!(k("goal.predict", json!({"sha": "f".repeat(40), "mode": "B", "expect": "better", "p_permille": 500})).unwrap_err().0, "goal.budget");
    // the owner judges the mode-B change
    assert!(line(&root, "/goal reject"));
    let oe = recs("goal_outcome").into_iter().find(|o| o["sha"] == "e".repeat(40).as_str()).unwrap();
    assert_eq!((oe["o"].as_u64(), oe["source"].as_str()), (Some(0), Some("owner_reject")));

    // stop: met is checked against the goal card's newest kept reading (D's, mean 50 ≤ 60) and attributed to A's
    // registered reading, which moved cost_turn beyond noise as P-000001 predicted
    let s = k("goal.stop", json!({"reason": "met"})).unwrap();
    assert_eq!((s["verified"].as_bool(), s["reason"].as_str(), s["attributed"]["prediction"].as_str()), (Some(true), Some("met"), Some("P-000001")), "{s}");
    let last = recs("goal").pop().unwrap();
    assert_eq!((last["op"].as_str(), last["reason"].as_str(), last["by"].as_str(), last["spent"]["rounds"].as_u64()), (Some("stop"), Some("met"), Some("agent"), Some(6)));
    assert_eq!(k("goal.stop", json!({"reason": "bored"})).unwrap_err().0, "root.args");
    let _ = std::fs::remove_dir_all(&dir); // exact path, made by test_home
}

/// Money metering: every settled egress request's cost while the goal is active; the Root stops the goal itself at its
/// budget.
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn the_budget_meter_stops_the_goal() {
    let (dir, root) = crate::grant::tests::test_home("gbud", "");
    egress_spent(&root, 500);
    assert!(!root.p.r("state/goals/state.json").exists(), "no goal: the meter writes nothing");
    assert!(line(&root, r#"/goal "x" usd=0.000100"#));
    egress_spent(&root, 60);
    assert_eq!(kernel(&root, "goal.status", &json!({})).unwrap()["goal"]["spent"]["musd"], 60, "every settled request counts, whatever its turn");
    turn_closed(&root, &Turn { seq: 1, kind: "background".into(), cost_musd: 60, ..Turn::default() });
    assert_eq!(kernel(&root, "goal.status", &json!({})).unwrap()["goal"]["spent"]["musd"], 60, "a closed turn adds nothing more (no double count)");
    assert!(line(&root, "/goal pause"));
    egress_spent(&root, 1000);
    assert!(line(&root, "/goal resume"));
    assert_eq!(kernel(&root, "goal.status", &json!({})).unwrap()["goal"]["spent"]["musd"], 60, "spend while paused is not the goal's");
    egress_spent(&root, 60);
    let stop = crate::lineage::records(&root.p.rh).unwrap().into_iter().rev().find(|r| r["kind"] == "goal").unwrap();
    assert_eq!((stop["data"]["op"].as_str(), stop["data"]["reason"].as_str(), stop["data"]["by"].as_str(), stop["data"]["budget"]["hit"].as_str()),
        (Some("stop"), Some("budget"), Some("root"), Some("usd")));
    assert_eq!(kernel(&root, "goal.status", &json!({})).unwrap()["goal"], Value::Null);
    let _ = std::fs::remove_dir_all(&dir); // exact path, made by test_home
}

/// A gate (`card:<id> gate`) fails unless the reading is better; a pending measure waits for its change to go live.
#[test]
fn a_gate_rolls_back_unless_better() {
    let mut st = State::default();
    measure(&mut st, "cost_turn", &"a".repeat(40), Some(3), true, None).unwrap();
    st.obs = (0..5).map(|i| obs(i, 10 + i as i64, "t0", 100 + i)).collect();
    let mut out = Vec::new();
    complete(&mut st, &[], &mut out);
    assert!(out.is_empty() && st.measures.len() == 1, "not live yet: still pending");
    let recs = vec![json!({"kind": "tuple", "seq": 9, "at": crate::util::rfc3339(20), "data": {"id": "tA", "tuple": {"body": {"sha": "a".repeat(40)}}}})];
    st.obs.extend((0..3).map(|i| obs(10 + i, 30 + i as i64, "tA", 102)));
    complete(&mut st, &recs, &mut out);
    assert!(st.measures.is_empty());
    assert!(out.iter().any(|o| matches!(o, Out::Rollback { to, reason, .. } if to == "t0" && reason == "gate:cost_turn")), "noise is not better: the self-imposed gate rolls back");
}

/// GBAL real-model proof (G4): P-000003 forecast a regression (`expect=worse`, p 0.9) and the owner's guardrail caught
/// it; it used to score o = 0 ("harm", Brier 0.81). The outcome now scores the forecast on its own card (worse beyond
/// noise: o = 1, Brier 0.01) and records the rollback as `harm`. The card reading needed 8 turns but the change was
/// rolled back after 5: it is read on the 5 it had. A reading with no noise floor scores nothing.
#[test]
fn g4_a_predicted_regression_caught_by_a_guardrail_is_a_hit() {
    let sha = "a".repeat(40);
    let mut st = State::default();
    st.goals.push(Goal { id: "G-000001".into(), kind: "card:requests_turn".into(), state: "active".into(), guard: vec![("cost_turn".into(), 300)], ..Goal::default() });
    st.preds.push(Pred { id: "P-000003".into(), goal: "G-000001".into(), sha: sha.clone(), card: Some("requests_turn".into()), expect: "worse".into(), p: 900, seq: 5,
        kind: "card:requests_turn".into(), resolved: false, harm: vec![], ..Pred::default() });
    measure(&mut st, "requests_turn", &sha, Some(8), false, None).unwrap();
    measure(&mut st, "cost_turn", &sha, None, true, Some(300)).unwrap();
    for m in &mut st.measures {
        m.goal = Some("G-000001".into());
    }
    let turn = |seq: u64, end: i64, tuple: &str, cost: u64, req: u32| Obs { seq, end, tuple: tuple.into(), cost, req, wall: 1000, input: 1000, cache: 900, ..Obs::default() };
    st.obs = [(320, 1), (330, 1), (340, 1), (325, 1), (335, 1)].iter().enumerate().map(|(i, &(c, r))| turn(i as u64, 10 + i as i64, "tP", c, r)).collect();
    let recs = vec![json!({"kind": "tuple", "seq": 9, "at": crate::util::rfc3339(20), "data": {"id": "tR", "tuple": {"body": {"sha": sha}}}})];
    st.obs.extend([(2400, 6), (2900, 8), (2600, 7), (3100, 9), (2800, 6)].iter().enumerate().map(|(i, &(c, r))| turn(20 + i as u64, 30 + i as i64, "tR", c, r)));
    let mut out = Vec::new();
    complete(&mut st, &recs, &mut out);
    assert!(st.measures.is_empty(), "both readings complete: the rolled-back change gets no more turns");
    let appended: Vec<(&str, &Value)> = out.iter().filter_map(|o| if let Out::Append(k, v) = o { Some((*k, v)) } else { None }).collect();
    let req = appended.iter().find(|(k, v)| *k == "card_reading" && v["card"] == "requests_turn").expect("the forecast card's reading").1;
    assert_eq!((req["verdict"].as_str(), req["n_after"].as_u64(), req["registered"].as_bool()), (Some("worse"), Some(5), Some(true)), "{req}");
    let oc = appended.iter().find(|(k, _)| *k == "goal_outcome").expect("one outcome").1;
    assert_eq!((oc["o"].as_u64(), oc["source"].as_str(), oc["p_permille"].as_u64(), oc["brier_ppm"].as_u64()), (Some(1), Some("card"), Some(900), Some(10_000)), "{oc}");
    assert_eq!(oc["harm"], json!(["guard:cost_turn"]));
    assert_eq!(appended.iter().filter(|(k, _)| *k == "goal_outcome").count(), 1);
    assert_eq!(out.iter().filter(|o| matches!(o, Out::Rollback { to, reason, .. } if to == "tP" && reason == "guardrail:cost_turn")).count(), 1, "the guardrail still rolls back, once");
    assert!(st.preds[0].resolved);

    // no noise floor (4 baseline turns): the prediction resolves with no outcome (no calibration point)
    let mut st2 = State::default();
    st2.preds.push(Pred { id: "P-1".into(), goal: "G".into(), sha: sha.clone(), card: Some("requests_turn".into()), expect: "better".into(), p: 700, seq: 5, kind: "k".into(), ..Pred::default() });
    measure(&mut st2, "requests_turn", &sha, Some(3), false, None).unwrap();
    st2.obs = (0..4).map(|i| turn(i, 10 + i as i64, "tP", 100, 3)).chain((0..3).map(|i| turn(10 + i, 30 + i as i64, "tR", 100, 1))).collect();
    let mut out = Vec::new();
    complete(&mut st2, &recs, &mut out);
    assert!(out.iter().any(|o| matches!(o, Out::Append("card_reading", v) if v["verdict"] == "no_noise_floor")));
    assert!(!out.iter().any(|o| matches!(o, Out::Append("goal_outcome", _))), "no noise floor: nothing to score");
    assert!(st2.preds[0].resolved);
}

/// GBAL rerun (G7): the commit rules refused the promote after the prediction, the agent amended and re-filed on the
/// final sha. A re-file that replaces the goal's newest prediction while its change never went live keeps the round and
/// the check-in step; the old prediction closes `superseded` with its pending readings, and never registers.
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn g7_a_refile_before_live_supersedes_and_spends_no_round() {
    let (dir, root) = crate::grant::tests::test_home("gsup", "");
    let k = |cmd: &str, a: Value| kernel(&root, cmd, &a);
    let recs = |kind: &str| crate::lineage::records(&root.p.rh).unwrap().into_iter().filter(|r| r["kind"] == kind).map(|r| r["data"].clone()).collect::<Vec<_>>();
    let at = |sha: &str, tid: &str| root.append("tuple", json!({"id": tid, "tuple": {"body": {"sha": sha}}})).unwrap();
    assert!(line(&root, r#"/goal "fewer requests" done=card:requests_turn<=2 rounds=1 guard=cost_turn:300"#));
    let pr = |sha: &str| k("goal.predict", json!({"sha": sha, "mode": "A", "card": "requests_turn", "expect": "better", "p_permille": 600}));
    let (s1, s2, s3) = ("1".repeat(40), "2".repeat(40), "3".repeat(40));
    let p1 = pr(&s1).unwrap();
    assert_eq!((p1["round"].as_u64(), p1["supersedes"].clone()), (Some(1), Value::Null));
    let st = k("goal.status", json!({})).unwrap();
    assert_eq!(st["pending"].as_array().unwrap().iter().filter(|m| m["sha"] == s1.as_str()).count(), 2, "goal card + guardrail: {st}");
    // two amends: each re-file supersedes the last; rounds=1 and the check-in are not spent again
    let p2 = pr(&s2).unwrap();
    assert_eq!((p2["round"].as_u64(), p2["supersedes"].as_str()), (Some(1), Some("P-000001")), "{p2}");
    let p3 = pr(&s3).unwrap();
    assert_eq!((p3["round"].as_u64(), p3["supersedes"].as_str()), (Some(1), Some("P-000002")), "{p3}");
    let st = k("goal.status", json!({})).unwrap();
    assert_eq!((st["goal"]["spent"]["rounds"].as_u64(), st["goal"]["rounds_before_checkin"].as_u64()), (Some(1), Some(2)), "{st}");
    let pending = st["pending"].as_array().unwrap();
    assert!(pending.iter().all(|m| m["sha"] == s3.as_str()) && pending.len() == 2, "the superseded predictions' readings are dropped: {st}");
    assert_eq!(st["predictions"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect::<Vec<_>>(), ["P-000003"], "{st}");
    let lp = recs("goal_predict");
    assert_eq!(lp.iter().map(|r| r["supersedes"].clone()).collect::<Vec<_>>(), [Value::Null, json!("P-000001"), json!("P-000002")]);
    // the final commit goes live; a new prediction now is a new round, refused by the owner's budget of 1
    let r3 = at(&s3, "t3");
    assert_eq!(pr(&"4".repeat(40)).unwrap_err().0, "goal.budget");
    // a superseded commit that goes live after all is not registered by its stale prediction
    for i in 0..5 {
        turn_closed(&root, &Turn { seq: 1 + i, kind: "owner".into(), end: 1000 + i as i64, tuple: Some("t0".into()), cost_musd: 100, requests: 3, ..Turn::default() });
    }
    k("card.measure", json!({"card": "requests_turn", "sha": s1, "n": 3})).unwrap();
    let r1 = at(&s1, "t1");
    let e3 = crate::util::parse_rfc3339(r3["at"].as_str().unwrap()).unwrap();
    let e1 = crate::util::parse_rfc3339(r1["at"].as_str().unwrap()).unwrap().max(e3 + 10);
    for i in 0..3 {
        turn_closed(&root, &Turn { seq: 10 + i, kind: "owner".into(), end: e1 + 1 + i as i64, tuple: Some("t1".into()), cost_musd: 100, requests: 1, ..Turn::default() });
    }
    let r = recs("card_reading").into_iter().find(|r| r["sha"] == s1.as_str() && r["card"] == "requests_turn").expect("s1's reading");
    assert_eq!((r["registered"].as_bool(), r["prediction"].clone()), (Some(false), Value::Null), "{r}");
    // a superseded prediction scores nothing; the change, live in the card goal with no forecast, is scored at the
    // Root's abstention forecast (L3 D3)
    let oc = recs("goal_outcome");
    assert!(oc.iter().all(|o| o["prediction"].is_null()), "{oc:?}");
    assert_eq!(oc.iter().map(|o| (o["source"].as_str(), o["p_permille"].as_u64(), o["expect"].as_str(), o["sha"].as_str())).collect::<Vec<_>>(),
        [(Some("unforecast"), Some(500), Some("better"), Some(s1.as_str()))]);
    let _ = std::fs::remove_dir_all(&dir); // exact path, made by test_home
}

/// `met` needs attribution: the threshold reached on a reading that is noise (the GBAL rerun's runs B and C: 2.4 → 2.0,
/// z +1.64, threshold ≤ 2) is `met_unattributed`, with an owner alert; a registered reading beyond noise in the
/// predicted direction makes it `met`.
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn met_requires_a_registered_change_beyond_noise() {
    for (after, want) in [([2, 2, 2, 2, 2], "met_unattributed"), ([1, 1, 1, 1, 1], "met")] {
        let (dir, root) = crate::grant::tests::test_home(if want == "met" { "gmet" } else { "gmun" }, "");
        let k = |cmd: &str, a: Value| kernel(&root, cmd, &a);
        assert!(line(&root, r#"/goal "fewer requests" done=card:requests_turn<=2"#));
        for (i, r) in [3, 3, 2, 2, 2].iter().enumerate() {
            turn_closed(&root, &Turn { seq: 1 + i as u64, kind: "owner".into(), end: 1000 + i as i64, tuple: Some("t0".into()), requests: *r, ..Turn::default() });
        }
        let sha = "a".repeat(40);
        k("goal.predict", json!({"sha": sha, "mode": "A", "card": "requests_turn", "expect": "better", "p_permille": 450})).unwrap();
        let ra = root.append("tuple", json!({"id": "tA", "tuple": {"body": {"sha": sha}}})).unwrap();
        let e = crate::util::parse_rfc3339(ra["at"].as_str().unwrap()).unwrap();
        for (i, r) in after.iter().enumerate() {
            turn_closed(&root, &Turn { seq: 10 + i as u64, kind: "owner".into(), end: e + 1 + i as i64, tuple: Some("tA".into()), requests: *r, ..Turn::default() });
        }
        // v0.5.1: a later reading an owner upgrade cancelled (goal.rs `cancel_upgraded`) is not a kept reading
        let gid = crate::lineage::records(&root.p.rh).unwrap().into_iter().find(|r| r["kind"] == "goal").unwrap()["data"]["id"].clone();
        root.append("card_reading", json!({"action": "cancelled", "card": "requests_turn", "goal": gid, "measure": "M-000009", "sha": "c".repeat(40),
            "tuple": "tC", "upgrade": "u-1", "verdict": "cancelled"})).unwrap();
        let s = k("goal.stop", json!({"reason": "met"})).unwrap();
        assert_eq!((s["reason"].as_str(), s["verified"].as_bool()), (Some(want), Some(want == "met")), "{s}");
        let last = crate::lineage::records(&root.p.rh).unwrap().into_iter().rev().find(|r| r["kind"] == "goal").unwrap();
        assert_eq!((last["data"]["reason"].as_str(), last["data"]["claimed"].as_str()), (Some(want), Some("met")), "{last}");
        let alerts = std::fs::read_to_string(root.p.r("alerts.jsonl")).unwrap_or_default();
        assert_eq!(alerts.contains("goal_met_unattributed"), want == "met_unattributed", "{alerts}");
        let _ = std::fs::remove_dir_all(&dir); // exact path, made by test_home
    }
}

/// TUI-PICKER: an agent question (`ask_owner`) stops the active goal's wall clock until the owner writes or the zone
/// makes a request; money, tokens and rounds are untouched; a paused goal is not touched.
#[test]
fn the_wall_clock_waits_while_the_owner_is_asked() {
    let mut g = Goal { state: "active".into(), since: 1_000, wall_s: 10, usd_musd: 500, rounds: 3, ..Goal::default() };
    assert_eq!(g.wall_ms(5_000), 4_000);
    g.wait(5_000);
    assert_eq!((g.wall_ms(5_000), g.wall_ms(9_000), g.wall_ms(1_000_000)), (4_000, 4_000, 4_000), "stopped while waiting");
    assert_eq!(g.budget_hit(1_000_000, false), None, "a long wait never spends the wall budget");
    g.wait(7_000);
    assert_eq!(g.waiting_since, Some(5_000), "a second question does not move the start of the wait");
    g.unwait(9_000);
    assert_eq!((g.waiting_since, g.wall_ms(9_000), g.wall_ms(10_000)), (None, 4_000, 5_000), "runs again from where it stopped");
    g.unwait(12_000);
    assert_eq!(g.wall_ms(12_000), 7_000, "no wait: nothing to resume");
    // a pause or stop during a wait banks only the active time
    g.wait(12_000);
    g.bank(20_000);
    assert_eq!((g.active_ms, g.waiting_since), (7_000, None));
    // paused or stopped goals do not wait
    let mut p = Goal { state: "paused".into(), active_ms: 3_000, ..Goal::default() };
    p.wait(1);
    assert_eq!((p.waiting_since, p.wall_ms(99)), (None, 3_000));
    assert_eq!((g.spent_musd, g.used_rounds, g.usd_musd, g.rounds), (0, 0, 500, 3), "money and rounds untouched");
    // a state.json from before this field still loads
    let old: Goal = serde_json::from_value(json!({"id": "G-1", "kind": "owner", "done": null, "usd_musd": 1, "wall_s": 1, "rounds": 1,
        "guard": [], "state": "active", "spent_musd": 0, "active_ms": 0, "since": 0, "used_rounds": 0, "since_checkin": 0})).unwrap();
    assert_eq!(old.waiting_since, None);
}

/// TUI-PICKER, through the Root's state: `asked` (the relayed ask frame) stops the clock; `resumed` (an owner line, or
/// the egress admitting a zone request) and a settled model request start it again; spend is metered throughout.
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn an_agent_question_pauses_the_goal_clock_until_the_owner_or_a_request() {
    let (dir, root) = crate::grant::tests::test_home("gask", "");
    asked(&root);
    assert!(!root.p.r("state/goals/state.json").exists(), "no goal: nothing written");
    assert!(line(&root, r#"/goal "x" usd=1.00"#));
    let st = |root: &Root| kernel(root, "goal.status", &json!({})).unwrap()["goal"].clone();
    // the goal has been active for 100 s
    with(&root, |s, _, _| {
        s.goals[0].since -= 100_000;
        Ok(())
    }).unwrap();
    assert_eq!((st(&root)["spent"]["wall_s"].as_i64(), st(&root)["waiting_owner"].as_bool()), (Some(100), Some(false)));
    asked(&root);
    assert_eq!(st(&root)["waiting_owner"], true);
    // the question was asked 40 s ago: 60 s of active time, and it stays 60 while the owner is away
    with(&root, |s, _, _| {
        s.goals[0].waiting_since = s.goals[0].waiting_since.map(|w| w - 40_000);
        Ok(())
    }).unwrap();
    assert_eq!(st(&root)["spent"]["wall_s"], 60);
    // the owner writes (link.rs `line` calls `resumed`): the clock runs again from 60 s
    resumed(&root);
    assert_eq!((st(&root)["spent"]["wall_s"].as_i64(), st(&root)["waiting_owner"].as_bool()), (Some(60), Some(false)));
    // a question, then a model request with no owner line: the request settles and the clock runs (admission calls
    // `resumed` too); its cost is the goal's as always
    asked(&root);
    assert_eq!(st(&root)["waiting_owner"], true);
    egress_spent(&root, 70);
    let g = st(&root);
    assert_eq!((g["waiting_owner"].as_bool(), g["spent"]["musd"].as_u64(), g["spent"]["rounds"].as_u64()), (Some(false), Some(70), Some(0)));
    // a paused goal never waits
    assert!(line(&root, "/goal pause"));
    asked(&root);
    assert_eq!(st(&root)["waiting_owner"], false);
    let _ = std::fs::remove_dir_all(&dir); // exact path, made by test_home
}

/// The second review of v0.5.1: an owner upgrade's commit (`lkg{upgrade}`) is a boundary no reading spans. A reading in
/// flight on a change from before it would count only the old tuple's turns (the upgraded tuple has another id) and,
/// on a failed gate, roll back to the parent from before, the old kernel. It ends `cancelled`, with a record. A change
/// made after the upgrade takes no baseline (and so no rollback target) from before it.
#[test]
fn readings_never_span_an_owner_upgrade() {
    let (a, b) = ("a".repeat(40), "b".repeat(40));
    let mut st = State::default();
    measure(&mut st, "cost_turn", &a, Some(3), true, None).unwrap();
    let at = |ms: i64| crate::util::rfc3339(ms);
    let mut recs = vec![json!({"kind": "tuple", "seq": 9, "at": at(20), "data": {"id": "tA", "tuple": {"body": {"sha": a}}}}),
        json!({"kind": "tuple", "seq": 11, "at": at(40), "data": {"id": "tU", "tuple": {"body": {"sha": a}}, "upgrade": {"id": "u-1"}}}),
        json!({"kind": "lkg", "seq": 12, "at": at(50), "data": {"tuple": "tU", "upgrade": "u-1"}})];
    st.obs = (0..5).map(|i| obs(i, 10 + i as i64, "t0", 100 + i)).collect();
    st.obs.extend((0..3).map(|i| obs(10 + i, 30 + i as i64, "tA", 102)));
    let mut out = Vec::new();
    complete(&mut st, &recs, &mut out);
    assert!(st.measures.is_empty(), "ended");
    assert!(!out.iter().any(|o| matches!(o, Out::Rollback { .. })), "never rolled back across the upgrade");
    assert!(out.iter().any(|o| matches!(o, Out::Append("card_reading", r) if r["action"] == "cancelled" && r["verdict"] == "cancelled"
        && r["upgrade"] == "u-1" && r["measure"] == "M-000001" && r["tuple"] == "tA")), "recorded");
    // a guardrail registered on the upgraded tuple (it keeps the body, so the reading's change is still tA) ends too
    let mut out = Vec::new();
    measure(&mut st, "cost_turn", &a, Some(3), true, Some(100)).unwrap();
    complete(&mut st, &recs, &mut out);
    assert!(st.measures.is_empty() && !out.iter().any(|o| matches!(o, Out::Rollback { .. })), "{}", out.len());
    // a change after the upgrade: its baseline is the upgraded tuple's turns only; with none, no rollback target
    recs.push(json!({"kind": "tuple", "seq": 14, "at": at(60), "data": {"id": "tB", "tuple": {"body": {"sha": b}}}}));
    measure(&mut st, "cost_turn", &b, Some(3), true, None).unwrap();
    st.obs.extend((0..3).map(|i| obs(20 + i, 61 + i as i64, "tB", 102)));
    let mut out = Vec::new();
    complete(&mut st, &recs, &mut out);
    assert!(st.measures.is_empty());
    assert!(!out.iter().any(|o| matches!(o, Out::Rollback { .. })), "no rollback to tA, a tuple of the old kernel");
    assert!(out.iter().any(|o| matches!(o, Out::Append("card_reading", r) if r["sha"] == b.as_str() && r["parent_tuple"] == "")));
}
