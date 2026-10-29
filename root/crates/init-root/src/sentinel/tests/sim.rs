//! Lane M #2 (v0.5.3): the sentinel's false-rollback rate when nothing changed, as the code stands: the chronic tests
//! exactly as `engine::judge` runs them (H5b, H8, H10 and H11 on units at α = 0.005 and the 8th largest of 40, H7 per
//! turn and H9 per request at α = 0.002: the "mixed α"), at each look (5, 10, 20 owner turns), per signal and per turn
//! model. The streams are RF-11's (`stream`, `Model`), with the change multiplying nothing. foundations.md P-3 states
//! only the union bound, 3 looks × (4 × 0.005 + 2 × 0.002) = 7.2 %; this measures the rate.
//!
//! `cargo test --offline -p init-root --bin init-root sentinel::tests::sim -- --ignored --nocapture --test-threads 1`,
//! with `INIT_STATS_OUT=<dir>` to write `<dir>/sentinel-null.json` (build/status/STATS-053.md).

use super::*;
use crate::goal::sim::{emit, provenance};
use std::collections::BTreeMap;

const SIGNALS: [&str; 6] = ["H5b", "H8", "H10", "H11", "H7", "H9"];

/// The signals that fire at each look of one null window (RF-11's `window_fires`, every look kept).
fn window(r: &mut Rng, m: Model) -> Vec<Vec<&'static str>> {
    let hist = engine::RING_M * engine::UNIT_MAX + 6;
    let turns = stream(r, m, hist + 20, hist, 1, Col::Cost);
    let ring: Vec<Base> = turns[..hist].iter().map(|t| t.0.clone()).collect();
    let req_ring: Vec<bool> = (0..400).map(|_| r.bern(0.02)).collect();
    engine::LOOKS.iter().map(|&n| {
        let win: Vec<Base> = turns[hist..hist + n].iter().map(|t| t.0.clone()).collect();
        let reqs = turns[hist..hist + n].iter().fold((0, 0), |a, t| (a.0 + t.1, a.1 + t.2));
        engine::judge(&ring, &req_ring, &win, reqs).into_iter().map(|h| h.signal).collect()
    }).collect()
}

fn rate(k: u64, n: u64) -> Value {
    let p = k as f64 / n as f64;
    json!({"k": k, "n": n, "rate": (p * 1e6).round() / 1e6, "se": ((p * (1.0 - p) / n as f64).sqrt() * 1e6).round() / 1e6})
}

fn model_row(name: &str, m: Model, n: u64, seed: u64) -> Value {
    let mut r = Rng(seed);
    let (mut any, mut first) = (0u64, [0u64; 3]);
    let mut at_look = [0u64; 3];
    let mut sig_look: BTreeMap<&str, [u64; 3]> = SIGNALS.iter().map(|s| (*s, [0; 3])).collect();
    let mut trigger: BTreeMap<&str, u64> = SIGNALS.iter().map(|s| (*s, 0)).collect();
    for _ in 0..n {
        let w = window(&mut r, m);
        for (i, hits) in w.iter().enumerate() {
            at_look[i] += u64::from(!hits.is_empty());
            for s in hits {
                sig_look.get_mut(s).unwrap()[i] += 1;
            }
        }
        // the engine acts on the first look that fires, and on that look's first hit (`looks`: `judge(..).next()`)
        if let Some((i, hits)) = w.iter().enumerate().find(|(_, h)| !h.is_empty()) {
            any += 1;
            first[i] += 1;
            *trigger.get_mut(hits[0]).unwrap() += 1;
        }
    }
    let looks = |a: [u64; 3]| json!({"5": rate(a[0], n), "10": rate(a[1], n), "20": rate(a[2], n)});
    json!({"model": name, "l": if m.l.is_finite() { json!(m.l) } else { json!("inf") }, "sigma_s": m.sigma_s, "windows": n,
        "false_rollback_per_window": rate(any, n), "first_fire_at_look": looks(first), "fires_at_look": looks(at_look),
        "per_signal_at_look": sig_look.into_iter().map(|(s, a)| (s.to_string(), looks(a))).collect::<serde_json::Map<_, _>>(),
        "triggering_signal": trigger.into_iter().map(|(s, k)| (s.to_string(), rate(k, n))).collect::<serde_json::Map<_, _>>()})
}

/// The exact size of the unit tests at each look (the exceedance tail at the threshold the engine uses), for `units`
/// window units: what a per-test rate can be at most under exchangeability.
fn exact_sizes() -> Value {
    let row = |units: u64| {
        let k = engine::k_exceed_j(units as usize, engine::RING_J_COST, engine::HARD_COST);
        json!({"units": units, "k": k, "tail_ppm": k.map(|k| stats::exceed_tail(engine::RING_M as u64, engine::RING_J_COST as u64, units, k).ppm())})
    };
    json!({"one_turn_per_unit": [row(5), row(10), row(20)], "one_long_session_units_of_6": [row(1), row(2), row(4)],
        "alpha_unit_tests": "5/1000 (HARD_COST)", "alpha_h7_h9": "2/1000 (HARD)", "union_bound_p3": 0.072})
}

#[test]
#[ignore = "lane M #2 tables: cargo test --offline -p init-root --bin init-root sentinel::tests::sim -- --ignored --nocapture --test-threads 1"]
fn sentinel_null_tables() {
    const N: u64 = 20_000;
    const SEED: u64 = 0x5e47_053d_0000_0002;
    let models = [("iid, a session per turn", IID), ("iid, one long session", IID_LONG), ("clustered L=4 σs=0.6", Model { l: 4.0, sigma_s: 0.6 }),
        ("clustered L=8 σs=0.6", Model { l: 8.0, sigma_s: 0.6 }), ("clustered L=4 σs=0.3", Model { l: 4.0, sigma_s: 0.3 })];
    let rows: Vec<Value> = models.iter().enumerate().map(|(i, (name, m))| model_row(name, *m, N, SEED ^ (i as u64 + 1))).collect();
    let v = json!({"provenance": provenance("cargo test --offline -p init-root --bin init-root sentinel::tests::sim -- --ignored --nocapture --test-threads 1",
        SEED, json!({"windows_per_model": N})), "exact_sizes": exact_sizes(), "models": rows});
    emit("sentinel-null", &v);
}
