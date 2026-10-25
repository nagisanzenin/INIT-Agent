//! R7 unit tests on test ledgers and the fake clock (ADR-011 §7; RF-25, RF-26): every input is a record, and the
//! clock is the `now` the step takes.

use super::*;

const T0: i64 = 1_893_456_000_000; // 2030-01-01T00:00:00Z

/// A chain builder: `at` in days after T0; Root-written unless the kind is a kernel kind.
struct Chain(Vec<Value>);
impl Chain {
    fn new(level: i64) -> Chain {
        let mut c = Chain(Vec::new());
        c.add(0.0, "genesis", json!({}));
        for l in 0..level {
            c.add(0.0, "ladder", json!({"cause": "unlock", "direction": "up", "from": l, "to": l + 1, "trigger": {"kind": "test_fixture", "seq": 0}}));
        }
        c
    }
    fn free(level: i64) -> Chain {
        let mut c = Chain::new(level);
        c.add(0.0, "charter", json!({"extend": {"mode": "free"}}));
        c
    }
    fn add(&mut self, day: f64, kind: &str, data: Value) -> &mut Chain {
        let by = if ["promotion", "gate", "body_safe", "owner"].contains(&kind) { "kernel:000000000000" } else { "root" };
        self.add_by(day, by, kind, data)
    }
    fn add_by(&mut self, day: f64, by: &str, kind: &str, data: Value) -> &mut Chain {
        let at = rfc3339(T0 + (day * DAY as f64) as i64);
        self.0.push(json!({"at": at, "by": by, "data": data, "kind": kind, "seq": self.0.len()}));
        self
    }
    /// One owner day's `turns` record (the day starts at `day`).
    fn turns(&mut self, day: f64, owner: u64) -> &mut Chain {
        let start = rfc3339(T0 + (day * DAY as f64) as i64);
        self.add(day + 1.0, "turns", json!({"background": 0, "background_harmed": 0, "day": &start[..10], "owner": owner, "owner_harmed": 0, "start": start}))
    }
    /// `n` groups opened and closed at `day`: outcome, attribution, window turns; each changes its tuple.
    fn groups(&mut self, day: f64, n: usize, outcome: &str, attribution: Option<&str>, turns: u64) -> &mut Chain {
        for _ in 0..n {
            let g = self.0.len() as u64;
            self.add(day, "change_open", json!({"group": g, "parent_tuple": format!("p{g}"), "tuple": format!("t{g}"), "caps": []}));
            self.add(day, "change_close", json!({"attribution": attribution, "group": g, "outcome": outcome, "turns": turns}));
        }
        self
    }
    fn clean(&mut self, day: f64, n: usize) -> &mut Chain {
        self.groups(day, n, "clean", None, 5)
    }
    fn direct(&mut self, day: f64, n: usize) -> &mut Chain {
        self.groups(day, n, "rolled_back", Some("direct"), 3)
    }
    fn step(&self, day: f64, cap: i64) -> Step {
        Level::fold(&self.0).step(T0 + (day * DAY as f64) as i64, cap)
    }
    fn crit(&self, day: f64, id: &str) -> Value {
        Level::fold(&self.0).criteria(T0 + (day * DAY as f64) as i64).into_iter().find(|c| c["id"] == id).unwrap_or_default()
    }
    fn unmet(&self, day: f64) -> Vec<String> {
        let l = Level::fold(&self.0);
        l.criteria(T0 + (day * DAY as f64) as i64).into_iter().filter(|c| c["met"] != true).map(|c| c["id"].as_str().unwrap().to_string()).collect()
    }
}

fn up(s: &Step) -> bool {
    matches!(s, Step::Up(_))
}

/// The L1 record at L0 on the free path: 7 days, 40 owner turns (in days that started after the entry), 10 clean
/// groups, the baseline ring, no demotion in 7 days.
fn l1_record() -> Chain {
    let mut c = Chain::free(0);
    for d in 0..5 {
        c.turns(f64::from(d), 10);
    }
    c.clean(5.5, 10);
    c
}

/// RF-25 (RT-GAP-1, ADR-011 §7.1): 20 kernel-reported LKG promotions under distinct shas, a clean sealed audit and 200
/// days unlock nothing with no owner turns and no clean groups; neither do kernel-signed `change_close` or `turns`.
#[test]
fn rf25_self_reported_promotions_unlock_nothing() {
    for mut c in [Chain::new(0), Chain::free(0)] {
        for i in 0..20 {
            c.add(1.0, "promotion", json!({"lkg": true, "sha": format!("s{i}"), "scope": "body"}));
        }
        c.add(2.0, "audit", json!({"regression": false, "score_permille": 900}));
        // a hostile kernel cannot append Root kinds (kinds.rs), and the fold ignores them even if it could
        c.add_by(3.0, "kernel:000000000000", "turns", json!({"owner": 400, "start": rfc3339(T0 + DAY)}));
        for g in 100..120 {
            c.add_by(3.0, "kernel:000000000000", "change_open", json!({"group": g, "parent_tuple": "a", "tuple": format!("b{g}")}));
            c.add_by(3.0, "kernel:000000000000", "change_close", json!({"group": g, "outcome": "clean", "turns": 20}));
        }
        assert_eq!(c.step(200.0, 4), Step::Stay);
        assert_eq!((c.crit(200.0, "owner_turns")["have"].as_i64(), c.crit(200.0, "clean_groups")["have"].as_i64()), (Some(0), Some(0)));
        assert!(c.crit(200.0, "min_promotions").is_null(), "promotions are no criterion");
    }
}

/// RF-26, L0 → L1: the whole record unlocks; each missing term holds the level, named in `next.criteria`.
#[test]
fn rf26_l1_needs_the_whole_record() {
    let c = l1_record();
    assert!(up(&c.step(7.0, 4)), "{:?}", c.unmet(7.0));
    assert_eq!(c.unmet(6.9), ["days_at_level"]);
    assert_eq!(c.step(7.0, 0), Step::Stay, "the charter's max (or a release build's SUPPORTED_MAX_LEVEL) caps it");

    let mut c = Chain::free(0);
    for d in 0..5 {
        c.turns(f64::from(d), 10);
    }
    c.clean(5.5, 9);
    assert_eq!(c.unmet(8.0), ["clean_groups"]);
    c.groups(5.6, 1, "clean", None, 4).add(5.6, "change_open", json!({"group": 900, "parent_tuple": "same", "tuple": "same"}));
    c.add(5.6, "change_close", json!({"group": 900, "outcome": "clean", "turns": 9}));
    assert_eq!(c.crit(8.0, "clean_groups")["have"], 9, "fewer than 5 owner turns, or an unchanged tuple, is not clean");
    c.clean(5.7, 1);
    assert!(up(&c.step(8.0, 4)));

    // owner turns count only in days that started at or after the entry record
    let mut c = Chain::free(0);
    c.add(2.5, "ladder", json!({"cause": "demotion", "direction": "down", "from": 0, "to": 0, "trigger": {"kind": "panic", "seq": 1}}));
    for d in 0..5 {
        c.turns(f64::from(d), 10);
    }
    c.clean(5.5, 10);
    assert_eq!(c.crit(20.0, "owner_turns")["have"], 20, "the days of 2030-01-04 and -05 only");
    c.turns(5.0, 19);
    assert_eq!(c.unmet(20.0), ["owner_turns"]);
    c.turns(6.0, 1);
    assert!(up(&c.step(20.0, 4)));

    // a demotion in the last 7 days; then 7 direct harm rollbacks in the last 30 closed groups
    let mut c = l1_record();
    c.add(6.0, "ladder", json!({"cause": "demotion", "direction": "down", "from": 0, "to": 0, "trigger": {"kind": "ceiling_hit", "seq": 3}}));
    assert!(c.unmet(12.0).contains(&"demotions_last_7_days".to_string()));
    let with_direct = |n: usize| {
        let mut c = Chain::free(0);
        c.direct(0.5, n);
        for d in 1..6 {
            c.turns(f64::from(d), 10);
        }
        c.clean(6.5, 10);
        c
    };
    assert!(up(&with_direct(6).step(8.0, 4)), "6 of the last 30 is the L1 limit");
    let mut c = with_direct(7);
    assert_eq!(c.unmet(8.0), ["direct_rollbacks_last_30_groups"]);
    c.clean(6.6, 14);
    assert_eq!(c.crit(8.0, "direct_rollbacks_last_30_groups")["have"], 6, "the oldest direct left the last 30 closes");
    assert!(up(&c.step(8.0, 4)));
}

/// RF-26: the baseline ring counts owner turns in days that started after the newest group close that was not clean.
#[test]
fn rf26_the_baseline_ring_restarts_after_a_harmful_close() {
    let mut c = Chain::free(0);
    for d in 0..3 {
        c.turns(f64::from(d), 20);
    }
    c.groups(3.5, 1, "rolled_back", Some("suspect"), 2).clean(3.6, 10);
    assert_eq!(c.crit(10.0, "baseline_ring")["have"], 0, "the 60 turns before the rollback do not count");
    c.turns(4.0, 39);
    assert_eq!(c.unmet(10.0), ["baseline_ring"]);
    c.turns(5.0, 1);
    assert!(up(&c.step(10.0, 4)));
    assert_eq!(c.crit(10.0, "baseline_ring")["have"], 40);
}

/// RF-26, L1 → L2 → L3 → L4 on test ledgers.
#[test]
fn rf26_l2_l3_l4_records() {
    let twin = |ok: bool| json!({"cost": {"median_ratio_permille": if ok { 980 } else { 1600 }}, "ended": {"cur": 30, "p_ppm": 400_000, "twin": 29}, "pairs": 42});
    let mut c = Chain::free(1);
    for d in 0..20 {
        c.turns(f64::from(d), 8);
    }
    c.clean(2.0, 40).direct(2.5, 4);
    c.add(10.0, "twin_report", twin(true)).add(17.0, "twin_report", twin(true));
    assert!(up(&c.step(21.0, 4)), "{:?}", c.unmet(21.0));
    assert_eq!(c.unmet(20.5), ["days_at_level"]);
    c.add(15.0, "cap_state", json!({"cap": "skill/x", "to": "quarantined"}));
    assert_eq!(c.unmet(21.0), ["quarantines_last_14_days"]);
    assert!(up(&c.step(29.1, 4)), "the quarantine left the 14 days");
    c.add(18.0, "twin_report", twin(false));
    assert_eq!(c.unmet(29.1), ["twin_reports_no_regression"]);
    c.add(19.0, "twin_report", json!({"regression": false})).add(20.0, "twin_report", json!({"regression": false}));
    assert!(up(&c.step(29.1, 4)), "X9's own flag counts, and the newest two are clean");
    c.direct(20.5, 1);
    assert_eq!(c.unmet(29.1), ["direct_rollbacks_last_30_groups"]);

    let mut c = Chain::free(2);
    for d in 0..29 {
        c.turns(f64::from(d), 14);
    }
    c.clean(1.0, 100).direct(1.5, 3);
    for w in 0..4 {
        c.add(3.0 + 7.0 * f64::from(w), "twin_report", twin(true));
    }
    assert!(up(&c.step(30.0, 4)), "{:?}", c.unmet(30.0));
    let mut short = Chain::free(2);
    for d in 0..29 {
        short.turns(f64::from(d), 14);
    }
    short.clean(1.0, 100);
    short.add(3.0, "twin_report", twin(true)).add(10.0, "twin_report", twin(false)).add(17.0, "twin_report", twin(true)).add(24.0, "twin_report", twin(true));
    assert_eq!(short.crit(30.0, "twin_reports_no_regression")["have"], 2, "4 consecutive reports with no regression");

    let mut c = Chain::free(3);
    for d in 0..29 {
        c.turns(f64::from(d), 14);
    }
    c.clean(1.0, 100);
    assert!(up(&c.step(30.0, 4)), "L4 needs no twin rule beyond L3's: {:?}", c.unmet(30.0));
    assert_eq!(c.step(30.0, 3), Step::Stay, "charter autonomy.max_level = 4");
    assert_eq!(Chain::free(4).step(900.0, 4), Step::Stay);
}

/// ADR-011 §7.3: direct harm rollbacks in the last 30 closed groups above twice the level's threshold demote one
/// level (`harm_rate`, no rollback); at L0 there is no threshold.
#[test]
fn harm_rate_demotes_without_rollback() {
    let mut c = Chain::free(2);
    c.clean(1.0, 10).direct(1.0, 8);
    assert_eq!(c.step(1.0, 4), Step::Stay, "8 is the L2 limit (2 × 4)");
    c.direct(1.0, 1);
    let seq = c.0.len() as u64 - 1;
    assert_eq!(c.step(1.0, 4), Step::Down("harm_rate".into(), seq));
    assert!(NO_ROLLBACK.contains(&"harm_rate"));
    c.add(1.0, "ladder", json!({"cause": "harm_rate", "direction": "down", "from": 2, "to": 1, "trigger": {"kind": "harm_rate", "seq": seq}}));
    c.direct(1.0, 1);
    assert_eq!(c.step(1.0, 4), Step::Stay, "10 > 2 × 6 is false at L1, and the incident is open anyway");
    let mut c = Chain::free(0);
    c.direct(1.0, 30);
    assert_eq!(c.step(1.0, 4), Step::Stay, "no harm_rate at L0");
}

/// ADR-011 §7.3 on the free path: H4 (a demoting refusal, an escapee), panic, a crash loop on lkg and quarantine debt
/// demote; sealed audits, mutation runs, gate canaries and body safe mode do not. `gated` keeps today's triggers.
#[test]
fn triggers_by_path() {
    let fire = [("ceiling_hit", json!({"demotes": true})), ("escapee", json!({"pid": 1})), ("panic", json!({})), ("crash_loop", json!({"action": "demote"})),
        ("harm", json!({"action": "demote", "caps": ["skill/x"], "signal": "quarantine_debt"}))];
    let quiet = [("audit", json!({"regression": true})), ("mutation", json!({"pass": false, "source": "agent"})), ("gate", json!({"reason": "canary"})),
        ("body_safe", json!({"on": true})), ("harm", json!({"action": "rollback_newest", "signal": "H5b"}))];
    for (kind, data) in &fire {
        let mut c = Chain::free(2);
        c.add(1.0, kind, data.clone());
        let want = if *kind == "harm" { "quarantine_debt" } else { kind };
        assert_eq!(c.step(1.0, 4), Step::Down(want.into(), 4), "free: {kind}");
    }
    for (kind, data) in &quiet {
        let mut c = Chain::free(2);
        c.add(1.0, kind, data.clone());
        assert_eq!(c.step(1.0, 4), Step::Stay, "free: {kind} does not demote");
    }
    for (kind, data) in quiet.iter().take(4) {
        let mut c = Chain::new(2);
        c.add(1.0, kind, data.clone());
        assert_eq!(c.step(1.0, 4), Step::Down((*kind).into(), 3), "gated: {kind}");
    }
    let mut c = Chain::new(2);
    c.add(1.0, "escapee", json!({}));
    assert_eq!(c.step(1.0, 4), Step::Stay, "gated: an escapee is reported, as before");
    assert_eq!(Level::fold(&Chain::free(0).0).json(4)["path"], "free");
}

/// RT-L7, RR-24: each trigger is pending until a `ladder down`; a burst inside the incident costs one level; the
/// incident ends at the next `zone_start` or after 600 s; no climb in safe mode or while panicked.
#[test]
fn incidents_and_coalescing() {
    let hit = || json!({"demotes": true});
    let mut c = Chain::new(2);
    c.add(1.0, "ceiling_hit", hit()).add(1.0, "ceiling_hit", hit());
    c.add(1.0, "ladder", json!({"direction": "down", "from": 2, "to": 1, "trigger": {"kind": "ceiling_hit", "seq": 3}}));
    for _ in 0..3 {
        c.add(1.0, "ceiling_hit", hit());
    }
    let l = Level::fold(&c.0);
    assert_eq!((l.level, l.pending.clone(), l.entered_seq), (1, None, 5), "five hits, one level; the hold time restarts");
    c.add(1.0 + 601.0 / 86_400.0, "ceiling_hit", hit());
    assert_eq!(c.step(1.1, 4), Step::Down("ceiling_hit".into(), 9), "after 600 s a new incident");
    let mut c = Chain::new(2);
    c.add(1.0, "ladder", json!({"direction": "down", "from": 2, "to": 1})).add(1.0, "zone_start", json!({})).add(1.0, "ceiling_hit", hit());
    assert_eq!(c.step(1.0, 4), Step::Down("ceiling_hit".into(), 5), "the rollback target is ready: the incident is over");
    let mut c = l1_record();
    c.add(6.0, "safe", json!({"on": true}));
    assert_eq!(c.step(8.0, 4), Step::Stay, "no climb in safe mode");
    let mut c = l1_record();
    c.add(5.9, "panic", json!({})).add(5.9, "ladder", json!({"direction": "down", "from": 0, "to": 0, "trigger": {"kind": "panic"}}));
    assert_eq!(c.step(20.0, 4), Step::Stay, "no climb while panicked");
}

#[test]
fn fold_follows_ladder_records() {
    let mut c = Chain::new(1);
    c.add(0.0, "promotion", json!({})).add(0.0, "ladder", json!({"from": 1, "to": 0, "direction": "down", "trigger": {"kind": "panic", "seq": 2}}));
    let l = Level::fold(&c.0[..3]);
    assert_eq!((l.level, l.entered_seq, l.seq), (1, 1, Some(2)));
    let l = Level::fold(&c.0);
    assert_eq!((l.level, l.entered_seq), (0, 3));
    assert_eq!(l.json(4)["last_change"]["trigger"]["kind"], "panic");
    assert_eq!(l.json(4)["cap"], cap(4));
    assert_eq!(l.json(4)["path"], "gated");
}

/// §3.9: the third demotion at L0 within 24 h is a repeat (safe mode); demotions from L1 and older ones are not.
#[test]
fn repeated_demotion_at_l0() {
    let down = |c: &mut Chain, day: f64, from: i64| {
        c.add(day, "ladder", json!({"cause": "demotion", "direction": "down", "from": from, "to": (from - 1).max(0)}));
    };
    let at = |d: f64| T0 + (d * DAY as f64) as i64;
    let mut c = Chain::new(1);
    down(&mut c, 1.0, 1);
    down(&mut c, 1.1, 0);
    down(&mut c, 1.2, 0);
    assert!(!Level::fold(&c.0).repeated_l0(at(1.2)), "the L1→L0 demotion does not count");
    down(&mut c, 1.3, 0);
    assert!(Level::fold(&c.0).repeated_l0(at(1.3)), "three at L0 within 24 h");
    assert!(!Level::fold(&c.0).repeated_l0(at(2.1) + 1), "the first left the 24 h window");
}

/// F2 (demo 2026-10-25): an incident at L0 is recorded as `direction: "floor"` (no level is lost). It still counts
/// toward the repeated-demotion rule and still holds the climb to L1 back.
#[test]
fn an_incident_at_l0_is_floor_not_a_demotion() {
    assert_eq!((down_dir(0), down_dir(1), down_dir(3)), ("floor", "down", "down"));
    let floor = |c: &mut Chain, day: f64| {
        c.add(day, "ladder", json!({"cause": "demotion", "direction": "floor", "from": 0, "to": 0}));
    };
    let at = |d: f64| T0 + (d * DAY as f64) as i64;
    let mut c = Chain::new(0);
    floor(&mut c, 1.0);
    floor(&mut c, 1.1);
    floor(&mut c, 1.2);
    let l = Level::fold(&c.0);
    assert!(l.repeated_l0(at(1.2)), "three floor incidents within 24 h still enter safe mode");
    let held = l.criteria(at(1.3)).into_iter().find(|x| x["id"] == "demotions_last_7_days").map(|x| (x["have"].clone(), x["met"].clone()));
    assert_eq!(held, Some((json!(3), json!(false))), "the climb to L1 is still held back");
    assert_eq!(l.level, 0);
}

/// WP21 D10: the counters `state/root.json` carries, folded from the records.
#[test]
fn root_json_counters_fold_from_the_records() {
    let mut c = Chain::new(0);
    for (k, d) in [("zone_exit", json!({"class": "crash"})), ("zone_exit", json!({"class": "stop"})), ("zone_exit", json!({"class": "crash"})),
        ("rollback", json!({})), ("kernel_upgrade", json!({"stage": "installed"})), ("kernel_upgrade", json!({"stage": "installed"})),
        ("kernel_upgrade", json!({"stage": "probation_fail"})), ("kernel_upgrade", json!({"stage": "gog_fail"}))] {
        c.add(1.0, k, d);
    }
    assert_eq!(Level::fold(&c.0).counters.json(), json!({"crashes_total": 2, "rollbacks": 1, "swaps_failed": 2, "swaps_ok": 1}));
}

/// `ledger --verify`: a climb whose criteria held and a demotion answering its trigger verify; a forged climb (a
/// `ladder up` with no record behind it) and a demotion with no trigger are reported; test-fixture ladders are skipped.
#[test]
fn verify_rechecks_every_level_change() {
    let mut c = l1_record();
    c.add(7.0, "ladder", json!({"cause": "unlock", "direction": "up", "from": 0, "to": 1}));
    c.add(8.0, "ceiling_hit", json!({"demotes": true}));
    let s = c.0.len() as u64 - 1;
    c.add(8.0, "ladder", json!({"cause": "demotion", "direction": "down", "from": 1, "to": 0, "trigger": {"kind": "ceiling_hit", "seq": s}}));
    c.add(8.1, "panic", json!({})).add(8.1, "ladder", json!({"cause": "demotion", "direction": "down", "from": 0, "to": 0, "trigger": {"kind": "panic", "seq": 0}}));
    assert_eq!(verify(&c.0), Vec::<String>::new());
    let mut forged = Chain::new(0);
    forged.add(30.0, "ladder", json!({"cause": "unlock", "direction": "up", "from": 0, "to": 1}));
    forged.add(31.0, "ladder", json!({"cause": "demotion", "direction": "down", "from": 1, "to": 0, "trigger": {"kind": "audit", "seq": 1}}));
    let bad = verify(&forged.0);
    assert_eq!(bad.len(), 2, "{bad:?}");
    assert!(bad[0].contains("unmet criteria") && bad[1].contains("no pending trigger"), "{bad:?}");
}

/// X10 item 5: `corrections_trend` equals the independent Python vectors (tests/fixtures/x67/trend.py, fractions),
/// field by field: r0 in lowest terms, the magnitude guard, the exact tail in ppm, the verdict.
#[test]
fn trend_vectors() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/x67/trend.json");
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(cases.len() >= 15);
    for v in cases {
        let u = |k: &str, i: usize| v[k][i].as_u64().unwrap();
        let t = corrections_trend((u("ref", 0), u("ref", 1)), (u("win", 0), u("win", 1)));
        assert_eq!((t.ready, t.fire, t.r0, t.p_ppm, t.rate_permille, t.r0_permille),
            (v["ready"] == true, v["fire"] == true, (u("r0", 0), u("r0", 1)), v["p_ppm"].as_u64(), v["rate_permille"].as_u64().unwrap(),
                v["r0_permille"].as_u64().unwrap()), "{v}");
    }
}

/// The fold: the first 14 free-path days (≥ 40 owner turns) are the reference; the last 14 after it the window. A
/// rise from 10% to 30% fires: every climb is blocked, the alert is due once (then recorded), nothing demotes; when
/// corrections fall back the block lifts.
#[test]
fn the_corrections_trend_blocks_level_ups_and_never_demotes() {
    let day = |c: &mut Chain, d: i64, owner: u64, corrected: u64| {
        let start = rfc3339(T0 + d * DAY);
        c.add((d + 1) as f64, "turns", json!({"day": &start[..10], "owner": owner, "owner_corrected": corrected, "start": start}));
    };
    let mut c = Chain::free(0);
    for d in 0..14 {
        day(&mut c, d, 32, 3);
    }
    c.clean(14.5, 10);
    for d in 14..20 {
        day(&mut c, d, 32, 3);
    }
    assert!(up(&c.step(21.0, 4)), "{:?}", c.unmet(21.0));
    for d in 20..28 {
        day(&mut c, d, 32, 10);
    }
    let l = Level::fold(&c.0);
    assert!(l.trend.ready && l.trend.fire, "{:?}", l.trend);
    assert_eq!(c.unmet(29.0), ["corrections_trend"]);
    assert_eq!(c.step(29.0, 4), Step::Stay, "blocked, and no demotion");
    assert!(l.trend_alert_due().is_some());
    c.add(28.5, "harm", json!({"action": "alert", "signal": "corrections_trend"}));
    assert!(Level::fold(&c.0).trend_alert_due().is_none(), "alerted once per episode");
    for d in 28..42 {
        day(&mut c, d, 32, 3);
    }
    assert!(!Level::fold(&c.0).trend.fire);
    assert!(up(&c.step(43.0, 4)), "the block lifts: {:?}", c.unmet(43.0));
}

/// xorshift64*.
struct Rng(u64);
impl Rng {
    fn hits(&mut self, n: u64, permille: u64) -> u64 {
        (0..n).filter(|_| {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) % 1000 < permille
        }).count() as u64
    }
}

/// Null and power at 32 owner turns a day (X10's 4 an hour, 8 active hours): with corrections at 10% throughout, 20
/// seeds × 365 days give no false alert episode; a rise from 10% to 35% over 6 weeks (X10's corrections salami) fires
/// in every seed, by day 21 in the median.
#[test]
fn the_corrections_trend_null_rate_and_power() {
    let run = |seed: u64, days: u64, rate: &dyn Fn(u64) -> u64| -> Vec<u64> {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let reference = (14 * 32, rng.hits(14 * 32, 100));
        let mut win: VecDeque<(u64, u64)> = VecDeque::new();
        let (mut fired, mut on) = (Vec::new(), false);
        for d in 0..days {
            bounded(&mut win, (32, rng.hits(32, rate(d))), 14);
            let w = win.iter().fold((0, 0), |a, x| (a.0 + x.0, a.1 + x.1));
            let f = corrections_trend(reference, w).fire;
            if f && !on {
                fired.push(d);
            }
            on = f;
        }
        fired
    };
    let null: usize = (0..20).map(|s| run(s, 365, &|_| 100).len()).sum();
    assert_eq!(null, 0, "false alert episodes in 20 years");
    let mut days: Vec<u64> = (0..20).map(|s| run(100 + s, 42, &|d| 100 + 250 * (d + 1) / 42).first().copied().unwrap_or(99)).collect();
    days.sort_unstable();
    assert!(days.iter().all(|d| *d < 42), "fired within 6 weeks: {days:?}");
    assert!(days[10] <= 21, "median {} days: {days:?}", days[10]);
    println!("corrections trend: null 0 episodes in 20 years; 10%→35% over 42 days fired on days {days:?}");
}
