//! Tests of `twin` (lane X9, ADR-011 RF-31, RF-32, RF-33; the statistics, the window, sampling, the store, the
//! report and the pointer). Test-only, outside the Root's size budget. All $0: `init-mockllm` on loopback with a
//! fake `test-` key, fixture bodies, explicit times (the fake clock of the pure functions). Homes live under
//! `target/test-homes/x9/` and are removed by exact path; the only processes signalled are the arms these tests
//! start (by the sealed runner, by pid) and the mock (by its handle).

use super::*;
use std::process::{Child, Command, Stdio};

const NIGHT: &str = "2026-10-22";

fn t(s: &str) -> i64 {
    util::parse_rfc3339(s).unwrap()
}

/// A fresh short base (lane sockets must fit `sun_path`).
fn base(tag: &str) -> PathBuf {
    let tg = fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap();
    let b = tg.join(format!("test-homes/x9/{tag}{}", std::process::id() % 10_000));
    let _ = fs::remove_dir_all(&b); // exact path, created by this test
    fs::create_dir_all(&b).unwrap();
    b
}

fn w(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
}

fn out(outcome: &str, cost: u64, wall: u64) -> Out {
    Out { outcome: outcome.into(), cost_musd: cost, wall_ms: wall, requests: 1, ..Out::default() }
}

fn pair(night: &str, cur: Out, twin: Out) -> Pair {
    Pair { night: night.into(), task: "x".into(), model: "deepseek/deepseek-flash".into(), order: "cur_first".into(), cur, twin }
}

fn entry(id: &str, at: i64, caps: &[&str]) -> Entry {
    Entry { id: id.into(), at, snapshot: "s".into(), line: "l".into(), turn: None, caps: caps.iter().map(|c| c.to_string()).collect(), replayed: None }
}

/// The exact tests against hand-computed fractions: P(X ≥ 9 | 10, ½) = 11/1024; two-sided 22/1024 = 0.021484375 →
/// 21485 ppm (rounded up); 2·P(X ≥ 8) = 112/1024 = 0.109375; ties and n = 0 give 1; the one-sided bad tail.
#[test]
fn stats_are_exact_integer_fractions() {
    assert_eq!(upper_ppm(10, 9), 10_743); // 11/1024 = 0.0107421875 → up
    assert_eq!(two_sided_ppm(9, 1), 21_485);
    assert_eq!(two_sided_ppm(1, 9), 21_485);
    assert_eq!(two_sided_ppm(8, 2), 109_375);
    assert_eq!(two_sided_ppm(0, 0), 1_000_000);
    assert_eq!(two_sided_ppm(3, 3), 1_000_000);
    assert_eq!(two_sided_ppm(5, 0), 62_500); // 2/32
    assert_eq!(two_sided_ppm(6, 0), 31_250); // 2/64: significant
    assert_eq!(upper_ppm(0, 0), 1_000_000);
    // 84 discordant pairs (beyond u128's C(168, 84) range is irrelevant here, but the bignum path is exercised)
    assert!(two_sided_ppm(52, 32) <= SIG_PPM && two_sided_ppm(50, 34) > SIG_PPM);
    // stats over pairs: ended, cost and wall signs, medians of the ratios, crashes
    let ps = [pair(NIGHT, out("ended", 200, 10), out("ended", 100, 20)), pair(NIGHT, out("crashed", 300, 10), out("ended", 100, 20)),
        pair(NIGHT, out("ended", 100, 10), out("capped", 100, 20))];
    let refs: Vec<&Pair> = ps.iter().collect();
    let s = stats(&refs);
    assert_eq!(s["ended"], json!({"cur": 2, "discordant": 2, "p_ppm": 1_000_000, "twin": 2}));
    assert_eq!((s["cost"]["cur_higher"].as_u64(), s["cost"]["twin_higher"].as_u64(), s["cost"]["median_ratio_permille"].as_u64()), (Some(2), Some(0), Some(2000)));
    assert_eq!((s["wall"]["twin_higher"].as_u64(), s["wall"]["median_ratio_permille"].as_u64()), (Some(3), Some(500)));
    assert_eq!(s["crashes"], json!({"cur": 1, "twin": 0}));
    assert!(icj(&s).is_ok(), "integers only");
}

/// The tick's rate limit: the first call (the sentinel value `i64::MIN`) is due and never overflows in a debug
/// build, the next within the period is not, a later one is, and a clock that moved back is due again.
#[test]
fn the_first_tick_is_due_and_never_overflows() {
    let at = AtomicI64::new(i64::MIN);
    assert!(due(&at, 1_000, TICK_MS));
    assert!(!due(&at, 1_000 + TICK_MS - 1, TICK_MS));
    assert!(due(&at, 1_000 + TICK_MS, TICK_MS));
    assert!(due(&at, 0, TICK_MS), "the clock moved back");
    let at = AtomicI64::new(i64::MIN);
    assert!(due(&at, i64::MIN + 1, TICK_MS) && due(&AtomicI64::new(i64::MIN), i64::MAX, SLOW_MS));
}

/// The off-peak window and the week: a pair starts from 17:00 UTC, the window ends 00:30 UTC next day; the night of
/// 00:10 is the previous date's; weeks start Monday 00:00 UTC (2026-10-19 is a Monday).
#[test]
fn the_night_window_and_the_week() {
    // the charter key's default is today's constant
    assert_eq!(charter::twin_window(charter::TWIN_WINDOW_UTC), Some(WIN));
    assert_eq!(charter::twin_window(&charter::Twin::default().window_utc), Some(WIN));
    assert_eq!(win_text(WIN), "17:00-00:30");
    assert_eq!(night_of(t("2026-10-22T16:59:59Z"), WIN), None);
    assert_eq!(night_of(t("2026-10-22T17:00:00Z"), WIN), Some((NIGHT.into(), t("2026-10-23T00:30:00Z"))));
    assert_eq!(night_of(t("2026-10-23T00:10:00Z"), WIN), Some((NIGHT.into(), t("2026-10-23T00:30:00Z"))));
    assert_eq!(night_of(t("2026-10-23T00:30:00Z"), WIN), None);
    // an owner window that does not wrap, and one that wraps further
    let early = charter::twin_window("01:00-07:15").unwrap();
    assert_eq!((night_of(t("2026-10-23T00:59:59Z"), early), win_text(early)), (None, "01:00-07:15".into()));
    assert_eq!(night_of(t("2026-10-23T07:00:00Z"), early), Some(("2026-10-23".into(), t("2026-10-23T07:15:00Z"))));
    assert_eq!(night_of(t("2026-10-23T07:15:00Z"), early), None);
    let late = charter::twin_window("22:00-06:00").unwrap();
    assert_eq!(night_of(t("2026-10-23T05:59:00Z"), late), Some((NIGHT.into(), t("2026-10-23T06:00:00Z"))));
    assert_eq!(night_of(t("2026-10-22T21:59:00Z"), late), None);
    assert_eq!(week_start(t("2026-10-22T12:00:00Z")), t("2026-10-19T00:00:00Z"));
    assert_eq!(week_start(t("2026-10-19T00:00:00Z")), t("2026-10-19T00:00:00Z"));
    assert_eq!(week_start(t("2026-10-25T23:59:59Z")), t("2026-10-19T00:00:00Z"));
    let pin = t("2026-10-14T12:00:00Z");
    assert_eq!(report_due(&[], pin, t("2026-10-26T00:30:00Z")), None, "Sunday's night may still run");
    assert_eq!(report_due(&[], pin, t("2026-10-26T01:00:00Z")), Some(t("2026-10-19T00:00:00Z")));
    let done = json!({"kind": "twin_report", "data": {"week": "2026-10-19"}});
    assert_eq!(report_due(&[done], pin, t("2026-10-27T09:00:00Z")), None, "once per week");
    assert_eq!(report_due(&[], t("2026-10-26T00:10:00Z"), t("2026-10-26T02:00:00Z")), None, "no twin during that week");
}

/// Dollar configuration is metadata; replay records all costs and still obeys operational gates.
#[test]
fn rf32_the_night_observes_cost_without_a_dollar_stop() {
    let cfg = Cfg { free: true, enabled: true, k: 6, day_musd: 1, run_musd: 300_000, run_requests: 60, run_ms: 900_000, win: WIN };
    let tasks: Vec<Entry> = (0..6).map(|i| entry(&format!("e{i}"), 0, &[])).collect();
    let mut caps = Vec::new();
    let mut arm = |w: Which, _: &Entry, cap: u64| -> Result<Out, String> {
        caps.push((w, cap));
        Ok(out("ended", 90_000, 1))
    };
    let o = run_night(&cfg, NIGHT, "m", &tasks, 0, &mut || true, &mut |_| None, &mut arm);
    assert_eq!((o.stop, o.pairs.len(), o.spent), ("done", 6, 1_080_000));
    assert!(caps.iter().all(|(_, cap)| *cap == cfg.run_musd));
    let o2 = run_night(&cfg, NIGHT, "m", &tasks[..1], 295_000, &mut || true, &mut |_| None,
        &mut |_, _, _| Ok(out("ended", 5, 1)));
    assert_eq!((o2.stop, o2.pairs.len(), o2.spent), ("done", 1, 295_010));
    // a gate (the window) stops before a pair; a lane that refused everything stops the night and drops the pair
    let o3 = run_night(&cfg, NIGHT, "m", &tasks, 0, &mut || false, &mut |_| Some("window"), &mut |_, _, _| panic!("gated"));
    assert_eq!(o3.stop, "window");
    let o4 = run_night(&cfg, NIGHT, "m", &tasks, 0, &mut || false, &mut |_| None,
        &mut |_, _, _| Ok(Out { outcome: "capped".into(), refused_only: true, ..Out::default() }));
    assert_eq!((o4.stop, o4.pairs.len()), ("refused", 0));
    let o5 = run_night(&cfg, NIGHT, "m", &tasks[..1], 0, &mut || false, &mut |_| None, &mut |w, _, _| Ok(out(if w == Which::Cur { "ended" } else { "crashed" }, 5, 1)));
    assert_eq!((o5.stop, o5.pairs[0].order.as_str(), o5.pairs[0].twin.outcome.as_str()), ("done", "twin_first", "crashed"));

    let b = base("s");
    let u = b.join("egress/usage");
    fs::create_dir_all(&u).unwrap();
    let l = |v: Value| format!("{v}\n");
    let run = lane_run(NIGHT, "abc");
    assert_eq!(run, "twin-20261022-abc");
    fs::write(u.join("2026-10-22.jsonl"), [
        l(json!({"t": "start", "id": format!("{run}-0"), "lane": "sealed", "run": run, "reserved_musd": 900})),
        l(json!({"t": "sealed", "id": format!("{run}-0"), "lane": "sealed", "run": run, "cost_musd": 120, "status": 200})),
        l(json!({"t": "start", "id": "other-0", "lane": "sealed", "run": "other", "reserved_musd": 5000})),
        l(json!({"t": "sealed", "id": "twin-20261021-x-0", "lane": "sealed", "run": "twin-20261021-x", "cost_musd": 777})),
    ].concat()).unwrap();
    fs::write(u.join("2026-10-23.jsonl"), [
        l(json!({"t": "start", "id": format!("{run}-1"), "lane": "sealed", "run": run, "reserved_musd": 400})), // the Root died here
    ].concat()).unwrap();
    assert_eq!(night_spent(&b, NIGHT), 120 + 400);
    assert_eq!(provider_errors(&b, &run, &[NIGHT.into()]), 0);
    fs::remove_dir_all(&b).unwrap();
}

/// Sampling: unreplayed tasks of the last 7 days only, at most k, those whose turn used a favoured cap first. The
/// favoured caps: added or modified since the last report, in a group closed `flagged` within 14 days, near decay.
#[test]
fn sampling_favours_flagged_new_and_near_decay_caps() {
    let now = t("2026-10-22T18:00:00Z");
    let mut all = vec![entry("old", now - 8 * DAY, &["ext/a"]), entry("plain1", now - DAY, &[]), entry("plain2", now - DAY, &["tool/read"]),
        entry("fav", now - 2 * DAY, &["skill/new"]), entry("done", now - DAY, &["skill/new"])];
    all[4].replayed = Some("2026-10-21".into());
    let fav: BTreeSet<String> = ["skill/new".to_string()].into();
    let mut n = 0u64;
    let mut rnd = || {
        n += 1;
        n
    };
    let s = sample(&all, now, 2, &fav, &mut rnd);
    assert_eq!(s.iter().map(|e| e.id.as_str()).collect::<Vec<_>>()[0], "fav");
    assert_eq!(s.len(), 2);
    assert!(s.iter().all(|e| e.id != "old" && e.id != "done"));
    assert_eq!(sample(&all, now, 6, &fav, &mut rnd).len(), 3);
    let rec = |kind: &str, at: &str, data: Value| json!({"kind": kind, "at": at, "data": data});
    let recs = [rec("change_open", "2026-10-01T00:00:00Z", json!({"group": 1, "caps": [{"id": "skill/old", "op": "add"}]})),
        rec("change_open", "2026-10-02T00:00:00Z", json!({"group": 2, "caps": [{"id": "ext/flag", "op": "modify"}, {"id": "ext/gone", "op": "remove"}]})),
        rec("change_close", "2026-10-15T00:00:00Z", json!({"group": 2, "outcome": "flagged"})),
        rec("change_open", "2026-10-20T00:00:00Z", json!({"group": 3, "caps": [{"id": "skill/new", "op": "add"}]}))];
    let f = favoured_of(&recs, &["mem/cold".into()], t("2026-10-19T00:00:00Z"), now);
    assert_eq!(f.into_iter().collect::<Vec<_>>(), ["ext/flag", "mem/cold", "skill/new"]);
    assert!(favoured_of(&recs, &[], t("2026-10-19T00:00:00Z"), t("2026-11-10T00:00:00Z")).len() == 1, "a flag older than 14 days");
}

/// The signed-rank cost test (X10's proposal, approved): the exact null by subset sums, X10's textbook critical
/// values (n = 10 → 45, n = 20 → 150 at α = 0.05), W⁺ on exact log-ratio ranks, both one-sided tails, and, when X10's
/// `judge.json` carries a `twin` section, every critical value and decision digit for digit.
#[test]
fn the_signed_rank_cost_test_matches_x10() {
    assert_eq!((signed_rank_k(10, SIG), signed_rank_k(20, SIG)), (Some(45), Some(150)));
    assert_eq!(signed_rank_k(4, SIG), None, "n = 4: even W⁺ = 10 has p = 1/16");
    assert_eq!(signed_rank_k(5, SIG), Some(15));
    // (1,1) is rank 1 (log ratio 0), (2,1) rank 2, (1,3) rank 3, (5,1) rank 4: W⁺ = 2 + 4 = 6
    let v = [(2, 1), (1, 3), (5, 1), (1, 1)];
    assert_eq!(w_plus(&v), 6);
    assert_eq!((rank_upper_ppm(4, 6), rank_upper_ppm(4, 4)), (437_500, 687_500)); // 7/16, 11/16
    assert_eq!(w_plus(&[(0, 5), (3, 0)]), 1, "an amount of 0 counts as 1: (3,0) has ratio 3, rank 1; (0,5) ratio 5, rank 2");
    assert_eq!(rank_upper_ppm(120, 0), 1_000_000);
    let ps: Vec<Pair> = v.iter().map(|&(c, tw)| pair(NIGHT, out("ended", c, 1), out("ended", tw, 1))).collect();
    let c = cost_test(&ps.iter().collect::<Vec<_>>());
    assert_eq!((c["test"].as_str(), c["pairs"].as_u64(), c["w_plus"].as_u64(), c["p_ppm"].as_u64(), c["lower_p_ppm"].as_u64(), c["k"].clone()),
        (Some("signed_rank"), Some(4), Some(6), Some(437_500), Some(687_500), Value::Null));
    let judge = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../kernel/crates/kernel-gate/tests/fixtures/extend_sim/judge.json");
    let j: Value = fs::read_to_string(&judge).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let Some(tw) = j.get("twin") else {
        eprintln!("SKIP: X10's judge.json has no `twin` section yet ({})", judge.display());
        return;
    };
    let alpha = (tw["alpha"][0].as_u64().unwrap(), tw["alpha"][1].as_u64().unwrap());
    for c in tw["critical"].as_array().unwrap() {
        assert_eq!(signed_rank_k(c[0].as_u64().unwrap(), alpha), c[1].as_u64(), "critical value {c}");
    }
    for (i, c) in tw["cases"].as_array().unwrap().iter().enumerate() {
        let v: Vec<(u64, u64)> = c["pairs"].as_array().unwrap().iter().map(|p| (p[0].as_u64().unwrap(), p[1].as_u64().unwrap())).collect();
        let (n, w) = (v.len() as u64, w_plus(&v));
        assert_eq!((w, signed_rank_k(n, alpha)), (c["w_plus"].as_u64().unwrap(), c["k"].as_u64()), "case {i}");
        assert_eq!(signed_rank_k(n, alpha).is_some_and(|k| w >= k), c["significant"].as_bool().unwrap(), "case {i}");
        assert_eq!(rank_upper_ppm(n, w) <= SIG_PPM, c["significant"].as_bool().unwrap(), "case {i}: the ppm form agrees");
    }
}

/// The weekly report: the week's pairs on the newest model; the cost test and the verdict over the last 14 nights
/// (fewer than 40 pairs is `insufficient`, which never sets `regression`; 42 good pairs is `no_regression`; the
/// current arm ending significantly fewer is a `regression`), the stop rule's test, and the words.
#[test]
fn the_report_its_regression_flag_and_the_stop_rule() {
    let w = t("2026-10-19T00:00:00Z");
    let good = |night: &str| pair(night, out("ended", 105, 10), out("ended", 100, 10));
    let mut ps: Vec<Pair> = (0..3).map(|_| good("2026-10-20")).collect();
    let r = report_of(&ps, w, 1234);
    assert_eq!((r["week"].as_str(), r["pairs"].as_u64(), r["spent_musd"].as_u64(), r["regression"].as_bool()), (Some("2026-10-19"), Some(3), Some(1234), Some(false)));
    assert_eq!((r["verdict"].as_str(), r["cost"]["test"].as_str(), r["cost"]["pairs"].as_u64()), (Some("insufficient"), Some("signed_rank"), Some(3)));
    assert!(reading(&r)["verdict"].as_str().unwrap().starts_with("insufficient: 3 pairs in 14 nights"));
    assert_eq!(r["last14"]["pairs"], 3);
    for k in ["week", "pairs", "ended", "cost", "wall", "crashes", "spent_musd"] {
        assert!(!r[k].is_null(), "ADR-011 field {k}");
    }
    assert!(icj(&r).is_ok());
    ps = (0..21).map(|_| good("2026-10-13")).chain((0..21).map(|_| good("2026-10-24"))).collect();
    ps.push(pair("2026-10-10", out("crashed", 1, 1), out("ended", 1, 1))); // 15 nights back: outside the 14
    let r = report_of(&ps, w, 0);
    assert_eq!((r["pairs"].as_u64(), r["last14"]["pairs"].as_u64(), r["regression"].as_bool(), r["verdict"].as_str()), (Some(21), Some(42), Some(false), Some("no_regression")));
    assert_eq!((r["cost"]["median_ratio_permille"].as_u64(), r["cost"]["pairs"].as_u64(), r["cost"]["w_plus"].as_u64()), (Some(1050), Some(42), Some(903)));
    // 8 tasks the current arm no longer ends: one-sided P(X ≥ 8 | 8) = 1/256 ≤ 0.05 → regression
    for p in ps.iter_mut().take(8) {
        p.cur.outcome = "capped".into();
    }
    let r = report_of(&ps, w, 0);
    assert_eq!((r["regression"].as_bool(), r["verdict"].as_str(), r["last14"]["ended_worse_p_ppm"].as_u64()), (Some(true), Some("regression"), Some(3907)));
    assert!(twin_better(&r) && r["cost"]["p_ppm"].as_u64() == Some(1), "5% costlier in all 42 pairs: P(W⁺ ≥ 903) = 2⁻⁴² ({r})");
    // costlier current arm in all 21 week pairs: the twin is better on cost
    let costly: Vec<Pair> = (0..21).map(|_| pair("2026-10-21", out("ended", 300, 10), out("ended", 100, 10))).collect();
    let r = report_of(&costly, w, 0);
    assert!(twin_better(&r));
    assert!(r["last14"]["cost_median_ratio_permille"].as_u64() == Some(3000) && r["regression"] == false && r["verdict"] == "insufficient");
    assert!(reading(&r)["cost"].as_str().unwrap().starts_with("over the last 14 nights the current system costs more per task than the twin (signed-rank p = 0.000001"),
        "{}", reading(&r));
    assert_eq!(reading(&r)["ended"], "not distinguishable at n = 21");
    let cheap: Vec<Pair> = (0..6).map(|_| pair("2026-10-21", out("ended", 50, 10), out("ended", 100, 10))).collect();
    let r = report_of(&cheap, w, 0);
    assert!(!twin_better(&r) && reading(&r)["cost"].as_str().unwrap().contains("costs less"), "{}", reading(&r));
    // a model change: only the newest model's pairs are reported
    let mut mixed = costly.clone();
    mixed[20].model = "deepseek/other".into();
    assert_eq!((report_of(&mixed, w, 0)["pairs"].as_u64(), report_of(&mixed, w, 0)["model"].as_str()), (Some(1), Some("deepseek/other")));
}

/// The store: a capture clones the session's workspace snapshot (a link is refused, nothing left), `tag_turn` adds
/// the turn's caps, and retention removes tasks older than 7 days by exact path. The diff of an untouched clone is 0.
#[cfg(target_os = "macos")]
#[test]
fn the_store_captures_tags_and_forgets() {
    let b = base("t");
    let (store, snap) = (b.join("rh/extend/replay"), b.join("rh/snapshots/s1"));
    w(&snap.join("seed.txt"), "seed");
    w(&snap.join("src/a.rs"), "fn a() {}");
    let now = t("2026-10-22T10:00:00Z");
    let e = capture_at(&store, &snap, "s1", "fix the parser", now).unwrap();
    assert_eq!(fs::read_to_string(store.join(&e.id).join("ws/src/a.rs")).unwrap(), "fn a() {}");
    assert_eq!(diff(&snap, &store.join(&e.id).join("ws")), (0, 0), "a clone keeps sizes and mtimes");
    assert_eq!(fs::metadata(store.join(&e.id)).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(fs::metadata(store.join(&e.id).join("entry.json")).unwrap().permissions().mode() & 0o777, 0o600);
    std::os::unix::fs::symlink(&snap, b.join("rh/snapshots/link")).unwrap();
    assert!(capture_at(&store, &b.join("rh/snapshots/link"), "link", "x", now).is_err(), "a link is never followed");
    assert_eq!(entries(&store).len(), 1, "nothing is left of a refused capture");
    tag_turn(&store, 7, now + 500, now + 60_000, vec!["skill/a".into()]);
    tag_turn(&store, 8, now + 500, now + 60_000, vec!["skill/b".into()]);
    let got = entries(&store);
    assert_eq!((got[0].turn, got[0].caps.clone()), (Some(7), vec!["skill/a".to_string()]), "the first closed turn only");
    w(&store.join(&e.id).join("ws/new.txt"), "12345");
    assert_eq!(diff(&snap, &store.join(&e.id).join("ws")), (1, 5));
    prune_store(&store, now + 6 * DAY);
    assert_eq!(entries(&store).len(), 1);
    prune_store(&store, now + 7 * DAY);
    assert!(entries(&store).is_empty() && !store.join(&e.id).exists(), "7 days, then removed by exact path");
    assert!(snap.join("seed.txt").exists(), "the access snapshot itself is never touched");
    fs::remove_dir_all(&b).unwrap();
}

fn mock_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("INIT_MOCKLLM_BIN") {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    let k = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../kernel/target");
    let mut c = vec![k.join("debug/init-mockllm")];
    c.extend(fs::read_dir(&k).into_iter().flatten().flatten().map(|e| e.path().join("debug/init-mockllm")));
    c.into_iter().filter(|p| p.is_file()).max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
}

struct Mock(Child);
impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.0.kill(); // our own child, by its handle
        let _ = self.0.wait();
    }
}

/// The fixture body: one model turn per request on the sealed lane (as s1fx's), its arm read from its own state
/// (`state/mem/arm`: the twin's frozen state says `twin`, the current snapshot `cur`), checks of what a replay arm
/// gets, and per-task behaviour from the original line.
/// The `[task x9]` marker routes its requests to the mock's give-up responder.
const BODY: &str = r#"#!/usr/bin/perl
use strict; use warnings; use JSON::PP; use IO::Socket::UNIX;
my $J = JSON::PP->new->canonical->utf8;
my $frame = eval { $J->decode(scalar(<STDIN>) // '{}') } || {};
my $text = $frame->{text} // '';
my ($home, $ws) = ($ENV{INIT_HOME} // '', $ENV{INIT_WORKSPACE} // '');
exit 8 unless -f "$home/body/GENESIS.md" && -f "$home/kernel/config.toml";
exit 9 unless -f "$ws/seed.txt" && ($ENV{INIT_ACCESS} // '') eq 'workspace';
exit 10 if -e "$home/state/sessions/old.jsonl" || -e "$home/state/run/ready";
exit 14 unless $text =~ /^task /;
open(my $m, '<', "$home/state/mem/arm") or exit 11; my $arm = <$m>; close $m; chomp $arm;
for (1 .. ($arm eq 'cur' ? 2 : 1)) { chat($text) or exit 12 }
open(my $o, '>', "$ws/out.txt") or exit 13; print $o "$arm\n"; close $o;
exit 3 if $arm eq 'cur' && $text =~ /\[crash\]/;
if ($arm eq 'cur' && $text =~ /\[slow\]/) { for (1 .. 60) { chat($text); sleep 1 } } # still working at the wall
exit 0;
sub chat {
    my ($t) = @_;
    my $body = $J->encode({model => 'deepseek-flash', stream => JSON::PP::true, max_tokens => 256, messages => [{role => 'user', content => "[task x9]\n$t"}]});
    my $s = IO::Socket::UNIX->new(Peer => $ENV{INIT_PROXY_SOCK} // '', Type => SOCK_STREAM) or return 0;
    print $s "POST /deepseek/chat/completions HTTP/1.1\r\nhost: proxy\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: "
        . length($body) . "\r\n\r\n" . $body;
    my $resp = do { local $/; <$s> } // '';
    close $s;
    return $resp =~ m{^HTTP/1\.[01] 200 };
}
"#;

/// RF-31 and RF-33: a mock replay of four stored tasks, both arms each, through `run_night` with the real arm runner:
/// sealed run roots, sealed lanes to `init-mockllm`, each arm on its own tuple and state, the task's workspace clone
/// and the original line. The current body costs two requests, crashes on `[crash]` and runs past the wall on
/// `[slow]` (one request a second, so it is still working at the wall: capped and killed); the twin ends every task. The report states n and p. RF-33: every run root and lane
/// socket is removed by exact path (also for an arm that fails before it spawns), and the ledger's night spend equals
/// what the lanes metered.
#[cfg(target_os = "macos")]
#[test]
fn rf31_rf33_mock_replay_both_arms_and_cleanup() {
    let Some(mock) = mock_bin() else {
        eprintln!("SKIP: init-mockllm is not built (set INIT_MOCKLLM_BIN)");
        return;
    };
    let b = base("r");
    let (rh, ih) = (b.join("rh"), b.join("ih"));
    for d in ["sealed/run", "store/blobs", "run", "keys/providers"] {
        fs::create_dir_all(rh.join(d)).unwrap();
    }
    for d in ["", "keys", "keys/providers"] {
        fs::set_permissions(rh.join(d), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let pf = b.join("mock.port");
    fs::create_dir_all(b.join("tasks")).unwrap();
    let _m = Mock(Command::new(mock).args(["--mode", "upstream", "--listen", "127.0.0.1:0", "--record-bodies", "none", "--port-file"]).arg(&pf)
        .arg("--tasks-dir").arg(b.join("tasks")).arg("--record-dir").arg(b.join("mockrec")).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    let t0 = Instant::now();
    while !fs::read_to_string(&pf).is_ok_and(|s| s.trim().parse::<u16>().is_ok()) {
        assert!(t0.elapsed() < Duration::from_secs(10), "mock port");
        std::thread::sleep(Duration::from_millis(50));
    }
    let port: u16 = fs::read_to_string(&pf).unwrap().trim().parse().unwrap();
    let key = rh.join("keys/providers/deepseek");
    fs::write(&key, "test-key-x9-5e5e5e").unwrap();
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    let text = format!("schema_version = 1\n[purpose]\ntext = \"X9 test.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n\
        [root]\ntest_mode = true\n[money]\nday_usd = \"5.00\"\ntotal_usd = \"10.00\"\n[actions.providers]\ndestinations = [\"127.0.0.1:{port}\"]\n\
        paths = [\"chat/completions\"]\n[actions.providers.models.\"deepseek-flash\"]\nmin_level = 0\ninput_musd_per_mtok = 300000\n\
        output_musd_per_mtok = 1200000\n[actions.providers.providers.deepseek]\nhost = \"127.0.0.1:{port}\"\n\
        [sealed]\nreserve_usd = \"0.00\"\naudit_usd = \"0.00\"\n[extend]\nmode = \"free\"\n");
    let cx = crate::charter::Ctx { root_home: &rh, init_home: &ih, home: &b };
    let mut ch = crate::charter::parse(&text, &cx).map(|x| x.0).unwrap_or_else(|e| panic!("{e:?}"));
    ch.sandbox.disk_floor_gib = 0; // the floor is off in this test (the machine may sit near it)
    let env = Env { rh: rh.clone(), ih: ih.clone(), home: b.clone(), info: json!({}), ch, level: 0, t0_sha: None };
    // the live INIT_HOME: the current model and config (both arms), and the two tuples' exported T0 trees
    w(&ih.join("state/kernel/boot.json"), r#"{"v":1,"model":{"provider":"deepseek","model":"deepseek-flash"},"env":{"kernel_version":"9.9.9"}}"#);
    w(&ih.join("kernel/config.toml"), "# live config\n");
    let (tsha, csha) = ("a".repeat(40), "c".repeat(40));
    for sha in [&tsha, &csha] {
        for f in ["GENESIS.md", "SELF.md"] {
            w(&ih.join("lineage/trees").join(&sha[..12]).join(f), "t0");
        }
    }
    let put = |s: &str| crate::tuple::put(&rh, &mut s.as_bytes(), None, 1 << 30).unwrap();
    let tup = |bin: &str, sha: &str| json!({"body": {"binary": bin, "sha": sha, "t0": null}, "ext": {"components": [], "set": null}, "kernel": {}, "v": 1});
    // the agent-state snapshots (S_A layout): the twin's frozen state and the current one; sessions never travel
    let (ts, cs) = (b.join("snap/tw"), b.join("snap/cu"));
    w(&ts.join("state/mem/arm"), "twin");
    w(&cs.join("state/mem/arm"), "cur");
    for s in [&ts, &cs] {
        w(&s.join("state/sessions/old.jsonl"), "{}\n");
        w(&s.join("state/run/ready"), "{}");
    }
    let twin = Src { tuple: tup(&put(BODY), &tsha), state: Some(ts) };
    let cur = Src { tuple: tup(&put(&format!("{BODY}# cur\n")), &csha), state: Some(cs) };
    // four stored tasks, each with a workspace snapshot
    let store = store_dir(&rh);
    let wsrc = b.join("wsnap");
    w(&wsrc.join("seed.txt"), "seed");
    let lines = ["task one", "task two", "task three [crash]", "task four [slow]"];
    let tasks: Vec<Entry> = lines.iter().map(|l| capture_at(&store, &wsrc, "s1", l, 0).unwrap()).collect();
    let cfg = Cfg { free: true, enabled: true, k: 6, day_musd: 300_000, run_musd: 300_000, run_requests: 60, run_ms: 8_000, win: WIN };
    let mut flip = false;
    let mut coin = || {
        flip = !flip;
        flip
    };
    let night = date(now_ms()); // the lanes' usage lines are dated by the real clock
    let mut arm = |w: Which, e: &Entry, cap: u64| run_arm(&env, &night, if w == Which::Cur { &cur } else { &twin }, e, &store, cap, &cfg);
    let o = run_night(&cfg, &night, "deepseek/deepseek-flash", &tasks, 0, &mut coin, &mut |_| None, &mut arm);
    assert_eq!((o.stop, o.pairs.len()), ("done", 4), "{:?} {:?}", o.error, o.pairs);
    let by = |l: &str| o.pairs.iter().find(|p| p.task == tasks[lines.iter().position(|x| *x == l).unwrap()].id).unwrap();
    for l in ["task one", "task two"] {
        let p = by(l);
        assert_eq!((p.cur.outcome.as_str(), p.cur.requests, p.twin.outcome.as_str(), p.twin.requests), ("ended", 2, "ended", 1), "{p:?}");
        assert!(p.cur.cost_musd > p.twin.cost_musd && p.twin.cost_musd > 0, "{p:?}");
        assert_eq!((p.cur.diff_files, p.twin.diff_files, p.cur.diff_bytes, p.twin.diff_bytes), (1, 1, 4, 5), "out.txt: `cur\\n`, `twin\\n`");
    }
    assert_eq!((by("task three [crash]").cur.outcome.as_str(), by("task three [crash]").twin.outcome.as_str()), ("crashed", "ended"));
    let slow = by("task four [slow]");
    assert_eq!((slow.cur.outcome.as_str(), slow.twin.outcome.as_str()), ("capped", "ended"), "{slow:?}");
    assert!(slow.cur.wall_ms >= 8_000 && slow.cur.wall_ms < 30_000, "killed at the wall: {slow:?}");
    assert_eq!(o.pairs.iter().map(|p| p.order.as_str()).collect::<Vec<_>>(), ["cur_first", "twin_first", "cur_first", "twin_first"]);
    let refs: Vec<&Pair> = o.pairs.iter().collect();
    let s = stats(&refs);
    assert_eq!(s["ended"], json!({"cur": 2, "discordant": 2, "p_ppm": 500_000, "twin": 4}), "{s}");
    assert_eq!((s["cost"]["cur_higher"].as_u64(), s["cost"]["p_ppm"].as_u64()), (Some(4), Some(125_000)), "{s}");
    assert_eq!(s["crashes"], json!({"cur": 1, "twin": 0}));
    let rep = report_of(&o.pairs, week_start(t(&format!("{night}T18:00:00Z"))), o.spent);
    assert_eq!((rep["pairs"].as_u64(), rep["ended"]["p_ppm"].as_u64(), rep["regression"].as_bool(), rep["verdict"].as_str()),
        (Some(4), Some(500_000), Some(false), Some("insufficient")));
    assert_eq!((rep["cost"]["w_plus"].as_u64(), rep["cost"]["p_ppm"].as_u64()), (Some(10), Some(62_500)), "W⁺ = 1+2+3+4, P = 1/16");
    assert_eq!(reading(&rep)["ended"], "not distinguishable at n = 4");
    eprintln!("X9 RF-31 report: {}", icj(&rep).unwrap());
    // the Root's money: the ledger's night spend is what the lanes metered (a restart reads this back)
    let metered: u64 = o.pairs.iter().map(|p| p.cur.cost_musd + p.twin.cost_musd).sum();
    assert_eq!((night_spent(&rh, &night), o.spent), (metered, metered));
    // X9 deviation 3 (INTEG-A): the twin pool. The sealed reserve is $0 in this charter, so a lane charged to it would
    // have refused every request; every usage line is the twin pool's, tagged with its night.
    let usage: Vec<Value> = fs::read_dir(rh.join("egress/usage")).unwrap().flatten()
        .flat_map(|e| fs::read_to_string(e.path()).unwrap().lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).collect::<Vec<_>>()).collect();
    assert!(!usage.is_empty() && usage.iter().all(|v| v["pool"] == "twin" && v["night"] == night.as_str() && v["lane"] == "sealed"), "{usage:?}");
    // Lowering legacy dollar metadata does not stop the next pair or erase its observed cost.
    let mut ch = env.ch.clone();
    ch.extend.twin.usd_day = format!("{}.{:06}", metered / 1_000_000, metered % 1_000_000);
    let low = Env { rh: rh.clone(), ih: ih.clone(), home: b.clone(), info: json!({}), ch, level: 0, t0_sha: None };
    let mut arm2 = |w: Which, e: &Entry, cap: u64| run_arm(&low, &night, if w == Which::Cur { &cur } else { &twin }, e, &store, cap, &cfg);
    let o2 = run_night(&cfg, &night, "deepseek/deepseek-flash", &tasks[..1], 0, &mut || true, &mut |_| None, &mut arm2);
    assert_eq!((o2.stop, o2.pairs.len()), ("done", 1), "{:?}", o2.error);
    assert_eq!(night_spent(&rh, &night), metered + o2.spent);
    // RF-33: an arm that fails before its body spawns (a binary not in the store) also leaves nothing
    let bad = Src { tuple: tup(&"0".repeat(64), &tsha), state: None };
    assert!(run_arm(&env, &night, &bad, &tasks[0], &store, 100_000, &cfg).unwrap_err().contains("not in the store"));
    assert_eq!(fs::read_dir(rh.join("sealed/run")).unwrap().count(), 0, "every run root is removed by exact path");
    let runs: Value = serde_json::from_str(&fs::read_to_string(rh.join("sealed/runs.json")).unwrap()).unwrap();
    assert_eq!(runs["runs"], json!([]), "and unlisted");
    assert!(fs::read_dir(rh.join("run")).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with("sl-")), "lane sockets removed");
    assert!(store.join(&tasks[0].id).join("ws/seed.txt").is_file() && !store.join(&tasks[0].id).join("ws/out.txt").exists(),
        "the stored workspace is never written by an arm");
    drop(_m);
    fs::remove_dir_all(&b).unwrap();
}

/// The pointer and the owner command on an installed test home (in process, no daemon, no kernel, no sealed lock):
/// the free path freezes the `lkg` tuple with a system snapshot (`twin` record; `Ptrs.twin`; retention pins the
/// snapshot by id); `/twin refreeze` needs the typed confirmation and only then re-freezes; a session's first line is
/// stored (and only the first); the report and the stop rule land in the lineage and the alerts; `twin.json` holds
/// aggregates only.
#[cfg(target_os = "macos")]
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn the_pointer_refreeze_capture_and_report_on_a_root() {
    let tg = fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap();
    let dir = tg.join(format!("test-homes/x9r-{}", std::process::id())); // ROOT_HOME ≤ 80 bytes in test builds
    let _ = fs::remove_dir_all(&dir); // exact path, created by this test
    let p = crate::install::Paths { rh: dir.join("rh"), ih: dir.join("ih"), home: dir.clone() };
    fs::create_dir_all(&p.rh).unwrap();
    w(&p.rh.join("charter.toml"), "schema_version = 1\n[purpose]\ntext = \"X9 test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\n\
        disk_min_free_gib = 0\n[sandbox]\ndisk_floor_gib = 1\n[root]\ntest_mode = true\n[extend]\nmode = \"free\"\n");
    let o = crate::install::Opts { release: None, charter: None, git: "/usr/bin/git".into(), with_testkit: false, toolchain: None,
        seed_source: None, seed_tag: None, no_service: true, bin_dir: None };
    crate::install::install(&p, &o).unwrap_or_else(|e| panic!("install: {e:?}"));
    let root = crate::link::Root::boot(p.clone()).unwrap();
    let rh = p.rh.clone();
    assert!(freeze(&root, "enable").unwrap_err().contains("no lkg"), "nothing to freeze before an lkg tuple");
    w(&p.ih.join("state/mem/data"), "frozen");
    root.append("lkg", json!({"tuple": "tAAAAAAAAAAAA"}));
    let pin = freeze(&root, "enable").unwrap();
    let snap = pin.snapshot.clone().expect("a system snapshot");
    assert_eq!((pin.tuple.as_str(), lock(&root.sup.ptrs).twin.as_deref()), ("tAAAAAAAAAAAA", Some("tAAAAAAAAAAAA")));
    assert_eq!(fs::read_to_string(crate::snapshot::dir(&rh).join(&snap).join("state/mem/data")).unwrap(), "frozen");
    let recs = crate::lineage::records(&rh).unwrap();
    assert_eq!(pin_of(&recs).map(|p| (p.tuple, p.snapshot, p.reason)), Some(("tAAAAAAAAAAAA".into(), Some(snap.clone()), "enable".into())));
    let metas = crate::snapshot::metas(&rh);
    assert!(crate::snapshot::keep_set(&metas, &recs, i64::MAX / 2).contains(&snap), "the twin's snapshot is pinned whatever its age");

    root.append("lkg", json!({"tuple": "tBBBBBBBBBBBB"}));
    assert!(owner_text(&root, "/twin refreeze"));
    assert_eq!(lock(&root.sup.ptrs).twin.as_deref(), Some("tAAAAAAAAAAAA"), "no re-freeze without the typed confirmation");
    assert!(owner_text(&root, "/twin refreeze confirm"));
    let recs = crate::lineage::records(&rh).unwrap();
    let p2 = pin_of(&recs).unwrap();
    assert_eq!((p2.tuple.as_str(), p2.reason.as_str(), lock(&root.sup.ptrs).twin.as_deref()), ("tBBBBBBBBBBBB", "owner_refreeze", Some("tBBBBBBBBBBBB")));
    assert!(owner_text(&root, "/twin") && !owner_text(&root, "/rollback"));
    assert_eq!(recs.iter().filter(|r| r["kind"] == "twin").count(), 2);

    // a session's first line enters the store (its workspace snapshot cloned); the second line and slash lines do not
    w(&rh.join("snapshots/20261022T100000.000Z-proj/seed.txt"), "seed");
    session_started(&root, Some(&json!({"id": "20261022T100000.000Z-proj"})));
    relayed(&root, "q1", &json!({"t": "line", "text": "  refactor the parser  "}));
    relayed(&root, "q2", &json!({"t": "line", "text": "and the lexer"}));
    let store = store_dir(&rh);
    let t1 = Instant::now();
    while entries(&store).is_empty() && t1.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let got = entries(&store);
    assert_eq!(got.iter().map(|e| (e.line.as_str(), e.snapshot.as_str())).collect::<Vec<_>>(), [("refactor the parser", "20261022T100000.000Z-proj")]);
    session_started(&root, Some(&json!({"id": "x"})));
    relayed(&root, "q3", &json!({"t": "line", "text": "/help"}));
    session_started(&root, None);
    relayed(&root, "q4", &json!({"t": "line", "text": "no snapshot, not replayable"}));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(entries(&store).len(), 1);

    // three weekly reports in which the twin is significantly better → the stop rule's alert
    let w0 = t("2026-09-28T00:00:00Z");
    let costly = |night: String| (0..8).map(move |_| pair(&night, out("ended", 300, 10), out("ended", 100, 10)));
    add_pairs(&rh, &(0..3).flat_map(|i| costly(date(w0 + i * 7 * DAY + DAY))).collect::<Vec<_>>(), w0 + 21 * DAY);
    for i in 0..3 {
        report(&root, w0 + i * 7 * DAY);
    }
    let reps: Vec<Value> = crate::lineage::records(&rh).unwrap().into_iter().filter(|r| r["kind"] == "twin_report").map(|r| r["data"].clone()).collect();
    assert_eq!(reps.iter().map(|d| d["week"].as_str().unwrap()).collect::<Vec<_>>(), ["2026-09-28", "2026-10-05", "2026-10-12"]);
    assert!(reps.iter().all(|d| d["pairs"] == 8 && d["verdict"] == "insufficient" && d["regression"] == false), "{reps:?}");
    assert_eq!(reps.iter().map(|d| (d["cost"]["pairs"].as_u64().unwrap(), d["cost"]["p_ppm"].as_u64().unwrap())).collect::<Vec<_>>(),
        [(8, 3_907), (16, 16), (16, 16)], "the 14-night signed-rank span: 1/256, then 2⁻¹⁶");
    assert!(fs::read_to_string(rh.join("alerts.jsonl")).unwrap().contains("\"code\":\"twin_worse\""));
    let tj = fs::read_to_string(p.ih.join("root/twin.json")).unwrap();
    assert!(tj.contains("\"tuple\":\"tBBBBBBBBBBBB\"") && tj.contains("\"replay_tasks\":1") && !tj.contains("parser"), "{tj}");
    assert!(tj.contains("\"last14\":{\"insufficient\":true"), "{tj}");
    drop(root);
    fs::remove_dir_all(&dir).unwrap();
}

/// v0.5.1: an owner upgrade's commit moves the twin onto the release's kernel and keeps its freeze (tuple::reanchor),
/// so the pin reads `frozen_at`: its replays, weekly reports and 90-day re-freeze clock are unchanged.
#[test]
fn an_upgrade_keeps_the_twin_freeze() {
    let t = |ms: i64| crate::util::rfc3339(ms);
    let recs = vec![json!({"kind": "twin", "at": t(1_000), "data": {"reason": "enable", "snapshot": "s1", "tuple": "tA"}}),
        json!({"kind": "twin", "at": t(9_000), "data": {"from": "tA", "frozen_at": t(1_000), "reason": "enable", "snapshot": "s1", "tuple": "tB", "upgrade": "u-1"}})];
    assert_eq!(pin_of(&recs), Some(Pin { tuple: "tB".into(), snapshot: Some("s1".into()), at: 1_000, reason: "enable".into() }));
}
