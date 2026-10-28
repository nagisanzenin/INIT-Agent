//! X3 unit tests (ADR-011 Work breakdown): RF-10 (the tail tables equal the Python vectors), RF-11 (synthetic null
//! streams: false rollbacks ≤ 2.5% per window) and RF-12 (each H signal fires on a scripted stream). The engine is
//! pure, so these run on explicit Root times with no process and no file. RF-13 and RF-14 run a Root daemon and the
//! fake kernel (tests/x3.rs).

use super::Turn;
use super::engine::{self, Act, Base, Ctx, Engine, Open, Pick};
use super::stats::{self, HARD, SOFT};
use serde_json::{Value, json};

// ------------------------------------------------------------------------------------------------ RF-10

fn vectors() -> Value {
    serde_json::from_str(include_str!("../../tests/fixtures/sentinel/tails.json")).unwrap()
}

fn u(v: &Value) -> u64 {
    v.as_u64().unwrap()
}

/// RF-10: every exceedance, binomial and Fisher tail equals Python's, digit for digit, and so do the thresholds.
#[test]
fn rf10_tails_equal_the_python_vectors() {
    let v = vectors();
    for e in v["exceed"].as_array().unwrap() {
        let t = stats::exceed_tail(u(&e["m"]), u(&e["j"]), u(&e["n"]), u(&e["k"]));
        assert_eq!((t.num.decimal(), t.den.decimal()), (e["num"].as_str().unwrap().to_string(), e["den"].as_str().unwrap().to_string()), "{e}");
    }
    for e in v["exceed_k"].as_array().unwrap() {
        let n = u(&e["n"]);
        assert_eq!(stats::exceed_k(40, 10, n, HARD), e["hard"].as_u64(), "{e}");
        assert_eq!(stats::exceed_k(40, 10, n, SOFT), e["soft"].as_u64(), "{e}");
    }
    for e in v["binom"].as_array().unwrap() {
        let t = stats::binom_tail(u(&e["n"]), u(&e["k"]), u(&e["a"]), u(&e["d"]));
        assert_eq!((t.num.decimal(), t.den.decimal()), (e["num"].as_str().unwrap().to_string(), e["den"].as_str().unwrap().to_string()), "{e}");
    }
    for e in v["binom_k"].as_array().unwrap() {
        let (n, a, d) = (u(&e["n"]), u(&e["a"]), u(&e["d"]));
        assert_eq!(stats::binom_k(n, a, d, HARD), e["hard"].as_u64(), "{e}");
        assert_eq!(stats::binom_k(n, a, d, SOFT), e["soft"].as_u64(), "{e}");
    }
    for e in v["h9"].as_array().unwrap() {
        let t = stats::binom_tail(u(&e["n"]), u(&e["k"]), u(&e["a"]), u(&e["d"]));
        assert_eq!(t.num.decimal(), e["num"].as_str().unwrap(), "{e}");
        assert_eq!(t.den.decimal(), e["den"].as_str().unwrap(), "{e}");
        assert_eq!(t.le(HARD), e["hard"].as_bool().unwrap(), "{e}");
    }
    for e in v["fisher"].as_array().unwrap() {
        let t = stats::fisher_tail(u(&e["N"]), u(&e["K"]), u(&e["n"]), u(&e["a"]));
        assert_eq!((t.num.decimal(), t.den.decimal()), (e["num"].as_str().unwrap().to_string(), e["den"].as_str().unwrap().to_string()), "{e}");
    }
    // the ADR's own tables (ADR-011 §3.3 H5b and H7, from ADR-010 §3)
    let k: Vec<Option<u64>> = [5, 10, 20].iter().map(|&n| engine::k_exceed(n, HARD)).collect();
    assert_eq!(k, [Some(5), Some(8), Some(13)], "H5b: ≥ 5 of 5, ≥ 8 of 10, ≥ 13 of 20");
    let k: Vec<Option<u64>> = [5, 10, 20].iter().map(|&n| engine::k_binom(n, (1, 10), HARD)).collect();
    assert_eq!(k, [Some(4), Some(5), Some(8)], "H7 at r0 = 100‰: ≥ 4 of 5, ≥ 5 of 10, ≥ 8 of 20");
    assert_eq!(engine::k_exceed(20, SOFT), Some(10), "S1: K ≥ 10 of 20");
    let k: Vec<Option<u64>> = [5, 10, 20].iter().map(|&n| engine::k_exceed_j(n, engine::RING_J_COST, engine::HARD_COST)).collect();
    assert_eq!(k, [Some(5), Some(7), Some(11)], "H5b as tuned (8th largest, α = 0.005): ≥ 5 of 5, ≥ 7 of 10, ≥ 11 of 20");
}

#[test]
fn bignum_and_small_helpers() {
    let mut b = stats::Big::from(u128::MAX);
    b.mul(u64::MAX);
    assert_eq!(b.decimal(), "6277101735386680763495507056286727952620534092958556749825"); // Python: (2**128-1)*(2**64-1)
    assert_eq!(b.div_rem(7), 3);
    assert_eq!(stats::binom(120, 60).decimal(), "96614908840363322603893139521372656"); // Python: math.comb(120, 60)
    assert_eq!(stats::median2(&[1, 5, 3]), 6);
    assert_eq!(stats::median2(&[1, 2, 3, 10]), 5);
    assert_eq!(stats::jth_largest(&[5, 1, 9, 7], 2), Some(7));
    assert_eq!(stats::max_rate((1, 20), (3, 42)), (1, 14));
    assert_eq!(stats::max_rate((1, 20), (2, 42)), (1, 20));
    let t = stats::exceed_tail(40, 10, 5, 5);
    assert!(t.le(HARD) && t.ppm() < 2000 && t.ppm() > 0, "{}", t.ppm());
    assert_eq!(stats::exceed_tail(40, 10, 5, 0).ppm(), 1_000_000);
}

// ------------------------------------------------------------------------------------------------ RF-11

/// xorshift64*, Box–Muller: synthetic data only (the tests under it are integer).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn unit(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn normal(&mut self) -> f64 {
        (-2.0 * self.unit().ln()).sqrt() * (std::f64::consts::TAU * self.unit()).cos()
    }
    fn bern(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

/// The turn models of RF-11 (ADR-010's simulation; X10's clustering model, build/status/X10.md): turns in sessions of
/// geometric length with mean `l` (1: every turn its own session; ∞: one long session) that share a log offset
/// S ~ N(0, σ_s²); per-turn sd √(1 − σ_s²); requests scaled by e^S; corrections 10% and errors 2% iid.
#[derive(Clone, Copy)]
struct Model {
    l: f64,
    sigma_s: f64,
}

const IID: Model = Model { l: 1.0, sigma_s: 0.0 };
const IID_LONG: Model = Model { l: f64::INFINITY, sigma_s: 0.0 };

/// Which quantity a harmful change multiplies: cost, wall, requests or request bytes.
#[derive(Clone, Copy, PartialEq)]
enum Col {
    Cost,
    Wall,
    Req,
    Bytes,
}

/// A stream of `n` turns as the engine would unit them: a new unit at each new session and after `UNIT_MAX` turns, and
/// at the window's first turn (`cut_at`). From `cut_at` on, `col` is `blow` times as large.
fn stream(r: &mut Rng, m: Model, n: usize, cut_at: usize, blow: u64, col: Col) -> Vec<(Base, u64, u64)> {
    let sigma_t = (1.0 - m.sigma_s * m.sigma_s).sqrt();
    let (mut s, mut unit, mut len) = (m.sigma_s * r.normal(), 1u64, 0usize);
    (0..n).map(|i| {
        let new_session = i > 0 && r.bern(1.0 / m.l);
        if new_session {
            s = m.sigma_s * r.normal();
        }
        if i > 0 && (new_session || len >= engine::UNIT_MAX || i == cut_at) {
            (unit, len) = (unit + 1, 0);
        }
        len += 1;
        let mut e = |scale: f64| (scale * (s + sigma_t * r.normal()).exp()).round().max(1.0) as u64;
        let (cost, wall, bytes) = (e(3000.0), e(40_000.0), e(60_000.0));
        let req = ((1 + r.next() % 8) as f64 * s.exp()).round().max(1.0) as u64;
        let x = |c: Col, v: u64| if i >= cut_at && col == c { v * blow } else { v };
        let (cost, wall, req, bytes) = (x(Col::Cost, cost), x(Col::Wall, wall), x(Col::Req, req), x(Col::Bytes, bytes));
        let errors = (0..req).filter(|_| r.bern(0.02)).count() as u64;
        (Base { seq: i as u64, unit, cost, wall, req, bytes, corr: r.bern(0.10), zspend: cost }, req, errors)
    }).collect()
}

/// One window (20 turns, looks at 5, 10, 20) after enough history for a full ring of 40 units: the first signal that
/// fires, if any (`only`: count only that signal).
fn window_fires(r: &mut Rng, m: Model, blow: (u64, Col), only: Option<&str>) -> Option<&'static str> {
    let hist = engine::RING_M * engine::UNIT_MAX + 6;
    let turns = stream(r, m, hist + 20, hist, blow.0, blow.1);
    let ring: Vec<Base> = turns[..hist].iter().map(|t| t.0.clone()).collect();
    let req_ring: Vec<bool> = (0..400).map(|_| r.bern(0.02)).collect();
    let (mut win, mut reqs) = (Vec::new(), (0, 0));
    for (b, n, e) in &turns[hist..] {
        win.push(b.clone());
        (reqs.0, reqs.1) = (reqs.0 + n, reqs.1 + e);
        if engine::LOOKS.contains(&win.len())
            && let Some(h) = engine::judge(&ring, &req_ring, &win, reqs).into_iter().find(|h| only.is_none_or(|o| o == h.signal))
        {
            return Some(h.signal);
        }
    }
    None
}

fn null_rate(seed: u64, m: Model, n: usize) -> (f64, std::collections::BTreeMap<&'static str, u32>) {
    let mut r = Rng(seed);
    let mut by = std::collections::BTreeMap::new();
    let fired = (0..n).filter_map(|_| window_fires(&mut r, m, (1, Col::Cost), None)).inspect(|s| *by.entry(*s).or_default() += 1).count();
    (fired as f64 / n as f64, by)
}

/// How often a 3× rise of `col` is caught by its own signal within the window.
fn power_of(seed: u64, m: Model, n: usize, col: Col) -> f64 {
    let sig = match col { Col::Cost => "H5b", Col::Wall => "H8", Col::Req => "H10", Col::Bytes => "H11" };
    let mut r = Rng(seed);
    (0..n).filter(|_| window_fires(&mut r, m, (3, col), Some(sig)).is_some()).count() as f64 / n as f64
}

fn power(seed: u64, m: Model, n: usize) -> f64 {
    power_of(seed, m, n, Col::Cost)
}

/// RF-11: on null streams of independent turns the chronic signals (H5b, H7, H8, H9, H10, H11 at their looks) roll
/// back at most 2.5% of windows, whether every turn is its own session or all turns share one long session (units
/// of 6). A 3× cost blow-up is caught by H5b in at least 60% of windows (definition of done 2).
#[test]
fn rf11_null_streams_false_rollback_at_most_2_5_percent() {
    for (name, m, seed) in [("iid, a session per turn", IID, 0x9e37_79b9_7f4a_7c15), ("iid, one long session", IID_LONG, 0x1234_5678_9abc_def1)] {
        let (rate, by) = null_rate(seed, m, 3000);
        let p = power(seed ^ 0xffff, m, 1500);
        eprintln!("RF-11 {name}: null false rollback {:.2}% {by:?}; 3x cost caught {:.1}%", rate * 100.0, p * 100.0);
        assert!(rate <= 0.025, "{name}: null false rollback {:.2}% per window > 2.5% ({by:?})", rate * 100.0);
        assert!(p >= 0.60, "{name}: 3× cost power {:.1}% < 60%", p * 100.0);
    }
    // v2_fix: in one long session a 20-turn window holds 4 units; H8, H10 and H11 must still fire (X10 at 20k: wall
    // 76.2%, requests 92.2%, bytes 45.8%; bytes is held back by H11's 3× magnitude guard)
    let (w, q, b) = (power_of(31, IID_LONG, 1500, Col::Wall), power_of(32, IID_LONG, 1500, Col::Req), power_of(33, IID_LONG, 1500, Col::Bytes));
    eprintln!("RF-11 one long session, 3x caught: wall {:.1}%, requests {:.1}%, bytes {:.1}%", w * 100.0, q * 100.0, b * 100.0);
    assert!(w >= 0.65 && q >= 0.80 && b >= 0.35, "long-session power: wall {w}, requests {q}, bytes {b}");
}

/// The clustering fix (X10's finding; build/status/X3.md): turns that share a session's offset are not exchangeable, and
/// per-turn tests rolled back 9.7–14.8% of null windows. On units the null bound holds under X10's clustered models.
#[test]
fn rf11_clustered_sessions_keep_the_null_bound() {
    // OPEN (build/status/X3.md, orchestrator decision 2026-10-22): with v2_fix, heavy clustering in 8-turn sessions
    // gives about 3.1% (X10 at 20k: 3.09%), over the 2.5% bound; it must be measured on 14 days of real turns before
    // `extend.mode = "free"` becomes the default. Pinned here at ≤ 4%.
    for (l, sigma_s, bound) in [(4.0, 0.6, 0.025), (8.0, 0.6, 0.04), (4.0, 0.3, 0.025)] {
        let (rate, by) = null_rate(0x5eed_c1a5_7e5d_0001 ^ l as u64, Model { l, sigma_s }, 3000);
        eprintln!("RF-11 clustered L = {l}, σ_s = {sigma_s}: null false rollback {:.2}% {by:?}", rate * 100.0);
        assert!(rate <= bound, "clustered L = {l}, σ_s = {sigma_s}: null false rollback {:.2}% > {:.1}% ({by:?})", rate * 100.0, bound * 100.0);
    }
    let p = power(77, Model { l: 4.0, sigma_s: 0.0 }, 1500);
    let pc = power(78, Model { l: 4.0, sigma_s: 0.6 }, 1500);
    eprintln!("RF-11 3x cost caught: sessions of 4 with no shared offset {:.1}%; clustered L = 4, σ_s = 0.6 {:.1}%", p * 100.0, pc * 100.0);
    assert!(p >= 0.60);
}

/// Units (§3.2 as tuned): one owner session's consecutive turns, gaps under 30 min, at most 6; a change starts a new one.
#[test]
fn units_follow_sessions_gaps_and_changes() {
    let mut e = Engine::new(Some("tP"));
    let mut t = T0;
    let mut units = Vec::new();
    let mut one = |e: &mut Engine, t: &mut i64, gap: i64| {
        *t += gap;
        say(e, &cxp(*t), &format!("u{t}"), "go");
        e.tick(&cxp(*t + 31_000));
        units.push(e.recent.back().unwrap().unit);
        *t += 31_000;
    };
    for _ in 0..7 {
        one(&mut e, &mut t, 60_000);
    }
    one(&mut e, &mut t, engine::UNIT_GAP_MS); // a 30-minute gap
    e.session_started();
    one(&mut e, &mut t, 60_000); // a new owner session
    let o = Open { sha: "e".repeat(40), scope: "t0".into(), caps: vec![], tuple: "tQ".into() };
    let p = e.plan(&cxp(t), &o).unwrap();
    e.commit(&cxp(t), &o, &p, None);
    one(&mut e, &mut t, 60_000); // the window's first turn
    assert_eq!(units, [1, 1, 1, 1, 1, 1, 2, 3, 4, 5], "6 per unit, a 30-minute gap, a session, a change");
    assert_eq!(engine::gm1(&[3]), 3);
    assert_eq!(engine::gm1(&[0, 3]), 1, "⌊√(1·4)⌋ − 1");
    assert_eq!(engine::gm1(&[9, 99]), 30, "⌊√(10·100)⌋ − 1");
    assert_eq!(engine::gm1(&[1_000_000_000_000; 6]), 1_000_000_000_000, "no overflow at 6 × 10¹²");
    assert_eq!(engine::gm1(&[u64::MAX, 1]), 6_074_000_998, "⌊√((2⁶⁴ − 1)·2)⌋ − 1, with x + 1 saturating");
}

// ------------------------------------------------------------------------------------------------ RF-12

const T0: i64 = 1_900_000_000_000;

/// The Root's view with the group's tuple `tC` live (the `tuple` record of `change_open` moved `current`).
fn cx(now: i64) -> Ctx {
    Ctx { now, level: 0, current: Some("tC".into()), running: true, day: now - now.rem_euclid(86_400_000), free: Some(100 << 30), free_mode: true }
}

/// The view before the group opened: the parent `tP` is live.
fn cxp(now: i64) -> Ctx {
    Ctx { current: Some("tP".into()), ..cx(now) }
}

/// An engine with full rings (40 clean owner turns of cost 1000 µUSD, wall 10 s, 2 requests of 1 KB, no corrections)
/// and one open group (seq 1, parent `tP`, caps `skill/x`), opened in the turn running at `T0`.
fn armed() -> (Engine, i64) {
    let mut e = Engine::new(Some("tP"));
    let mut t = T0;
    for i in 0..40 {
        e.session_started(); // each baseline turn is its own task: 40 units
        e.line(&cxp(t), &format!("q{i}"), "hello");
        e.ack(&format!("q{i}"));
        for j in 0..2 {
            let id = format!("b{i}-{j}");
            e.req_start(&cxp(t + 1000), &id, 1000 + (i % 7) * 10);
            e.req_end(&cxp(t + 10_000 + i as i64 * 10), &id, 500 + (i % 5) * 10, false);
        }
        t += 60_000;
        e.tick(&cxp(t));
    }
    while e.req_ring.len() < engine::REQ_RING {
        e.req_ring.push_back(false);
    }
    assert_eq!((e.ring.len(), e.req_ring.len()), (40, engine::REQ_RING), "the rings are full");
    say(&mut e, &cxp(t), "q-open", "build a tool");
    let o = Open { sha: "a".repeat(40), scope: "t1".into(), caps: vec!["ext/x".into()], tuple: "tC".into() };
    let p = e.plan(&cxp(t + 1000), &o).unwrap();
    assert_eq!((p.group, p.first, p.parent.as_str()), (1, true, "tP"));
    e.commit(&cxp(t + 1000), &o, &p, Some("s1".into()));
    e.tick(&cx(t + 60_000)); // the turn the group was opened in ends (it is not a window turn)
    (e, t + 60_000)
}

/// An owner line the kernel acknowledges and answers with one cheap model request (a heard line).
fn say(e: &mut Engine, c: &Ctx, qid: &str, text: &str) {
    e.line(c, qid, text);
    e.ack(qid);
    let id = format!("say-{qid}");
    e.req_start(&Ctx { now: c.now + 100, ..c.clone() }, &id, 10);
    e.req_end(&Ctx { now: c.now + 200, ..c.clone() }, &id, 1, false);
}

type Shape = (u32, u64, i64, u64, u32, &'static str);
type Case = (&'static str, fn(usize) -> Shape);

/// One owner turn with `n` requests, each `cost` µUSD, `bytes` long, the first `errors` of them failing.
#[allow(clippy::too_many_arguments)]
fn turn(e: &mut Engine, t: &mut i64, n: u32, cost: u64, wall: i64, bytes: u64, errors: u32, line: &str) -> Vec<Act> {
    e.session_started(); // one task per turn, so each window turn is its own unit
    let mut out = e.line(&cx(*t), &format!("q{t}"), line);
    e.ack(&format!("q{t}"));
    for j in 0..n {
        let id = format!("r{t}-{j}");
        out.extend(e.req_start(&cx(*t + 500), &id, bytes));
        out.extend(e.req_end(&cx(*t + wall), &id, cost, j < errors));
    }
    *t += wall + 31_000;
    out.extend(e.tick(&cx(*t)));
    *t += 60_000;
    out
}

fn harm(acts: &[Act]) -> Option<Value> {
    acts.iter().find_map(|a| match a {
        Act::Append("harm", d) => Some(d.clone()),
        _ => None,
    })
}

fn closes(acts: &[Act]) -> Vec<Value> {
    acts.iter().filter_map(|a| match a {
        Act::Append("change_close", d) => Some(d.clone()),
        _ => None,
    }).collect()
}

fn assert_fires(acts: &[Act], signal: &str, outcome: &str, attribution: Option<&str>) {
    let h = harm(acts).unwrap_or_else(|| panic!("{signal} did not fire: {acts:?}"));
    assert_eq!(h["signal"], signal, "{h}");
    let c = closes(acts);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!((c[0]["outcome"].as_str(), c[0]["attribution"].as_str(), c[0]["trigger"].as_str()), (Some(outcome), attribution, Some(signal)), "{c:?}");
}

fn rolled_back_to_parent(acts: &[Act]) -> bool {
    acts.iter().any(|a| matches!(a, Act::Event("extend.rollback", d) if d["to"] == "tP"))
}

/// A run of `k` turns; the actions of all of them.
fn turns(e: &mut Engine, t: &mut i64, k: usize, f: impl Fn(usize) -> Shape) -> Vec<Act> {
    (0..k).flat_map(|i| {
        let (n, c, w, b, er, l) = f(i);
        turn(e, t, n, c, w, b, er, l)
    }).collect()
}

/// RF-12, the chronic signals: each fires at its first look on a stream that breaks it alone, rolls back the newest
/// (only) group as `direct`, quarantines its caps through X6's hook and sends `extend.rollback` to the parent.
#[test]
fn rf12_chronic_signals_fire() {
    let cases: [Case; 6] = [
        ("H5b", |_| (2, 5000, 10_000, 1000, 0, "go")),
        ("H8", |_| (2, 510, 100_000, 1000, 0, "go")),
        ("H10", |_| (6, 170, 10_000, 330, 0, "go")),
        ("H11", |_| (2, 510, 10_000, 9000, 0, "go")),
        ("H7", |i| (2, 510, 10_000, 1000, 0, if i == 2 { "go" } else { "/stop" })),
        ("H9", |_| (2, 510, 10_000, 1000, 1, "go")),
    ];
    for (sig, f) in cases {
        let (mut e, mut t) = armed();
        let acts = turns(&mut e, &mut t, 5, f);
        assert_fires(&acts, sig, "rolled_back", Some("direct"));
        assert!(rolled_back_to_parent(&acts), "{sig}: {acts:?}");
        assert!(acts.contains(&Act::Direct(vec!["ext/x".into()], false)), "{sig}: the caps go to X6");
        assert!(e.groups.is_empty() && e.rollback.is_some(), "{sig}");
        if sig == "H11" {
            assert!(acts.iter().any(|a| matches!(a, Act::Alert("egress_volume", _))));
        }
    }
    // a quiet stream at the baseline's level: no look fires, and the window closes clean at 20 turns after 2 h
    let (mut e, mut t) = armed();
    let acts = turns(&mut e, &mut t, 20, |_| (2, 510, 10_000, 1000, 0, "go"));
    assert!(harm(&acts).is_none(), "{acts:?}");
    let acts = e.tick(&cx(t + 3 * engine::HOUR_MS));
    let c = closes(&acts);
    assert_eq!((c.len(), c[0]["outcome"].as_str(), c[0]["turns"].as_u64()), (1, Some("clean"), Some(20)), "{c:?}");
    assert!(acts.contains(&Act::Append("lkg", json!({"tuple": "tC"}))), "lkg moves to the clean group's tuple");
}

/// RF-12, the acute signals and the zone veto.
#[test]
fn rf12_acute_signals_fire() {
    // H1: two body crashes within 600 s → every open group (one: direct); the kernel is told to roll back
    let (mut e, t) = armed();
    assert!(e.body_exit(&cx(t), true).is_empty());
    assert!(e.body_exit(&cx(t + 1000), false).is_empty(), "a clean exit is not a crash");
    let acts = e.body_exit(&cx(t + 2000), true);
    assert_fires(&acts, "H1", "rolled_back", Some("direct"));
    assert!(rolled_back_to_parent(&acts));
    // H1 escalation: the target crashes twice in 600 s → R2 (X4's snapshot) with a whole-system restart
    e.body_exit(&cx(t + 3000), true);
    let acts = e.body_exit(&cx(t + 4000), true);
    assert!(acts.iter().any(|a| matches!(a, Act::Restart { to: Some(to), restore: Some(s), .. } if to == "tP" && s == "s1")), "{acts:?}");

    // H1 (kernel): one kernel crash with a group open; no kernel runs, so the Root restarts on the parent at once
    let (mut e, t) = armed();
    let acts = e.kernel_crash(&Ctx { running: false, ..cx(t) });
    assert_fires(&acts, "H1", "rolled_back", Some("direct"));
    assert!(acts.iter().any(|a| matches!(a, Act::Restart { to: Some(to), reason: "rollback_system", .. } if to == "tP")), "{acts:?}");
    assert!(e.body_exit(&cx(t + 1000), true).is_empty(), "bodies dying with their kernel are not crashes");

    // H2: two lines with no ack in 30 s
    let (mut e, mut t) = armed();
    t += 600_000;
    e.tick(&cx(t));
    e.line(&cx(t), "d1", "hello?");
    assert!(harm(&e.tick(&cx(t + 31_000))).is_none(), "one deaf line is not yet a trigger");
    e.line(&cx(t + 40_000), "d2", "hello??");
    let acts = e.tick(&cx(t + 71_000));
    assert_fires(&acts, "H2", "rolled_back", Some("direct"));
    assert!(acts.iter().any(|a| matches!(a, Act::Alert("deaf", _))));

    // H3: the owner's /reject <sha> names the group: direct, quarantined
    let (mut e, t) = armed();
    let acts = e.line(&cx(t), "r1", &format!("/reject {}", &"a".repeat(40)[..12]));
    assert_fires(&acts, "H3", "rolled_back", Some("direct"));
    // /rollback ext/x: by cap; the named cap is quarantined even with no group
    let (mut e, t) = armed();
    let acts = e.reversal(&cx(t), Some("ext/x"), "/rollback");
    assert_fires(&acts, "H3", "rolled_back", Some("direct"));
    assert!(acts.iter().filter(|a| matches!(a, Act::Direct(..))).count() == 2);

    // H4: a demoting refusal: the open groups go as collateral; the demotion is the ledger's
    let (mut e, t) = armed();
    let acts = e.scope(&cx(t));
    assert_fires(&acts, "H4", "collateral", None);
    assert_eq!(harm(&acts).unwrap()["action"], "demote");

    // H5a: $0.60 in an hour → the newest group, and new groups are frozen for 1 h
    let (mut e, mut t) = armed();
    let acts = turns(&mut e, &mut t, 1, |_| (1, 600_000, 10_000, 1000, 0, "go"));
    assert_fires(&acts, "H5a", "rolled_back", Some("direct"));
    let o = Open { sha: "b".repeat(40), scope: "t0".into(), tuple: "tD".into(), ..Open::default() };
    assert_eq!(e.plan(&cx(t), &o).unwrap_err().0, "extend.budget", "frozen");
    e.tick(&cx(t + engine::HOUR_MS)); // the kernel never confirmed: the whole-system fallback ends the pending rollback
    assert!(e.plan(&cx(t + engine::HOUR_MS), &o).is_ok(), "the freeze ends after 1 h");

    // H6: two mem_cap kills in 24 h; > 120 spawns in an hour (churn: all groups)
    let (mut e, t) = armed();
    assert!(e.mem_event(&cx(t), "mem_cap").is_empty());
    assert_fires(&e.mem_event(&cx(t + 3_600_000), "mem_cap"), "H6", "rolled_back", Some("direct"));
    let (mut e, t) = armed();
    let acts: Vec<Act> = (0..121).flat_map(|i| e.spawned(&cx(t + i * 1000))).collect();
    assert_fires(&acts, "H6", "rolled_back", Some("direct"));
    // H6 disk floor with INIT_HOME grown ≥ 2 GiB since the group opened (the free-space proxy)
    let (mut e, t) = armed();
    assert!(e.mem_event(&Ctx { free: Some(99 << 30), ..cx(t) }, "disk_floor").is_empty());
    assert_fires(&e.mem_event(&Ctx { free: Some(97 << 30), ..cx(t) }, "disk_floor"), "H6", "rolled_back", Some("direct"));

    // H12: a zone veto is honoured and never attributed
    let (mut e, t) = armed();
    let acts = e.veto(&cx(t), 1).unwrap();
    assert_fires(&acts, "H12", "rolled_back", None);
    assert!(!acts.iter().any(|a| matches!(a, Act::Direct(..))), "a veto quarantines nothing");
    assert!(e.veto(&cx(t), 9).is_err());
}

/// §3.4: with two groups open, a chronic trigger takes the newest (direct) and the older keeps its window; an acute
/// churn signal takes both as `suspect`; a named group takes the newer one as `collateral`.
#[test]
fn attribution_lifo_suspect_collateral() {
    let open2 = |e: &mut Engine, t: &mut i64| {
        say(e, &cx(*t), "q-2", "another");
        let o = Open { sha: "c".repeat(40), scope: "t0".into(), caps: vec!["skill/y".into()], tuple: "tE".into() };
        let p = e.plan(&cx(*t + 10), &o).unwrap();
        assert_eq!((p.group, p.first, p.parent.as_str()), (2, true, "tC"));
        e.commit(&cx(*t + 10), &o, &p, None);
        *t += 60_000;
    };
    let ctxc = |t: i64| Ctx { current: Some("tE".into()), ..cx(t) };
    let (mut e, mut t) = armed();
    open2(&mut e, &mut t);
    let acts = e.trigger(&ctxc(t), "H5b", 5, 5, Pick::Newest, "rollback_newest");
    let c = closes(&acts);
    assert_eq!((c.len(), c[0]["group"].as_u64(), c[0]["attribution"].as_str()), (1, Some(2), Some("direct")));
    assert!(acts.iter().any(|a| matches!(a, Act::Event("extend.rollback", d) if d["to"] == "tC")));
    assert_eq!(e.groups.len(), 1);

    let (mut e, mut t) = armed();
    open2(&mut e, &mut t);
    let c = closes(&e.trigger(&ctxc(t), "H1", 2, 2, Pick::All, "rollback_all"));
    assert_eq!(c.iter().map(|c| c["attribution"].as_str().unwrap()).collect::<Vec<_>>(), ["suspect", "suspect"]);

    let (mut e, mut t) = armed();
    open2(&mut e, &mut t);
    let acts = e.reversal(&ctxc(t), Some(&"a".repeat(9)), "/rollback");
    let c = closes(&acts);
    assert_eq!(c.iter().map(|c| (c["group"].as_u64().unwrap(), c["outcome"].as_str().unwrap())).collect::<Vec<_>>(),
        [(1, "rolled_back"), (2, "collateral")]);
    assert!(acts.iter().any(|a| matches!(a, Act::Event("extend.rollback", d) if d["to"] == "tP")), "to group 1's parent");
}

/// §3.5: `collateral` and `suspect` changes are re-admitted once, alone; a `direct` one never; re-admissions do not
/// count against the day; two `direct` rollbacks of a cap within 14 days set the repeat ban.
#[test]
fn readmission_and_repeat_ban() {
    let (mut e, mut t) = armed();
    say(&mut e, &cx(t), "q-2", "another");
    let o2 = Open { sha: "c".repeat(40), scope: "t0".into(), caps: vec!["skill/y".into()], tuple: "tE".into() };
    let p = e.plan(&cx(t), &o2).unwrap();
    e.commit(&cx(t), &o2, &p, None);
    e.reversal(&cx(t), Some("ext/x"), "/rollback"); // group 1 direct, group 2 collateral
    e.rollback = None;
    t += 60_000;
    let o1 = Open { sha: "a".repeat(40), scope: "t1".into(), caps: vec!["ext/x".into()], tuple: "tC".into() };
    assert_eq!(e.plan(&cx(t), &o1).unwrap_err().0, "extend.readmit", "direct: never re-admitted");
    let p = e.plan(&cx(t), &o2).unwrap();
    assert!(p.readmit && p.first);
    let before = e.day;
    e.commit(&cx(t), &o2, &p, None);
    assert_eq!(e.day, before, "a re-admission does not count against changes_day");
    let acts = e.veto(&cx(t), p.group).unwrap();
    assert!(!closes(&acts).is_empty());
    e.rollback = None;
    assert_eq!(e.plan(&cx(t), &o2).unwrap_err().0, "extend.readmit", "only once");
    // the repeat ban: a second direct rollback of ext/x within 14 days
    let o3 = Open { sha: "d".repeat(40), scope: "t1".into(), caps: vec!["ext/x".into()], tuple: "tF".into() };
    let p = e.plan(&cx(t), &o3).unwrap();
    e.commit(&cx(t), &o3, &p, None);
    let acts = e.reversal(&cx(t + 1000), Some("ext/x"), "/rollback");
    assert!(acts.contains(&Act::Direct(vec!["ext/x".into()], true)), "{acts:?}");
}

/// §2.2 and §4.5: a change in the same turn joins the group (a new version of a cap replaces the old one); the window
/// runs from the last join; the open-group budget holds at L0.
#[test]
fn groups_join_and_budget() {
    let (mut e, t) = armed();
    say(&mut e, &cx(t), "q-x", "more");
    let mk = |sha: char, cap: &str, tuple: &str| Open { sha: sha.to_string().repeat(40), scope: "t1".into(), caps: vec![cap.into()], tuple: tuple.into() };
    for (i, o) in [mk('b', "ext/y", "tG"), mk('c', "ext/y", "tH")].iter().enumerate() {
        let p = e.plan(&cx(t + 10), o).unwrap();
        assert_eq!((p.first, p.group), (i == 0, 2), "the second change joins the first's group");
        e.commit(&cx(t + 10), o, &p, None);
    }
    assert_eq!(e.groups[1].changes.len(), 1, "a new version replaces the old within the group");
    assert_eq!(e.groups[1].tuple, "tH");
    for (i, c) in ['d', 'e'].into_iter().enumerate() {
        e.tick(&cx(t + 100_000 * (i as i64 + 1) - 1)); // the previous turn ends
        say(&mut e, &cx(t + 100_000 * (i as i64 + 1)), &format!("q-{c}"), "next");
        let o = mk(c, &format!("ext/{c}"), "tZ");
        let p = e.plan(&cx(t + 100_000 * (i as i64 + 1)), &o).unwrap();
        e.commit(&cx(t), &o, &p, None);
    }
    e.tick(&cx(t + 499_999));
    say(&mut e, &cx(t + 500_000), "q-f", "next");
    assert_eq!(e.plan(&cx(t + 500_000), &mk('f', "ext/f", "tY")).unwrap_err().0, "extend.budget", "4 open groups at L0");
    let k = Open { sha: "9".repeat(40), scope: "k".into(), tuple: "tK".into(), ..Open::default() };
    assert_eq!(e.plan(&cx(t), &k).unwrap_err().0, "extend.budget", "kernel changes need L1 and no open group");
}

/// §3.1: background turns, the 15-minute cap, steers and late corrections; caps keep `root` evidence and sum.
#[test]
fn turns_owner_background_and_caps() {
    let mut e = Engine::new(Some("tP"));
    let mut out = e.req_start(&cx(T0), "x1", 100);
    e.req_caps("x1", &[("ext/a".into(), 1, "zone"), ("ext/a".into(), 2, "root")]);
    out.extend(e.req_end(&cx(T0 + 1000), "x1", 7, false));
    assert_eq!(e.turn.as_ref().unwrap().kind, "background");
    out.extend(e.line(&cx(T0 + 2000), "o1", "hi")); // the owner speaks: the background turn ends
    let closed: Vec<Turn> = out.iter().filter_map(|a| if let Act::TurnClosed(t) = a { Some(t.clone()) } else { None }).collect();
    assert_eq!((closed[0].kind.as_str(), closed[0].cost_musd), ("background", 7));
    assert_eq!(closed[0].caps["ext/a"], (3, "root".to_string()));
    e.line(&cx(T0 + 3000), "o2", "and also"); // a steer: a weak signal, never a correction (MERGE)
    assert_eq!((e.turn.as_ref().unwrap().correction, e.turn.as_ref().unwrap().steered), (false, 1));
    let acts = e.tick(&cx(T0 + 2000 + engine::QUIET_MS));
    let t = acts.iter().find_map(|a| if let Act::TurnClosed(t) = a { Some(t.clone()) } else { None }).unwrap();
    assert_eq!((t.kind.as_str(), t.zspend_musd, t.correction, t.steered), ("owner", 7, false, 1), "background spend goes to the next owner turn");
    e.interrupt(&cx(T0 + 100_000)); // a Root-handled correction just after the turn
    assert!(e.recent.back().unwrap().correction);
    // the cap: a turn with a request in flight ends at 15 minutes as `capped`, a harmed turn
    e.line(&cx(T0 + 1_000_000), "o3", "long");
    e.req_start(&cx(T0 + 1_000_100), "x2", 10);
    let acts = e.tick(&cx(T0 + 1_000_000 + engine::TURN_CAP_MS));
    let t = acts.iter().find_map(|a| if let Act::TurnClosed(t) = a { Some(t.clone()) } else { None }).unwrap();
    assert!(t.capped && t.harmed());
}

/// The busy lie (H2's 900 s rule): a kernel that acknowledges lines and claims to be busy, but never reaches the
/// model, is deaf after 900 s; with no group open, the kernel is restarted once.
#[test]
fn deafness_busy_lie_and_restart() {
    let mut e = Engine::new(Some("tP"));
    e.hb(true);
    e.line(&cx(T0), "l1", "hello");
    e.ack("l1");
    assert!(harm(&e.tick(&cx(T0 + 300_000))).is_none(), "busy: the 120 s rule does not apply");
    let acts = e.tick(&cx(T0 + engine::ANY_MS));
    assert_eq!(harm(&acts).unwrap()["signal"], "H2");
    assert!(acts.iter().any(|a| matches!(a, Act::Restart { to: None, .. })), "{acts:?}");
    e.line(&cx(T0 + 1_000_000), "l2", "hello");
    e.ack("l2");
    assert!(!e.tick(&cx(T0 + 1_000_000 + engine::ANY_MS)).iter().any(|a| matches!(a, Act::Restart { .. })), "one restart an hour");
    // `extend.mode = "gated"` (the default): with no group open the monitor only watches
    let mut e = Engine::new(Some("tP"));
    e.line(&cx(T0), "g1", "hello");
    e.ack("g1");
    let acts = e.tick(&Ctx { free_mode: false, ..cx(T0 + engine::ANY_MS) });
    assert!(harm(&acts).is_none() && !acts.iter().any(|a| matches!(a, Act::Restart { .. } | Act::Alert(..))), "{acts:?}");
    // a heard line resets the streak
    let mut e = Engine::new(Some("tP"));
    e.line(&cx(T0), "h1", "a");
    e.tick(&cx(T0 + 31_000)); // deaf (no ack): streak 1
    e.line(&cx(T0 + 40_000), "h2", "b");
    e.ack("h2");
    e.req_start(&cx(T0 + 41_000), "z", 1);
    e.tick(&cx(T0 + 42_000));
    assert_eq!(e.streak, 0);
}

/// An ack that wins the race with its line (link.rs sends `owner.in`, then reports the line; the GBAL e2e saw a fast
/// kernel answer in between) still counts: two such lines are heard, not two deaf lines that roll every group back.
#[test]
fn an_ack_before_its_line_is_not_deafness() {
    let (mut e, mut t) = armed();
    for q in ["e1", "e2", "e3"] {
        e.ack(q);
        e.line(&cx(t), q, "work on the repo");
        e.req_start(&cx(t + 100), &format!("r-{q}"), 10);
        e.req_end(&cx(t + 200), &format!("r-{q}"), 1, false);
        t += 31_000;
        let acts = e.tick(&cx(t));
        assert!(harm(&acts).is_none(), "{q}: {acts:?}");
        assert_eq!(e.streak, 0, "{q}");
    }
    assert!(e.lines.is_empty() && e.early_acks.is_empty());
    // an ack whose line never comes (not running, or another session's) is forgotten after 64
    for i in 0..100 {
        e.ack(&format!("x{i}"));
    }
    assert_eq!((e.early_acks.len(), e.early_acks.front().map(String::as_str)), (64, Some("x36")));
}

/// The rollback mechanics: the kernel's `body_rollback` to the target's body sha confirms; otherwise the deadline
/// gives a whole-system restart at 120 s.
#[test]
fn rollback_confirmation_and_fallback() {
    let (mut e, t) = armed();
    e.veto(&cx(t), 1).unwrap();
    let body = |id: &str| (id == "tP").then(|| "p".repeat(40));
    assert!(e.confirmed(&cx(t), &"x".repeat(40), body).is_empty(), "another sha confirms nothing");
    let acts = e.confirmed(&Ctx { current: Some("tC".into()), ..cx(t + 5000) }, &"p".repeat(40), body);
    assert!(acts.contains(&Act::Append("rollback", json!({"from": "tC", "reason": "H12", "target": "parent", "to": "tP"}))), "{acts:?}");
    assert!(e.rollback.is_none());
    let (mut e, t) = armed();
    e.veto(&cx(t), 1).unwrap();
    assert!(!e.tick(&cx(t + engine::ROLLBACK_MS - 1)).iter().any(|a| matches!(a, Act::Restart { .. })));
    let acts = e.tick(&cx(t + engine::ROLLBACK_MS));
    assert!(acts.iter().any(|a| matches!(a, Act::Restart { to: Some(to), reason: "rollback_system", .. } if to == "tP")), "{acts:?}");
}

/// The lineage is authoritative for groups and history: a fold of `change_open`/`change_close` rebuilds them.
#[test]
fn groups_fold_from_the_lineage() {
    let r = |kind: &str, at: &str, data: Value| json!({"at": at, "data": data, "kind": kind});
    let recs = vec![
        r("change_open", "2030-01-01T00:00:00Z", json!({"group": 3, "sha": "a1", "scope": "t1", "caps": [{"id": "ext/a", "op": "add"}],
            "parent_tuple": "tP", "tuple": "tA", "snapshot": "3"})),
        r("change_open", "2030-01-01T00:01:00Z", json!({"group": 4, "sha": "b1", "scope": "t0", "caps": [], "parent_tuple": "tA", "tuple": "tB"})),
        r("change_close", "2030-01-01T01:00:00Z", json!({"group": 3, "outcome": "rolled_back", "attribution": "direct"})),
    ];
    let mut e = Engine::new(None);
    super::fold(&mut e, &recs);
    assert_eq!(e.groups.iter().map(|g| g.seq).collect::<Vec<_>>(), [4]);
    assert_eq!((e.next_group, e.hist["a1"].status.as_str()), (5, "direct"));
    assert_eq!(e.directs["ext/a"].len(), 1);
}

/// X10's shared decision vectors (kernel-gate `stats::extend_sim::judge_vectors`): the simulator and the Root agree on
/// every look's hits and on S1, exactly. Read at run time from the kernel workspace; skipped (with a note) when that
/// file is absent, so the Root never fails to build on another workspace's fixture. `cases` are per-turn (each turn is
/// its own unit here); their H5b rows are compared only when the file's H5b rank and α are the Root's. `units`, when
/// present, carries unit ids and is compared in full.
#[test]
fn x10_judge_vectors() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../kernel/crates/kernel-gate/tests/fixtures/extend_sim/judge.json");
    let Ok(src) = std::fs::read_to_string(path) else {
        eprintln!("x10_judge_vectors: {path} is absent; skipped");
        return;
    };
    let v: Value = serde_json::from_str(&src).unwrap();
    let run = |sec: &Value, units: bool| {
        let d = &sec["defaults"];
        assert_eq!((d["ring_m"].as_u64(), d["ring_j"].as_u64(), d["req_ring"].as_u64()),
            (Some(engine::RING_M as u64), Some(engine::RING_J as u64), Some(engine::REQ_RING as u64)));
        assert_eq!(d["looks"], json!(engine::LOOKS));
        // a signal's rows are compared only when the file's rank and α for it are the Root's: H5b (`cost_j`), H8/H10/H11
        // (`ex_j`, else the per-turn `ring_j`/`hard`); H7 and H9 always
        let ours = json!([engine::HARD_COST.0, engine::HARD_COST.1]);
        let same_h5b = d["cost_j"].as_u64() == Some(engine::RING_J_COST as u64) && d["cost_alpha"] == ours;
        let ex = (d.get("ex_j").unwrap_or(&d["ring_j"]).as_u64(), d.get("ex_alpha").unwrap_or(&d["hard"]));
        let same_ex = ex.0 == Some(engine::RING_J_COST as u64) && *ex.1 == ours;
        let base = |i: usize, b: &Value| Base { seq: i as u64, unit: if units { u(&b[6]) } else { i as u64 + 1 }, cost: u(&b[0]), wall: u(&b[1]),
            req: u(&b[2]), bytes: u(&b[3]), corr: u(&b[4]) == 1, zspend: u(&b[5]) };
        let cases = sec["cases"].as_array().unwrap();
        assert!(cases.len() >= 20);
        for c in cases {
            let ring: Vec<Base> = c["ring"].as_array().unwrap().iter().enumerate().map(|(i, b)| base(i, b)).collect();
            let (e, n) = (u(&c["req_ring"][0]) as usize, u(&c["req_ring"][1]) as usize);
            let req_ring: Vec<bool> = (0..n).map(|i| i < e).collect();
            let win: Vec<Base> = c["win"].as_array().unwrap().iter().enumerate().map(|(i, b)| base(1000 + i, b)).collect();
            let cols: [fn(&Base) -> u64; 5] = [|b| b.cost, |b| b.wall, |b| b.req, |b| b.bytes, |b| b.zspend];
            let per_unit = |turns: &[Base]| -> Value {
                let v: Vec<Vec<u64>> = cols.iter().map(|f| engine::unit_values(turns, *f)).collect();
                Value::from((0..v[0].len()).map(|i| json!([v[0][i], v[1][i], v[2][i], v[3][i], v[4][i]])).collect::<Vec<_>>())
            };
            if units {
                assert_eq!(per_unit(&ring), c["ring_units"], "{}: the ring's unit values (gm1)", c["case"]);
            }
            for l in c["looks"].as_array().unwrap() {
                let k = u(&l["n"]) as usize;
                let reqs = (u(&l["reqs"][0]), u(&l["reqs"][1]));
                if units {
                    assert_eq!(per_unit(&win[..k]), l["unit_values"], "{} n={k}: the window's unit values", c["case"]);
                    assert_eq!(engine::unit_values(&win[..k], |b| b.cost).len() as u64, u(&l["units"]), "{} n={k}", c["case"]);
                }
                let keep = |h: &Value| match h[0].as_str() {
                    Some("H5b") => same_h5b,
                    Some("H8" | "H10" | "H11") => same_ex,
                    _ => true,
                };
                let got: Vec<Value> = engine::judge(&ring, &req_ring, &win[..k], reqs).iter().map(|h| json!([h.signal, h.value, h.threshold]))
                    .filter(keep).collect();
                let want: Vec<Value> = l["hits"].as_array().unwrap().iter().filter(|h| keep(h)).cloned().collect();
                assert_eq!(got, want, "{} n={k}", c["case"]);
            }
            if same_h5b || !units {
                assert_eq!(engine::soft(&ring, &win), c["soft"].as_bool().unwrap(), "{}", c["case"]);
            }
        }
        same_h5b && same_ex
    };
    let full = run(&json!({"defaults": v["defaults"], "cases": v["cases"]}), false);
    eprintln!("x10_judge_vectors: per-turn cases {}", if full { "in full" } else { "on H7, H9 and S1 (their H5b, H8, H10, H11 rows predate the tuning)" });
    if v["units"].is_object() {
        assert!(run(&v["units"], true), "the units section must carry the Root's rank and α for H5b, H8, H10 and H11 (v2_fix)");
        eprintln!("x10_judge_vectors: units section in full");
    }
}

/// The full-size measurement behind RF-11 (ignored: minutes). `X3_SIM_N` windows per row (default 20,000):
/// `cargo test … -- --ignored --nocapture rf11_full_size`.
#[test]
#[ignore]
fn rf11_full_size() {
    let n: usize = std::env::var("X3_SIM_N").ok().and_then(|s| s.parse().ok()).unwrap_or(20_000);
    let rows = [("iid, a session per turn", IID), ("iid, one long session", IID_LONG), ("sessions of 4, no offset", Model { l: 4.0, sigma_s: 0.0 }),
        ("clustered L = 4, σ_s = 0.3", Model { l: 4.0, sigma_s: 0.3 }), ("clustered L = 4, σ_s = 0.6", Model { l: 4.0, sigma_s: 0.6 }),
        ("clustered L = 8, σ_s = 0.6", Model { l: 8.0, sigma_s: 0.6 })];
    for (i, (name, m)) in rows.into_iter().enumerate() {
        let (rate, by) = null_rate(1000 + i as u64, m, n);
        let p = power(2000 + i as u64, m, n / 2);
        let se = |p: f64, n: usize| (p * (1.0 - p) / n as f64).sqrt() * 100.0;
        let [w, q, b] = [Col::Wall, Col::Req, Col::Bytes].map(|c| power_of(3000 + i as u64, m, n / 4, c) * 100.0);
        eprintln!("FULL {name}: null {:.2}% ± {:.2} {by:?}; 3x power: cost (H5b) {:.1}% ± {:.1}, wall {w:.1}%, requests {q:.1}%, bytes {b:.1}%",
            rate * 100.0, se(rate, n), p * 100.0, se(p, n / 2));
    }
}

// ------------------------------------------------------------------------------------------------ MERGE

/// An engine with one owner turn open at `T0` (the line that starts it is neither a steer nor a correction).
fn open_turn() -> Engine {
    let mut e = Engine::new(Some("tP"));
    e.frame(&cx(T0), "s0", &json!({"t": "line", "text": "fix the flaky test"}));
    let t = e.turn.as_ref().unwrap();
    assert_eq!((t.kind.as_str(), t.correction, t.steered), ("owner", false, 0));
    e
}

fn now_turn(e: &Engine) -> (bool, u32) {
    let t = e.turn.as_ref().unwrap();
    (t.correction, t.steered)
}

/// MERGE (STEER's "Root sentinel" proposal): a turn's correction comes only from owner interrupts. Plain steers count
/// as `steered` (a weak signal), `/later` lines count as nothing, and a retracted steer gives its count back.
#[test]
fn corrections_are_owner_interrupts_only() {
    let line = |text: &str| json!({"t": "line", "text": text});
    // a plain steer: no correction
    let mut e = open_turn();
    e.frame(&cx(T0 + 1000), "s1", &line("and also rename the test"));
    assert_eq!(now_turn(&e), (false, 1), "a steer is not a correction");
    // `/later …`, and the kernel's read-only commands: neither
    let mut e = open_turn();
    for (i, l) in ["/later then update the changelog", "/status", "/kernel"].iter().enumerate() {
        e.frame(&cx(T0 + 1000 + i as i64), &format!("l{i}"), &line(l));
    }
    assert_eq!(now_turn(&e), (false, 0), "a follow-up is not a correction");
    // a retracted steer: its count goes back (the kernel echo's seq names it); an unknown seq changes nothing
    let mut e = open_turn();
    e.frame(&cx(T0 + 1000), "s1", &line("use nextest"));
    e.frame(&cx(T0 + 1100), "s2", &line("and also the docs"));
    e.ack("s1");
    e.echoed(7, "use nextest");
    e.ack("s2");
    e.echoed(8, "and also the docs");
    assert_eq!(now_turn(&e), (false, 2));
    e.frame(&cx(T0 + 2000), "r1", &json!({"t": "retract", "seq": 99}));
    assert_eq!(now_turn(&e), (false, 2), "an unknown seq takes nothing back");
    e.frame(&cx(T0 + 2000), "r2", &json!({"t": "retract", "seq": 8}));
    assert_eq!(now_turn(&e), (false, 1), "the retracted line no longer counts");
    e.frame(&cx(T0 + 2100), "r3", &json!({"t": "retract", "seq": 8}));
    assert_eq!(now_turn(&e), (false, 1), "a retract counts once");
    // a long steer whose echo the kernel cut to fit the owner line still matches; after the turn closed, the closed
    // turn gives the count back
    let mut e = open_turn();
    let long = "x".repeat(5000);
    e.frame(&cx(T0 + 1000), "s1", &line(&long));
    e.echoed(3, &format!("{} [kernel: showing 900 of 5000 bytes, cut to fit the owner line; the session log has the full record]", &long[..900]));
    e.tick(&cx(T0 + 1000 + engine::QUIET_MS));
    assert!(e.turn.is_none());
    e.frame(&cx(T0 + 40_000), "r1", &json!({"t": "retract", "seq": 3}));
    let last = e.recent.back().unwrap();
    assert_eq!((last.correction, last.steered), (false, 0));
    // owner interrupts: Esc (and Ctrl-C), interrupt-and-steer, `/stop`, `/panic`, `/undo`, `/reject`
    type Case = (&'static str, fn(&mut Engine));
    let cases: [Case; 6] = [
        ("esc", |e| drop(e.frame(&cx(T0 + 1000), "i1", &json!({"t": "interrupt"})))),
        ("then:continue", |e| drop(e.frame(&cx(T0 + 1000), "i1", &json!({"t": "interrupt", "then": "continue"})))),
        ("/stop", |e| drop(e.frame(&cx(T0 + 1000), "i1", &json!({"t": "line", "text": "/stop"})))),
        ("/panic", |e| drop(e.panic(&cx(T0 + 1000)))),
        // `owner_text` marks it (`interrupt`), then the line is relayed
        ("/undo", |e| {
            e.interrupt(&cx(T0 + 1000));
            e.frame(&cx(T0 + 1000), "i1", &json!({"t": "line", "text": "/undo"}));
        }),
        ("/reject", |e| drop(e.frame(&cx(T0 + 1000), "i1", &json!({"t": "line", "text": "/reject job-1"})))),
    ];
    for (name, f) in cases {
        let mut e = open_turn();
        f(&mut e);
        let corr = e.turn.as_ref().map_or_else(|| e.recent.back().unwrap().correction, |t| t.correction);
        assert!(corr, "{name} is a correction");
    }
    // `/stop` with words after it is a line to the body (the kernel reads `/stop` alone): a steer
    let mut e = open_turn();
    e.frame(&cx(T0 + 1000), "s1", &line("/stop the tests from flaking"));
    assert_eq!(now_turn(&e), (false, 1));
}

/// R-UNDO (v0.5.2): `/undo list` (the Root's own line, which only reads the snapshots) corrects no turn; `/undo`,
/// `/undo <id>` and `/undo <id> confirm` do (`owner_text` marks them).
#[test]
fn undo_list_is_not_a_correction() {
    assert!(!super::undo_corrects("/undo list") && !super::undo_corrects("  /undo   list "));
    for l in ["/undo", "/undo confirm", "/undo 20301010T000000.000Z-proj", "/undo 20301010T000000.000Z-proj confirm"] {
        assert!(super::undo_corrects(l), "{l}");
    }
}

/// MERGE: several corrections in one turn count once (the flag is per turn), so the closed turn adds one corrected
/// turn to the rings and to the corrections trend's `owner_corrected`.
#[test]
fn several_corrections_in_one_turn_count_once() {
    let mut e = open_turn();
    e.frame(&cx(T0 + 1000), "s1", &json!({"t": "line", "text": "and also"}));
    e.frame(&cx(T0 + 2000), "i1", &json!({"t": "interrupt"}));
    e.frame(&cx(T0 + 3000), "i2", &json!({"t": "interrupt", "then": "continue"}));
    e.frame(&cx(T0 + 4000), "s2", &json!({"t": "line", "text": "/stop"}));
    e.interrupt(&cx(T0 + 5000));
    let acts = e.tick(&cx(T0 + 5000 + engine::QUIET_MS));
    let closed: Vec<Turn> = acts.iter().filter_map(|a| if let Act::TurnClosed(t) = a { Some(t.clone()) } else { None }).collect();
    assert_eq!(closed.len(), 1);
    assert_eq!((closed[0].correction, closed[0].steered), (true, 1));
    assert_eq!(e.recent.iter().filter(|t| t.kind == "owner" && t.correction).count(), 1, "one corrected turn");
}

/// MERGE (DELEGATE's proposal): `delegate` sub-task requests (`x-egress-subtask`) are counted apart on the turn, so N
/// sub-task requests a turn (4× the main loop's requests and 8× its cost) leave H10 and H5b unchanged; the same
/// traffic from the main loop still fires. (H5a, the hourly burst, still counts every request.)
#[test]
fn subtask_traffic_is_not_a_per_turn_spike() {
    fn sub_turn(e: &mut Engine, t: &mut i64, sub: bool) -> Vec<Act> {
        e.session_started();
        let q = format!("q{t}");
        let mut out = e.line(&cx(*t), &q, "go");
        e.ack(&q);
        for j in 0..2 {
            let id = format!("m{t}-{j}");
            out.extend(e.req_start_as(&cx(*t + 500), &id, 1000, false));
            out.extend(e.req_end(&cx(*t + 10_000), &id, 510, false));
        }
        for j in 0..8 {
            let id = format!("d{t}-{j}");
            out.extend(e.req_start_as(&cx(*t + 600), &id, 0, sub));
            out.extend(e.req_end(&cx(*t + 9_000), &id, 1000, false));
        }
        *t += 10_000 + 31_000;
        out.extend(e.tick(&cx(*t)));
        *t += 60_000;
        out
    }
    let (mut e, mut t) = armed();
    let acts: Vec<Act> = (0..20).flat_map(|_| sub_turn(&mut e, &mut t, true)).collect();
    assert!(harm(&acts).is_none(), "sub-task traffic fired a signal: {acts:?}");
    let last = e.recent.iter().rev().find(|x| x.kind == "owner").unwrap();
    assert_eq!((last.requests, last.sub_requests, last.cost_musd, last.sub_cost_musd), (10, 8, 2 * 510 + 8 * 1000, 8 * 1000));
    assert_eq!(last.zspend_musd, 2 * 510 + 8 * 1000, "the zone spend still counts every request");
    let b = &e.groups[0].win[0];
    assert_eq!((b.req, b.cost), (2, 2 * 510), "H10 and H5b see the main loop's share");
    // the same traffic from the main loop is a spike
    let (mut e, mut t) = armed();
    let acts: Vec<Act> = (0..5).flat_map(|_| sub_turn(&mut e, &mut t, false)).collect();
    let h = harm(&acts).expect("a main-loop spike fires");
    assert!(h["signal"] == "H5b" || h["signal"] == "H10", "{h}");
}

/// GBAL real-model proof (2026-10-25): the kernel's idle closes the owner turn, so the next line is a new turn, not a
/// steer; an idle with a request in flight, or before any request, changes nothing (the quiet rule still applies).
#[test]
fn kernel_idle_closes_the_owner_turn() {
    let mut e = Engine::new(Some("tP"));
    let mut out = e.line(&cx(T0), "o1", "how many lines?");
    assert!(e.idle(&cx(T0 + 10)).is_empty(), "no request yet: the turn stays open");
    out.extend(e.req_start(&cx(T0 + 100), "x1", 100));
    assert!(e.idle(&cx(T0 + 150)).is_empty(), "a request in flight: the turn stays open");
    out.extend(e.req_end(&cx(T0 + 1000), "x1", 7, false));
    out.extend(e.idle(&cx(T0 + 1100)));
    let closed: Vec<Turn> = out.iter().filter_map(|a| if let Act::TurnClosed(t) = a { Some(t.clone()) } else { None }).collect();
    assert_eq!(closed.len(), 1);
    assert_eq!((closed[0].kind.as_str(), closed[0].requests, closed[0].cost_musd, closed[0].steered, closed[0].end), ("owner", 1, 7, 0, T0 + 1000));
    // the owner's next line, 2 s later, is a new turn
    e.line(&cx(T0 + 3000), "o2", "and the first line?");
    let t = e.turn.as_ref().unwrap();
    assert_eq!((t.kind.as_str(), t.steered, t.start), ("owner", 0, T0 + 3000));
    // a line while the body works is still a steer
    e.req_start(&cx(T0 + 3100), "x2", 100);
    e.line(&cx(T0 + 3200), "o3", "also count words");
    assert_eq!(e.turn.as_ref().unwrap().steered, 1);
}
