//! X6 tests ($0): RF-22 (the decay table, and decay, archive and its debt on an installed test home), RF-23 (the exact
//! Fisher tail; null usage quarantines at most 1 per 30 days; a harmful cap is quarantined at the designed power;
//! quarantine debt, re-adds, release and the 72 h step), RF-24 (bloat refusal and the shrink pass).

use super::*;
use std::fs;

const T0: i64 = 1_893_456_000_000; // 2030-01-01T00:00:00Z: later than any clock these unit tests run on

fn ends(n: usize, from: i64, step: i64) -> VecDeque<i64> {
    (0..n).map(|i| from + i as i64 * step).collect()
}

/// RF-22: the decay table. Too young (3 days, 40 owner turns since registration) → not evaluated; then `cold` at 40
/// owner turns and 7 days without use, `archive_due` at 120 and 21; a use resets; the charter can only lengthen it.
#[test]
fn rf22_decay_table() {
    let x = Extend::default();
    let reg = T0;
    let e = ends(130, T0 + HOUR, HOUR); // 130 owner turns, one an hour from T0 + 1 h
    let at = |d: i64| T0 + d * DAY;
    assert_eq!(decay(&x, at(2), reg, None, &e), None, "younger than 3 days");
    assert_eq!(decay(&x, at(9), reg, None, &ends(39, T0 + HOUR, HOUR)), None, "fewer than 40 owner turns since registration");
    let cases = [
        (at(6), None, "active"),                         // never used, 130 turns but 6 days
        (at(7), None, "cold"),                           // never used: 7 days, 130 turns
        (at(20), None, "cold"),                          // 120+ turns but 20 days
        (at(21), None, "archive_due"),                   // 120 turns and 21 days
        (at(30), Some(T0 + 100 * HOUR), "cold"),         // 30 turns since use (< 40)? no: 30 < 40 → active below
    ];
    for (now, last, want) in &cases[..4] {
        assert_eq!(decay(&x, *now, reg, *last, &e), Some(*want), "{now} {last:?}");
    }
    assert_eq!(decay(&x, cases[4].0, reg, cases[4].1, &e), Some("active"), "30 owner turns since the last use");
    assert_eq!(decay(&x, at(30), reg, Some(T0 + 90 * HOUR), &e), Some("cold"), "40 owner turns and 26 days since the last use");
    assert_eq!(decay(&x, at(30), reg, Some(T0 + 10 * HOUR), &e), Some("archive_due"), "121 owner turns and 29 days");
    let long = Extend { cold_turns: 60, cold_days: 10, ..Extend::default() };
    assert_eq!(decay(&long, at(30), reg, Some(T0 + 90 * HOUR), &e), Some("active"), "the charter lengthened cold to 60 turns");
    assert_eq!(turns_since(&ends(ENDS_MAX, 10, 1), 0), u64::MAX, "beyond the kept times: more than any rule needs");
}

/// RF-23: the tail is exact. Hand values, Vandermonde at N = 120 (the u128 bound), and the p in ppm.
#[test]
fn rf23_fisher_tail_is_exact() {
    assert_eq!(fisher_tail(10, 5, 5, 5), Some((1, 252)));
    assert_eq!(fisher_tail(20, 5, 4, 3), Some((155, 4845)));
    let (t, total) = fisher_tail(120, 60, 60, 0).unwrap();
    assert_eq!((t, total), (total, binom(120, 60).unwrap()), "Σ C(60,x)·C(60,60−x) = C(120,60)");
    assert_eq!(total, 96_614_908_840_363_322_603_893_139_521_372_656);
    assert_eq!(fisher_tail(120, 30, 20, 21).unwrap().0, 0);
    let all: Vec<(BTreeSet<String>, bool)> = (0..120).map(|i| (["skill/a".to_string()].into(), i % 3 == 0)).collect();
    let c = correlate(&all, &["skill/a".into()]);
    assert!(c[0].untestable && !c[0].quarantine, "used in every turn: no comparison group");
}

/// xorshift64*: a deterministic stream for the simulations.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn hit(&mut self, permille: u64) -> bool {
        self.next() % 1000 < permille
    }
}

/// One simulated stream: 30 turns a day, the last 120 tested each day, 20 caps with uses between 5% and 60%, harm 15%
/// (plus `harmful`: a cap used 20% of the time with harm 50% when used). Returns the days a quarantine fired per cap.
fn simulate(seed: u64, days: usize, harmful: bool) -> Vec<(String, usize)> {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let caps: Vec<(String, u64)> = (0..20).map(|i| (format!("skill/c{i}"), 50 + i * 29)).collect();
    let mut all: Vec<String> = caps.iter().map(|c| c.0.clone()).collect();
    if harmful {
        all.push("skill/bad".into());
    }
    let mut window: VecDeque<(BTreeSet<String>, bool)> = VecDeque::new();
    let mut fired: Vec<(String, usize)> = Vec::new();
    let mut out_until: BTreeMap<String, usize> = BTreeMap::new();
    for day in 0..days {
        for _ in 0..30 {
            let mut used: BTreeSet<String> = caps.iter().filter(|c| rng.hit(c.1)).map(|c| c.0.clone()).collect();
            let bad = harmful && rng.hit(200);
            if bad {
                used.insert("skill/bad".into());
            }
            let harmed = rng.hit(if bad { 500 } else { 150 });
            window.push_back((used, harmed));
            while window.len() > CORR_N {
                window.pop_front();
            }
        }
        if window.len() < CORR_N {
            continue;
        }
        let live: Vec<String> = all.iter().filter(|c| out_until.get(*c).is_none_or(|u| day >= *u)).cloned().collect();
        let turns: Vec<(BTreeSet<String>, bool)> = window.iter().cloned().collect();
        for c in correlate(&turns, &live).into_iter().filter(|c| c.quarantine) {
            out_until.insert(c.cap.clone(), day + 7); // quarantined: out of the tests until a new version, 7 days on
            fired.push((c.cap, day));
        }
    }
    fired
}

/// RF-23: on null usage (harm independent of every cap) false quarantines stay ≤ 1 per 30 days (ADR-011 DoD 2),
/// summed over 20 seeds × 300 days; a harmful cap (harm 50% when used vs 15%, used in 20% of turns) is quarantined
/// within 7 days of the first full window in ≥ 90% of seeds, and the null caps beside it stay as quiet.
#[test]
fn rf23_null_usage_rarely_quarantines_and_a_harmful_cap_is_caught() {
    let (seeds, days) = (20u64, 300usize);
    let null: usize = (0..seeds).map(|s| simulate(s, days, false).len()).sum();
    let months = seeds as f64 * days as f64 / 30.0;
    assert!((null as f64) / months <= 1.0, "{null} false quarantines in {months} months");
    let mut caught = 0;
    let mut false_beside = 0;
    for s in 0..seeds {
        let f = simulate(100 + s, 30, true);
        caught += usize::from(f.iter().any(|(c, d)| c == "skill/bad" && *d <= 4 + 7));
        false_beside += f.iter().filter(|(c, _)| c != "skill/bad").count();
    }
    assert!(caught * 10 >= seeds as usize * 9, "caught in {caught} of {seeds}");
    assert!(false_beside <= seeds as usize, "{false_beside} false quarantines beside the harmful cap in {seeds} months");
    println!("RF-23: null {null} quarantines in {months} months; harmful caught {caught}/{seeds}; false beside {false_beside}");
}

/// RF-24: over budget a change that grows a measured quantity is refused; one that shrinks or keeps it passes; the
/// budget scales with the level's factor.
#[test]
fn rf24_bloat_refusal_and_shrink_pass() {
    let x = Extend::default();
    let l0 = budget(&x, 0);
    assert_eq!(l0, Measure { caps: 64, t0: 4 << 20, ext: 128 << 20 });
    assert_eq!(budget(&x, 1), Measure { caps: 96, t0: 6 << 20, ext: 192 << 20 });
    let at = Measure { caps: 64, t0: 1 << 20, ext: 1 << 20 };
    assert_eq!(bloat_check(&at, &Measure { caps: 64, ..at }, &l0), Ok(()), "at the budget, unchanged");
    let e = bloat_check(&at, &Measure { caps: 65, ..at }, &l0).unwrap_err();
    assert!(e.starts_with("active caps: 65"), "{e}");
    let over = Measure { caps: 70, t0: 5 << 20, ext: 1 << 20 };
    assert_eq!(bloat_check(&over, &Measure { caps: 69, t0: 5 << 20, ext: 2 << 20 }, &l0), Ok(()), "over budget, but shrinking caps and growing ext within it");
    assert_eq!(bloat_check(&over, &Measure { caps: 70, t0: (5 << 20) - 1, ext: 1 << 20 }, &l0), Ok(()), "a smaller t0 passes");
    assert!(bloat_check(&over, &Measure { caps: 70, t0: (5 << 20) + 1, ext: 1 << 20 }, &l0).unwrap_err().starts_with("t0 bytes"));
    assert!(bloat_check(&at, &Measure { ext: (128 << 20) + 1, ..at }, &l0).unwrap_err().starts_with("ext set bytes"));
}

fn turn(seq: u64, kind: &str, end: i64, caps: &[&str], harmed: bool) -> Turn {
    Turn { seq, kind: kind.into(), start: end - 1000, end, requests: 1, correction: harmed,
        caps: caps.iter().map(|c| (c.to_string(), (1, "zone".to_string()))).collect(), ..Turn::default() }
}

fn change(caps: &[(&str, &str, &str)]) -> Value {
    json!({"candidate": {"body": {"binary": "", "sha": "a".repeat(40)}}, "scope": "t2", "sha": "a".repeat(40),
        "caps": caps.iter().map(|(id, op, ver)| json!({"id": id, "op": op, "ver": if *op == "remove" { Value::Null } else { json!(ver) }, "prev_ver": null})).collect::<Vec<_>>()})
}

fn records(root: &Root, kind: &str) -> Vec<Value> {
    crate::lineage::records(&root.p.rh).unwrap().into_iter().filter(|r| r["kind"] == kind).map(|r| r["data"].clone()).collect()
}

/// RF-22 and RF-23 on an installed test home in free mode (in process; the clock is passed in): owner days become
/// `turns` records; an unused cap decays to `cold` and `archive_due`; archive debt refuses an added cap after 7 days and
/// the move clears it; a direct rollback quarantines with debt, re-add bans and `/release`; the 72 h step records the
/// demoting harm; `/gated` refuses `change.open` outright.
#[cfg(target_os = "macos")]
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn rf22_rf23_selection_on_a_test_home() {
    let (dir, root) = crate::grant::tests::test_home("s", "[extend]\nmode = \"free\"\n");
    // three zone caps registered now (real clock), evaluated in 2030
    root.append("change_open", json!({"caps": [{"evidence": "zone", "id": "mem/used", "op": "add", "prev_ver": null, "ver": "1"},
        {"evidence": "zone", "id": "mem/idle", "op": "add", "prev_ver": null, "ver": "1"}, {"evidence": "zone", "id": "loop/old", "op": "add", "prev_ver": null, "ver": "1"}],
        "group": 1, "parent_tuple": "tp", "scope": "t2", "sha": "a".repeat(40), "tuple": "t1"})).unwrap();
    root.append("change_close", json!({"attribution": null, "group": 1, "outcome": "clean", "turns": 6})).unwrap();
    // 130 owner turns, one an hour from T0 (two owner days: 24 + 106); `loop/old` used at turn 5 only
    for i in 0..130u64 {
        let caps: &[&str] = if i == 5 { &["mem/used", "loop/old"] } else { &["mem/used"] };
        turn_closed(&root, &turn(i, "owner", T0 + (i as i64 + 1) * HOUR, caps, false));
        if i == 1 {
            turn_closed(&root, &turn(200, "background", T0 + 2 * HOUR + 60_000, &[], true));
        }
    }
    tick_at(&root, T0 + 30 * DAY);
    let days = records(&root, "turns");
    assert!(days.len() >= 2, "{days:?}");
    assert_eq!((days[0]["owner"].as_u64(), days[0]["background"].as_u64(), days[0]["background_harmed"].as_u64()), (Some(23), Some(1), Some(1)));
    assert_eq!(days.iter().map(|d| d["owner"].as_u64().unwrap()).sum::<u64>(), 130);
    let states: BTreeMap<String, String> = records(&root, "cap_state").into_iter().map(|d| (d["cap"].as_str().unwrap().into(), d["to"].as_str().unwrap().into())).collect();
    assert_eq!(states.get("mem/idle").map(String::as_str), Some("archive_due"), "never used: 130 owner turns, 30 days");
    assert_eq!(states.get("loop/old").map(String::as_str), Some("archive_due"), "124 owner turns and 29 days since its one use");
    assert!(!states.contains_key("mem/used"), "used in the last turn: stays active");
    let now = T0 + 30 * DAY;
    // archive debt: an added cap passes for 7 days, then is refused until the move; modifications always pass
    let add = change(&[("mem/new", "add", "1")]);
    assert!(admit_at(&root, &add, now + DAY).is_ok());
    assert_eq!(admit_at(&root, &add, now + 8 * DAY).unwrap_err().0, "extend.archive_debt");
    assert!(admit_at(&root, &change(&[("mem/used", "modify", "2")]), now + 8 * DAY).is_ok());
    root.append("change_open", json!({"caps": [{"evidence": "zone", "id": "mem/idle", "op": "remove", "prev_ver": "1", "ver": null},
        {"evidence": "zone", "id": "loop/old", "op": "remove", "prev_ver": "1", "ver": null}], "group": 2, "parent_tuple": "t1", "scope": "t2",
        "sha": "b".repeat(40), "tuple": "t2"})).unwrap();
    tick_at(&root, now + 8 * DAY);
    assert_eq!(records(&root, "cap_state").iter().filter(|d| d["to"] == "archived").count(), 2, "the moves are recorded");
    assert!(admit_at(&root, &add, now + 8 * DAY).is_ok(), "no archive debt left");

    // quarantine: a direct rollback of mem/used; debt refuses everything but its removal; re-adds wait; /release
    direct(&root, &["mem/used".into()], false);
    let q = records(&root, "cap_state").into_iter().find(|d| d["to"] == "quarantined").unwrap();
    assert_eq!((q["cap"].as_str(), q["reason"].as_str(), q["ver"].as_str()), (Some("mem/used"), Some("direct_rollback"), Some("1")));
    direct(&root, &["mem/used".into()], false);
    assert_eq!(records(&root, "cap_state").iter().filter(|d| d["to"] == "quarantined").count(), 1, "once");
    let t = now_ms();
    assert_eq!(admit_at(&root, &add, t).unwrap_err().0, "extend.quarantine_debt");
    let rm = change(&[("mem/used", "remove", "")]);
    assert!(admit_at(&root, &rm, t).is_ok(), "its removal is admitted");
    root.append("change_open", json!({"caps": [{"evidence": "zone", "id": "mem/used", "op": "remove", "prev_ver": "1", "ver": null}], "group": 3,
        "parent_tuple": "t2", "scope": "t2", "sha": "c".repeat(40), "tuple": "t3"})).unwrap();
    assert!(admit_at(&root, &add, t).is_ok(), "the debt is paid");
    assert_eq!(admit_at(&root, &change(&[("mem/used", "add", "2")]), t + DAY).unwrap_err().0, "extend.quarantine_debt", "a new version waits 7 days");
    assert!(admit_at(&root, &change(&[("mem/used", "add", "2")]), t + 8 * DAY).is_ok());
    assert_eq!(admit_at(&root, &change(&[("mem/used", "add", "1")]), t + 8 * DAY).unwrap_err().0, "extend.quarantine_debt", "the quarantined version only after /release");
    assert!(crate::grant::line(&root, "/release mem/used"));
    assert!(admit_at(&root, &change(&[("mem/used", "add", "1")]), t + DAY).is_ok());
    assert_eq!(records(&root, "cap_state").last().unwrap()["to"], "archived", "released while removed: it sits in quarantine/");
    assert_eq!(release(&root, "mem/used").unwrap_err().0, "root.not_found");

    // the 72 h step: a quarantined version still live after 72 h → the demoting harm record and the alert
    root.append("change_open", json!({"caps": [{"evidence": "zone", "id": "tool/x", "op": "add", "prev_ver": null, "ver": "9"}], "group": 4,
        "parent_tuple": "t3", "scope": "t2", "sha": "d".repeat(40), "tuple": "t4"})).unwrap();
    direct(&root, &["tool/x".into()], true);
    let qx = records(&root, "cap_state").into_iter().rfind(|d| d["cap"] == "tool/x").unwrap();
    let until = parse_rfc3339(qx["until"].as_str().unwrap()).unwrap();
    assert!(until - now_ms() > 29 * DAY, "a repeat ban lasts 30 days");
    tick_at(&root, now_ms() + 71 * HOUR);
    assert!(records(&root, "harm").is_empty());
    tick_at(&root, now_ms() + 73 * HOUR);
    let harm = records(&root, "harm");
    assert_eq!((harm.len(), harm[0]["signal"].as_str(), harm[0]["action"].as_str(), harm[0]["caps"][0].as_str()), (1, Some("quarantine_debt"), Some("demote"), Some("tool/x")));
    tick_at(&root, now_ms() + 80 * HOUR);
    assert_eq!(records(&root, "harm").len(), 1, "once per quarantine");
    let down = records(&root, "ladder").into_iter().rfind(|d| matches!(d["direction"].as_str(), Some("down" | "floor"))).unwrap(); // F2: "floor" at L0
    assert_eq!((down["trigger"]["kind"].as_str(), down["cause"].as_str()), (Some("quarantine_debt"), Some("demotion")), "R7 demoted, rolling nothing back");
    drop(root);
    fs::remove_dir_all(&dir).unwrap();
}

/// The gated path refuses `change.open` before any check (X3's Need), and an unknown home starts empty.
#[cfg(target_os = "macos")]
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn gated_refuses_change_open() {
    let (dir, root) = crate::grant::tests::test_home("q", "");
    let e = admit(&root, &change(&[("mem/a", "add", "1")]), true).unwrap_err();
    assert_eq!((e.0, e.2["reason"].as_str()), ("root.mode", Some("gated")));
    assert!(view(&root)["quarantined"].as_array().unwrap().is_empty());
    drop(root);
    fs::remove_dir_all(&dir).unwrap();
}

/// ADR-011 §7.4 (INTEG-A): a `dep` change passes A5 only when a live owner grant covers its exact crate set; no set,
/// a different version or checksum, a subset or a superset, and a pending or denied request are all `extend.dep_grant`.
/// Other scopes never need one.
#[cfg(target_os = "macos")]
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn a_dep_change_needs_a_live_grant_of_its_exact_crate_set() {
    let (dir, root) = crate::grant::tests::test_home("d", "[extend]\nmode = \"free\"\n");
    let krate = |n: &str, v: &str, c: char| json!({"checksum": c.to_string().repeat(64), "name": n, "version": v});
    let set = json!([krate("regex", "1.11.1", 'a'), krate("memchr", "2.7.4", 'b')]);
    let dep = |crates: Value| {
        let mut c = change(&[]);
        c["scope"] = json!("dep");
        c["crates"] = crates;
        c
    };
    let refused = |crates: Value| admit(&root, &dep(crates), true).err().map(|e| e.0);
    assert_eq!(refused(set.clone()), Some("extend.dep_grant"), "no grant yet");
    let e = admit(&root, &dep(Value::Null), true).unwrap_err();
    assert_eq!((e.0, e.2["reason"].as_str()), ("extend.dep_grant", Some("no live dep grant covers this exact crate set")));
    assert!(admit(&root, &change(&[]), true).is_ok(), "a t2 change needs no dep grant");
    let id = crate::grant::request(&root, &json!({"kind": "dep", "value": set, "why": "a regex engine for the log tool"})).unwrap()["id"]
        .as_str().unwrap().to_string();
    assert_eq!(refused(set.clone()), Some("extend.dep_grant"), "a pending request covers nothing");
    crate::grant::decide(&root, &id, true, None, Some(crate::access::PHRASE)).unwrap();
    // the exact set passes, in any order; anything else is refused
    assert!(admit(&root, &dep(set.clone()), true).is_ok());
    assert!(admit(&root, &dep(json!([krate("memchr", "2.7.4", 'b'), krate("regex", "1.11.1", 'a')])), true).is_ok(), "order does not matter");
    for other in [json!([krate("regex", "1.11.1", 'a')]), json!([krate("regex", "1.11.1", 'a'), krate("memchr", "2.7.4", 'b'), krate("cc", "1.0.0", 'c')]),
        json!([krate("regex", "1.11.2", 'a'), krate("memchr", "2.7.4", 'b')]), json!([krate("regex", "1.11.1", 'c'), krate("memchr", "2.7.4", 'b')])] {
        assert_eq!(refused(other.clone()), Some("extend.dep_grant"), "{other}");
    }
    drop(root);
    fs::remove_dir_all(&dir).unwrap();
}

/// The newest tuple without a quarantined version: the parent of the change that introduced it, if formed ≤ 14 days ago.
#[test]
fn the_rollback_target_of_a_stuck_quarantine() {
    let r = |at: i64, kind: &str, data: Value| json!({"at": rfc3339(at), "by": "root", "data": data, "kind": kind});
    let recs = vec![r(T0, "tuple", json!({"id": "tA"})),
        r(T0 + DAY, "change_open", json!({"caps": [{"id": "skill/x", "op": "add", "ver": "v1"}], "parent_tuple": "tA", "tuple": "tB"})),
        r(T0 + 2 * DAY, "change_open", json!({"caps": [{"id": "skill/y", "op": "add", "ver": "v1"}], "parent_tuple": "tB", "tuple": "tC"}))];
    assert_eq!(without(&recs, "skill/x", Some("v1"), T0 + 10 * DAY), Some("tA".into()));
    assert_eq!(without(&recs, "skill/x", Some("v1"), T0 + 15 * DAY), None, "older than 14 days");
    assert_eq!(without(&recs, "skill/y", Some("v1"), T0 + 3 * DAY), Some("tB".into()), "formed by the change that opened it");
    assert_eq!(without(&recs, "skill/x", Some("v2"), T0 + 3 * DAY), None);
}

